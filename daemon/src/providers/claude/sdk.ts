// The seam between the Claude provider and the outside world: the Claude Agent SDK and the
// `claude` executable. Tests replace both with stubs; nothing else in the provider imports the
// SDK's runtime code.
//
// Only SDK functions and options that are declared in the SDK's own type definitions
// (`@anthropic-ai/claude-agent-sdk/sdk.d.ts`, 0.3.329) are used, with one guarded exception:
// `getSettings()` on a running query (see `SdkQuery.getSettings`). Subagents (D151) are listed
// and read through the SDK (`listSubagents`, `getSubagentMessages`); what the SDK does not
// expose of one — its type, description and whether it finished — is read from the sidecar
// Claude Code writes beside its transcript (`agent-<id>.meta.json`, which the SDK's own
// `importSessionToStore` carries along), best effort: see `subagentFiles`.

import { execFile } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { parentPort, Worker, workerData } from "node:worker_threads";
import { windowsExecutable } from "../windows-executable.js";
import type {
  AccountInfo,
  ForkSessionOptions,
  ForkSessionResult,
  GetSessionInfoOptions,
  GetSessionMessagesOptions,
  GetSubagentMessagesOptions,
  ListSubagentsOptions,
  ListSessionsOptions,
  ModelInfo as SdkModelInfo,
  Options,
  PermissionMode,
  SDKMessage,
  SDKSessionInfo,
  SDKUserMessage,
  SessionMessage,
  SessionMutationOptions,
} from "@anthropic-ai/claude-agent-sdk";

/** What the provider uses of a running `query()`. */
export interface SdkQuery extends AsyncIterable<SDKMessage> {
  // The bundled SDK implements cancelQueued on interrupt() and the CLI advertises it
  // in system/init, but this parameter is missing from the public Query declaration.
  interrupt(options?: { cancelQueued?: boolean }): Promise<unknown>;
  stopTask?(taskId: string): Promise<void>;
  setModel(model?: string): Promise<void>;
  setPermissionMode(mode: PermissionMode): Promise<void>;
  applyFlagSettings(settings: Record<string, unknown>): Promise<void>;
  supportedModels(): Promise<SdkModelInfo[]>;
  accountInfo(): Promise<AccountInfo>;
  initializationResult(): Promise<{ pid?: number; claude_code_version?: string; [k: string]: unknown }>;
  /**
   * Present at runtime on 0.3.329 (it answers the CLI's `get_settings` control request, whose
   * types *are* declared) but not declared on `Query`. Used only to report the model and effort
   * the harness really applied; every caller checks that it exists and tolerates failure.
   */
  getSettings?(): Promise<{ applied?: { model?: string | null; effort?: string | null } }>;
  close(): void;
}

export interface ClaudeSdk {
  query(params: { prompt: AsyncIterable<SDKUserMessage>; options: Options }): SdkQuery;
  dispose?(): void;
  listSessions(options?: ListSessionsOptions): Promise<SDKSessionInfo[]>;
  /** A session's subagent ids (D151): `~/.claude/projects/<dir>/<sessionId>/subagents/…/agent-<id>.jsonl`. */
  listSubagents(sessionId: string, options?: ListSubagentsOptions): Promise<string[]>;
  /** One subagent's stored transcript; each message carries the Agent tool call it runs under. */
  getSubagentMessages(sessionId: string, agentId: string, options?: GetSubagentMessagesOptions): Promise<SessionMessage[]>;
  getSessionInfo(sessionId: string, options?: GetSessionInfoOptions): Promise<SDKSessionInfo | undefined>;
  getSessionMessages(sessionId: string, options?: GetSessionMessagesOptions): Promise<SessionMessage[]>;
  forkSession(sessionId: string, options?: ForkSessionOptions): Promise<ForkSessionResult>;
  renameSession(sessionId: string, title: string, options?: SessionMutationOptions): Promise<void>;
  archiveSession(sessionId: string, options?: SessionMutationOptions): Promise<void>;
  unarchiveSession(sessionId: string, options?: SessionMutationOptions): Promise<void>;
  deleteSession(sessionId: string, options?: SessionMutationOptions): Promise<void>;
}

