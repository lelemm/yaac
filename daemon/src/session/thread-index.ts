// The thread index (docs/03-protocol.md#the-thread-index).
//
// yaacd's own index of every thread on the target, with a change counter. A sync token is a
// position in that counter, `idx<generation>:<counter>` — YAAC's, not any harness's. A token
// from another generation (or from the future, after an unclean stop) gets a full listing.
// Threads are removed only when the harness positively reports them gone (or YAAC deletes
// them); a failed or incomplete scan marks the instance stale and keeps its threads.

import * as path from "node:path";
import type { ThreadSummary, ThreadSyncResult } from "../protocol/generated/types.js";
import { canonical } from "../protocol/json.js";
import { randomId, readJson, writeJsonAtomic } from "../util/fsx.js";

/** Who holds a thread, for rendering `control` relative to each client. */
export interface Holder {
  /** Client key driving the running turn. */
  driver?: string | null;
  driverHost?: string | null;
  /** Another program holds it. */
  external?: string | null;
}

interface Entry {
  changeSeq: number;
  /** The summary with `control: "free"`; the per-client control is applied when sent. */
  summary: ThreadSummary;
  holder: Holder;
}

interface IndexFile {
  generation: string;
  counter: number;
  entries: Record<string, Entry>;
  tombstones: Record<string, number>;
  stale: string[];
}

export const SYNC_PAGE_MAX = 64 * 1024;
const MAX_TOMBSTONES = 10000;

export class ThreadIndex {
  private data: IndexFile;
  private readonly file: string;
  private saveTimer: NodeJS.Timeout | null = null;

  constructor(home: string, cleanStart: boolean) {
    this.file = path.join(home, "index.json");
    let loaded: IndexFile | undefined;
    try {
      loaded = readJson<IndexFile>(this.file);
    } catch {
      loaded = undefined;
    }
    // Process inventory needs a fresh observation after a daemon restart. Move the token
    // forward for clients that cached a running row before the restart.
    if (loaded) {
      for (const entry of Object.values(loaded.entries)) {
        if (!entry.summary.runningCommands?.length) continue;
        delete entry.summary.runningCommands;
        entry.changeSeq = ++loaded.counter;
      }
    }
    this.data = loaded ?? { generation: randomId("", 6), counter: 0, entries: {}, tombstones: {}, stale: [] };
    // After an unclean stop the counter on disk may be behind tokens already handed out:
    // a new generation makes every old token a full listing.
    if (loaded && !cleanStart) this.data.generation = randomId("", 6);
    this.save();
  }

  get token(): string {
    return `idx${this.data.generation}:${this.data.counter}`;
  }

  get(id: string): { summary: ThreadSummary; holder: Holder } | undefined {
    return this.data.entries[id];
  }

  ids(): string[] {
    return Object.keys(this.data.entries);
  }

  /** Stores a summary; returns whether anything changed (and the counter moved). */
  upsert(summary: ThreadSummary, holder: Holder): boolean {
    const prev = this.data.entries[summary.id];
    const base = { ...summary, control: "free" as const, controlHolder: null };
    if (prev && canonical(prev.summary) === canonical(base) && canonical(prev.holder) === canonical(holder)) return false;
    this.data.counter += 1;
    this.data.entries[summary.id] = { changeSeq: this.data.counter, summary: base, holder };
    delete this.data.tombstones[summary.id];
    this.scheduleSave();
    return true;
  }

  remove(id: string): boolean {
    if (!this.data.entries[id]) return false;
    delete this.data.entries[id];
    this.data.counter += 1;
    this.data.tombstones[id] = this.data.counter;
    const ids = Object.keys(this.data.tombstones);
    if (ids.length > MAX_TOMBSTONES) {
      ids.sort((a, b) => this.data.tombstones[a]! - this.data.tombstones[b]!);
      for (const old of ids.slice(0, ids.length - MAX_TOMBSTONES)) delete this.data.tombstones[old];
    }
    this.scheduleSave();
    return true;
  }

  setStale(instance: string, stale: boolean): void {
    const has = this.data.stale.includes(instance);
    if (stale && !has) this.data.stale.push(instance);
    if (!stale && has) this.data.stale = this.data.stale.filter((s) => s !== instance);
    this.scheduleSave();
  }

  staleInstances(): string[] {
    return [...this.data.stale];
  }

  private parse(token: string | undefined): number | null {
    if (!token) return null;
    const m = /^idx([^:]+):(\d+)$/.exec(token);
    if (!m || m[1] !== this.data.generation) return null;
    const n = Number(m[2]);
    if (n > this.data.counter) return null;
    // Tombstones older than the oldest kept one are gone: such a token cannot be served.
    const oldest = Math.min(...Object.values(this.data.tombstones), Number.POSITIVE_INFINITY);
    if (Object.keys(this.data.tombstones).length >= MAX_TOMBSTONES && n < oldest) return null;
    return n;
  }

  /** `thread/sync`: incremental from a token, or a full listing; paged by bytes. */
  sync(token: string | undefined, maxBytes: number | null | undefined, render: (s: ThreadSummary, h: Holder) => ThreadSummary): ThreadSyncResult {
    const limit = Math.max(1024, Math.min(maxBytes ?? SYNC_PAGE_MAX, SYNC_PAGE_MAX));
    const since = this.parse(token);
    const full = since === null;
    const from = since ?? 0;
    const changed = Object.values(this.data.entries)
      .filter((e) => e.changeSeq > from)
      .sort((a, b) => a.changeSeq - b.changeSeq);
    const page: ThreadSummary[] = [];
    let bytes = 0;
    let last = from;
    let more = false;
    for (const e of changed) {
      const s = render(e.summary, e.holder);
      const size = Buffer.byteLength(canonical(s));
      if (page.length > 0 && bytes + size > limit) {
        more = true;
        break;
      }
      page.push(s);
      bytes += size;
      last = e.changeSeq;
    }
    const removed = full
      ? []
      : Object.entries(this.data.tombstones)
          .filter(([, seq]) => seq > from && (!more || seq <= last))
          .map(([id]) => id);
    const result: ThreadSyncResult = {
      changed: page,
      removed,
      token: more ? `idx${this.data.generation}:${last}` : this.token,
      more,
      full,
    };
    if (this.data.stale.length > 0) result.staleInstances = [...this.data.stale];
    return result;
  }

  private scheduleSave(): void {
    if (this.saveTimer) return;
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      this.save();
    }, 50);
  }

  save(): void {
    if (this.saveTimer) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    writeJsonAtomic(this.file, this.data);
  }
}
