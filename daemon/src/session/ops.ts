// Operation ids (docs/03-protocol.md#idempotency).
//
// Every state-changing request carries a client-generated `opId`. The record is written to
// disk (fsync) *before* acting and again *after*. A repeated `opId` returns the recorded
// outcome instead of acting again; one still in flight in this process is joined. A record
// with a beginning and no end was cut off by a crash: unless the outcome can be reconciled,
// it is `outcome_unknown` — shown to the user, never retried automatically.

import * as fs from "node:fs";
import * as path from "node:path";
import { RpcError } from "../protocol/rpc.js";
import type { ErrorObject } from "../protocol/generated/types.js";

type Outcome = { ok: true; result: unknown } | { ok: false; error: ErrorObject };

interface OpRecord {
  opId: string;
  method: string;
  at: number;
  outcome?: Outcome;
}

type Line =
  | { op: string; method: string; phase: "begin"; at: number }
  | { op: string; phase: "end"; outcome: Outcome; at: number }
  | { op: string; phase: "forget"; at: number };

/** Reconciles an operation cut off by a crash; `null` when the outcome cannot be told. */
export type Reconciler = (method: string, opId: string) => Promise<Outcome | null>;

export const OP_RETENTION_MS = 14 * 24 * 3600 * 1000;

export class OpLog {
  private records = new Map<string, OpRecord>();
  private inflight = new Map<string, Promise<unknown>>();
  private fd: number;

  constructor(
    private readonly file: string,
    private readonly reconcile: Reconciler = async () => null,
  ) {
    fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
    this.load();
    this.fd = fs.openSync(file, "a", 0o600);
  }

  private load(): void {
    let text = "";
    try {
      text = fs.readFileSync(this.file, "utf8");
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code !== "ENOENT") throw e;
    }
    for (const line of text.split("\n")) {
      if (!line) continue;
      let l: Line;
      try {
        l = JSON.parse(line) as Line;
      } catch {
        continue; // a partial last line from a crash
      }
      if (l.phase === "begin") this.records.set(l.op, { opId: l.op, method: l.method, at: l.at });
      else if (l.phase === "forget") this.records.delete(l.op);
      else {
        const r = this.records.get(l.op);
        if (r) r.outcome = l.outcome;
      }
    }
    // Keep the file bounded: rewrite without records older than the retention period.
    const cutoff = Date.now() - OP_RETENTION_MS;
    let dropped = false;
    for (const [k, r] of this.records) {
      if (r.at < cutoff) {
        this.records.delete(k);
        dropped = true;
      }
    }
    if (dropped) {
      const lines: string[] = [];
      for (const r of this.records.values()) {
        lines.push(JSON.stringify({ op: r.opId, method: r.method, phase: "begin", at: r.at }));
        if (r.outcome) lines.push(JSON.stringify({ op: r.opId, phase: "end", outcome: r.outcome, at: r.at }));
      }
      const tmp = `${this.file}.tmp`;
      fs.writeFileSync(tmp, lines.map((l) => `${l}\n`).join(""), { mode: 0o600 });
      fs.renameSync(tmp, this.file);
    }
  }

  private write(line: Line): void {
    fs.writeSync(this.fd, `${JSON.stringify(line)}\n`);
    fs.fsyncSync(this.fd);
  }

  /** Whether an operation id has been seen. */
  has(opId: string): boolean {
    return this.records.has(opId);
  }

  /**
   * Runs `act` at most once for `opId`. Errors thrown by `act` before it handed anything to a
   * harness may be marked `retryable` (via `forget`), so a corrected request is not stuck.
   */
  async run<T>(method: string, opId: unknown, act: () => Promise<T>): Promise<T> {
    if (typeof opId !== "string" || opId.length === 0) throw new RpcError("invalid_params", "opId is required");
    const pending = this.inflight.get(opId);
    if (pending) return (await pending) as T;
    const rec = this.records.get(opId);
    if (rec) {
      if (rec.method !== method) throw new RpcError("invalid_params", `opId ${opId} was used for ${rec.method}`);
      if (!rec.outcome) {
        const reconciled = await this.reconcile(method, opId);
        rec.outcome = reconciled ?? {
          ok: false,
          error: new RpcError(
            "outcome_unknown",
            `${method} may or may not have taken effect: yaacd stopped before it could record the outcome. It will not be retried automatically.`,
            { opId },
          ).toObject(),
        };
        this.write({ op: opId, phase: "end", outcome: rec.outcome, at: Date.now() });
      }
      if (rec.outcome.ok) return rec.outcome.result as T;
      const e = rec.outcome.error;
      throw new RpcError(e.code, e.message, e.data);
    }
    const record: OpRecord = { opId, method, at: Date.now() };
    this.records.set(opId, record);
    this.write({ op: opId, method, phase: "begin", at: record.at });
    const p = (async () => {
      try {
        const result = await act();
        record.outcome = { ok: true, result };
        return result;
      } catch (e) {
        if (e instanceof NotActed) {
          // Nothing was handed to a harness: forget the op so the client may retry it.
          this.records.delete(opId);
          this.write({ op: opId, phase: "forget", at: Date.now() });
          throw e.error;
        }
        const err = e instanceof RpcError ? e : new RpcError("internal_error", (e as Error)?.message ?? String(e));
        record.outcome = { ok: false, error: err.toObject() };
        throw err;
      } finally {
        this.inflight.delete(opId);
        if (record.outcome) this.write({ op: opId, phase: "end", outcome: record.outcome, at: Date.now() });
      }
    })();
    this.inflight.set(opId, p);
    return p;
  }

  close(): void {
    try {
      fs.closeSync(this.fd);
    } catch {
      /* already closed */
    }
  }
}

/** Thrown by an operation that was rejected before anything was handed to a harness. */
export class NotActed extends Error {
  constructor(readonly error: RpcError) {
    super(error.message);
  }
}
