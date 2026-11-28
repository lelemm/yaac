// The receiving end of the hub hand-over, on the old hub's existing connection.

import * as fs from "node:fs";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { spawn } from "node:child_process";
import type { ConnectionView } from "../protocol/connection.js";
import { NOTIFICATIONS } from "../protocol/generated/constants.js";
import type {
  HubHostAbortParams, HubHostInstallParams, HubHostInstallResult, HubHostLastTransfer, HubHostProbeResult,
  HubHostProgressParams, HubHostPutCompleteParams, HubHostPutParams, HubHostPutResult, HubHostStartParams,
  HubHostStartResult, HubHostTailscale, HubHostWslService,
} from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import { ReceiveWindow } from "../protocol/scheduler.js";
import { run, type RunResult } from "../util/exec.js";
import { readJson, writeJsonAtomic } from "../util/fsx.js";
import { connectDaemonSocket } from "./server.js";
import { UPLOAD_MAX_CONCURRENT, UPLOAD_WINDOW } from "./uploads.js";
import {
  HUB_BINARY, HUB_CONFIG, hubPaths, prepareParent, promoteHubFiles, StagedFile, validateTransferFile, validateTransferId,
  type HubProfilePaths,
} from "./hub-host-files.js";

const PORT = 47380;
const WSL_INTEROP_MARKER = "/proc/sys/fs/binfmt_misc/WSLInterop";
/** Present when systemd is the system manager (`sd_booted()`). */
const SYSTEMD_MARKER = "/run/systemd/system";
/** What the hub's services (`systemd --user` here, the system unit in WSL) take from `yaacd`'s environment. */
const SERVICE_ENV = ["HOME", "YAAC_CONFIG_DIR", "YAAC_DATA_DIR", "YAAC_RUNTIME_DIR", "YAAC_HOME", "PATH"] as const;
const BINARY_DOWNLOAD_MS = 5 * 60_000;
const WAIT_FOR_PUT_MS = 30_000;
const WINDOWS_TAILSCALE = "C:\\Program Files\\Tailscale\\tailscale.exe";

export interface HubHostOptions {
  /** Allow a fixture server to deliver a hub binary over HTTP. */
  allowHttpForTests?: boolean;
  /** Keep test binds away from the real attach port. */
  probePortForTests?: number;
  /** Make WSL detection independent of the machine running the tests. */
  wslInteropPathForTests?: string;
  /** Exercise the Windows receiver with fixture executables on Linux. */
  platformForTests?: "linux" | "win32";
  /** Make systemd detection independent of the machine running the tests. */
  systemdMarkerForTests?: string;
}

interface WindowsHubInfo {
  exe: string;
  build: string;
  running: boolean;
  paths: HubProfilePaths;
}

interface Transfer {
  id: string;
  root: string;
  paths: HubProfilePaths;
  owner: ConnectionView;
  state: "staging" | "validating" | "armed" | "launching";
  startHub: boolean;
  pending: Map<string, PendingFile>;
  files: Set<string>;
  errors: Map<string, string>;
}

interface PendingFile {
  file: StagedFile;
  settled: Promise<void>;
  resolve: () => void;
  cancel: AbortController;
  window?: ReceiveWindow;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Whether nothing answers on `host:port`. Only connects, never listens: listening on a network
 * address makes Windows Firewall ask the user about Node (seen when merely listing transfer
 * targets), and in WSL with mirrored networking it would reserve the port on Windows too.
 */
function nothingListening(host: string, port: number, timeoutMs = 1500): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = net.connect({ host, port });
    const done = (free: boolean) => { socket.destroy(); resolve(free); };
    socket.setTimeout(timeoutMs, () => done(true));
    socket.once("connect", () => done(false));
    socket.once("error", (e: NodeJS.ErrnoException) => done(e.code === "ECONNREFUSED" || e.code === "EHOSTUNREACH" || e.code === "ENETUNREACH" || e.code === "EADDRNOTAVAIL" || e.code === "ETIMEDOUT"));
  });
}

