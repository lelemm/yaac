// The per-thread journal (docs/03-protocol.md#journal-and-replay).
//
// An append-only JSON-lines file of events with `(epoch, seq)` positions, kept in memory as
// well (it is bounded by the ceiling). Events are appended whether or not a client is
// attached. Completed items' delta runs are compacted into one `itemUpdated` covering the run
// (`throughSeq`); events at or below the lowest acknowledged position are trimmed; a size
// ceiling trims the oldest as a last resort. A corrupt journal is started afresh in a new
// epoch, so clients holding the old epoch get a `reset`.

import * as fs from "node:fs";
import * as path from "node:path";
import type { Item, ThreadEvent } from "../protocol/generated/types.js";
import { applyDelta, clone, eventItemId } from "./items.js";
import { ensureDir, readJson, writeJsonAtomic } from "../util/fsx.js";

/** Who the event concerns for per-client rendering (control is relative to the receiver). */
export interface EntryMeta {
  /** The client key driving the thread, for `controlChanged`. */
  driver?: string | null;
  /** Another program holds the thread, for `controlChanged`. */
  external?: boolean;
}

export interface JournalEntry {
  seq: number;
  /** Last sequence number this entry covers (compacted runs); `seq` when absent. */
  through?: number;
  event: ThreadEvent;
  meta?: EntryMeta;
}

export function lastSeqOf(e: JournalEntry): number {
  return e.through ?? e.seq;
}

interface MetaFile {
  epoch: number;
  headSeq: number;
  trimmedThrough: number;
  /** The journal holds the thread's whole life (nothing trimmed, started at creation). */
  complete: boolean;
  acks: Record<string, { seq: number; at: number }>;
}

export interface JournalOptions {
  /** Per-thread size cap in bytes; the oldest events are trimmed beyond it. */
  ceilingBytes: number;
  /** A client not seen for this long no longer holds the journal back. */
  ackRetentionMs: number;
}

export const DEFAULT_JOURNAL_OPTIONS: JournalOptions = {
  ceilingBytes: 8 * 1024 * 1024,
  ackRetentionMs: 7 * 24 * 3600 * 1000,
};

export class Journal {
  private meta: MetaFile;
  private list: JournalEntry[] = [];
  private sizes: number[] = [];
  private bytes = 0;
  private fd: number | null = null;
  private readonly eventsFile: string;
  private readonly metaFile: string;
  private listeners = new Set<(entry: JournalEntry) => void>();

  private constructor(
    readonly dir: string,
    private readonly opts: JournalOptions,
    meta: MetaFile,
  ) {
    this.meta = meta;
    this.eventsFile = path.join(dir, "events.jsonl");
    this.metaFile = path.join(dir, "meta.json");
  }

  /** Opens (or creates) the journal in `dir`. `fresh`: a newly created thread. */
  static open(dir: string, opts: JournalOptions, fresh: boolean): Journal {
    ensureDir(dir);
    const metaFile = path.join(dir, "meta.json");
    let meta: MetaFile | undefined;
    let metaBroken = false;
    try {
      meta = readJson<MetaFile>(metaFile);
      if (meta && (typeof meta.epoch !== "number" || typeof meta.headSeq !== "number")) {
        meta = undefined;
        metaBroken = true;
      }
    } catch {
      metaBroken = true;
    }
    const eventsFile = path.join(dir, "events.jsonl");
    const hasEvents = fs.existsSync(eventsFile) && fs.statSync(eventsFile).size > 0;
    if (!meta) {
      if (metaBroken || hasEvents) {
        // The position bookkeeping is gone: start afresh in an epoch no client can hold.
        Journal.setAside(eventsFile);
        meta = { epoch: Math.floor(Date.now() / 1000), headSeq: 0, trimmedThrough: 0, complete: false, acks: {} };
      } else {
        meta = { epoch: 1, headSeq: 0, trimmedThrough: 0, complete: fresh, acks: {} };
      }
      writeJsonAtomic(metaFile, meta);
    }
    meta.acks ??= {};
    const j = new Journal(dir, opts, meta);
    j.load();
    return j;
  }

  private static setAside(file: string): void {
    if (fs.existsSync(file)) fs.renameSync(file, `${file}.corrupt-${Date.now()}`);
  }

