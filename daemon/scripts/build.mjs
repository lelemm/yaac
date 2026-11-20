// Bundles yaacd into one self-contained ES module: dist/yaacd.mjs.
import { build } from "esbuild";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const build_id = process.env.YAAC_BUILD_ID ?? `${pkg.version}+dev`;

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
  banner: { js: "#!/usr/bin/env node" },
  define: { __YAAC_BUILD__: JSON.stringify(build_id) },
});
