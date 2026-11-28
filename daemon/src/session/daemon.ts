import { terminalUnavailableReason } from "./terminals.js";
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
import { listenExclusive, removePipeKey, sniffConnection, socketPathFor } from "./server.js";
import { Mesh, type MeshOptions } from "../mesh/mesh.js";
import { serveBridge } from "../mesh/mcp.js";
import { Forge, type ForgeOptions } from "../forge/forge.js";
import { PrTools } from "../forge/pr-tools.js";
import { Skills } from "./skills.js";
import { RateLimits } from "./ratelimits.js";
import { HubHost, type HubHostOptions } from "./hub-host.js";

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
  /** The in-use re-check period (P386); tests shorten it. */
  controlRefreshMs?: number;
  /** How long "Interrupt and take over" waits for the holder (P387); tests shorten it. */
  takeOverWaitMs?: number;
  journal?: Partial<JournalOptions>;
  client?: Partial<ClientOptions>;
  tempDir?: string;
  /** Called once the daemon has stopped. The CLI exits the process here. */
  onExit?: (reason: string) => void;
  /** Do not listen on a socket (in-process tests attach streams directly). */
  noSocket?: boolean;
  /** The agent mesh's options (tests shorten its waits). */
  mesh?: Partial<Omit<MeshOptions, "log" | "socketPath" | "home" | "env">>;
  /** The forge's runners (tests stand in a fake `gh` / `az`). */
  forge?: Partial<Pick<ForgeOptions, "runner" | "http" | "now" | "minAskMs" | "debounceMs">>;
  hubHost?: HubHostOptions;
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
  /** Pull requests (D119–D121). */
  readonly forge: Forge;
  readonly rateLimits: RateLimits;
  readonly socketPath: string;
  /** Extra request handlers by method name (for later packages, e.g. the mesh). */
  readonly extraMethods = new Map<string, MethodHandler>();
  private server: net.Server | null = null;
  /** The socket file this daemon bound (device and inode), to remove only that one on stop. */
  private socketId: { dev: number; ino: number } | null = null;
  private connections = new Set<ServerConnection>();
  private localLinks = new Set<net.Socket>();
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
  private readonly hubHost?: HubHost;
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
      ...(config.controlRefreshMs !== undefined ? { controlRefreshMs: config.controlRefreshMs } : {}),
      ...(config.takeOverWaitMs !== undefined ? { takeOverWaitMs: config.takeOverWaitMs } : {}),
    });
    const hubHostPlatform = config.hubHost?.platformForTests ?? process.platform;
    if (hubHostPlatform === "linux" || hubHostPlatform === "win32") {
      this.hubHost = new HubHost(config.env, () => this.core.uploads.allocateStream(), (l) => this.log(l), config.hubHost);
    }
    this.core.activity.on(() => this.checkIdle());
    // The agent mesh (WP7): its tools, mailbox, and the daemon side of the hub's relay.
    this.mesh = new Mesh(this.core, { ...config.mesh, log: (l) => this.log(l), socketPath: this.socketPath, home: config.home, env: config.env });
    for (const [method, handler] of this.mesh.methods()) this.extraMethods.set(method, handler);
    // Pull requests (D119–D121): the forge methods and the agents' PR tools.
    this.forge = new Forge(this.core, { home: config.home, env: config.env, targetName: () => this.mesh.selfName(), log: (l) => this.log(l), ...config.forge });
    for (const [method, handler] of this.forge.methods()) this.extraMethods.set(method, handler);
    this.mesh.prTools = new PrTools(this.forge);
    // Rate limits (D161): each instance's account and usage windows.
    this.rateLimits = new RateLimits(this.core.registry, { broadcast: (m, p) => this.core.broadcast(m, p), log: (l) => this.log(l) });
    for (const [method, handler] of this.rateLimits.methods()) this.extraMethods.set(method, handler);
    // Skills: what an instance offers for a folder.
    for (const [method, handler] of new Skills(this.core.registry).methods()) this.extraMethods.set(method, handler);
  }

  /** Starts a daemon: state, providers, the thread index, and (unless `noSocket`) the socket. */
  static async start(input: DaemonConfig): Promise<Daemon> {
    const config = { ...input, env: input.env ?? process.env, idleTimeoutMs: input.idleTimeoutMs ?? DEFAULT_IDLE_TIMEOUT_MS };
    ensureDir(config.home);
    fs.chmodSync(config.home, 0o700);
    const stateFile = path.join(config.home, "state.json");
    const runDir = path.join(config.home, "run");
    const socket = config.socketPath ?? socketPathFor(config.home, config.env);
    let daemon: Daemon | undefined;
    // Bind IPC before constructing the thread index or writing any shared daemon state.
    const server = config.noSocket ? null : await listenExclusive(socket, runDir, (s) => {
      const serving = daemon;
      if (!serving) {
        s.destroy();
        return;
      }
      s.on("error", () => {});
      serving.localLinks.add(s);
      s.once("close", () => serving.localLinks.delete(s));
      void serving.ready.then(async () => {
        const kind = await sniffConnection(s);
        if (serving.isStopping || !kind) {
          s.destroy();
          return;
        }
        if (kind === "bridge") serveBridge(s, (ticket, call) => serving.mesh.callWithTicket(ticket, call), (l) => serving.log(l));
        else if (kind === "channel") serving.core.channels.attach(s);
        else {
          serving.localLinks.delete(s);
          serving.attach(s);
        }
        s.resume();
      });
    });
    try {
      let state: StateFile | undefined;
      try {
        state = readJson<StateFile>(stateFile);
      } catch {
        state = undefined;
      }
      if (state && state.stateFormat > STATE_FORMAT) {
        throw new Error(`the state in ${config.home} was written by a newer yaacd (state format ${state.stateFormat}); daemons only move forward`);
      }
      daemon = new Daemon(config, state?.clean ?? true);
      daemon.server = server;
      if (server && process.platform !== "win32") daemon.socketId = fileId(socket);
      writeJsonAtomic(stateFile, { stateFormat: STATE_FORMAT, clean: false } satisfies StateFile);
      await daemon.core.start();
      daemon.mesh.start();
      daemon.markReady();
      daemon.log(`serving build ${config.build} on ${socket}`);
      daemon.checkIdle();
      return daemon;
    } catch (error) {
      server?.close();
      throw error;
    }
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
        return new ClientSession(this.core, view, n, {
          diagnostics: () => this.diagnostics(),
          extraMethods: this.extraMethods,
          log: (l) => this.log(l),
          hubHost: this.hubHost,
          hubHostLaunchDone: () => this.checkIdle(),
        }, this.clientOptions);
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

  terminalSupport() { return { unavailableReason: terminalUnavailableReason }; }

  hubHostSupported() { return this.hubHost !== undefined; }

  channelsSupported() { return !this.config.noSocket; }

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
          `running commands: ${this.core.busy().runningCommands}`,
          `threads: ${this.core.allThreads().length}`,
          `hand-over scheduled: ${this.handoverPending ? "yes" : "no"}`,
        ],
      },
    ];
  }

  // ── Idle and stopping ──────────────────────────────────────────────────────────────────

  private checkIdle(): void {
    if (this.stopping) return;
    const b = this.core.busy();
    const workIdle = b.runningTurns === 0 && b.pendingRequests === 0 && b.runningCommands === 0 && !this.hubHost?.launching;
    if (this.handoverPending && workIdle) {
      // Attached clients do not hold a hand-over back.
      void this.stop("handover idle", false);
      return;
    }
    // Registered channels and initialized hosts keep the daemon alive. A liveness probe
    // or connection that never finishes its handshake does not restart the idle period.
    const idle = workIdle && this.hosts.size === 0 && this.core.channels.connected === 0;
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
      const now = this.core.busy();
      if (this.hosts.size > 0 || this.core.channels.connected > 0 || now.runningTurns > 0 || now.pendingRequests > 0 || now.runningCommands > 0 || this.hubHost?.launching) return;
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
      await Promise.all([...this.localLinks].map((socket) => new Promise<void>((resolve) => {
        socket.once("close", resolve);
        socket.destroy();
      })));
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
      await this.hubHost?.waitForLaunch();
      this.mesh.close();
      this.forge.close();
      this.rateLimits.close();
      await this.core.close();
      // Closing a Unix listener can remove its socket before the state is flushed.
      // Keep the pid file as the ownership marker until all writes are complete.
      if (!this.server || this.ownsPidFile()) {
        writeJsonAtomic(path.join(this.config.home, "state.json"), { stateFormat: STATE_FORMAT, clean: true } satisfies StateFile);
        if (this.server) {
          if (process.platform === "win32") removePipeKey(this.socketPath);
          else if (this.ownsSocket()) fs.rmSync(this.socketPath, { force: true });
          fs.rmSync(path.join(this.config.home, "run", "yaacd.pid"), { force: true });
        }
      }
      this.config.onExit?.(reason);
    })();
    return this.stopping;
  }

  private ownsPidFile(): boolean {
    try {
      return fs.readFileSync(path.join(this.config.home, "run", "yaacd.pid"), "utf8").trim() === String(process.pid);
    } catch {
      return false;
    }
  }

  /** Whether the Unix socket file still belongs to this daemon. */
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
