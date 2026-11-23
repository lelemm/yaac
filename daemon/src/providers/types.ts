// The provider contract inside yaacd (docs/04-providers.md#the-contract).
//
// A provider translates one harness into the YAAC item model. It never sees the wire
// protocol, a client, a journal, or a protocol version: the session layer owns all of that.
// Wire value types (items, descriptors, capabilities, …) come from the generated bindings.

import type {
  AppliedSettings,
  BackendKind,
  BackendOrigin,
  Capability,
  ContextUsage,
  HarnessSubagent,
  DeltaField,
  EffortId,
  InputPart,
  Item,
  ItemId,
  ModeId,
  ModeInfo,
  ModelId,
  ModelInfo,
  OriginKind,
  ProviderInstanceConfig,
  ProviderStatus,
  RequestResponse,
  StoreIdentity,
  ThreadEffective,
  Timestamp,
  TokenUsage,
  TurnId,
  TurnOrigin,
  TurnStatus,
  ApprovalRequest,
  QuestionRequest,
  FormRequest,
  RequestId,
  MeshPermissionRequest,
} from "../protocol/generated/types.js";
import type { EventSource } from "../util/emitter.js";
import type { RateLimitReport } from "./ratelimits.js";

/** Whatever the harness uses to resume that exact conversation (Codex: `Thread.id`). */
export type NativeId = string;

/** The harness may have compacted, but its connection or confirmation was lost. */
export class CompactionUnconfirmed extends Error {}

export interface PageRequest {
  cursor?: string | null;
  /** A hint; providers may return fewer or more. */
  limit?: number;
}

/**
 * A scan for the thread index under the discovery caps (docs/05-threads.md#discovery-is-capped-by-age-and-by-number).
 * With either cap set, a provider lists **newest first by last activity** and may stop early
 * (return no cursor) at the first conversation last active before `activeSince`, or once it
 * has returned `max` conversations of one listing (a provider with several listings, such as
 * active and archived, applies it to each). yaacd applies both caps itself, so a provider may
 * ignore them; it must never return a partial page as complete for another reason.
 */
export interface ScanRequest extends PageRequest {
  activeSince?: Timestamp | null;
  max?: number | null;
  /** A hint for "Search older threads": only conversations whose title contains this. */
  search?: string | null;
}

/** One conversation as the harness lists it. */
export interface NativeThreadInfo {
  native: NativeId;
  title?: string | null;
  preview?: string | null;
  cwd: string;
  model?: ModelId | null;
  createdAt: Timestamp;
  updatedAt: Timestamp;
  /**
   * How it came to exist as far as the harness can tell: `interactive` or `programmatic`.
   * `yaac` is decided by yaacd from its own registry, never by the provider.
   */
  originKind: OriginKind;
  archived?: boolean;
  /** Native id of the conversation it was natively forked from, when the harness says. */
  forkedFrom?: NativeId | null;
  /**
   * Set when this conversation is one of the harness's **own** sub-agents (D151): the native
   * id of the conversation whose agent started it. yaacd lists it under that parent.
   */
  parent?: NativeId | null;
  /** What the harness says of it as a sub-agent; present exactly when `parent` is. */
  subagent?: HarnessSubagent | null;
}

export interface ThreadPage {
  threads: NativeThreadInfo[];
  /** Next page; absent or null on the last page. */
  cursor?: string | null;
  /** False when the harness could not give a complete listing (the instance is then stale). */
  complete?: boolean;
}

/** Items of a conversation's stored history, oldest first. */
export interface ItemPage {
  items: Item[];
  /** Next (newer) page; absent or null on the last page. */
  cursor?: string | null;
}