function checkRun(label: string, result: RunResult): void {
  if (result.code !== 0) throw new Error(`${label}: ${result.stderr.trim() || result.failure || `exit ${result.code}`}`);
}

/** Quote a value for a unit directive, including systemd's specifier expansion. */
function unitValue(value: string): string {
  return `"${value.replace(/[\x00-\x1f\x7f"\\%]/g, (c) => {
    if (c === "%") return "%%";
    if (c === '"' || c === "\\") return "\\" + c;
    return "\\x" + c.charCodeAt(0).toString(16).padStart(2, "0");
  })}"`;
}

export class HubHost {
  private readonly platform: NodeJS.Platform;
  private readonly linuxPaths?: ReturnType<typeof hubPaths>;
  private readingWindowsHub?: Promise<WindowsHubInfo | undefined>;
  private readonly closedOwners = new WeakSet<ConnectionView>();
  private readonly transfers = new Map<string, Transfer>();
  private readonly streams = new Map<number, { transfer: Transfer; pending: PendingFile; remainingCredit: number }>();
  private launchJob: Promise<void> | null = null;

  constructor(
    private readonly env: NodeJS.ProcessEnv,
    private readonly allocateStream: () => number,
    private readonly log: (line: string) => void,
    private readonly options: HubHostOptions = {},
  ) {
    this.platform = options.platformForTests ?? process.platform;
    if (this.platform !== "win32") this.linuxPaths = hubPaths(env);
  }

  get launching(): boolean { return this.launchJob !== null; }

  waitForLaunch(): Promise<void> {
    return this.launchJob ?? Promise.resolve();
  }

  async probe(): Promise<HubHostProbeResult> {
    const [tailscale, hubInfo, linuxRunning] = await Promise.all([
      this.tailscale(),
      this.platform === "win32" ? this.windowsHubInfo() : undefined,
      this.linuxPaths ? this.linuxHubRunning(this.linuxPaths) : false,
    ]);
    const port = this.options.probePortForTests ?? PORT;
    const portFree = tailscale.ips.length > 0 && (await Promise.all(tailscale.ips.map((ip) => nothingListening(ip, port)))).every(Boolean);
    const interop = this.options.wslInteropPathForTests ?? WSL_INTEROP_MARKER;
    const distro = this.linuxPaths && (this.env.WSL_DISTRO_NAME?.trim() || (fs.existsSync(interop) ? "WSL" : undefined));
    const paths = hubInfo?.paths ?? this.linuxPaths;
    const lastTransfer = paths && this.lastTransfer(paths);
    return {
      os: this.platform, arch: os.machine(), tailscale, portFree, hubRunning: hubInfo?.running ?? linuxRunning,
      ...(hubInfo && { hubBuild: hubInfo.build }),
      ...(distro && { wsl: { distro, service: this.wslService(this.linuxPaths!) } }), ...(lastTransfer && { lastTransfer }),
    };
  }

  /**
   * What the PC needs to run a moved hub in this distro as a system service (P416). `yaacd`
   * never has root: the PC installs the unit through `wsl.exe -u root` once this connection
   * has closed. The profile directories are written out, so the service, `yaac hub-bridge` in
   * a WSL session and this `yaacd` all agree on them.
   */
  private wslService(paths: ReturnType<typeof hubPaths>): HubHostWslService {
    const { username: user, uid } = os.userInfo();
    return {
      systemd: fs.existsSync(this.options.systemdMarkerForTests ?? SYSTEMD_MARKER),
      user, uid, dataDir: paths.data, environment: this.serviceEnvironment(paths),
    };
  }

  private serviceEnvironment(paths: ReturnType<typeof hubPaths>): string[] {
    const env: NodeJS.ProcessEnv = { ...this.env, HOME: paths.home, YAAC_CONFIG_DIR: paths.config, YAAC_DATA_DIR: paths.data, YAAC_RUNTIME_DIR: paths.runtime };
    return SERVICE_ENV.flatMap((key) => env[key] ? [`${key}=${env[key]}`] : []);
  }

