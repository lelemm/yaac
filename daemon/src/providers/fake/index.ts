// The fake provider (kind `fake`): a harness simulator for tests and for driving a real hub
// end to end with no harness installed and no tokens spent. Drivable programmatically (the
// methods under "Test controls") and by `/fake` directives in the message text (README.md).
//
// Models, efforts, and modes here are invented and obviously fake. Nothing else in yaacd
// may hardcode a model.

import * as fs from "node:fs";
import * as path from "node:path";
import type {
  AppliedSettings,
  Capability,
  Item,
  ModelInfo,
  QuestionAnswers,
  RequestResponse,
  ThreadEffective,
  TurnId,
  TurnOrigin,
  TurnStatus,
} from "../../protocol/generated/types.js";
import { Emitter } from "../../util/emitter.js";
import { nowIso, readJson, sha256Hex, writeJsonAtomic } from "../../util/fsx.js";
import type {
  ExternalControl,
  ForkOptions,
  HarnessProvider,
  ItemPage,
  MeshAttachment,
  MeshHooks,
  NativeId,
  NativeThreadInfo,
  PageRequest,
  ScanRequest,
  ProbeResult,
  ProviderContext,
  ProviderEvent,
  ProviderFactory,
  ProviderInput,
  RaisedRequest,
  StartThreadParams,
  ThreadPage,
  TurnOverrides,
} from "../types.js";
import { chunks, parseScript, type Step } from "./script.js";

export const FAKE_KIND = "fake";

const MODELS: ModelInfo[] = [
  {
    id: "fake-large",
    displayName: "Fake Large",
    description: "Invented by the fake provider. Sees images.",
    isDefault: true,
    capabilities: [{ capability: "imageInput", support: "supported" }],
    efforts: [
      { id: "fake-low", displayName: "Fake low" },
      { id: "fake-high", displayName: "Fake high" },
    ],
    defaultEffort: "fake-low",
    contextWindow: 100000,
  },
  {
    id: "fake-small",
    displayName: "Fake Small",
    description: "Invented by the fake provider. Cannot see images.",
    capabilities: [{ capability: "imageInput", support: "unsupported" }],
    efforts: [{ id: "fake-low", displayName: "Fake low" }],
    defaultEffort: "fake-low",
    contextWindow: 20000,
  },
  {
    id: "fake-mystery",
    displayName: "Fake Mystery",
    description: "Invented by the fake provider. Reports nothing about its inputs.",
  },
];

const MODES = [
  { id: "fake-ask", displayName: "Fake: ask first", isDefault: true },
  { id: "fake-auto", displayName: "Fake: auto" },
];

interface StoredThread {
  info: NativeThreadInfo;
  items: Item[];
  turns: number;
  settings: AppliedSettings;
  /** Turns the harness accepted (for crash-window tests: the harness's own evidence). */
  turnLog: TurnId[];
}

interface StoreFile {
  threads: Record<string, StoredThread>;
  seq: number;
}

interface PendingRequest {
  requestId: string;
  itemId: string;
  turnId: TurnId;
  request: RaisedRequest;
  resolve: (answer: RequestResponse | null) => void;
  crashOnAnswer: boolean;
}

interface LiveTurn {
  turnId: TurnId;
  /** `/fake deaf`: an interrupt is ignored (only `forceStop` ends the turn). */
  deaf?: boolean;
  abort: AbortController;
  steered: string[];
  done: Promise<void>;
}

class Aborted extends Error {}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(new Aborted());
    const t = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(t);
      reject(new Aborted());
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

export interface FakeOptions {
  nativeFork: boolean;
  seed: boolean;
  paceMs: number;
  status: ProbeResult["status"];
  storeDir: string;
  /** Offer `steer` (FAKE_STEER=0: a harness that cannot steer). */
  steer: boolean;
  /** Offer `forceStop` (FAKE_FORCE_STOP=0: a shared harness process yaacd cannot end). */
  forceStop: boolean;
}

export class FakeProvider implements HarnessProvider {
  readonly events = new Emitter<ProviderEvent>();
  private store: StoreFile;
  private readonly storeFile: string;
  private live = new Map<NativeId, LiveTurn>();
  private requests = new Map<string, PendingRequest>();
  private external = new Map<NativeId, ExternalControl>();
  private timers = new Set<NodeJS.Timeout>();
  private disposed = false;

  // ── Test controls ──────────────────────────────────────────────────────────────────────
  /** Every input handed to the harness, in order (tests read secrets back from here). */
  readonly received: Array<{ thread: NativeId; input: ProviderInput }> = [];
  /** Every answer handed to the harness, in full (including secrets). */
  readonly answers: Array<{ thread: NativeId; requestId: string; response: RequestResponse }> = [];
  /** The next scan fails (the instance becomes stale; no thread is removed). */
  failNextScan = false;
  /** Conversations returned by every `scanThreads` call so far (tests: early stop). */
  scannedCount = 0;
  /** Conversations looked up by id (`lookupThread`). */
  readonly lookedUp: NativeId[] = [];
  /** Threads whose harness process was ended (`forceStop`). */
  readonly forceStopped: NativeId[] = [];
  /** Called inside startTurn/startThread after the harness accepted the turn, before returning. */
  afterAccept: ((thread: NativeId, turn: TurnId) => Promise<void> | void) | null = null;
  /** The agent mesh (WP7): the hooks, each thread's attachment, and the `$` variables of `/fake mesh-*`. */
  private meshHooks: MeshHooks | null = null;
  private readonly attachments = new Map<NativeId, MeshAttachment>();
  private readonly meshVars = new Map<NativeId, { spawned: string[]; from: string | null }>();

