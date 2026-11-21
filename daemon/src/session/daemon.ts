// `yaacd serve`: the persistent per-user daemon (docs/02-architecture.md#daemon-lifecycle).
//
// It keeps running turns going and journaling while no host is attached, and exits by itself
// only when idle — no client attached, no turn running, no request pending — for the idle
// period, or when asked to hand over (`daemon/handover`).

import * as fs from "node:fs";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import type { Duplex } from "node:stream";
import type { DaemonBusy, HandoverState, HandoverWhen, TargetFacts } from "../protocol/generated/types.js";
import { ServerConnection, type FrozenCoreHost } from "../protocol/connection.js";
import type { ProviderFactory } from "../providers/types.js";
import { fakeProviderFactory } from "../providers/fake/index.js";
import { codexProviderFactory } from "../providers/codex/index.js";
import { claudeProviderFactory } from "../providers/claude/index.js";
import { ensureDir, readJson, writeJsonAtomic } from "../util/fsx.js";
import { Core } from "./core.js";
import { ClientSession, DEFAULT_CLIENT_OPTIONS, type ClientOptions, type MethodHandler } from "./client.js";
import { DEFAULT_JOURNAL_OPTIONS, type JournalOptions } from "./journal.js";
import { listenExclusive, socketPathFor } from "./server.js";
import { Mesh, type MeshOptions } from "../mesh/mesh.js";
import { serveBridge, sniffBridge } from "../mesh/mcp.js";

/** The on-disk state format. Separate from the protocol version and the build. */
export const STATE_FORMAT = 1;

export const DEFAULT_IDLE_TIMEOUT_MS = 10 * 60 * 1000;
/**
 * How long a connection that has not finished `initialize` may hold off an idle exit. A host
 * initializes at once; a connection that never does (a liveness probe, a stuck client) is not
 * a host, and must not keep the daemon up or restart its idle period.
 */
export const HANDSHAKE_GRACE_MS = 10_000;

export interface DaemonConfig {
  home: string;
  build: string;
  env?: NodeJS.ProcessEnv;
  idleTimeoutMs?: number;
  /** Override the socket (default: `socketPathFor(home)`). */
  socketPath?: string;
  /** Provider kinds besides the built-in fake. */
  factories?: ProviderFactory[];
  coalesceMs?: number;
  coalesceBytes?: number;
  journal?: Partial<JournalOptions>;
  client?: Partial<ClientOptions>;
  tempDir?: string;
  /** Called once the daemon has stopped. The CLI exits the process here. */
  onExit?: (reason: string) => void;
  /** Do not listen on a socket (in-process tests attach streams directly). */
  noSocket?: boolean;
  /** The agent mesh's options (tests shorten its waits). */
  mesh?: Partial<Omit<MeshOptions, "log" | "socketPath" | "home" | "env">>;
}

interface StateFile {
  stateFormat: number;
  clean: boolean;
}

export function defaultHome(env: NodeJS.ProcessEnv = process.env): string {
  return path.resolve(env.YAAC_HOME ?? path.join(os.homedir(), ".yaac"));
}

export function targetFacts(): TargetFacts {
  const platform = process.platform === "darwin" ? "macos" : process.platform === "win32" ? "windows" : process.platform;
  let machine: string = process.arch;
  try {
    machine = os.machine();
  } catch {
    /* older runtimes */
  }
  return { os: platform, arch: machine, hostname: os.hostname(), user: os.userInfo().username, home: os.homedir() };
}

export class Daemon implements FrozenCoreHost {
  readonly core: Core;
  /** The agent mesh (WP7). */
  readonly mesh: Mesh;
  readonly socketPath: string;
  /** Extra request handlers by method name (for later packages, e.g. the mesh). */
  readonly extraMethods = new Map<string, MethodHandler>();
  private server: net.Server | null = null;
  /** The socket file this daemon bound (device and inode), to remove only that one on stop. */
  private socketId: { dev: number; ino: number } | null = null;
  private connections = new Set<ServerConnection>();
  /** The connections that finished `initialize`: the hosts, which alone hold off an idle exit. */
  private hosts = new Set<ServerConnection>();
  /** When each connection was accepted (for the handshake grace). */
  private acceptedAt = new WeakMap<ServerConnection, number>();
  private connectionCount = 0;
  private idleTimer: NodeJS.Timeout | null = null;
  private handoverPending = false;
  private stopping: Promise<void> | null = null;
  private readonly logFile: string;
  private readonly startedAt = Date.now();
  private readonly clientOptions: ClientOptions;
  private markReady!: () => void;
  /** Resolves once state is loaded and providers are probed; connections wait for it. */
  readonly ready = new Promise<void>((resolve) => {
    this.markReady = resolve;
  });