type StoreApi = Omit<ClaudeSdk, "query" | "dispose">;
type StoreMethod = keyof StoreApi;
type StoreRequest = { id: number; method: StoreMethod; args: unknown[] };
type StoreReply = { id: number } & ({ value: unknown } | { error: string });

function invokeStore(sdk: StoreApi, method: StoreMethod, args: unknown[]): Promise<unknown> {
  const run = sdk[method] as (...args: unknown[]) => Promise<unknown>;
  return run(...args);
}

/**
 * The SDK's on-disk session functions use its process environment. A worker gives a
 * different Claude home its own environment, even while other instances read or write.
 */
class StoreWorker {
  private readonly worker: Worker;
  private readonly pending = new Map<number, { resolve(value: unknown): void; reject(error: unknown): void }>();
  private nextId = 0;
  private closing = false;

  constructor(configDir: string, onExit: () => void) {
    // This URL is sdk.js in tests and yaacd.mjs in the single-file distribution.
    this.worker = new Worker(new URL(import.meta.url), { workerData: { claudeStoreDir: configDir } });
    this.worker.on("message", (reply: StoreReply) => {
      const request = this.pending.get(reply.id);
      if (!request) return;
      this.pending.delete(reply.id);
      if ("error" in reply) request.reject(new Error(reply.error));
      else request.resolve(reply.value);
      this.whenIdle();
    });
    this.worker.on("error", (error) => this.fail(error));
    this.worker.on("exit", (code) => {
      this.fail(new Error(`Claude session worker exited (${code})`));
      onExit();
    });
    this.worker.unref();
  }

  call(method: StoreMethod, args: unknown[]): Promise<unknown> {
    const id = ++this.nextId;
    return new Promise((resolve, reject) => {
      this.worker.ref();
      this.pending.set(id, { resolve, reject });
      try {
        this.worker.postMessage({ id, method, args } satisfies StoreRequest);
      } catch (error) {
        this.pending.delete(id);
        this.whenIdle();
        reject(error);
      }
    });
  }

  close(): void {
    this.closing = true;
    this.whenIdle();
  }

  private whenIdle(): void {
    if (this.pending.size) return;
    if (this.closing) void this.worker.terminate();
    else this.worker.unref();
  }

  private fail(error: Error): void {
    for (const request of this.pending.values()) request.reject(error);
    this.pending.clear();
  }
}

if (parentPort && typeof workerData?.claudeStoreDir === "string") {
  const port = parentPort;
  process.env.CLAUDE_CONFIG_DIR = workerData.claudeStoreDir;
  let sdk: Promise<StoreApi> | undefined;
  port.on("message", async ({ id, method, args }: StoreRequest) => {
    try {
      sdk ??= import("@anthropic-ai/claude-agent-sdk");
      port.postMessage({ id, value: await invokeStore(await sdk, method, args) } satisfies StoreReply);
    } catch (error) {
      port.postMessage({ id, error: error instanceof Error ? error.message : String(error) } satisfies StoreReply);
    }
  });
}

/** Live queries take their own environment. Stored sessions use the probed Claude home. */
export async function loadRealSdk(configDir: () => string): Promise<ClaudeSdk> {
  const sdk = await import("@anthropic-ai/claude-agent-sdk");
  const workers = new Map<string, StoreWorker>();
  let disposed = false;
  async function call<K extends StoreMethod>(method: K, ...args: Parameters<StoreApi[K]>): Promise<Awaited<ReturnType<StoreApi[K]>>> {
    if (disposed) throw new Error("The Claude provider was closed");
    const dir = path.resolve(configDir());
    const daemonDir = path.resolve(process.env.CLAUDE_CONFIG_DIR ?? path.join(os.homedir(), ".claude"));
    if (dir === daemonDir) return await invokeStore(sdk, method, args) as Awaited<ReturnType<StoreApi[K]>>;
    let worker = workers.get(dir);
    if (!worker) {
      worker = new StoreWorker(dir, () => workers.delete(dir));
      workers.set(dir, worker);
    }
    return await worker.call(method, args) as Awaited<ReturnType<StoreApi[K]>>;
  }
  return {
    query: (params) => sdk.query(params) as unknown as SdkQuery,
    dispose: () => {
      disposed = true;
      for (const worker of workers.values()) worker.close();
      workers.clear();
    },
    listSessions: (o) => call("listSessions", o),
    listSubagents: (id, o) => call("listSubagents", id, o),
    getSubagentMessages: (id, agent, o) => call("getSubagentMessages", id, agent, o),
    getSessionInfo: (id, o) => call("getSessionInfo", id, o),
    getSessionMessages: (id, o) => call("getSessionMessages", id, o),
    forkSession: (id, o) => call("forkSession", id, o),
    renameSession: (id, t, o) => call("renameSession", id, t, o),
    archiveSession: (id, o) => call("archiveSession", id, o),
    unarchiveSession: (id, o) => call("unarchiveSession", id, o),
    deleteSession: (id, o) => call("deleteSession", id, o),
  };
}