/** What the probe found. yaacd adds `instance`, `kind`, `name`, `revision`, and the " · name" suffix. */
export interface ProbeResult {
  /** Base display name of the kind ("Codex"). */
  displayName: string;
  backend: BackendKind;
  harnessVersion?: string | null;
  store: StoreIdentity;
  status: ProviderStatus;
  statusMessage?: string | null;
  capabilities: Capability[];
  /** From the harness. Never hardcoded (except inside the fake provider). */
  models: ModelInfo[];
  modelsComplete: boolean;
  modes: ModeInfo[];
  /** When `models` was read from the harness (default: now). */
  modelsReadAt?: Timestamp | null;
  /** For a backend that is a separate process: whether it was found running or started by yaacd. */
  backendOrigin?: BackendOrigin | null;
}

export interface StartThreadParams {
  cwd: string;
  model?: ModelId | null;
  effort?: EffortId | null;
  mode?: ModeId | null;
  /**
   * Fast mode for this turn (D125), resolved by yaacd: `true` only when the model has
   * `fastMode`. The provider maps it to its harness's own option.
   */
  fast?: boolean | null;
}

export interface TurnOverrides {
  model?: ModelId | null;
  effort?: EffortId | null;
  mode?: ModeId | null;
  /**
   * Fast mode (D125). From a client: the choice for this and later turns (absent: the
   * thread's setting). Handed to a provider: resolved by yaacd for this turn — always
   * `true` or `false`, `true` only when the model has `fastMode`.
   */
  fast?: boolean | null;
  /**
   * The working directory for this and later turns (D144). Set by yaacd only for a thread a
   * `thread/move` moved, and only under `threadMoveWorkingDirectory`; a provider that declares
   * that capability applies it (and uses it when it resumes the conversation).
   */
  cwd?: string | null;
}

/**
 * A message as handed to the harness. `parts` are the original input parts (with upload ids);
 * `text` is the text parts joined, with the fixed "Attached files" block appended for file
 * parts (docs/06-attachments.md); `images` are image parts resolved to files on the target.
 */
export interface ProviderInput {
  parts: InputPart[];
  text: string;
  images: Array<{ uploadId: string; path: string; mediaType: string | null }>;
  files: Array<{ uploadId: string; path: string; name: string }>;
  /**
   * Further folders the harness must be able to read for this conversation from now on — a
   * handoff fork's transcript folder. A provider that confines reads (Claude Code) keeps them
   * for the conversation's whole life.
   */
  readable?: string[];
  /**
   * The thread's attachment folder on the target (created, possibly empty): every image and file
   * of this thread, now and later, lies in it. A provider that confines reads and fixes its
   * readable folders when it opens a conversation (Claude Code) makes it readable from the start,
   * so a later attachment needs no re-open (D160).
   */
  uploadsDir?: string;
  /** Who this input comes from; `mesh` input must never be presented as the user speaking. */
  origin: TurnOrigin;
  /**
   * For `mesh` input: the agent messages this turn carries, sender and body as separate
   * fields. `text` already holds them in the fixed frame (docs/10-agent-mesh.md#how-the-agent-sees-it),
   * for harnesses that cannot take agent input in a non-user role.
   */
  mesh?: MeshInputMessage[];
}

/** One agent message handed to a harness (docs/10-agent-mesh.md#how-the-agent-sees-it). */
export interface MeshInputMessage {
  id: string;
  kind: "message" | "result" | "notice" | string;
  /** The sender's address, stamped by yaacd. */
  from: string;
  /** "Codex agent \"Orchestrator\" on local". */
  fromLabel: string;
  text: string;
  /** The first message of a spawned worker: its task from the thread that started it. */
  task?: boolean;
}

// ── The agent mesh: how a provider offers the `yaac` tools to one thread ────────────────

export interface MeshToolCall {
  tool: string;
  args: Record<string, unknown>;
  /** What the harness itself says about the call (Codex: `_meta.threadId`). */
  meta?: Record<string, unknown> | null;
  signal?: AbortSignal;
}

export interface MeshToolResult {
  text: string;
  isError: boolean;
}

/**
 * The `yaac` tools for one thread (docs/10-agent-mesh.md#attaching-the-mcp-server). It carries
 * an attachment ticket: every call made through it acts as the thread the ticket is bound to.
 */