  private constructor(
    readonly config: DaemonConfig & { env: NodeJS.ProcessEnv; idleTimeoutMs: number },
    cleanStart: boolean,
  ) {
    ensureDir(config.home);
    ensureDir(path.join(config.home, "logs"));
    this.logFile = path.join(config.home, "logs", "yaacd.log");
    this.socketPath = config.socketPath ?? socketPathFor(config.home, config.env);
    this.clientOptions = { ...DEFAULT_CLIENT_OPTIONS, ...config.client };
    this.core = new Core({
      home: config.home,
      env: config.env,
      factories: [fakeProviderFactory, codexProviderFactory, claudeProviderFactory, ...(config.factories ?? [])],
      log: (l) => this.log(l),
      coalesceMs: config.coalesceMs ?? 25,
      coalesceBytes: config.coalesceBytes ?? 8 * 1024,
      journal: { ...DEFAULT_JOURNAL_OPTIONS, ...config.journal },
      cleanStart,
      ...(config.tempDir ? { tempDir: config.tempDir } : {}),
    });
    this.core.activity.on(() => this.checkIdle());
    // The agent mesh (WP7): its tools, mailbox, and the daemon side of the hub's relay.
    this.mesh = new Mesh(this.core, { ...config.mesh, log: (l) => this.log(l), socketPath: this.socketPath, home: config.home, env: config.env });
    for (const [method, handler] of this.mesh.methods()) this.extraMethods.set(method, handler);
  }

  /** Starts a daemon: state, providers, the thread index, and (unless `noSocket`) the socket. */
  static async start(input: DaemonConfig): Promise<Daemon> {
    const config = { ...input, env: input.env ?? process.env, idleTimeoutMs: input.idleTimeoutMs ?? DEFAULT_IDLE_TIMEOUT_MS };
    ensureDir(config.home);
    fs.chmodSync(config.home, 0o700);
    const stateFile = path.join(config.home, "state.json");
    let state: StateFile | undefined;
    try {
      state = readJson<StateFile>(stateFile);
    } catch {
      state = undefined;
    }
    if (state && state.stateFormat > STATE_FORMAT) {
      throw new Error(`the state in ${config.home} was written by a newer yaacd (state format ${state.stateFormat}); daemons only move forward`);
    }
    const daemon = new Daemon(config, state ? state.clean : true);
    if (!config.noSocket) {
      // The socket is the lock: bind it before touching any state another daemon might own.
      // Connections accepted before start-up finishes wait for `ready`.
      daemon.server = await listenExclusive(daemon.socketPath, path.join(config.home, "run"), (s) => {
        void daemon.ready.then(async () => {
          // A `yaacd mcp` bridge announces itself with a magic no YAAC frame can start with.
          if (await sniffBridge(s)) serveBridge(s, (ticket, call) => daemon.mesh.callWithTicket(ticket, call), (l) => daemon.log(l));
          else daemon.attach(s);
          s.resume();
        });
      });
      daemon.socketId = fileId(daemon.socketPath);
    }
    writeJsonAtomic(stateFile, { stateFormat: STATE_FORMAT, clean: false } satisfies StateFile);
    await daemon.core.start();
    daemon.mesh.start();
    daemon.markReady();
    daemon.log(`serving build ${config.build} on ${daemon.socketPath}`);
    daemon.checkIdle();
    return daemon;
  }

  log(line: string): void {
    try {
      fs.appendFileSync(this.logFile, `${new Date().toISOString()} ${line}\n`, { mode: 0o600 });
    } catch {
      /* logging must never take the daemon down */
    }
  }

  /** Attaches one host connection (a socket, or any duplex stream in tests). */
  attach(stream: Duplex): ServerConnection {
    const n = ++this.connectionCount;
    const conn: ServerConnection = new ServerConnection(
      stream,
      this,
      (view) => {
        // `initialize` succeeded: from now on this connection is a host.
        if (!conn.isClosed) this.hosts.add(conn);
        setImmediate(() => this.checkIdle());
        return new ClientSession(this.core, view, n, { diagnostics: () => this.diagnostics(), extraMethods: this.extraMethods, log: (l) => this.log(l) }, this.clientOptions);
      },
      (l) => this.log(`[conn ${n}] ${l}`),
    );
    this.connections.add(conn);
    this.acceptedAt.set(conn, Date.now());
    conn.onClose(() => {
      this.connections.delete(conn);
      const wasHost = this.hosts.delete(conn);
      // A connection that never initialized neither held the idle period nor ends it.
      if (wasHost) this.checkIdle();
    });
    if (this.stopping) conn.close();
    return conn;
  }

  // ── FrozenCoreHost ─────────────────────────────────────────────────────────────────────

  daemonInfo() {
    return { build: this.config.build, stateFormat: STATE_FORMAT };
  }