  constructor(
    private readonly ctx: ProviderContext,
    readonly options: FakeOptions,
  ) {
    fs.mkdirSync(options.storeDir, { recursive: true, mode: 0o700 });
    this.storeFile = path.join(options.storeDir, "store.json");
    if (options.forceStop) this.forceStop = (thread) => this.forceStopImpl(thread);
    const existing = readJson<StoreFile>(this.storeFile);
    this.store = existing ?? { threads: {}, seq: 0 };
    if (!existing && options.seed) this.seed();
    this.save();
  }

  private save(): void {
    writeJsonAtomic(this.storeFile, this.store);
  }

  private next(prefix: string): string {
    this.store.seq += 1;
    return `${prefix}${this.store.seq}`;
  }

  private seed(): void {
    const t0 = "2026-11-01T09:00:00Z";
    const add = (native: string, title: string, originKind: "interactive" | "programmatic", text: string, reply: string) => {
      this.store.threads[native] = {
        info: { native, title, preview: reply.slice(0, 80), cwd: this.ctx.home, createdAt: t0, updatedAt: t0, originKind },
        items: [
          { id: "ext-u1", turnId: "ext-t1", status: "completed", kind: "userMessage", parts: [{ type: "text", text }] },
          { id: "ext-a1", turnId: "ext-t1", status: "completed", kind: "agentMessage", text: reply },
        ],
        turns: 1,
        settings: { model: "fake-large" },
        turnLog: ["ext-t1"],
      };
    };
    add("ext-terminal-1", "Refactor the parser (started in a terminal)", "interactive", "Refactor the parser.", "Done: the parser is split into lexer and grammar.");
    add("ext-job-1", "Nightly lint (background job)", "programmatic", "Run the linter.", "Lint passed with 0 warnings.");
  }

  private thread(native: NativeId): StoredThread {
    const t = this.store.threads[native];
    if (!t) throw new Error(`fake: no conversation ${native}`);
    return t;
  }

  private emit(event: ProviderEvent): void {
    if (!this.disposed) this.events.emit(event);
  }

  private later(ms: number, fn: () => void): void {
    const t = setTimeout(() => {
      this.timers.delete(t);
      if (!this.disposed) fn();
    }, ms);
    this.timers.add(t);
  }

  // ── HarnessProvider ────────────────────────────────────────────────────────────────────

  async probe(): Promise<ProbeResult> {
    const caps: Capability[] = [
      "threadList",
      "threadRead",
      "threadResume",
      "threadRename",
      "threadArchive",
      "threadDelete",
      "followExternal",
      "streamText",
      "streamReasoning",
      "interrupt",
      "approvals",
      "userQuestions",
      "modes",
      "modelSwitch",
      "effortSwitch",
      "imageInput",
      "reasoning",
      "tokenUsage",
      "contextUsage",
      "modelList",
      "plan",
    ];
    if (this.options.nativeFork) caps.push("threadFork");
    if (this.options.steer) caps.push("steer");
    if (this.meshHooks) caps.push("meshTools");
    return {
      displayName: "Fake",
      backend: "sdk",
      harnessVersion: "fake-1.0",
      store: { key: `fake:${sha256Hex(path.resolve(this.options.storeDir)).slice(0, 16)}`, home: this.options.storeDir, account: "fake-user" },
      status: this.options.status,
      statusMessage: this.options.status === "ready" ? null : `the fake provider is configured as ${this.options.status}`,
      capabilities: caps,
      models: MODELS,
      modelsComplete: true,
      modes: MODES,
    };
  }

  async scanThreads(page: ScanRequest): Promise<ThreadPage> {
    if (this.failNextScan) {
      this.failNextScan = false;
      throw new Error("fake: the listing failed");
    }
    // Newest first by last activity, as both real harnesses can list.
    let all = Object.values(this.store.threads)
      .map((t) => t.info)
      .sort((a, b) => (a.updatedAt < b.updatedAt ? 1 : a.updatedAt > b.updatedAt ? -1 : a.native < b.native ? -1 : 1));
    if (page.search) all = all.filter((t) => (t.title ?? "").toLowerCase().includes(page.search!.toLowerCase()));
    const start = Number.parseInt(page.cursor ?? "0", 10) || 0;
    const size = page.limit ?? 50;
    let slice = all.slice(start, start + size);
    let more = start + size < all.length;
    // The discovery caps: stop at the first conversation too old, or once `max` were listed.
    if (page.activeSince) {
      const i = slice.findIndex((t) => t.updatedAt < page.activeSince!);
      if (i >= 0) {
        slice = slice.slice(0, i);
        more = false;
      }
    }
    if (page.max && start + slice.length >= page.max) {
      slice = slice.slice(0, Math.max(0, page.max - start));
      more = false;
    }
    this.scannedCount += slice.length;
    return { threads: structuredClone(slice), cursor: more ? String(start + size) : null, complete: true };
  }