  private async tailscale(): Promise<HubHostTailscale> {
    const args = ["status", "--json"];
    const options = { env: this.env, timeoutMs: 4000 };
    let result = await run("tailscale", args, options);
    if (result.code !== 0 && this.platform === "win32") result = await run(WINDOWS_TAILSCALE, args, options);
    if (result.code !== 0) return { running: false, ips: [] };
    try {
      // Tailscale uses uint64 user IDs as User map keys. Preserve the raw decimal digits.
      const status = JSON.parse(result.stdout, (key, value, context?: { source: string }) =>
        key === "UserID" && typeof value === "number" ? context?.source ?? String(value) : value);
      const running = status.BackendState === "Running";
      const ips = status.Self?.TailscaleIPs ?? status.TailscaleIPs;
      const login = status.User?.[status.Self?.UserID]?.LoginName;
      return {
        running,
        ips: Array.isArray(ips) ? [...new Set<string>(ips.filter((ip) => typeof ip === "string" && net.isIP(ip)))] : [],
        ...(typeof login === "string" && login.length > 0 && { login }),
      };
    } catch {
      return { running: false, ips: [] };
    }
  }

  /** Share concurrent lookups without caching the running state or the installed build. */
  private windowsHubInfo(): Promise<WindowsHubInfo | undefined> {
    return this.readingWindowsHub ??= this.readWindowsHubInfo().finally(() => { this.readingWindowsHub = undefined; });
  }

  private async readWindowsHubInfo(): Promise<WindowsHubInfo | undefined> {
    const exe = this.env.YAAC_EXE;
    if (!exe) return undefined;
    const result = await run(exe, ["hub-info"], { env: this.env, timeoutMs: 4000 });
    if (result.code !== 0) return undefined;
    try {
      const { build, configDir, dataDir, runtimeDir, running } = JSON.parse(result.stdout);
      if (typeof build !== "string" || !build.trim() || typeof running !== "boolean") return undefined;
      if (![configDir, dataDir, runtimeDir].every((dir) => typeof dir === "string" && dir.length > 0)) return undefined;
      return { exe, build, running, paths: { config: path.resolve(configDir), data: path.resolve(dataDir), runtime: path.resolve(runtimeDir) } };
    } catch {
      return undefined;
    }
  }

  private async linuxHubRunning(paths: HubProfilePaths): Promise<boolean> {
    const lock = path.join(paths.runtime, "hub.lock");
    if (fs.lstatSync(lock, { throwIfNoEntry: false })?.isFile()) {
      // The lock file outlives the hub. Check the OS lock and only use the pid as a fallback.
      const result = await run("flock", ["-n", "-E", "42", lock, "true"], { env: this.env, timeoutMs: 1000 });
      if (result.code === 42) return true;
      if (result.code !== 0) {
        try {
          const pid = fs.readFileSync(lock, "utf8").trim();
          if (/^[1-9][0-9]*$/.test(pid)) {
            const [program, mode] = fs.readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0");
            if (path.basename(program) === "yaac" && mode === "hub") return true;
          }
        } catch { /* stale lock */ }
      }
    }
    const socket = path.join(paths.runtime, "hub.sock");
    if (!fs.lstatSync(socket, { throwIfNoEntry: false })?.isSocket()) return false;
    const connection = await connectDaemonSocket(socket, 500);
    connection?.destroy();
    return connection !== null;
  }

