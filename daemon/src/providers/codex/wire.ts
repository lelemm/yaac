// The subset of the Codex app-server protocol this provider uses, written by hand from the
// bindings Codex emits itself (`codex app-server generate-ts [--experimental]`, daemon 0.171.0).
//
// These are deliberately loose: the daemon can be newer than this file, so every object may
// carry fields not listed here, enumerations may grow, and unknown methods and notifications
// are tolerated by the adapter. Nothing here names a model or an effort level.

export type JsonValue = unknown;

export interface InitializeResponse {
  userAgent?: string;
  /** Absolute path to the server's `$CODEX_HOME`. */
  codexHome?: string;
  platformFamily?: string;
  platformOs?: string;
}

export interface ReasoningEffortOption {
  reasoningEffort: string;
  description?: string | null;
}

export interface Model {
  id: string;
  model?: string;
  displayName?: string;
  description?: string | null;
  hidden?: boolean;
  isDefault?: boolean;
  supportedReasoningEfforts?: ReasoningEffortOption[];
  defaultReasoningEffort?: string | null;
  /** `text`, `image`, `audio`, … — open. */
  inputModalities?: string[];
  /** The service tiers this model can run on (`{ id, name, description }`), e.g. Fast. */
  serviceTiers?: ModelServiceTier[];
  /** Deprecated by Codex in favour of `serviceTiers`: the extra speed tiers' names. */
  additionalSpeedTiers?: string[];
}

export interface ModelServiceTier {
  id: string;
  name?: string;
  description?: string;
}

export interface Paged<T> {
  data: T[];
  nextCursor?: string | null;
}

export type Account =
  | { type: "chatgpt"; email?: string | null; planType?: string | null; workspaceName?: string | null }
  | { type: string; [k: string]: unknown };

export interface GetAccountResponse {
  account: Account | null;
  requiresOpenaiAuth?: boolean;
}

export type ThreadStatus =
  | { type: "notLoaded" }
  | { type: "idle" }
  | { type: "systemError" }
  | { type: "active"; activeFlags?: string[] }
  | { type: string };

export type SessionSource = string | { [k: string]: unknown };

export interface Thread {
  id: string;
  ephemeral?: boolean;
  threadSource?: string | null;
  sessionId?: string;
  forkedFromId?: string | null;
  parentThreadId?: string | null;
  preview?: string;
  model?: string | null;
  reasoningEffort?: string | null;
  createdAt?: number;
  updatedAt?: number;
  status?: ThreadStatus;
  cwd?: string;
  originator?: string | null;
  source?: SessionSource;
  /** A sub-agent's nickname and role (`spawn_agent`, D151; 0.170.0). */
  agentNickname?: string | null;
  agentRole?: string | null;
  name?: string | null;
  turns?: Turn[];
}

export interface TurnError {
  message?: string;
  additionalDetails?: string | null;
}

export interface Turn {
  id: string;
  items?: ThreadItem[];
  status?: "completed" | "interrupted" | "failed" | "inProgress" | string;
  error?: TurnError | null;
  startedAt?: number | null;
  completedAt?: number | null;
}

/** A thread item. Flat on the wire, discriminated by `type`; open. */
export interface ThreadItem {
  type: string;
  id: string;
  [k: string]: unknown;
}

export interface ThreadItemEntry {
  turnId: string;
  item: ThreadItem;
}

export interface TokenUsageBreakdown {
  totalTokens?: number;
  inputTokens?: number;
  cachedInputTokens?: number;
  outputTokens?: number;
  reasoningOutputTokens?: number;
}

export interface ThreadTokenUsage {
  total?: TokenUsageBreakdown;
  last?: TokenUsageBreakdown;
  modelContextWindow?: number | null;
}

export type UserInput =
  | { type: "text"; text: string; text_elements: unknown[] }
  | { type: "localImage"; path: string }
  | { type: "skill"; name: string; path: string };

/** A JSON-RPC id as Codex uses it: a number or a string. */
export type RpcId = number | string;

export interface RpcMessage {
  id?: RpcId;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: { code?: number; message?: string; data?: unknown };
}

/** The source kinds `thread/list` accepts (0.171.0). Sent explicitly; see `scanThreads`. */
export const THREAD_SOURCE_KINDS = [
  "cli",
  "vscode",
  "exec",
  "appServer",
  "subAgent",
  "subAgentReview",
  "subAgentCompact",
  "subAgentThreadSpawn",
  "subAgentOther",
  "unknown",
] as const;

/** Source kinds that mean "a person in a Codex interface". Everything else is programmatic. */
export const INTERACTIVE_SOURCES = new Set(["cli", "vscode"]);