  async lookupThread(native: NativeId): Promise<NativeThreadInfo | null> {
    this.lookedUp.push(native);
    const t = this.store.threads[native];
    return t ? structuredClone(t.info) : null;
  }

  async readThread(thread: NativeId, page: PageRequest): Promise<ItemPage> {
    const t = this.thread(thread);
    const start = Number.parseInt(page.cursor ?? "0", 10) || 0;
    const size = page.limit ?? 200;
    return {
      items: structuredClone(t.items.slice(start, start + size)),
      cursor: start + size < t.items.length ? String(start + size) : null,
    };
  }

  async startThread(params: StartThreadParams, input: ProviderInput): Promise<{ thread: NativeId; turn: TurnId }> {
    const native = this.next("fk-");
    const now = nowIso();
    const firstText = input.text.split("\n").find((l) => l.trim() && !l.startsWith("/fake")) ?? input.text;
    this.store.threads[native] = {
      info: {
        native,
        title: firstText.slice(0, 60) || "New fake conversation",
        preview: firstText.slice(0, 80),
        cwd: params.cwd,
        model: params.model ?? "fake-large",
        createdAt: now,
        updatedAt: now,
        originKind: "programmatic",
      },
      items: [],
      turns: 0,
      settings: { model: params.model ?? "fake-large", effort: params.effort ?? null, mode: params.mode ?? "fake-ask" },
      turnLog: [],
    };
    this.attachMeshTo(native, params.cwd, "start");
    const turn = await this.beginTurn(native, input, "user");
    return { thread: native, turn };
  }

  async startTurn(thread: NativeId, input: ProviderInput, overrides: TurnOverrides): Promise<TurnId> {
    const t = this.thread(thread);
    if (this.live.has(thread)) throw new Error("fake: a turn is already running");
    const ext = this.external.get(thread);
    if (ext?.external) throw new Error("fake: the conversation is open elsewhere");
    if (overrides.model) t.settings.model = overrides.model;
    if (overrides.effort) t.settings.effort = overrides.effort;
    if (overrides.mode) t.settings.mode = overrides.mode;
    if (!this.attachments.has(thread)) this.attachMeshTo(thread, t.info.cwd, "resume");
    return this.beginTurn(thread, input, input.origin);
  }

  private async beginTurn(native: NativeId, input: ProviderInput, origin: TurnOrigin): Promise<TurnId> {
    const t = this.thread(native);
    t.turns += 1;
    const turnId = `turn-${t.turns}`;
    t.turnLog.push(turnId);
    t.items.push({ id: `user:${turnId}`, turnId, status: "completed", kind: "userMessage", parts: input.parts });
    t.info.updatedAt = nowIso();
    this.save();
    this.received.push({ thread: native, input });
    const lastFrom = [...(input.mesh ?? [])].reverse().find((m) => m.kind !== "notice")?.from;
    if (lastFrom) this.vars(native).from = lastFrom;
    const script = parseScript(input.text);
    if (script.steps.some((s) => s.k === "crash")) {
      // The harness has accepted the turn; yaacd dies before it can record that.
      process.kill(process.pid, "SIGKILL");
    }
    if (this.afterAccept) await this.afterAccept(native, turnId);
    const abort = new AbortController();
    const live: LiveTurn = { turnId, abort, steered: [], done: Promise.resolve() };
    this.live.set(native, live);
    live.done = new Promise<void>((resolve) => setImmediate(resolve)).then(() =>
      this.runTurn(native, live, script.plain ? [{ k: "say", text: `Echo: ${input.text}` }] : script.steps, origin),
    );
    return turnId;
  }

