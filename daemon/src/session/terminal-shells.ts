import { accessSync, constants, readFileSync, statSync } from "node:fs";
import { userInfo } from "node:os";
import { basename, delimiter, join, win32 } from "node:path";
import type { TerminalShell } from "../protocol/generated/types.js";

interface ShellHost {
  platform: NodeJS.Platform;
  env: NodeJS.ProcessEnv;
  login(): string | null;
  listed(): string[];
  executable(path: string): boolean;
}

const local: ShellHost = {
  platform: process.platform, env: process.env,
  login() { try { return userInfo().shell; } catch { return null; } },
  listed() { try { return readFileSync("/etc/shells", "utf8").split(/\r?\n/).map(line => line.split("#")[0]!.trim()); } catch { return []; } },
  executable(path) { try { accessSync(path, constants.X_OK); return statSync(path).isFile(); } catch { return false; } },
};

export function shells(host: ShellHost = local): TerminalShell[] {
  const windows = host.platform === "win32";
  const pathJoin = windows ? win32.join : join;
  const dirs = (host.env.PATH ?? "").split(windows ? ";" : delimiter).filter(Boolean);
  const found = new Map<string, TerminalShell>();
  function add(path: string | null | undefined, name?: string) {
    if (path && host.executable(path) && !found.has(path)) found.set(path, { path, name: name ?? basename(path) });
  }
  function search(file: string, label?: string) {
    add(dirs.map(dir => pathJoin(dir, file)).find(host.executable), label);
  }
  if (windows) {
    search("pwsh.exe", "PowerShell 7");
    search("powershell.exe", "Windows PowerShell");
    if (host.env.SystemRoot) add(win32.join(host.env.SystemRoot, "System32/WindowsPowerShell/v1.0/powershell.exe"), "Windows PowerShell");
    search("cmd.exe", "Command Prompt");
    add(host.env.ComSpec, "Command Prompt");
    for (const root of [host.env.ProgramFiles, host.env["ProgramFiles(x86)"], host.env.LOCALAPPDATA && win32.join(host.env.LOCALAPPDATA, "Programs")]) {
      if (root) add(win32.join(root, "Git/bin/bash.exe"), "Git Bash");
    }
    const git = dirs.map(dir => pathJoin(dir, "git.exe")).find(host.executable);
    if (git) add(win32.resolve(win32.dirname(git), "../bin/bash.exe"), "Git Bash");
  } else {
    add(host.login());
    add(host.env.SHELL);
    search("bash");
    add("/bin/bash");
    search("sh");
    add("/bin/sh");
    for (const path of host.listed()) if (path.startsWith("/")) add(path);
  }
  return [...found.values()];
}

export function shellArgs(file: string, platform = process.platform): string[] {
  if (platform !== "win32") return ["-l", "-i"];
  switch (win32.basename(file).toLowerCase()) {
    case "cmd.exe": return ["/K", "chcp 65001>nul"];
    case "bash.exe": return ["-l", "-i"];
    default: return ["-NoLogo", "-NoExit", "-Command", "[Console]::InputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)"];
  }
}
