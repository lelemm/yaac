// Bundles yaacd into one self-contained ES module: dist/yaacd.mjs.
import { build } from "esbuild";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
// `<version>+<build id>`: the id is the one number every artefact of a build shares
// (scripts/build-id.sh, passed as YAAC_BUILD_ID); without it this is a development build.
const id = (process.env.YAAC_BUILD_ID ?? "").trim();
const build_id = `${pkg.version}+${/^[0-9]+$/.test(id) ? id : "dev"}`;

await build({
  entryPoints: [join(root, "src/main.ts")],
  outfile: join(root, "dist/yaacd.mjs"),
  bundle: true,
  platform: "node",
  target: "node22",
  format: "esm",
  sourcemap: false,
  legalComments: "none",
  logLevel: "warning",
  // Some dependencies (ws) are CommonJS and call require() for Node built-ins. An ES module
  // has no require, so the bundle defines one; without it the daemon dies at start-up with
  // "Dynamic require of \"events\" is not supported".
  banner: {
    js: [
      "#!/usr/bin/env node",
      'import { createRequire as __yaacCreateRequire } from "node:module";',
      "const require = __yaacCreateRequire(import.meta.url);",
    ].join("\n"),
  },
  define: { __YAAC_BUILD__: JSON.stringify(build_id) },
});

// `yaacd mcp` as its own small bundle: one runs per live Codex thread, so it holds only the
// hand-written MCP loop, the tool definitions, and the socket link (no SDK, no providers).
await build({
  entryPoints: [join(root, "src/mcp-main.ts")],
  outfile: join(root, "dist/yaacd-mcp.mjs"),
  bundle: true,
  platform: "node",
  target: "node22",
  format: "esm",
  sourcemap: false,
  legalComments: "none",
  logLevel: "warning",
  minify: true,
  banner: { js: "#!/usr/bin/env node" },
});