export interface MeshAttachment {
  readonly ticket: string;
  /** The identity block for the harness's instructions for this thread. */
  readonly identity: string;
  /** Tool-call timeout to configure for the server, in seconds. */
  readonly toolTimeoutSec: number;
  /** Binds the ticket to the harness's id for the thread — before its first turn runs. */
  bind(native: NativeId): void;
  /** Calls a tool as this thread (in-process harnesses, and the fake). */
  call(call: MeshToolCall): Promise<MeshToolResult>;
  /** An in-process MCP server (an `McpServer`), for harnesses that host one (Claude Agent SDK). */
  inProcessServer(): unknown;
  /** How to launch the stdio bridge, for harnesses that start MCP servers themselves (Codex). */
  stdio(): { command: string; args: string[]; env: Record<string, string> };
}

/** What the mesh gives a provider instance (`HarnessProvider.attachMesh`). */
export interface MeshHooks {
  /**
   * A thread is being opened: started, resumed, or forked. `native` is null when the harness
   * has not assigned the id yet (then `bind` it as soon as it has). Null: no tools for it.
   */
  open(req: { native: NativeId | null; cwd: string | null; phase: "start" | "resume" | "fork" }): MeshAttachment | null;
}

export interface ForkOptions extends TurnOverrides {
  /** Fork after this turn; the last completed turn when absent. */
  atTurn?: TurnId | null;
  cwd: string;
}

/** Whether something other than yaacd is driving (or has open) a conversation. */
export interface ExternalControl {
  external: boolean;
  /** "a terminal", a program name, … for "In use in …". */
  holder?: string | null;
  /** The provider cannot tell. yaacd treats this as free: only positive evidence makes a thread read-only (D72). */
  uncertain?: boolean;
  /**
   * Stable identities of what holds it (a process, a background session, a turn another
   * client started), for take over (D140): a takeover lasts while every holder is one it was
   * taken from. Absent: the provider cannot name the holder, and a takeover is kept.
   */
  holderIds?: string[];
  /** The holder will not let YAAC continue the thread now (`thread/takeOver` is refused): why. */
  takeOverBlocked?: string | null;
}

/**
 * What became of `trustFolder` (D147). `marked`: the harness now has the folder trusted.
 * `trusted`: it already had. `declined`: the harness's configuration already says something
 * else about the folder (the user marked it untrusted there) — left as it is. `failed`: the
 * harness refused or could not be asked; `uncertain` when the write may have happened anyway
 * (no reply). yaacd never retries it.
 */
export type FolderTrustOutcome =
  | { outcome: "marked" }
  | { outcome: "trusted" }
  | { outcome: "declined"; level: string }
  | { outcome: "failed"; reason: string; uncertain?: boolean };

/** A request the harness raised: an approval, a question, or a form. */
export type RaisedRequest =
  | ({ requestKind: "approval" } & ApprovalRequest)
  | ({ requestKind: "question" } & QuestionRequest)
  | ({ requestKind: "form" } & FormRequest)
  /** Raised by yaacd itself (the agent mesh), never by a harness. */
  | ({ requestKind: "meshPermission" } & MeshPermissionRequest);

/**
 * Normalised events. Every event names the native conversation it belongs to.
 *
 * Rules:
 * - `turnStarted` comes before anything else of a turn. For a turn yaacd asked for
 *   (`startThread`, `startTurn`), the provider must emit it — before or after the call
 *   resolves — with the same turn id the call returns.
 * - Do **not** emit `userMessage` items for input yaacd sent: yaacd journals the original
 *   input parts itself. Emit them only for messages that came from elsewhere.
 * - `requestRaised` must be emitted the moment the harness asks; yaacd journals it before
 *   anything else happens to it. Never answer or time out a request inside the provider.
 * - Item ids are unique within the conversation.
 */