  private async runTurn(native: NativeId, live: LiveTurn, steps: Step[], origin: TurnOrigin): Promise<void> {
    const { turnId, abort } = live;
    const signal = abort.signal;
    let pace = this.options.paceMs;
    let status: TurnStatus = "completed";
    let error: string | null = null;
    let afterTurn: Array<() => void> = [];
    let mods: { crashOnAnswer: boolean; elsewhereMs: number | null; expireMs: number | null } = {
      crashOnAnswer: false,
      elsewhereMs: null,
      expireMs: null,
    };
    let n = 0;
    const id = (k: string) => `${turnId}-${k}${++n}`;
    this.emit({ type: "turnStarted", thread: native, turnId, origin, at: nowIso() });
    try {
      for (const step of steps) {
        if (signal.aborted) throw new Aborted();
        switch (step.k) {
          case "say":
            await this.stream(native, turnId, id("msg"), "agentMessage", step.text, pace, signal);
            break;
          case "reason":
            await this.stream(native, turnId, id("rsn"), "reasoning", step.text, pace, signal);
            break;
          case "tool":
            await this.tool(native, turnId, id("tool"), step.command, step.output, step.exitCode, pace, signal);
            break;
          case "bigOutput": {
            const line = "0123456789abcdefghijklmnopqrstuvwxyz the quick brown fox jumps over the lazy dog\n";
            const out = line.repeat(Math.ceil(step.bytes / line.length)).slice(0, step.bytes);
            await this.tool(native, turnId, id("tool"), `generate ${step.bytes} bytes`, out, 0, 0, signal, 64 * 1024);
            break;
          }
          case "plan": {
            const item: Item = {
              id: id("plan"),
              turnId,
              status: "completed",
              kind: "plan",
              steps: step.steps.map((text, i) => ({ text, state: i === 0 ? "inProgress" : "pending" })),
            };
            this.record(native, item);
            this.emit({ type: "itemStarted", thread: native, item });
            this.emit({ type: "itemCompleted", thread: native, item });
            break;
          }
          case "approve": {
            const request: RaisedRequest = {
              requestKind: "approval",
              approvalKind: "command",
              title: `Run \`${step.command}\``,
              command: step.command,
              cwd: this.thread(native).info.cwd,
              reason: "The fake agent wants to run a command.",
              decisions: [
                { id: "allow", label: "Allow once", kind: "allow" },
                { id: "allowSession", label: "Allow for this session", kind: "allowSession" },
                { id: "deny", label: "Deny", kind: "deny", acceptsReason: true },
              ],
            };
            const answer = await this.ask(native, turnId, request, mods, signal);
            mods = { crashOnAnswer: false, elsewhereMs: null, expireMs: null };
            const decision = answer?.requestKind === "approval" ? answer.decision : null;
            if (decision === "allow" || decision === "allowSession") {
              await this.tool(native, turnId, id("tool"), step.command, `ran ${step.command}`, 0, pace, signal);
            } else {
              const item: Item = { id: id("tool"), turnId, status: "declined", kind: "toolCall", tool: "command", title: step.command };
              this.record(native, item);
              this.emit({ type: "itemStarted", thread: native, item });
              this.emit({ type: "itemCompleted", thread: native, item });
              await this.stream(native, turnId, id("msg"), "agentMessage", answer ? "Understood, I will not run it." : "The approval went away.", pace, signal);
            }
            break;
          }
          case "ask": {
            const request: RaisedRequest = {
              requestKind: "question",
              questions: [
                {
                  id: "q1",
                  header: step.secret ? "Secret" : "Question",
                  text: step.text,
                  options: step.options.map((label) => ({ label })),
                  multiSelect: step.multi,
                  allowOther: step.options.length === 0 || !step.secret,
                  secret: step.secret,
                },
              ],
            };
            const answer = await this.ask(native, turnId, request, mods, signal);
            mods = { crashOnAnswer: false, elsewhereMs: null, expireMs: null };
            let reply: string;
            if (!answer || answer.requestKind !== "question") reply = "The question went away.";
            else {
              const a = (answer as QuestionAnswers).answers.q1;
              if (step.secret) reply = `Received the secret (${(a?.other ?? "").length} characters).`;
              else reply = `You chose: ${[...(a?.selected ?? []), ...(a?.other ? [a.other] : [])].join(", ") || "nothing"}.`;
            }
            await this.stream(native, turnId, id("msg"), "agentMessage", reply, pace, signal);
            break;
          }
          case "deaf":
            live.deaf = true;
            break;
          case "edit": {
            // A file change, as every provider reports one: unified-diff text in `output` (D86).
            const p = step.path;
            const diff = [
              `--- a/${p}`,
              `+++ b/${p}`,
              "@@ -1,4 +1,5 @@",
              " fn main() {",
              '-    println!("hello");',
              '+    let name = "fake";',
              '+    println!("hello, {name}");',
              "     run();",
              " }",
              "",
            ].join("\n");
            const itemId = id("edit");
            const started: Item = { id: itemId, turnId, status: "inProgress", kind: "toolCall", tool: "fileChange", title: `Editing ${p}`, input: { path: p } } as Item;
            this.emit({ type: "itemStarted", thread: native, item: started });
            await sleep(pace, signal);
            const done: Item = { ...started, status: "completed", title: `Edited ${p}`, output: diff } as Item;
            this.record(native, done);
            this.emit({ type: "itemCompleted", thread: native, item: done });
            break;
          }
          case "stall":
            await new Promise<never>((_, reject) => {
              if (signal.aborted) reject(new Aborted());
              signal.addEventListener("abort", () => reject(new Aborted()), { once: true });
            });
            break;
          case "sleep":
            await sleep(step.ms, signal);
            break;
          case "pace":
            pace = step.ms;
            break;
          case "fail":
            status = "failed";
            error = step.message;
            throw new Error(step.message);
          case "inUse":
            afterTurn.push(() => {
              this.setExternal(native, { external: true, holder: step.holder });
              if (step.ms !== null) this.later(step.ms, () => this.setExternal(native, { external: false }));
            });
            break;
          case "externalTurn":
            afterTurn.push(() => this.later(50, () => void this.runExternalTurn(native, step.text, step.ms)));
            break;
          case "outsideThread":
            this.addOutsideThread(step.title);
            break;
          case "usage":
            this.emit({ type: "usageUpdated", thread: native, tokens: { input: 100, output: 50, total: 150 }, context: { used: 150, window: 100000 } });
            break;
          case "crash":
            break; // handled when the turn was accepted
          case "crashOnAnswer":
            mods.crashOnAnswer = true;
            break;
          case "resolveElsewhere":
            mods.elsewhereMs = step.ms;
            break;
          case "expire":
            mods.expireMs = step.ms;
            break;
          case "mesh":
            await this.meshCall(native, turnId, id("mesh"), step, signal);
            break;
        }
        for (const s of live.steered.splice(0)) {
          await this.stream(native, turnId, id("msg"), "agentMessage", `Steered: ${s}`, pace, signal);
        }
      }
    } catch (e) {
      if (e instanceof Aborted) status = "interrupted";
      else {
        status = "failed";
        error = error ?? (e as Error).message;
      }
      afterTurn = status === "interrupted" ? [] : afterTurn;
    }
    for (const [rid, r] of this.requests) {
      if (r.turnId === turnId && this.findOwner(rid) === native) this.requests.delete(rid);
    }
    this.live.delete(native);
    const t = this.store.threads[native];
    if (t) {
      t.info.updatedAt = nowIso();
      this.save();
    }
    this.emit({
      type: "turnCompleted",
      thread: native,
      turnId,
      status,
      error,
      usage: { input: 120, output: 40, total: 160 },
      at: nowIso(),
    });
    for (const f of afterTurn) f();
  }

