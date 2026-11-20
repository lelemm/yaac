// The seam between the Claude provider and the outside world: the Claude Agent SDK and the
// `claude` executable. Tests replace both with stubs; nothing else in the provider imports the
// SDK's runtime code.
//
// Only SDK functions and options that are declared in the SDK's own type definitions
// (`@anthropic-ai/claude-agent-sdk/sdk.d.ts`, 0.3.329) are used, with one guarded exception:
// `getSettings()` on a running query (see `SdkQuery.getSettings`).

import { execFile } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import type {
  AccountInfo,
  ForkSessionOptions,
  ForkSessionResult,
  GetSessionInfoOptions,
  GetSessionMessagesOptions,
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
    getSessionInfo: (id, o) => sdk.getSessionInfo(id, o),
    getSessionMessages: (id, o) => sdk.getSessionMessages(id, o),
    forkSession: (id, o) => sdk.forkSession(id, o),
    renameSession: (id, t, o) => sdk.renameSession(id, t, o),
    archiveSession: (id, o) => sdk.archiveSession(id, o),
    unarchiveSession: (id, o) => sdk.unarchiveSession(id, o),
    deleteSession: (id, o) => sdk.deleteSession(id, o),
  };
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