export type ProviderEvent =
  | { type: "turnStarted"; thread: NativeId; turnId: TurnId; origin?: TurnOrigin; at?: Timestamp }
  | {
      type: "turnCompleted";
      thread: NativeId;
      turnId: TurnId;
      status: TurnStatus;
      error?: string | null;
      usage?: TokenUsage | null;
      at?: Timestamp;
    }
  | { type: "itemStarted"; thread: NativeId; item: Item }
  | { type: "itemDelta"; thread: NativeId; itemId: ItemId; field: DeltaField; delta: string }
  | { type: "itemUpdated"; thread: NativeId; item: Item }
  | { type: "itemCompleted"; thread: NativeId; item: Item }
  | {
      type: "requestRaised";
      thread: NativeId;
      turnId: TurnId;
      requestId: RequestId;
      request: RaisedRequest;
      blocking?: boolean;
    }
  | {
      type: "requestResolved";
      thread: NativeId;
      requestId: RequestId;
      /** `client`: the answer yaacd handed over took effect. `elsewhere`: someone else answered. */
      by: "client" | "elsewhere";
      answer?: RequestResponse | null;
    }
  | { type: "requestExpired"; thread: NativeId; requestId: RequestId }
  | { type: "controlChanged"; thread: NativeId; control: ExternalControl }
  | { type: "settingsChanged"; thread: NativeId; applied: AppliedSettings }
  /** The harness refused fast mode for this conversation (e.g. the account cannot have it):
   *  `yaacd` turns the thread's fast setting off (owner, D133). */
  | { type: "fastRefused"; thread: NativeId }
  | { type: "usageUpdated"; thread: NativeId; tokens?: TokenUsage | null; context?: ContextUsage | null }
  /** A conversation appeared or changed in the harness's listing (including outside YAAC). */
  | { type: "threadChanged"; info: NativeThreadInfo }
  /** The harness positively reports a conversation gone. */
  | { type: "threadRemoved"; thread: NativeId }
  /** Anything in the descriptor changed (models, sign-in, …): yaacd re-probes. */
  | { type: "descriptorChanged" }
  /** The account or its rate-limit windows changed (`rateLimits`, D161). */
  | { type: "rateLimitsChanged"; report: RateLimitReport };

/** The harness may have accepted input before its response was lost. Never resend it. */
export class InputOutcomeUnknown extends Error {}

/** The harness explicitly declined peer input under the recipient’s policy. */
export class PeerInputDenied extends Error {}

/** What a provider implements inside yaacd. */
export interface HarnessProvider {
  /** Install, sign-in, backend, models. Called at start, on `provider/refresh`, on `descriptorChanged`. */
  probe(): Promise<ProbeResult>;
  /** One page of the conversation listing, for the thread index (see `ScanRequest`). */
  scanThreads(page: ScanRequest): Promise<ThreadPage>;
  /**
   * One conversation by the harness's id, whether or not it is within the discovery caps
   * (a lookup by id that missed the index). Null: the harness has no such conversation.
   */
  lookupThread?(native: NativeId): Promise<NativeThreadInfo | null>;
  /** One page of a conversation's stored history, oldest first. */
  readThread(thread: NativeId, page: PageRequest): Promise<ItemPage>;

  /** Creates a conversation and sends its first message, as one operation. */
  startThread(params: StartThreadParams, input: ProviderInput): Promise<{ thread: NativeId; turn: TurnId }>;
  /** Starts a turn on an idle conversation yaacd holds. */
  startTurn(thread: NativeId, input: ProviderInput, overrides: TurnOverrides): Promise<TurnId>;
  /**
   * Offer peer input without interrupting the turn. Null means no handoff was made (the
   * mailbox can wait for idle). Call beforeSend synchronously just before handing it over;
   * a false result means the daemon no longer authorizes the handoff.
   */
  deliverPeer?(thread: NativeId, turn: TurnId, input: ProviderInput, beforeSend: () => boolean, beforeFollowOn: () => void): Promise<TurnId | null>;
  /** Runs manual compaction and waits for native confirmation. */
  compact?(thread: NativeId): Promise<void>;
  interrupt(thread: NativeId, turn: TurnId): Promise<void>;
  /** Hands an answer to the harness. Confirm with `requestResolved { by: "client" }`. */
  respond(thread: NativeId, requestId: RequestId, response: RequestResponse): Promise<void>;