  private findOwner(requestId: string): NativeId | null {
    for (const [native, t] of Object.entries(this.store.threads)) {
      if (t.items.some((i) => i.kind === "request" && (i as { requestId?: string }).requestId === requestId)) return native;
    }
    return null;
  }

  private record(native: NativeId, item: Item): void {
    const t = this.store.threads[native];
    if (!t) return;
    const idx = t.items.findIndex((i) => i.id === item.id);
    if (idx >= 0) t.items[idx] = structuredClone(item);
    else t.items.push(structuredClone(item));
    this.save();
  }

  private async stream(
    native: NativeId,
    turnId: TurnId,
    itemId: string,
    kind: "agentMessage" | "reasoning",
    text: string,
    pace: number,
    signal: AbortSignal,
  ): Promise<void> {
    const field = kind === "agentMessage" ? "text" : "reasoning";
    const start: Item =
      kind === "agentMessage"
        ? { id: itemId, turnId, status: "inProgress", kind, text: "" }
        : { id: itemId, turnId, status: "inProgress", kind, reasoning: "" };
    this.emit({ type: "itemStarted", thread: native, item: start });
    let acc = "";
    for (const piece of chunks(text)) {
      if (pace > 0) await sleep(pace, signal);
      else if (signal.aborted) throw new Aborted();
      acc += piece;
      this.emit({ type: "itemDelta", thread: native, itemId, field, delta: piece });
    }
    const done: Item = { ...start, status: "completed", ...(kind === "agentMessage" ? { text: acc } : { reasoning: acc }) } as Item;
    this.record(native, done);
    this.emit({ type: "itemCompleted", thread: native, item: done });
  }

  private async tool(
    native: NativeId,
    turnId: TurnId,
    itemId: string,
    command: string,
    output: string,
    exitCode: number,
    pace: number,
    signal: AbortSignal,
    chunkSize?: number,
  ): Promise<void> {
    const start: Item = { id: itemId, turnId, status: "inProgress", kind: "toolCall", tool: "command", title: command, input: { command }, cwd: this.thread(native).info.cwd, output: "" };
    this.emit({ type: "itemStarted", thread: native, item: start });
    const pieces = chunkSize ? [] as string[] : chunks(output, 2);
    if (chunkSize) for (let i = 0; i < output.length; i += chunkSize) pieces.push(output.slice(i, i + chunkSize));
    for (const piece of pieces) {
      if (pace > 0) await sleep(pace, signal);
      else if (signal.aborted) throw new Aborted();
      this.emit({ type: "itemDelta", thread: native, itemId, field: "output", delta: piece });
    }
    const done: Item = { ...start, status: "completed", output, exitCode, outputBytes: Buffer.byteLength(output) } as Item;
    // The harness's own store keeps the output; a 2 MB string in the fake's JSON store is fine.
    this.record(native, done);
    this.emit({ type: "itemCompleted", thread: native, item: done });
  }

  private ask(
    native: NativeId,
    turnId: TurnId,
    request: RaisedRequest,
    mods: { crashOnAnswer: boolean; elsewhereMs: number | null; expireMs: number | null },
    signal: AbortSignal,
  ): Promise<RequestResponse | null> {
    const requestId = this.next("req-");
    const itemId = `${turnId}-${requestId}`;
    this.record(native, { id: itemId, turnId, status: "inProgress", kind: "request", requestId, state: "pending", blocking: true, ...request } as Item);
    return new Promise<RequestResponse | null>((resolve, reject) => {
      const onAbort = () => {
        this.requests.delete(requestId);
        reject(new Aborted());
      };
      signal.addEventListener("abort", onAbort, { once: true });
      this.requests.set(requestId, {
        requestId,
        itemId,
        turnId,
        request,
        crashOnAnswer: mods.crashOnAnswer,
        resolve: (answer) => {
          signal.removeEventListener("abort", onAbort);
          this.requests.delete(requestId);
          resolve(answer);
        },
      });
      this.emit({ type: "requestRaised", thread: native, turnId, requestId, request, blocking: true });
      if (mods.elsewhereMs !== null) this.later(mods.elsewhereMs, () => this.resolveElsewhere(native, requestId));
      if (mods.expireMs !== null) this.later(mods.expireMs, () => this.expireRequest(native, requestId));
    });
  }