  private lastTransfer(paths: HubProfilePaths): HubHostLastTransfer | undefined {
    const candidates: Array<{ file: string; time: number }> = [];
    try {
      for (const entry of fs.readdirSync(paths.data, { withFileTypes: true })) {
        if (!entry.isFile() || !/^transfer-[A-Za-z0-9_-]+\.result$/.test(entry.name)) continue;
        const file = path.join(paths.data, entry.name);
        const stat = fs.statSync(file, { throwIfNoEntry: false });
        if (stat) candidates.push({ file, time: stat.mtimeMs });
      }
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
    candidates.sort((a, b) => b.time - a.time || b.file.localeCompare(a.file));
    return candidates.length ? readJson<HubHostLastTransfer>(candidates[0]!.file) : undefined;
  }

  private resultPath(paths: HubProfilePaths, opId: string): string {
    return path.join(paths.data, `transfer-${opId}.result`);
  }

  private staging(owner: ConnectionView, opId: string, paths: HubProfilePaths): Transfer {
    if (this.closedOwners.has(owner)) throw new RpcError("hub_transfer_failed", "the transfer connection closed");
    const existing = this.transfers.get(opId);
    if (existing) {
      if (existing.owner !== owner || existing.state !== "staging") {
        throw new RpcError("hub_transfer_busy", "the hub transfer is in use");
      }
      return existing;
    }
    if (fs.existsSync(this.resultPath(paths, opId))) throw new RpcError("hub_transfer_busy", "the hub transfer already finished");
    const root = path.join(paths.data, `transfer-${opId}`);
    prepareParent(paths.data, root);
    try {
      fs.mkdirSync(root, { mode: 0o700 });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "EEXIST") {
        throw new RpcError("hub_transfer_busy", "staging already exists for this hub transfer");
      }
      throw error;
    }
    const transfer: Transfer = { id: opId, root, paths, owner, state: "staging", startHub: true, pending: new Map(), files: new Set(), errors: new Map() };
    this.transfers.set(opId, transfer);
    return transfer;
  }

  private begin(transfer: Transfer, name: string, size: number, sha256: string, window?: ReceiveWindow): PendingFile {
    if (transfer.pending.has(name) || transfer.pending.size >= UPLOAD_MAX_CONCURRENT) {
      throw new RpcError("hub_transfer_busy", "wait for the active hub transfer files to finish");
    }
    const file = new StagedFile(transfer.root, name, size, sha256);
    let resolve!: () => void;
    const settled = new Promise<void>((done) => { resolve = done; });
    const pending = { file, settled, resolve, cancel: new AbortController(), window };
    transfer.pending.set(name, pending);
    return pending;
  }

  private settle(transfer: Transfer, pending: PendingFile, error?: string): void {
    const { name } = pending.file;
    if (transfer.pending.get(name) !== pending) return;
    try {
      pending.file.discard();
    } catch (cleanupError) {
      error = [error, messageOf(cleanupError)].filter(Boolean).join(". ");
    }
    transfer.pending.delete(name);
    if (error) transfer.errors.set(name, error);
    else {
      transfer.files.add(name);
      transfer.errors.delete(name);
    }
    pending.resolve();
    if (pending.window) {
      const { stream } = pending.window;
      this.streams.delete(stream);
      const complete: HubHostPutCompleteParams = { stream, ...(error && { message: error }) };
      transfer.owner.notify(NOTIFICATIONS.HUB_HOST_PUT_COMPLETE, complete);
    }
  }

  private downloadUrl(raw: string, base?: URL): URL {
    const invalid = () => new RpcError("invalid_params", "hub downloads require an HTTPS URL");
    if (typeof raw !== "string") throw invalid();
    let url: URL;
    try { url = new URL(raw, base); } catch { throw invalid(); }
    if (url.username || url.password || (url.protocol !== "https:" && !(this.options.allowHttpForTests && url.protocol === "http:"))) {
      throw invalid();
    }
    return url;
  }

  private async download(url: URL, signal: AbortSignal): Promise<Response> {
    for (let redirects = 0; redirects <= 5; redirects++) {
      const response = await fetch(url, { redirect: "manual", signal });
      if (response.ok && response.body) return response;
      await response.body?.cancel();
      const location = response.headers.get("location");
      if (![301, 302, 303, 307, 308].includes(response.status) || !location) {
        throw new Error(`hub download returned HTTP ${response.status}`);
      }
      url = this.downloadUrl(location, url);
    }
    throw new Error("too many redirects downloading the hub");
  }

