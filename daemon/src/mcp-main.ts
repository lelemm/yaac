// The `yaacd mcp` bridge as its own minimal bundle (dist/yaacd-mcp.mjs): one runs per live
// Codex thread, so it carries no SDK and no provider code — only the hand-written MCP loop,
// the tool definitions, and the link to the daemon's socket (src/mesh/bridge.ts).

import { runBridge } from "./mesh/bridge.js";

runBridge(process.env, [process.argv[1]!]).catch((e) => {
  process.stderr.write(`yaacd mcp: ${(e as Error)?.message ?? e}\n`);
  process.exit(1);
});