  async interrupt(thread: NativeId, turn: TurnId): Promise<void> {
    const live = this.live.get(thread);
    if (!live || live.turnId !== turn) return;
    if (live.deaf) return; // a harness that does not comply
    live.abort.abort();
    await live.done;
  }

  /** Present only with `forceStop` (see the constructor). */
  forceStop?: (thread: NativeId) => Promise<void>;

  private async forceStopImpl(thread: NativeId): Promise<void> {
    this.forceStopped.push(thread);
    const live = this.live.get(thread);
    if (!live) return;
    live.abort.abort();
    await live.done;
  }

  async respond(thread: NativeId, requestId: string, response: RequestResponse): Promise<void> {
    const pending = this.requests.get(requestId);
    if (!pending) throw new Error(`fake: request ${requestId} is not pending`);
    this.answers.push({ thread, requestId, response });
    if (pending.crashOnAnswer) {
      // yaacd handed the answer over and dies before the harness confirms it.
      process.kill(process.pid, "SIGKILL");
    }
    this.markRequest(thread, requestId, "resolved");
    this.emit({ type: "requestResolved", thread, requestId, by: "client", answer: response });
    pending.resolve(response);
  }

  private markRequest(native: NativeId, requestId: string, state: "resolved" | "expired"): void {
    const t = this.store.threads[native];
    const item = t?.items.find((i) => i.kind === "request" && (i as { requestId?: string }).requestId === requestId);
    if (item) {
      (item as { state: string }).state = state;
      item.status = "completed";
      this.save();
    }
  }

  async effective(thread: NativeId): Promise<ThreadEffective> {
    const t = this.thread(thread);
    const model = MODELS.find((m) => m.id === t.settings.model) ?? MODELS[0]!;
    const image = model.capabilities?.find((c) => c.capability === "imageInput")?.support ?? "unknown";
    const descriptor = await this.probe();
    return {
      capabilities: descriptor.capabilities.filter((c) => c !== "imageInput" || image === "supported"),
      imageInput: image,
      imageFormats: image === "supported" ? ["image/png", "image/jpeg", "image/gif", "image/webp"] : [],
      efforts: model.efforts ?? [],
      modes: MODES,
      applied: { model: t.settings.model ?? model.id, effort: t.settings.effort ?? model.defaultEffort ?? null, mode: t.settings.mode ?? "fake-ask" },
    };
  }

  async control(thread: NativeId): Promise<ExternalControl> {
    return this.external.get(thread) ?? { external: false };
  }

  async runningTurn(thread: NativeId): Promise<TurnId | null> {
    return this.live.get(thread)?.turnId ?? null;
  }

  async forkThread(thread: NativeId, opts: ForkOptions): Promise<NativeId> {
    if (!this.options.nativeFork) throw new Error("fake: native fork is disabled");
    const src = this.thread(thread);
    const native = this.next("fk-");
    const now = nowIso();
    let items = src.items;
    const live = this.live.get(thread);
    const cut = opts.atTurn ?? null;
    if (cut) {
      const last = items.map((i) => i.turnId).lastIndexOf(cut);
      if (last >= 0) items = items.slice(0, last + 1);
    } else if (live) {
      items = items.filter((i) => i.turnId !== live.turnId);
    }
    this.store.threads[native] = {
      info: { ...src.info, native, title: `${src.info.title ?? "Conversation"} (fork)`, createdAt: now, updatedAt: now, forkedFrom: thread, cwd: opts.cwd },
      items: structuredClone(items),
      turns: src.turns,
      settings: { ...src.settings, ...(opts.model ? { model: opts.model } : {}), ...(opts.effort ? { effort: opts.effort } : {}) },
      turnLog: [],
    };
    this.save();
    return native;
  }

  async steer(thread: NativeId, turn: TurnId, input: ProviderInput): Promise<void> {
    if (!this.options.steer) throw new Error("fake: this instance cannot steer");
    this.received.push({ thread, input });
    const live = this.live.get(thread);
    if (!live || live.turnId !== turn) throw new Error("fake: that turn is not running");
    live.steered.push(input.text);
  }

  async rename(thread: NativeId, title: string): Promise<void> {
    const t = this.thread(thread);
    t.info.title = title;
    t.info.updatedAt = nowIso();
    this.save();
    this.emit({ type: "threadChanged", info: { ...t.info } });
  }

  async archive(thread: NativeId): Promise<void> {
    const t = this.thread(thread);
    t.info.archived = true;
    this.save();
    this.emit({ type: "threadChanged", info: { ...t.info } });
  }

