// Folder trust (D147, docs/05-threads.md#folder-trust): when YAAC starts a thread in a folder
// of a user's project, the harness is told that folder is trusted — under the instance's
// `folderTrust` capability and its "Trust folders YAAC works in" switch — so the harness's own
// interface does not ask about it later.
//
// Each folder is handled once per instance and harness store: the outcome (marked, already
// trusted, left as the user set it, failed) is kept in `folder-trust.json` under the yaacd
// home, and a folder with an outcome is never asked about again. That is what keeps a failure
// from being retried (invariant 10) and a folder the user later untrusted in the harness from
// being marked again.

import * as path from "node:path";
import type { Instance } from "../providers/registry.js";
import type { FolderTrustOutcome } from "../providers/types.js";
import { nowIso, readJson, writeJsonAtomic } from "../util/fsx.js";

/** How long a thread start waits for the harness to answer, at most. */
export const FOLDER_TRUST_TIMEOUT_MS = 10_000;

interface Entry {
  outcome: FolderTrustOutcome["outcome"];
  at: string;
  reason?: string;
}

interface TrustFile {
  /** By `instance|store key`, then by folder. */
  folders: Record<string, Record<string, Entry>>;
}

export class FolderTrust {
  private data: TrustFile;
  private inFlight = new Map<string, Promise<string | null>>();

  constructor(
    private readonly file: string,
    private readonly log: (line: string) => void,
  ) {
    this.data = readJson<TrustFile>(file) ?? { folders: {} };
    if (!this.data.folders || typeof this.data.folders !== "object") this.data = { folders: {} };
  }

  /** Whether this instance marks folders: the capability, a provider that can, and the switch on. */
  static applies(instance: Instance): boolean {
    return (
      instance.config.trustFolders !== false &&
      (instance.descriptor.capabilities ?? []).includes("folderTrust") &&
      typeof instance.provider?.trustFolder === "function"
    );
  }

  /** What was recorded for a folder, if anything (tests, diagnostics). */
  recorded(instance: Instance, folder: string): Entry | null {
    return this.data.folders[storeKey(instance)]?.[path.resolve(folder)] ?? null;
  }

  /**
   * Marks `folder` trusted in the instance's harness unless it was handled before. Resolves to
   * the notice for the thread ("Marked … as trusted in Codex", or why not), or null when there
   * is nothing to say. Never throws; concurrent calls for one folder share one attempt.
   */
  ensure(instance: Instance, folder: string): Promise<string | null> {
    if (!FolderTrust.applies(instance)) return Promise.resolve(null);
    const key = storeKey(instance);
    const dir = path.resolve(folder);
    if (this.data.folders[key]?.[dir]) return Promise.resolve(null);
    const flight = `${key}\u0000${dir}`;
    const running = this.inFlight.get(flight);
    // A second thread in the same folder at the same moment: the first one gets the notice.
    if (running) return running.then(() => null);
    const p = this.attempt(instance, key, dir).finally(() => this.inFlight.delete(flight));
    this.inFlight.set(flight, p);
    return p;
  }

  private async attempt(instance: Instance, key: string, dir: string): Promise<string | null> {
    const harness = harnessName(instance);
    let r: FolderTrustOutcome;
    try {
      r = await withTimeout(instance.provider!.trustFolder!(dir, { timeoutMs: FOLDER_TRUST_TIMEOUT_MS }), FOLDER_TRUST_TIMEOUT_MS + 1_000);
    } catch (e) {
      r = { outcome: "failed", reason: (e as Error).message, uncertain: true };
    }
    const entry: Entry = { outcome: r.outcome, at: nowIso() };
    if (r.outcome === "failed") entry.reason = r.reason;
    (this.data.folders[key] ??= {})[dir] = entry;
    try {
      writeJsonAtomic(this.file, this.data);
    } catch (e) {
      this.log(`could not save folder trust: ${(e as Error).message}`);
    }
    switch (r.outcome) {
      case "marked":
        return `Marked ${dir} as trusted in ${harness}`;
      case "trusted":
      case "declined":
        return null;
      case "failed":
        this.log(`marking a folder trusted in ${instance.config.id} failed: ${r.reason}`);
        return r.uncertain
          ? `${harness} did not answer whether it marked ${dir} as trusted (${r.reason}). YAAC will not ask again; ${harness} may ask about this folder itself.`
          : `Could not mark ${dir} as trusted in ${harness}: ${r.reason}. YAAC will not try again; ${harness} may ask about this folder itself.`;
    }
  }
}

function storeKey(instance: Instance): string {
  return `${instance.config.id}|${instance.descriptor.store?.key ?? ""}`;
}

/** The harness's name without the instance name ("Codex", not "Codex · work"). */
function harnessName(instance: Instance): string {
  return instance.descriptor.displayName.split(" · ")[0] || instance.descriptor.displayName;
}

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(`no answer within ${Math.round(ms / 1000)} s`)), ms);
    t.unref?.();
    p.then(
      (v) => {
        clearTimeout(t);
        resolve(v);
      },
      (e) => {
        clearTimeout(t);
        reject(e);
      },
    );
  });
}