  targetFacts(): TargetFacts {
    return targetFacts();
  }

  busy(): DaemonBusy {
    const b = this.core.busy();
    return { runningTurns: b.runningTurns, pendingRequests: b.pendingRequests, attachedClients: this.connections.size };
  }

  handover(when: HandoverWhen): HandoverState {
    if (when === "now" || this.stopping) {
      setImmediate(() => void this.stop("handover now", true));
      return "exiting";
    }
    this.handoverPending = true;
    setImmediate(() => this.checkIdle());
    return "scheduled";
  }

  private diagnostics(): Array<{ title: string; lines: string[] }> {
    const b = this.busy();
    return [
      {
        title: "daemon",
        lines: [
          `build: ${this.config.build}`,
          `state format: ${STATE_FORMAT}`,
          `pid: ${process.pid}`,
          `runtime: node ${process.versions.node}`,
          `up: ${Math.round((Date.now() - this.startedAt) / 1000)} s`,
          `attached clients: ${b.attachedClients}; running turns: ${b.runningTurns}; pending requests: ${b.pendingRequests}`,
          `threads: ${this.core.allThreads().length}`,
          `hand-over scheduled: ${this.handoverPending ? "yes" : "no"}`,
        ],
      },
    ];
  }

  // ── Idle and stopping ──────────────────────────────────────────────────────────────────

  private checkIdle(): void {
    if (this.stopping) return;
    const b = this.busy();
    const workIdle = b.runningTurns === 0 && b.pendingRequests === 0;
    if (this.handoverPending && workIdle) {
      // Attached clients do not hold a hand-over back.
      void this.stop("handover idle", false);
      return;
    }
    // Only hosts count: a connection that has not initialized (a probe, a client mid-handshake)
    // neither holds the daemon nor restarts the idle period. Activity that is not a turn or a
    // request (journal writes, uploads, mesh housekeeping) does not either.
    const idle = workIdle && this.hosts.size === 0;
    if (idle && !this.idleTimer && this.config.idleTimeoutMs > 0) {
      this.armIdle(this.config.idleTimeoutMs);
    } else if (!idle && this.idleTimer) {
      clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
  }

  private armIdle(ms: number): void {
    this.idleTimer = setTimeout(() => {
      this.idleTimer = null;
      if (this.stopping) return;
      const now = this.busy();
      if (this.hosts.size > 0 || now.runningTurns > 0 || now.pendingRequests > 0) return;
      // A connection still within its handshake grace may be a host about to initialize: give it
      // the rest of its grace, never more.
      const youngest = Math.max(0, ...[...this.connections].filter((c) => !this.hosts.has(c)).map((c) => this.acceptedAt.get(c) ?? 0));
      const left = youngest + HANDSHAKE_GRACE_MS - Date.now();
      if (left > 0) return this.armIdle(left);
      void this.stop("idle", false);
    }, ms);
    this.idleTimer.unref?.();
  }

  /** Stops serving. `interrupt`: running turns are interrupted, marked as interrupted by an update. */
  stop(reason: string, interrupt = false): Promise<void> {
    if (this.stopping) return this.stopping;
    this.stopping = (async () => {
      this.log(`stopping: ${reason}`);
      if (this.idleTimer) clearTimeout(this.idleTimer);
      this.server?.close();
      if (interrupt) await this.core.interruptAllForUpdate();
      await Promise.all(
        [...this.connections].map(
          (c) =>
            new Promise<void>((resolve) => {
              if (c.isClosed) return resolve();
              c.onClose(() => resolve());
              c.close();
              setTimeout(() => {
                c.destroy();
                resolve();
              }, 2000).unref?.();
            }),
        ),
      );
      this.mesh.close();
      await this.core.close();
      // Only a socket this process still owns: a newer daemon may already have bound the path
      // (once this one stopped answering), and its socket and pid file must stay.
      if (this.server && this.ownsSocket()) {
        fs.rmSync(this.socketPath, { force: true });
        fs.rmSync(path.join(this.config.home, "run", "yaacd.pid"), { force: true });
      }
      writeJsonAtomic(path.join(this.config.home, "state.json"), { stateFormat: STATE_FORMAT, clean: true } satisfies StateFile);
      this.config.onExit?.(reason);
    })();
    return this.stopping;
  }

  /** Whether the socket path is still the file this daemon bound. */
  ownsSocket(): boolean {
    const now = fileId(this.socketPath);
    return !!now && !!this.socketId && now.dev === this.socketId.dev && now.ino === this.socketId.ino;
  }

  get isStopping(): boolean {
    return this.stopping !== null;
  }
}

function fileId(file: string): { dev: number; ino: number } | null {
  try {
    const st = fs.statSync(file);
    return { dev: st.dev, ino: st.ino };
  } catch {
    return null;
  }
}