  async unarchive(thread: NativeId): Promise<void> {
    const t = this.thread(thread);
    t.info.archived = false;
    this.save();
    this.emit({ type: "threadChanged", info: { ...t.info } });
  }

  async remove(thread: NativeId): Promise<void> {
    delete this.store.threads[thread];
    this.save();
  }

  async dispose(): Promise<void> {
    this.disposed = true;
    for (const t of this.timers) clearTimeout(t);
    this.timers.clear();
    for (const live of this.live.values()) live.abort.abort();
  }

  // ── The agent mesh ─────────────────────────────────────────────────────────────────────

  attachMesh(hooks: MeshHooks | null): void {
    this.meshHooks = hooks;
    if (!hooks) this.attachments.clear();
  }

  private attachMeshTo(native: NativeId, cwd: string, phase: "start" | "resume"): void {
    const att = this.meshHooks?.open({ native, cwd, phase });
    if (att) this.attachments.set(native, att);
  }

  /** The attachment a thread's tool calls go through (tests call tools with it directly). */
  meshAttachment(native: NativeId): MeshAttachment | undefined {
    return this.attachments.get(native);
  }

  private vars(native: NativeId): { spawned: string[]; from: string | null } {
    let v = this.meshVars.get(native);
    if (!v) {
      v = { spawned: [], from: null };
      this.meshVars.set(native, v);
    }
    return v;
  }

  /** `$spawned`, `$spawned1`…, `$from` (the last agent that messaged this thread). */
  private substitute(native: NativeId, text: string): string {
    const v = this.vars(native);
    return text
      .replace(/\$spawned(\d+)/g, (_m, n: string) => v.spawned[Number(n) - 1] ?? `$spawned${n}`)
      .replace(/\$spawned\b/g, () => v.spawned.at(-1) ?? "$spawned")
      .replace(/\$from\b/g, () => v.from ?? "$from");
  }

  private meshArgs(native: NativeId, step: Extract<Step, { k: "mesh" }>): Record<string, unknown> {
    const raw = this.substitute(native, step.args);
    const kv = (text: string, bare: (w: string, out: Record<string, unknown>) => void): Record<string, unknown> => {
      const out: Record<string, unknown> = {};
      for (const w of text.split(/\s+/).filter(Boolean)) {
        const eq = w.indexOf("=");
        if (eq > 0) {
          const k = w.slice(0, eq);
          const val = w.slice(eq + 1);
          out[k] = val === "true" ? true : val === "false" ? false : /^\d+$/.test(val) && k !== "cursor" ? Number(val) : val;
        } else bare(w, out);
      }
      return out;
    };
    switch (step.form) {
      case "json":
        return raw ? (JSON.parse(raw) as Record<string, unknown>) : {};
      case "send": {
        const bar = raw.indexOf("|");
        return { to: (bar < 0 ? raw : raw.slice(0, bar)).trim(), text: bar < 0 ? "" : raw.slice(bar + 1).trim() };
      }
      case "spawn": {
        const bar = raw.indexOf("|");
        const head = bar < 0 ? "" : raw.slice(0, bar);
        const text = (bar < 0 ? raw : raw.slice(bar + 1)).trim().replace(/\\n/g, "\n");
        return { ...kv(head, () => {}), text };
      }
      case "positional": {
        const key = step.tool === "find_thread" ? "query" : step.tool === "notify_when_done" ? "to" : "thread";
        return { [key]: raw.trim() };
      }
      case "kv":
        return kv(raw, (w, out) => {
          if (step.tool === "wait") out.threads = [...((out.threads as string[]) ?? []), ...(w === "$all" ? this.vars(native).spawned : [w])];
          else if (step.tool === "list_threads") out.scope = w;
        });
    }
  }