// ── Subagent files (D151) ────────────────────────────────────────────────────────────────

/** The fields of Claude Code's `agent-<id>.meta.json` sidecar yaacd uses (2.1.330). */
export interface SubagentSidecar {
  agentType?: string;
  description?: string;
  model?: string;
  /** The subagent handed its result back. */
  finished?: boolean;
  /** The user (or its parent) stopped it. */
  stoppedByUser?: boolean;
  /** The Agent tool call that started it, in the parent's transcript. */
  toolUseId?: string;
}

/** One subagent's files on disk. */
export interface SubagentFile {
  transcript: string;
  meta: SubagentSidecar | null;
  /** A workflow's journal says it ended: `completed` or `failed`. */
  outcome?: "completed" | "failed";
  createdMs: number;
  modifiedMs: number;
}

function readJsonFile(p: string): unknown {
  try {
    return JSON.parse(fs.readFileSync(p, "utf8"));
  } catch {
    return null;
  }
}

/**
 * The files of a session's subagents, by agent id: every `agent-<id>.jsonl` under
 * `<configDir>/projects/<any>/<sessionId>/subagents/` (workflow agents sit one folder down,
 * beside the workflow's `journal.jsonl`), with its sidecar and times. Read only; missing or
 * unreadable files are left out.
 */
export function subagentFiles(configDir: string, sessionId: string): Map<string, SubagentFile> {
  const out = new Map<string, SubagentFile>();
  const projects = path.join(configDir, "projects");
  let dirs: string[];
  try {
    dirs = fs.readdirSync(projects);
  } catch {
    return out;
  }
  const walk = (dir: string, depth: number) => {
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    const outcomes = new Map<string, "completed" | "failed">();
    const journal = entries.find((e) => e.isFile() && e.name === "journal.jsonl");
    if (journal) {
      try {
        for (const line of fs.readFileSync(path.join(dir, journal.name), "utf8").split("\n")) {
          if (!line.trim()) continue;
          const e = JSON.parse(line) as { type?: unknown; agentId?: unknown };
          if (typeof e.agentId === "string" && (e.type === "result" || e.type === "failed")) outcomes.set(e.agentId, e.type === "result" ? "completed" : "failed");
        }
      } catch {
        /* a journal being written: what was read stands */
      }
    }
    for (const e of entries) {
      const p = path.join(dir, e.name);
      if (e.isDirectory() && depth < 4) walk(p, depth + 1);
      else if (e.isFile() && e.name.startsWith("agent-") && e.name.endsWith(".jsonl")) {
        const id = e.name.slice(6, -6);
        let st: fs.Stats;
        try {
          st = fs.statSync(p);
        } catch {
          continue;
        }
        const meta = readJsonFile(p.replace(/\.jsonl$/, ".meta.json"));
        const outcome = outcomes.get(id);
        out.set(id, {
          transcript: p,
          meta: meta && typeof meta === "object" ? (meta as SubagentSidecar) : null,
          ...(outcome ? { outcome } : {}),
          createdMs: st.birthtimeMs > 0 ? st.birthtimeMs : st.mtimeMs,
          modifiedMs: st.mtimeMs,
        });
      }
    }
  };
  for (const d of dirs) {
    const sub = path.join(projects, d, sessionId, "subagents");
    if (fs.existsSync(sub)) walk(sub, 0);
  }
  return out;
}

// ── The `claude` executable ──────────────────────────────────────────────────────────────

/** `claude auth status --json`, the fields we use. */
export interface AuthStatus {
  loggedIn: boolean;
  configDirectory?: string | null;
}