  async install(owner: ConnectionView, { opId, url: raw, size, sha256 }: HubHostInstallParams): Promise<HubHostInstallResult> {
    if (!this.linuxPaths) throw new RpcError("hub_transfer_failed", "Windows transfers use the installed YAAC executable");
    validateTransferFile(HUB_BINARY, size, sha256);
    const url = this.downloadUrl(raw);
    validateTransferId(opId);
    const transfer = this.staging(owner, opId, this.linuxPaths);
    const pending = this.begin(transfer, HUB_BINARY, size, sha256);
    const notify = (done: number) => {
      const progress: HubHostProgressParams = { opId, done, total: size };
      owner.notify(NOTIFICATIONS.HUB_HOST_PROGRESS, progress);
    };
    const signal = AbortSignal.any([pending.cancel.signal, AbortSignal.timeout(BINARY_DOWNLOAD_MS)]);
    try {
      notify(0);
      const response = await this.download(url, signal);
      const reader = response.body!.getReader();
      try {
        let last = Date.now();
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          pending.file.write(Buffer.from(value));
          if (Date.now() - last >= 100 && pending.file.received < size) {
            notify(pending.file.received);
            last = Date.now();
          }
        }
        pending.file.finish();
      } finally {
        await reader.cancel().catch(() => {});
      }
      this.settle(transfer, pending);
      notify(size);
      return { path: pending.file.path };
    } catch (error) {
      const message = messageOf(error);
      this.settle(transfer, pending, message);
      throw new RpcError("hub_transfer_failed", message);
    }
  }

  async put(owner: ConnectionView, { opId, name, size, sha256 }: HubHostPutParams): Promise<HubHostPutResult> {
    validateTransferFile(name, size, sha256);
    validateTransferId(opId);
    if (this.platform === "win32") {
      if (name === HUB_BINARY) throw new RpcError("hub_transfer_failed", "Windows transfers use the installed YAAC executable");
      if (name.split("/").some((part) => /\.$|^(?:CON|PRN|AUX|NUL|COM[0-9]|LPT[0-9])(?:\.|$)/i.test(part))) {
        throw new RpcError("invalid_params", "invalid Windows hub transfer file name");
      }
    }
    const paths = this.transfers.get(opId)?.paths ?? this.linuxPaths ?? (await this.windowsHubInfo())?.paths;
    if (!paths) throw new RpcError("hub_transfer_failed", "could not read hub-info from the installed Windows YAAC (YAAC_EXE)");
    const transfer = this.staging(owner, opId, paths);
    const stream = this.allocateStream();
    const window = new ReceiveWindow(stream, Math.min(UPLOAD_WINDOW, size));
    const pending = this.begin(transfer, name, size, sha256, window);
    this.streams.set(stream, { transfer, pending, remainingCredit: size - window.initialCredit() });
    // The reply names the stream. An empty file can finish before there is a data frame.
    if (size === 0) setImmediate(() => this.receive(owner, stream, Buffer.alloc(0)));
    return { stream, credit: window.initialCredit() };
  }

  receive(owner: ConnectionView, stream: number, bytes: Buffer): boolean {
    const streamState = this.streams.get(stream);
    if (!streamState || streamState.transfer.owner !== owner) return false;
    const { transfer, pending } = streamState;
    const { file, window } = pending;
    window!.onReceived(bytes.length);
    try {
      file.write(bytes);
      if (file.received === file.size) {
        file.finish();
        this.settle(transfer, pending);
      } else {
        // Explicitly cap replenishment to the remaining bytes. The shared window's
        // accounting may allow more, but the hub is only granted the declared size.
        const credit = Math.min(streamState.remainingCredit, window!.onConsumed(bytes.length) ?? 0);
        if (credit > 0) {
          streamState.remainingCredit -= credit;
          owner.grantCredit(stream, credit);
        }
      }
    } catch (error) {
      this.settle(transfer, pending, messageOf(error));
    }
    return true;
  }

  async start(owner: ConnectionView, { opId, armOnClose, startHub = true }: HubHostStartParams): Promise<HubHostStartResult> {
    validateTransferId(opId);
    if (armOnClose !== true) throw new RpcError("invalid_params", "armOnClose must be true");
    if (typeof startHub !== "boolean") throw new RpcError("invalid_params", "startHub must be a boolean");
    const transfer = this.transfers.get(opId);
    if (!transfer || transfer.owner !== owner) throw new RpcError("hub_transfer_failed", "no staging for this connection");
    if (transfer.state === "armed" && transfer.startHub === startHub) return { armed: true };
    if (transfer.state !== "staging") throw new RpcError("hub_transfer_busy", "the hub transfer is in use");
    transfer.state = "validating";
    let timer: NodeJS.Timeout | undefined;
    try {
      // Control requests can overtake bulk data. Wait for writes that have already opened.
      await Promise.race([
        Promise.all([...transfer.pending.values()].map((pending) => pending.settled)),
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(new RpcError("hub_transfer_failed", "timed out waiting for hub transfer files")), WAIT_FOR_PUT_MS);
          timer.unref();
        }),
      ]);
      if (this.transfers.get(opId) !== transfer) throw new RpcError("hub_transfer_failed", "the hub transfer was cancelled");
      for (const [name, error] of transfer.errors) throw new RpcError("hub_transfer_failed", `${name}: ${error}`);
      const required = this.linuxPaths ? [HUB_CONFIG, HUB_BINARY] : [HUB_CONFIG];
      if (!required.every((name) => transfer.files.has(name) && fs.lstatSync(path.join(transfer.root, name), { throwIfNoEntry: false })?.isFile())) {
        throw new RpcError("hub_transfer_failed", this.linuxPaths ? "hub binary and config are required before starting" : "hub config is required before starting");
      }
      if (this.launching || [...this.transfers.values()].some((candidate) => candidate.state === "armed")) {
        throw new RpcError("hub_transfer_busy", "a hub hand-over is already armed");
      }
      transfer.startHub = startHub;
      transfer.state = "armed";
      return { armed: true };
    } finally {
      if (timer) clearTimeout(timer);
      if (transfer.state === "validating") transfer.state = "staging";
    }
  }

  async abort(owner: ConnectionView, { opId }: HubHostAbortParams): Promise<void> {
    validateTransferId(opId);
    const paths = this.transfers.get(opId)?.paths ?? this.linuxPaths ?? (await this.windowsHubInfo())?.paths;
    const transfer = this.transfers.get(opId);
    if (!transfer) {
      if (!paths) return;
      const root = path.join(paths.data, `transfer-${opId}`);
      // Preserve backups if a previous daemon exited after promoting files.
      if (!fs.existsSync(this.resultPath(paths, opId)) && !fs.existsSync(path.join(root, "replaced"))) {
        fs.rmSync(root, { recursive: true, force: true });
      }
      return;
    }
    if (transfer.owner !== owner || transfer.state === "launching") {
      throw new RpcError("hub_transfer_busy", "the hub transfer is in use");
    }
    this.discard(transfer);
  }

  private discard(transfer: Transfer): void {
    this.transfers.delete(transfer.id);
    for (const pending of transfer.pending.values()) {
      pending.cancel.abort();
      this.settle(transfer, pending, "the hub transfer was cancelled");
    }
    fs.rmSync(transfer.root, { recursive: true, force: true });
  }

  close(owner: ConnectionView, onLaunchDone: () => void): void {
    this.closedOwners.add(owner);
    for (const transfer of this.transfers.values()) {
      if (transfer.owner !== owner) continue;
      if (transfer.state === "armed") {
        transfer.state = "launching";
        this.launchJob = this.launch(transfer).finally(() => {
          this.transfers.delete(transfer.id);
          this.launchJob = null;
          onLaunchDone();
        });
      } else if (transfer.state !== "launching") {
        try { this.discard(transfer); } catch (error) { this.log(`cleaning up hub transfer: ${messageOf(error)}`); }
      }
    }
  }

  private async launch(transfer: Transfer): Promise<void> {
    const result: HubHostLastTransfer = { opId: transfer.id, status: transfer.startHub ? "started" : "placed" };
    try {
      if (this.linuxPaths) await this.launchLinux(transfer, this.linuxPaths);
      else await this.launchWindows(transfer);
    } catch (error) {
      result.status = "failed";
      result.message = messageOf(error);
      this.log(`hub transfer ${transfer.id}: ${result.message}`);
    }
    try {
      writeJsonAtomic(this.resultPath(transfer.paths, transfer.id), result);
    } catch (error) {
      this.log(`writing hub transfer result: ${messageOf(error)}`);
    }
  }

  private async launchWindows(transfer: Transfer): Promise<void> {
    const info = await this.windowsHubInfo();
    if (!info) throw new Error("could not read hub-info from the installed Windows YAAC (YAAC_EXE)");
    if (info.running) throw new Error("a hub is already running on the target");
    const { paths } = transfer;
    if (info.paths.config !== paths.config || info.paths.data !== paths.data || info.paths.runtime !== paths.runtime) {
      throw new Error("the Windows YAAC hub directories changed during transfer");
    }
    promoteHubFiles(transfer.root, transfer.files, paths, { windows: true });
    if (!transfer.startHub) return;
    const start = await run(info.exe, ["autostart", "sync", "--start"], { env: this.env, timeoutMs: 10_000 });
    checkRun("yaac autostart sync --start", start);
    if (start.stdout.trim() === "off") await this.spawnHub(info.exe, this.env);
  }

  private async launchLinux(transfer: Transfer, paths: ReturnType<typeof hubPaths>): Promise<void> {
    if (await this.linuxHubRunning(paths)) throw new Error("a hub is already running on the target");
    const bin = path.join(paths.data, HUB_BINARY);
    const env: NodeJS.ProcessEnv = { ...this.env, HOME: paths.home, YAAC_CONFIG_DIR: paths.config, YAAC_DATA_DIR: paths.data, YAAC_RUNTIME_DIR: paths.runtime };
    const options = { env, timeoutMs: 10_000 };
    const systemd = transfer.startHub && (await run("systemctl", ["--user", "show-environment"], options)).code === 0;
    const serviceName = "yaac-hub.service";
    const logfile = path.join(paths.data, "hub.log");
    const service = systemd ? {
      autostart: paths.autostart,
      file: path.join(paths.configHome, "systemd", "user", serviceName),
      content: [
        "[Unit]", "Description=YAAC hub", "[Service]", "Type=exec",
        ...this.serviceEnvironment(paths).map((entry) => `Environment=${unitValue(entry)}`),
        `ExecStart=:${unitValue(bin)} hub`,
        "[Install]", "WantedBy=default.target", "",
      ].join("\n"),
    } : undefined;
    promoteHubFiles(transfer.root, transfer.files, paths, { service });
    if (systemd) {
      checkRun("systemctl --user enable --now", await run("systemctl", ["--user", "enable", "--now", serviceName], options));
    } else if (transfer.startHub) {
      const status = await run(bin, ["autostart", "status"], options);
      if (status.code === 0 && /^(on|off)$/.test(status.stdout.trim())) {
        const start = await run(bin, ["autostart", "sync", "--start"], options);
        checkRun("yaac autostart sync --start", start);
        // `sync --start` only starts a hub whose config enables start at login.
        if (start.stdout.trim() === "off") await this.spawnHub(bin, env, logfile);
      } else {
        await this.spawnHub(bin, env, logfile);
      }
    }
  }

  private async spawnHub(bin: string, env: NodeJS.ProcessEnv, logfile?: string): Promise<void> {
    const log = logfile === undefined ? undefined :
      fs.openSync(logfile, fs.constants.O_WRONLY | fs.constants.O_APPEND | fs.constants.O_CREAT | fs.constants.O_NOFOLLOW, 0o600);
    try {
      await new Promise<void>((resolve, reject) => {
        const child = spawn(bin, ["hub"], {
          env, detached: true, windowsHide: this.platform === "win32", stdio: log === undefined ? "ignore" : ["ignore", log, log],
        });
        child.once("error", reject);
        child.once("spawn", () => { child.unref(); resolve(); });
      });
    } finally {
      if (log !== undefined) fs.closeSync(log);
    }
  }
}