  private async meshCall(native: NativeId, turnId: TurnId, itemId: string, step: Extract<Step, { k: "mesh" }>, signal: AbortSignal): Promise<void> {
    const att = this.attachments.get(native);
    let args: Record<string, unknown> = {};
    let output: string;
    let failed = false;
    const start: Item = { id: itemId, turnId, status: "inProgress", kind: "toolCall", tool: "mcp", title: `yaac.${step.tool}`, input: {}, output: "" };
    try {
      args = this.meshArgs(native, step);
      (start as { input?: unknown }).input = args;
      this.emit({ type: "itemStarted", thread: native, item: start });
      if (!att) throw new Error("the yaac tools are not attached to this thread");
      const r = await att.call({ tool: step.tool, args, meta: { threadId: native }, signal });
      output = r.text;
      failed = r.isError;
      const v = this.vars(native);
      for (const m of r.text.matchAll(/^Address: (\S+)$/gm)) v.spawned.push(m[1]!);
      const froms = [...r.text.matchAll(/\(address (\S+), id /g)];
      if (froms.length > 0) v.from = froms.at(-1)![1]!;
    } catch (e) {
      if (signal.aborted) throw new Aborted();
      output = (e as Error).message;
      failed = true;
      if (!start.input || Object.keys(args).length === 0) this.emit({ type: "itemStarted", thread: native, item: start });
    }
    if (signal.aborted) throw new Aborted();
    const done: Item = { ...start, status: failed ? "failed" : "completed", output } as Item;
    this.record(native, done);
    this.emit({ type: "itemCompleted", thread: native, item: done });
  }

  // ── Test controls ──────────────────────────────────────────────────────────────────────

  /** Marks a conversation as held (or released) by another program. */
  setExternal(native: NativeId, control: ExternalControl): void {
    if (control.external) this.external.set(native, control);
    else this.external.delete(native);
    this.emit({ type: "controlChanged", thread: native, control });
  }

  /** Another program takes the conversation, runs a turn, and lets go after `holdMs`. */
  async runExternalTurn(native: NativeId, text: string, holdMs = 500, extra: Step[] = []): Promise<void> {
    this.setExternal(native, { external: true, holder: "fake terminal" });
    const t = this.thread(native);
    t.turns += 1;
    const turnId = `turn-${t.turns}`;
    const user: Item = { id: `user:${turnId}`, turnId, status: "completed", kind: "userMessage", parts: [{ type: "text", text: "(typed in the terminal)" }] };
    this.record(native, user);
    const live: LiveTurn = { turnId, abort: new AbortController(), steered: [], done: Promise.resolve() };
    this.live.set(native, live);
    // The provider reports the user message of a turn yaacd did not start.
    const run = this.runTurn(native, live, [...extra, { k: "say", text }], "external");
    setImmediate(() => this.emit({ type: "itemStarted", thread: native, item: user }));
    live.done = run;
    await run;
    this.later(holdMs, () => this.setExternal(native, { external: false }));
  }

  /** Someone else (a terminal) answers a pending request. */
  resolveElsewhere(native: NativeId, requestId: string): void {
    const pending = this.requests.get(requestId);
    if (!pending) return;
    const answer: RequestResponse =
      pending.request.requestKind === "approval" ? { requestKind: "approval", decision: "allow" } : { requestKind: "question", answers: { q1: { other: "answered in the terminal" } } };
    this.markRequest(native, requestId, "resolved");
    this.emit({ type: "requestResolved", thread: native, requestId, by: "elsewhere", answer });
    pending.resolve(answer);
  }

  /** The harness drops a pending request without an answer. */
  expireRequest(native: NativeId, requestId: string): void {
    const pending = this.requests.get(requestId);
    if (!pending) return;
    this.markRequest(native, requestId, "expired");
    this.emit({ type: "requestExpired", thread: native, requestId });
    pending.resolve(null);
  }

  /** A conversation appears that was created outside YAAC. */
  addOutsideThread(title: string, originKind: "interactive" | "programmatic" = "interactive"): NativeId {
    const native = this.next("ext-");
    const now = nowIso();
    const info: NativeThreadInfo = { native, title, preview: title, cwd: this.ctx.home, createdAt: now, updatedAt: now, originKind };
    this.store.threads[native] = { info, items: [], turns: 0, settings: { model: "fake-large" }, turnLog: [] };
    this.save();
    this.emit({ type: "threadChanged", info });
    return native;
  }

  /**
   * `n` conversations "created outside YAAC" last active `daysOld` days ago and earlier (one
   * minute apart, newest first), with no event: as a long-used harness has them on disk.
   */
  addOldThreads(n: number, daysOld: number, prefix = "old-"): NativeId[] {
    const out: NativeId[] = [];
    const base = Date.now() - daysOld * 86_400_000;
    for (let i = 0; i < n; i++) {
      const native = `${prefix}${i}`;
      const at = new Date(base - i * 60_000).toISOString();
      this.store.threads[native] = { info: { native, title: `Old conversation ${i}`, preview: "", cwd: this.ctx.home, createdAt: at, updatedAt: at, originKind: "interactive" }, items: [], turns: 0, settings: { model: "fake-large" }, turnLog: [] };
      out.push(native);
    }
    this.save();
    return out;
  }

  /** Pending request ids of a conversation. */
  pendingRequests(): string[] {
    return [...this.requests.keys()];
  }

  /** Every conversation the fake harness stores (tests). */
  nativeIds(): NativeId[] {
    return Object.keys(this.store.threads);
  }

  turnLog(native: NativeId): TurnId[] {
    return [...(this.store.threads[native]?.turnLog ?? [])];
  }
}

/** The factory. Present without configuration only when `YAAC_FAKE_PROVIDER=1`. */
export const fakeProviderFactory: ProviderFactory = {
  kind: FAKE_KIND,
  implicit: (env) => env.YAAC_FAKE_PROVIDER === "1",
  create(ctx: ProviderContext): FakeProvider {
    const env = ctx.config.env ?? {};
    const status = (env.FAKE_STATUS as ProbeResult["status"] | undefined) ?? "ready";
    return new FakeProvider(ctx, {
      nativeFork: env.FAKE_NATIVE_FORK !== "0",
      seed: env.FAKE_SEED !== "0",
      paceMs: Number.parseInt(env.FAKE_PACE_MS ?? "5", 10) || 0,
      status,
      storeDir: env.FAKE_STORE ? path.resolve(env.FAKE_STORE) : path.join(ctx.stateDir, "store"),
      steer: env.FAKE_STEER !== "0",
      forceStop: env.FAKE_FORCE_STOP !== "0",
    });
  },
};