  private load(): void {
    let text = "";
    try {
      text = fs.readFileSync(this.eventsFile, "utf8");
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code !== "ENOENT") throw e;
    }
    // A trailing partial line is the remains of a crash mid-write: that event was never
    // acknowledged or sent, so it is cut off. Any other unreadable line means corruption.
    const cut = text.endsWith("\n") || text.length === 0 ? text.length : text.lastIndexOf("\n") + 1;
    const truncateTo = cut < text.length ? Buffer.byteLength(text.slice(0, cut), "utf8") : -1;
    let corrupt = false;
    for (const line of text.slice(0, cut).split("\n")) {
      if (line.length === 0) continue;
      try {
        const entry = JSON.parse(line) as JournalEntry;
        if (typeof entry.seq !== "number" || !entry.event) throw new Error("bad entry");
        const size = Buffer.byteLength(line, "utf8") + 1;
        this.list.push(entry);
        this.sizes.push(size);
        this.bytes += size;
      } catch {
        corrupt = true;
        break;
      }
    }
    if (corrupt) {
      Journal.setAside(this.eventsFile);
      this.list = [];
      this.sizes = [];
      this.bytes = 0;
      this.meta = { epoch: this.meta.epoch + 1, headSeq: 0, trimmedThrough: 0, complete: false, acks: {} };
      this.writeMeta();
    } else if (truncateTo >= 0) {
      fs.truncateSync(this.eventsFile, truncateTo);
    }
    const last = this.list.at(-1);
    if (last && lastSeqOf(last) > this.meta.headSeq) this.meta.headSeq = lastSeqOf(last);
    this.fd = fs.openSync(this.eventsFile, "a", 0o600);
  }

  get epoch(): number {
    return this.meta.epoch;
  }

  get headSeq(): number {
    return this.meta.headSeq;
  }

  /** Events at or below this have been dropped; a position below it gets a reset. */
  get trimmedThrough(): number {
    return this.meta.trimmedThrough;
  }

  /** Whether the journal holds the whole life of the thread. */
  get complete(): boolean {
    return this.meta.complete;
  }

  get sizeBytes(): number {
    return this.bytes;
  }

  entries(): readonly JournalEntry[] {
    return this.list;
  }

  onAppend(listener: (entry: JournalEntry) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /**
   * Appends one event. `durable`: fsync before returning (requests are journaled before any
   * client sees them, and must survive a crash).
   */
  append(event: ThreadEvent, meta?: EntryMeta, durable = false): JournalEntry {
    const entry: JournalEntry = { seq: this.meta.headSeq + 1, event };
    if (meta) entry.meta = meta;
    const line = `${JSON.stringify(entry)}\n`;
    fs.writeSync(this.fd!, line);
    if (durable) fs.fsyncSync(this.fd!);
    this.meta.headSeq = entry.seq;
    this.list.push(entry);
    const size = Buffer.byteLength(line, "utf8");
    this.sizes.push(size);
    this.bytes += size;
    if (this.bytes > this.opts.ceilingBytes) this.enforceCeiling();
    for (const l of [...this.listeners]) l(entry);
    return entry;
  }

  /** Index of the first entry covering a sequence number above `seq`. */
  indexAfter(seq: number): number {
    let lo = 0;
    let hi = this.list.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (lastSeqOf(this.list[mid]!) > seq) hi = mid;
      else lo = mid + 1;
    }
    return lo;
  }

  /** Whether a client at `(epoch, afterSeq)` can be replayed to rather than reset. */
  canReplay(epoch: number, afterSeq: number): boolean {
    return epoch === this.meta.epoch && afterSeq >= this.meta.trimmedThrough && afterSeq <= this.meta.headSeq;
  }

  /** The item as it was after every event up to and including `seq`; null if unknown. */
  itemStateAt(itemId: string, seq: number): Item | null {
    let state: Item | null = null;
    for (const e of this.list) {
      if (e.seq > seq) break;
      if (eventItemId(e.event) !== itemId) continue;
      const ev = e.event as unknown as Record<string, unknown>;
      if (ev.item) state = clone(ev.item as Item);
      else if (e.event.type === "itemDelta" && state) applyDelta(state, e.event.field, e.event.delta);
      else if (e.event.type === "requestChanged" && state) {
        const s = state as unknown as Record<string, unknown>;
        s.state = ev.state;
        if (ev.resolution !== undefined) s.resolution = ev.resolution;
      }
    }
    return state;
  }

  /**
   * Replaces each completed item's contiguous runs of `itemDelta`/`itemUpdated` events with
   * one `itemUpdated` snapshot covering the run. Returns how many events were folded away.
   */
  compact(): number {
    const completed = new Set<string>();
    for (const e of this.list) if (e.event.type === "itemCompleted") completed.add(e.event.item.id);
    const out: JournalEntry[] = [];
    let folded = 0;
    for (let i = 0; i < this.list.length; ) {
      const e = this.list[i]!;
      const id = eventItemId(e.event);
      const foldable = (x: JournalEntry) =>
        (x.event.type === "itemDelta" || x.event.type === "itemUpdated") && eventItemId(x.event) === id;
      if (id && completed.has(id) && foldable(e)) {
        let j = i + 1;
        while (j < this.list.length && foldable(this.list[j]!)) j++;
        if (j - i >= 2) {
          const last = this.list[j - 1]!;
          const through = lastSeqOf(last);
          const state = this.itemStateAt(id, through);
          if (state) {
            out.push({ seq: e.seq, through, event: { type: "itemUpdated", item: state, throughSeq: through } });
            folded += j - i - 1;
            i = j;
            continue;
          }
        }
      }
      out.push(e);
      i++;
    }
    if (folded > 0) this.rewrite(out);
    return folded;
  }

  /** Records a client's durable position and trims what every known client has. */
  ack(client: string, epoch: number, seq: number, now = Date.now()): void {
    if (epoch !== this.meta.epoch) return;
    const prev = this.meta.acks[client];
    this.meta.acks[client] = { seq: Math.min(Math.max(seq, prev?.seq ?? 0), this.meta.headSeq), at: now };
    this.trimToAcks(now);
    this.writeMeta();
  }

  /** Marks a client as seen, without moving its position. */
  touch(client: string, now = Date.now()): void {
    const prev = this.meta.acks[client];
    if (prev) prev.at = now;
  }

  private trimToAcks(now: number): void {
    const known = Object.entries(this.meta.acks).filter(([, a]) => now - a.at <= this.opts.ackRetentionMs);
    for (const [k, a] of Object.entries(this.meta.acks)) {
      if (now - a.at > this.opts.ackRetentionMs) delete this.meta.acks[k];
    }
    if (known.length === 0) return;
    const low = Math.min(...known.map(([, a]) => a.seq));
    if (low > this.meta.trimmedThrough) this.trimThrough(low);
  }

  /** Drops every event at or below `seq`. */
  trimThrough(seq: number): void {
    const idx = this.indexAfter(seq);
    // A compacted entry straddling `seq` is kept whole; the trim point is what is really gone.
    const kept = this.list.slice(idx);
    const firstKept = kept[0];
    const trimmed = firstKept ? Math.min(seq, firstKept.seq - 1) : Math.min(seq, this.meta.headSeq);
    if (trimmed <= this.meta.trimmedThrough && idx === 0) return;
    this.meta.trimmedThrough = Math.max(this.meta.trimmedThrough, trimmed);
    this.meta.complete = false;
    this.rewrite(kept);
  }

  private enforceCeiling(): void {
    let drop = 0;
    let bytes = this.bytes;
    while (drop < this.list.length - 1 && bytes > (this.opts.ceilingBytes * 3) / 4) {
      bytes -= this.sizes[drop]!;
      drop++;
    }
    if (drop === 0) return;
    const through = lastSeqOf(this.list[drop - 1]!);
    this.meta.trimmedThrough = Math.max(this.meta.trimmedThrough, through);
    this.meta.complete = false;
    this.rewrite(this.list.slice(drop));
  }

  private rewrite(entries: JournalEntry[]): void {
    const lines = entries.map((e) => `${JSON.stringify(e)}\n`);
    const tmp = `${this.eventsFile}.tmp`;
    const fd = fs.openSync(tmp, "w", 0o600);
    try {
      fs.writeSync(fd, lines.join(""));
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
    // Meta first: if we crash between the two, headSeq/trim point are still conservative.
    this.writeMeta();
    fs.renameSync(tmp, this.eventsFile);
    if (this.fd !== null) fs.closeSync(this.fd);
    this.fd = fs.openSync(this.eventsFile, "a", 0o600);
    this.list = entries;
    this.sizes = lines.map((l) => Buffer.byteLength(l, "utf8"));
    this.bytes = this.sizes.reduce((a, b) => a + b, 0);
  }

  writeMeta(): void {
    writeJsonAtomic(this.metaFile, this.meta);
  }

  close(): void {
    if (this.fd !== null) {
      try {
        this.writeMeta();
      } catch {
        /* the directory may be gone (thread deleted) */
      }
      fs.closeSync(this.fd);
      this.fd = null;
    }
  }

  /** Deletes the journal's files (thread deleted). */
  destroy(): void {
    if (this.fd !== null) fs.closeSync(this.fd);
    this.fd = null;
    fs.rmSync(this.dir, { recursive: true, force: true });
  }
}