/** One entry of `claude agents --json` (interactive entries carry `pid`, background ones `id`). */
export interface AgentEntry {
  sessionId?: unknown;
  pid?: unknown;
  id?: unknown;
  kind?: unknown;
  name?: unknown;
  state?: unknown;
  status?: unknown;
}

export interface ClaudeCli {
  /** The executable's version string, or null when it cannot be run. */
  version(): Promise<string | null>;
  /** Sign-in state, or null when the command failed. */
  authStatus(): Promise<AuthStatus | null>;
  /** Live sessions (interactive and background). Throws when the command fails or is unreadable. */
  agents(): Promise<AgentEntry[]>;
  /** `claude stop <id>`: stops a background session (its conversation is kept). */
  stop?(id: string): Promise<void>;
}

function run(file: string, args: string[], env: NodeJS.ProcessEnv, timeoutMs: number): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(file, args, { env, timeout: timeoutMs, maxBuffer: 8 * 1024 * 1024, windowsHide: true }, (err, stdout) => {
      if (err) reject(err);
      else resolve(String(stdout));
    });
  });
}

export function realCli(executable: string, env: NodeJS.ProcessEnv): ClaudeCli {
  return {
    async version() {
      try {
        const out = (await run(executable, ["--version"], env, 15000)).trim();
        // "2.1.329 (Claude Code)" — keep the first token; used for the descriptor.
        return out.split(/\s+/)[0] || null;
      } catch {
        return null;
      }
    },
    async authStatus() {
      try {
        const parsed = JSON.parse(await run(executable, ["auth", "status", "--json"], env, 15000)) as Record<string, unknown>;
        return {
          loggedIn: parsed.loggedIn === true,
          configDirectory: typeof parsed.configDirectory === "string" ? parsed.configDirectory : null,
        };
      } catch (e) {
        // `auth status` exits non-zero when signed out but still prints its JSON.
        const out = (e as { stdout?: unknown }).stdout;
        if (typeof out === "string" && out.trim().startsWith("{")) {
          try {
            const parsed = JSON.parse(out) as Record<string, unknown>;
            return { loggedIn: parsed.loggedIn === true, configDirectory: typeof parsed.configDirectory === "string" ? parsed.configDirectory : null };
          } catch {
            /* fall through */
          }
        }
        return null;
      }
    },
    async agents() {
      const parsed = JSON.parse(await run(executable, ["agents", "--json"], env, 15000)) as unknown;
      if (!Array.isArray(parsed)) throw new Error("claude agents --json did not print a list");
      return parsed as AgentEntry[];
    },
    async stop(id: string) {
      await run(executable, ["stop", id], env, 15000);
    },
  };
}

/** The name of the instance-configuration (or process) variable that names the executable. */
export const EXECUTABLE_VAR = "YAAC_CLAUDE_EXECUTABLE";

function isExecutableFile(p: string): boolean {
  try {
    const st = fs.statSync(p);
    if (!st.isFile()) return false;
    if (process.platform !== "win32") fs.accessSync(p, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * Finds the installed `claude`: an explicit override (instance configuration, then the
 * environment), then `PATH`, then the installer's usual locations. Returns the real path, so
 * that a launcher symlink that is later repointed does not change what an open query runs.
 */
export function findExecutable(env: NodeJS.ProcessEnv, override?: string | null): string | null {
  const explicit = override ?? env[EXECUTABLE_VAR];
  if (explicit) return isExecutableFile(explicit) ? fs.realpathSync(explicit) : null;
  // Only the given environment's home: an environment without one (a test's) finds nothing there.
  const home = env.HOME ?? env.USERPROFILE;
  const launcherDirs = home ? [path.join(home, ".local", "bin"), path.join(home, ".claude", "local")] : [];
  if (process.platform === "win32") {
    const exe = windowsExecutable(env, "claude.exe", launcherDirs, [`@anthropic-ai/claude-code-win32-${process.arch}`],
      "Claude-Data/claude_*/claude-code/*/*/claude.exe");
    return exe ? fs.realpathSync(exe) : null;
  }
  const dirs = (env.PATH ?? env.Path ?? "").split(path.delimiter).filter(Boolean);
  dirs.push(...launcherDirs);
  for (const dir of dirs) {
    const candidate = path.join(dir, "claude");
    if (isExecutableFile(candidate)) return fs.realpathSync(candidate);
  }
  return null;
}
