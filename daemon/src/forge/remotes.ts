// What yaacd has learnt about a git remote, kept on the target (`~/.yaac/remotes.json`, D119).
//
// A generic record per remote — keyed by the normalised remote URL, with an open `data` map —
// rather than a forge-specific schema: the Azure DevOps tenant that can reach an organisation
// is one entry (`azureTenant`), a fork's upstream another (`githubBase`), and later forges add
// their own without a migration.

import * as path from "node:path";
import { normaliseRemote } from "../session/fsinfo.js";
import { nowIso, readJson, writeJsonAtomic } from "../util/fsx.js";

export interface RemoteRecord {
  /** Free-form facts, by name. */
  data: Record<string, unknown>;
  updatedAt: string;
}

interface RemotesFile {
  remotes: Record<string, RemoteRecord>;
}

export class RemoteStore {
  private file: string;
  private state: RemotesFile;

  constructor(home: string) {
    this.file = path.join(home, "remotes.json");
    let loaded: RemotesFile | undefined;
    try {
      loaded = readJson<RemotesFile>(this.file);
    } catch {
      loaded = undefined;
    }
    this.state = loaded && typeof loaded.remotes === "object" && loaded.remotes ? loaded : { remotes: {} };
  }

  static key(remoteUrl: string): string {
    return normaliseRemote(remoteUrl);
  }

  get<T = unknown>(remoteUrl: string, name: string): T | undefined {
    return this.state.remotes[RemoteStore.key(remoteUrl)]?.data[name] as T | undefined;
  }

  /** Sets (or, with `undefined`, removes) one fact, and writes the file. */
  set(remoteUrl: string, name: string, value: unknown): void {
    const key = RemoteStore.key(remoteUrl);
    const rec = this.state.remotes[key] ?? { data: {}, updatedAt: nowIso() };
    if (value === undefined) delete rec.data[name];
    else rec.data[name] = value;
    rec.updatedAt = nowIso();
    this.state.remotes[key] = rec;
    writeJsonAtomic(this.file, this.state);
  }
}
