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
  HubHostStartResult, HubHostTailscale,
} from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import { ReceiveWindow } from "../protocol/scheduler.js";
import { run, type RunResult } from "../util/exec.js";
import { readJson, writeJsonAtomic } from "../util/fsx.js";
import { connectDaemonSocket } from "./server.js";
import { UPLOAD_MAX_CONCURRENT, UPLOAD_WINDOW } from "./uploads.js";
import {
  HUB_BINARY, HUB_CONFIG, hubPaths, prepareParent, promoteHubFiles, StagedFile, validateTransferFile, validateTransferId,
} from "./hub-host-files.js";

const PORT = 47380;
const BINARY_DOWNLOAD_MS = 5 * 60_000;
const WAIT_FOR_PUT_MS = 30_000;

interface Transfer {
  id: string;
  root: string;
  owner: ConnectionView;
  state: "staging" | "validating" | "armed" | "launching";
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

function canBind(host: string): Promise<boolean> {
  return new Promise((resolve) => {
    const server = net.createServer((socket) => socket.destroy());
    server.once("error", () => resolve(false));
    server.listen({ host, port: PORT, exclusive: true }, () => server.close((error) => resolve(!error)));
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
  private readonly paths: ReturnType<typeof hubPaths>;
  private readonly transfers = new Map<string, Transfer>();
  private readonly streams = new Map<number, { transfer: Transfer; pending: PendingFile; remainingCredit: number }>();
  private launchJob: Promise<void> | null = null;

  constructor(
    private readonly env: NodeJS.ProcessEnv,
    private readonly allocateStream: () => number,
    private readonly log: (line: string) => void,
    private readonly allowHttpForTests = false,
  ) {
    this.paths = hubPaths(env);
  }

  get launching(): boolean { return this.launchJob !== null; }

  waitForLaunch(): Promise<void> {
    return this.launchJob ?? Promise.resolve();
  }

  async probe(): Promise<HubHostProbeResult> {
    const [tailscale, hubRunning] = await Promise.all([this.tailscale(), this.hubRunning()]);
    const portFree = tailscale.ips.length > 0 && (await Promise.all(tailscale.ips.map(canBind))).every(Boolean);
    const lastTransfer = this.lastTransfer();
    return { os: process.platform, arch: os.machine(), tailscale, portFree, hubRunning, ...(lastTransfer && { lastTransfer }) };
  }

  private async tailscale(): Promise<HubHostTailscale> {
    const result = await run("tailscale", ["status", "--json"], { env: this.env, timeoutMs: 4000 });
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

  private async hubRunning(): Promise<boolean> {
    const lock = path.join(this.paths.runtime, "hub.lock");
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
    const socket = path.join(this.paths.runtime, "hub.sock");
    if (!fs.lstatSync(socket, { throwIfNoEntry: false })?.isSocket()) return false;
    const connection = await connectDaemonSocket(socket, 500);
    connection?.destroy();
    return connection !== null;
  }

  private lastTransfer(): HubHostLastTransfer | undefined {
    const candidates: Array<{ file: string; time: number }> = [];
    try {
      for (const entry of fs.readdirSync(this.paths.data, { withFileTypes: true })) {
        if (!entry.isFile() || !/^transfer-[A-Za-z0-9_-]+\.result$/.test(entry.name)) continue;
        const file = path.join(this.paths.data, entry.name);
        const stat = fs.statSync(file, { throwIfNoEntry: false });
        if (stat) candidates.push({ file, time: stat.mtimeMs });
      }
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
    candidates.sort((a, b) => b.time - a.time || b.file.localeCompare(a.file));
    return candidates.length ? readJson<HubHostLastTransfer>(candidates[0]!.file) : undefined;
  }

  private resultPath(opId: string): string {
    return path.join(this.paths.data, `transfer-${opId}.result`);
  }

  private staging(owner: ConnectionView, opId: string): Transfer {
    validateTransferId(opId);
    const existing = this.transfers.get(opId);
    if (existing) {
      if (existing.owner !== owner || existing.state !== "staging") {
        throw new RpcError("hub_transfer_busy", "the hub transfer is in use");
      }
      return existing;
    }
    if (fs.existsSync(this.resultPath(opId))) throw new RpcError("hub_transfer_busy", "the hub transfer already finished");
    const root = path.join(this.paths.data, `transfer-${opId}`);
    prepareParent(this.paths.data, root);
    try {
      fs.mkdirSync(root, { mode: 0o700 });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "EEXIST") {
        throw new RpcError("hub_transfer_busy", "staging already exists for this hub transfer");
      }
      throw error;
    }
    const transfer: Transfer = { id: opId, root, owner, state: "staging", pending: new Map(), files: new Set(), errors: new Map() };
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
    if (url.username || url.password || (url.protocol !== "https:" && !(this.allowHttpForTests && url.protocol === "http:"))) {
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
    validateTransferFile(HUB_BINARY, size, sha256);
    const url = this.downloadUrl(raw);
    const transfer = this.staging(owner, opId);
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

  put(owner: ConnectionView, { opId, name, size, sha256 }: HubHostPutParams): HubHostPutResult {
    validateTransferFile(name, size, sha256);
    const transfer = this.staging(owner, opId);
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

  async start(owner: ConnectionView, { opId, armOnClose }: HubHostStartParams): Promise<HubHostStartResult> {
    validateTransferId(opId);
    if (armOnClose !== true) throw new RpcError("invalid_params", "armOnClose must be true");
    const transfer = this.transfers.get(opId);
    if (!transfer || transfer.owner !== owner) throw new RpcError("hub_transfer_failed", "no staging for this connection");
    if (transfer.state === "armed") return { armed: true };
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
      if (![HUB_CONFIG, HUB_BINARY].every((name) => transfer.files.has(name) && fs.lstatSync(path.join(transfer.root, name), { throwIfNoEntry: false })?.isFile())) {
        throw new RpcError("hub_transfer_failed", "hub binary and config are required before starting");
      }
      if (this.launching || [...this.transfers.values()].some((candidate) => candidate.state === "armed")) {
        throw new RpcError("hub_transfer_busy", "a hub hand-over is already armed");
      }
      transfer.state = "armed";
      return { armed: true };
    } finally {
      if (timer) clearTimeout(timer);
      if (transfer.state === "validating") transfer.state = "staging";
    }
  }

  abort(owner: ConnectionView, { opId }: HubHostAbortParams): void {
    validateTransferId(opId);
    const transfer = this.transfers.get(opId);
    if (!transfer) {
      const root = path.join(this.paths.data, `transfer-${opId}`);
      // Preserve backups if a previous daemon exited after promoting files.
      if (!fs.existsSync(this.resultPath(opId)) && !fs.existsSync(path.join(root, "replaced"))) {
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
    const result: HubHostLastTransfer = { opId: transfer.id, status: "started" };
    try {
      if (await this.hubRunning()) throw new Error("a hub is already running on the target");
      const { paths } = this;
      const bin = path.join(paths.data, HUB_BINARY);
      const env: NodeJS.ProcessEnv = { ...this.env, HOME: paths.home, YAAC_CONFIG_DIR: paths.config, YAAC_DATA_DIR: paths.data, YAAC_RUNTIME_DIR: paths.runtime };
      const options = { env, timeoutMs: 10_000 };
      const systemd = (await run("systemctl", ["--user", "show-environment"], options)).code === 0;
      const serviceName = "yaac-hub.service";
      const logfile = path.join(paths.data, "hub.log");
      const service = systemd ? {
        file: path.join(paths.configHome, "systemd", "user", serviceName),
        content: [
          "[Unit]", "Description=YAAC hub", "[Service]", "Type=exec",
          ...["HOME", "YAAC_CONFIG_DIR", "YAAC_DATA_DIR", "YAAC_RUNTIME_DIR", "YAAC_HOME", "PATH"].flatMap((key) =>
            env[key] ? [`Environment=${unitValue(`${key}=${env[key]}`)}`] : []),
          `ExecStart=:${unitValue(bin)} hub`,
          "[Install]", "WantedBy=default.target", "",
        ].join("\n"),
      } : undefined;
      promoteHubFiles(transfer.root, transfer.files, paths, service);
      if (systemd) {
        checkRun("systemctl --user enable --now", await run("systemctl", ["--user", "enable", "--now", serviceName], options));
      } else {
        const status = await run(bin, ["autostart", "status"], options);
        if (status.code === 0 && /^(on|off)$/.test(status.stdout.trim())) {
          const start = await run(bin, ["autostart", "sync", "--start"], options);
          checkRun("yaac autostart sync --start", start);
          // `sync --start` only starts a hub whose config enables start at login.
          if (start.stdout.trim() === "off") await this.spawnHub(bin, logfile, env);
        } else {
          await this.spawnHub(bin, logfile, env);
        }
      }
    } catch (error) {
      result.status = "failed";
      result.message = messageOf(error);
      this.log(`hub transfer ${transfer.id}: ${result.message}`);
    }
    try {
      writeJsonAtomic(this.resultPath(transfer.id), result);
    } catch (error) {
      this.log(`writing hub transfer result: ${messageOf(error)}`);
    }
  }

  private async spawnHub(bin: string, logfile: string, env: NodeJS.ProcessEnv): Promise<void> {
    const log = fs.openSync(logfile, fs.constants.O_WRONLY | fs.constants.O_APPEND | fs.constants.O_CREAT | fs.constants.O_NOFOLLOW, 0o600);
    try {
      await new Promise<void>((resolve, reject) => {
        const child = spawn(bin, ["hub"], { env, detached: true, stdio: ["ignore", log, log] });
        child.once("error", reject);
        child.once("spawn", () => { child.unref(); resolve(); });
      });
    } finally {
      fs.closeSync(log);
    }
  }
}