  /** Capabilities and settings for one conversation, given its model and current state. */
  effective(thread: NativeId): Promise<ThreadEffective>;
  /** Whether another client or program is currently driving (or has open) this conversation. */
  control(thread: NativeId): Promise<ExternalControl>;
  /**
   * The turn the harness is running in this conversation, if any. Used after a yaacd restart
   * to tell a turn that survived from one that died with the old process. Absent: turns do
   * not survive a yaacd restart.
   */
  runningTurn?(thread: NativeId): Promise<TurnId | null>;

  // Optional — presence must match the declared capabilities.
  forkThread?(thread: NativeId, opts: ForkOptions): Promise<NativeId>;
  steer?(thread: NativeId, turn: TurnId, input: ProviderInput): Promise<void>;
  rename?(thread: NativeId, title: string): Promise<void>;
  archive?(thread: NativeId): Promise<void>;
  /** The harness's own unarchive. */
  unarchive?(thread: NativeId): Promise<void>;
  /**
   * Ends the harness process serving this conversation, for a turn that did not stop when
   * interrupted. Only where yaacd owns that process (not a shared harness daemon).
   */
  forceStop?(thread: NativeId): Promise<void>;
  remove?(thread: NativeId): Promise<void>;
  /**
   * Marks a folder trusted in the harness's own configuration, through an interface the
   * harness supports (`folderTrust`, D147): read first, written only when the harness has no
   * word on the folder yet, never downgraded or removed. Bounded by `timeoutMs`.
   */
  trustFolder?(folder: string, opts: { timeoutMs: number }): Promise<FolderTrustOutcome>;
  /**
   * The agent mesh: offer the `yaac` tools to this instance's threads. A provider that
   * implements it declares the `meshTools` capability once attached.
   */
  attachMesh?(hooks: MeshHooks | null): void;
  /**
   * The account this instance runs as and its rate-limit windows (`rateLimits`, D161), as last
   * reported by the harness; null before anything was. `refresh`: read the harness again when
   * the provider's own interval allows (yaacd already holds five minutes between asks).
   */
  rateLimits?(opts: { refresh: boolean }): Promise<RateLimitReport | null>;

  readonly events: EventSource<ProviderEvent>;
  dispose(): Promise<void>;
}

/** Context yaacd gives a provider instance. */
export interface ProviderContext {
  /**
   * The instance configuration, as the provider should apply it: the stored one, with its
   * harness home (P300) already put in `env` under the kind's own variable.
   */
  config: ProviderInstanceConfig;
  /** A private state directory for this instance, under the yaacd home. */
  stateDir: string;
  /** The yaacd home directory (`~/.yaac`, or `YAAC_HOME`). */
  home: string;
  /**
   * The daemon's configured environment — the base a provider builds a harness's environment
   * from (the instance's own `config.env` goes on top). Providers never read `process.env`.
   */
  env: NodeJS.ProcessEnv;
  /** Diagnostic log. Never message content. */
  log(line: string): void;
}

/** One provider kind. Registered with `ProviderRegistry.registerKind`. */
export interface ProviderFactory {
  /** The open kind string (`codex`, `claude`, `fake`). */
  kind: string;
  /** Whether an implicit default instance of this kind exists without configuration. */
  implicit(env: NodeJS.ProcessEnv): boolean;
  create(ctx: ProviderContext): HarnessProvider;
  /**
   * The kind's harness home (P300), when it has one the user may set: the registry then
   * offers `configurableHome` and maps the instance's `home` to `variable`.
   */
  home?: HarnessHome;
}

/** Where a kind keeps its configuration, sessions and sign-in (P300). */
export interface HarnessHome {
  /** The environment variable that names it (Codex: `CODEX_HOME`). */
  variable: string;
  /** The harness's own default when nothing names it, from the given environment. */
  defaultHome(env: NodeJS.ProcessEnv): string;
  /** The store identity key a home leads to — the same key the provider's probe reports. */
  storeKey(home: string): string;
}
