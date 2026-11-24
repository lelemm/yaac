import { execFile } from "node:child_process";
import * as fs from "node:fs/promises";
import { endianness } from "node:os";
import { basename } from "node:path";
import { promisify } from "node:util";
import type { PreviewPort } from "../protocol/generated/types.js";

const run = promisify(execFile);
const options = { timeout: 5000, maxBuffer: 4 * 1024 * 1024, windowsHide: true };
type Listener = Pick<PreviewPort, "host" | "port">;

function previewAddress(host: string): boolean {
  return host === "0.0.0.0" || host === "::" || host === "::1" || /^127\.\d+\.\d+\.\d+$/.test(host);
}

function internal(command: string, args = ""): boolean {
  return /^(?:yaac|yaacd|yaac-hub)(?:\.exe)?$/i.test(command) ||
    /(?:^|[\\/\s])yaacd\.mjs(?:\s|$)/i.test(args) ||
    (/^codex(?:\.exe)?$/i.test(command) && (!args || /\bapp-server\b/.test(args)));
}

/** Linux prints addresses as machine-endian 32-bit words (also for IPv6). */
export function procListeners(table: string, uid: number): Map<string, Listener> {
  const found = new Map<string, Listener>();
  for (const row of table.split("\n")) {
    const cols = row.trim().split(/\s+/);
    if (cols[3] !== "0A" || Number(cols[7]) !== uid || !cols[9]) continue;
    const [hex, portHex] = cols[1]!.split(":");
    if (!hex || !portHex || !/^(?:[\da-f]{8}|[\da-f]{32})$/i.test(hex)) continue;
    const bytes = Buffer.from(hex, "hex");
    if (endianness() === "LE") bytes.swap32();
    const host = bytes.length === 4 ? [...bytes].join(".") :
      bytes.every((b) => b === 0) ? "::" : bytes.subarray(0, 15).every((b) => b === 0) && bytes[15] === 1 ? "::1" : "";
    if (previewAddress(host)) found.set(cols[9], { host, port: parseInt(portHex, 16) });
  }
  return found;
}

export async function linuxPorts(root: string, uid: number): Promise<PreviewPort[]> {
  const tables = await Promise.all(["tcp", "tcp6"].map((file) => fs.readFile(root + "/net/" + file, "utf8").catch(() => "")));
  const sockets = procListeners(tables.join("\n"), uid);
  const ports: PreviewPort[] = [];
  if (!sockets.size) return ports;
  for (const name of await fs.readdir(root)) {
    if (!/^\d+$/.test(name) || Number(name) === process.pid) continue;
    const dir = root + "/" + name;
    try {
      if ((await fs.stat(dir)).uid !== uid) continue;
      const listeners = new Set<Listener>();
      for (const fd of await fs.readdir(dir + "/fd")) {
        const link = await fs.readlink(dir + "/fd/" + fd).catch(() => "");
        const listener = sockets.get(/^socket:\[(\d+)\]$/.exec(link)?.[1] ?? "");
        if (listener) listeners.add(listener);
      }
      if (!listeners.size) continue;
      const args = (await fs.readFile(dir + "/cmdline", "utf8")).split("\0");
      const command = basename(args[0] ?? "");
      if (internal(command, args.join(" "))) continue;
      const cwd = await fs.readlink(dir + "/cwd").catch(() => undefined);
      for (const listener of listeners) ports.push({ ...listener, pid: Number(name), command, cwd });
    } catch { /* A process may exit while its fds are being read. */ }
  }
  return ports;
}

/** Parse lsof field output. -a -u <uid> has already restricted the process list. */
export function macPorts(output: string, dirs = ""): PreviewPort[] {
  let pid = 0, command = "", wildcard = "0.0.0.0";
  const cwd = new Map<number, string>();
  for (const line of dirs.split("\n")) {
    if (line.startsWith("p")) pid = Number(line.slice(1));
    if (line.startsWith("n")) cwd.set(pid, line.slice(1));
  }
  const ports: PreviewPort[] = [];
  for (const line of output.split("\n")) {
    if (line.startsWith("p")) { pid = Number(line.slice(1)); command = ""; }
    if (line.startsWith("t")) wildcard = line === "tIPv6" ? "::" : "0.0.0.0";
    if (line.startsWith("c")) command = basename(line.slice(1));
    if (line.startsWith("n") && pid !== process.pid && !internal(command)) {
      const endpoint = /^n(?:\[([^\]]+)\]|([^:]+)):(\d+)$/.exec(line);
      if (!endpoint) continue;
      const address = endpoint[1] ?? endpoint[2]!;
      const host = address === "*" ? wildcard : address;
      if (previewAddress(host)) ports.push({ port: Number(endpoint[3]), host, pid, command, cwd: cwd.get(pid) });
    }
  }
  return ports;
}

// Query ownership by SID. Name-only checks mix accounts from different domains.
const windowsQuery = `
$me = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$rows = @(Get-NetTCPConnection -State Listen -ErrorAction Stop | Group-Object OwningProcess | ForEach-Object {
  $proc = Get-CimInstance Win32_Process -Filter ("ProcessId=" + $_.Name)
  if ($proc -and (Invoke-CimMethod -InputObject $proc -MethodName GetOwnerSid).Sid -eq $me) {
    foreach ($ep in $_.Group) {
      [pscustomobject]@{port=$ep.LocalPort;host=$ep.LocalAddress;pid=$proc.ProcessId;command=$proc.Name}
    }
  }
})
ConvertTo-Json -Compress -InputObject $rows
`;

export function windowsPorts(output: string): PreviewPort[] {
  const rows: unknown = JSON.parse(output.replace(/^\uFEFF/, ""));
  if (!Array.isArray(rows)) return [];
  return rows.filter((r) => r && Number.isInteger(r.port) && r.port >= 1 && r.port <= 65535 &&
    Number.isInteger(r.pid) && r.pid !== process.pid && typeof r.command === "string" &&
    !internal(r.command) && typeof r.host === "string" && previewAddress(r.host));
}

export async function discoverPorts(): Promise<PreviewPort[]> {
  let ports: PreviewPort[] = [];
  try {
    if (process.platform === "linux") ports = await linuxPorts("/proc", process.getuid!());
    if (process.platform === "darwin") {
      const uid = String(process.getuid!());
      const listeners = await run("lsof", ["-nP", "-a", "-u", uid, "-iTCP", "-sTCP:LISTEN", "-Fpctn"], options);
      const cwd = await run("lsof", ["-nP", "-a", "-u", uid, "-d", "cwd", "-Fpn"], options).catch(() => ({ stdout: "" }));
      ports = macPorts(listeners.stdout, cwd.stdout);
    }
    if (process.platform === "win32") {
      ports = windowsPorts((await run("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", windowsQuery], options)).stdout);
    }
  } catch { /* Missing OS tooling or permissions leave manual port entry available. */ }
  return ports.sort((a, b) => a.port - b.port || a.host.localeCompare(b.host) || (a.pid ?? 0) - (b.pid ?? 0));
}
