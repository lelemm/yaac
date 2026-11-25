// yaacd — the YAAC daemon that runs on each target.
//
//   yaacd serve      the persistent per-user daemon, on a user-only Unix socket
//   yaacd connect    a stdio ⇄ socket bridge; starts the daemon (detached) if none is serving
//   yaacd mcp        the `yaac` MCP server on stdio, for one harness thread (started by the
//                    harness; see src/mesh/bridge.ts)
//   yaacd version    print the build
//
// Environment: YAAC_HOME (state directory, default ~/.yaac), YAAC_SOCKET (socket path),
// YAAC_IDLE_TIMEOUT_MS (idle exit, default 10 minutes; 0 = never), YAAC_FAKE_PROVIDER=1 (an
// implicit fake provider instance), YAAC_MESH_WORKER_POLL_MS (how often remote workers' states
// are asked for).

import { spawn } from "node:child_process";
import { isMainThread } from "node:worker_threads";
// Each subcommand loads only what it needs: `yaacd mcp` runs once per Codex thread and must
// stay small, so the daemon's modules are imported lazily.

declare const __YAAC_BUILD__: string | undefined;
export const BUILD: string = typeof __YAAC_BUILD__ === "string" ? __YAAC_BUILD__ : "0.1.0+dev";

function flag(args: string[], name: string): string | undefined {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
}

async function serve(args: string[]): Promise<void> {
  // Before anything is started: harnesses, terminals and `!` commands get the user's umask,
  // not stage 0's private 077 (yaacd's own directories set their modes explicitly).
  (await import("./util/umask.js")).restoreUserUmask();
  const { Daemon, DEFAULT_IDLE_TIMEOUT_MS, defaultHome } = await import("./session/daemon.js");
  const { AlreadyServing } = await import("./session/server.js");
  const env = process.env;
  const idleRaw = flag(args, "--idle-timeout") ?? env.YAAC_IDLE_TIMEOUT_MS;
  const idleTimeoutMs = idleRaw !== undefined && /^\d+$/.test(idleRaw) ? Number(idleRaw) : DEFAULT_IDLE_TIMEOUT_MS;
  let daemon: import("./session/daemon.js").Daemon;
  try {
    daemon = await Daemon.start({
      home: defaultHome(env),
      build: BUILD,
      env,
      idleTimeoutMs,
      // How often a parent asks other targets for its workers' states (default 10 s).
      ...(env.YAAC_MESH_WORKER_POLL_MS && /^\d+$/.test(env.YAAC_MESH_WORKER_POLL_MS) ? { mesh: { workerPollMs: Number(env.YAAC_MESH_WORKER_POLL_MS) } } : {}),
      onExit: () => process.exit(0),
    });
  } catch (e) {
    if (e instanceof AlreadyServing) {
      process.stderr.write(`yaacd: ${e.message}\n`);
      process.exit(0);
    }
    throw e;
  }
  for (const sig of ["SIGTERM", "SIGINT", "SIGHUP"] as const) {
    process.on(sig, () => void daemon.stop(`signal ${sig}`));
  }
}

async function connect(): Promise<void> {
  const env = process.env;
  const { defaultHome } = await import("./session/daemon.js");
  const { socketPathFor, connectDaemonSocket } = await import("./session/server.js");
  const home = defaultHome(env);
  const socket = socketPathFor(home, env);
  let sock = await connectDaemonSocket(socket);
  if (!sock) {
    // Nothing is serving: start the daemon, detached, so it outlives this bridge.
    const child = spawn(process.execPath, [...process.execArgv, process.argv[1]!, "serve"], {
      detached: true,
      windowsHide: true,
      stdio: "ignore",
      env,
    });
    child.unref();
    const deadline = Date.now() + 15000;
    while (!sock && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, 50));
      sock = await connectDaemonSocket(socket);
    }
    if (!sock) {
      process.stderr.write(`yaacd connect: no daemon is serving on ${socket}\n`);
      process.exit(1);
    }
  }
  const s = sock;
  process.stdin.pipe(s);
  s.pipe(process.stdout);
  process.stdin.on("end", () => s.end());
  s.on("close", () => {
    process.stdout.end(() => process.exit(0));
  });
  s.on("error", () => process.exit(1));
}

async function main(): Promise<void> {
  const [cmd, ...args] = process.argv.slice(2);
  switch (cmd) {
    case "serve":
      await serve(args);
      return;
    case "connect":
      await connect();
      return;
    case "mcp": {
      const { runBridge } = await import("./mesh/bridge.js");
      await runBridge(process.env, [process.argv[1]!]);
      return;
    }
    case "version":
    case "--version":
      process.stdout.write(`${BUILD}\n`);
      return;
    default:
      process.stderr.write("usage: yaacd serve [--idle-timeout <ms>] | yaacd connect | yaacd mcp | yaacd version\n");
      process.exit(2);
  }
}

if (isMainThread) {
  main().catch((e) => {
    process.stderr.write(`yaacd: ${(e as Error)?.stack ?? e}\n`);
    process.exit(1);
  });
} else {
  // The Claude session worker runs from this same bundle in a packaged daemon.
  void import("./providers/claude/sdk.js");
}
