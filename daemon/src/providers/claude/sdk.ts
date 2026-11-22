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
import * as path from "node:path";
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
  interrupt(): Promise<unknown>;
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

/** The real SDK, loaded on first use so that yaacd pays for it only when Claude is used. */
export async function loadRealSdk(): Promise<ClaudeSdk> {
  const sdk = await import("@anthropic-ai/claude-agent-sdk");
  return {
    query: (params) => sdk.query(params) as unknown as SdkQuery,
    listSessions: (o) => sdk.listSessions(o),
    listSubagents: (id, o) => sdk.listSubagents(id, o),
    getSubagentMessages: (id, agent, o) => sdk.getSubagentMessages(id, agent, o),
    getSessionInfo: (id, o) => sdk.getSessionInfo(id, o),
    getSessionMessages: (id, o) => sdk.getSessionMessages(id, o),
    forkSession: (id, o) => sdk.forkSession(id, o),
    renameSession: (id, t, o) => sdk.renameSession(id, t, o),
    archiveSession: (id, o) => sdk.archiveSession(id, o),
    unarchiveSession: (id, o) => sdk.unarchiveSession(id, o),
    deleteSession: (id, o) => sdk.deleteSession(id, o),
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
        // "2.1.329 (Claude Code)" — keep the first token; it is only shown, never compared.
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
  const names = process.platform === "win32" ? ["claude.exe", "claude.cmd", "claude"] : ["claude"];
  const dirs = (env.PATH ?? env.Path ?? "").split(path.delimiter).filter(Boolean);
  // Only the given environment's home: an environment without one (a test's) finds nothing.
  const home = env.HOME ?? env.USERPROFILE;
  if (home) dirs.push(path.join(home, ".local", "bin"), path.join(home, ".claude", "local"));
  for (const dir of dirs) {
    for (const name of names) {
      const candidate = path.join(dir, name);
      if (isExecutableFile(candidate)) return fs.realpathSync(candidate);
    }
  }
  return null;
}
