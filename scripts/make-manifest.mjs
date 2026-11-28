#!/usr/bin/env node
// Writes a release's manifest.json: what the release holds, for YAAC's in-app updater
// (docs/install.md#the-manifest).
//
//   node scripts/make-manifest.mjs --version 0.1.0 --build 1005 --tag v0.1.0 \
//        --repo lelemm/yaac --out dist/release/manifest.json \
//        installer=dist/release/YAAC-Setup-0.1.0-build.1005.exe app=dist/release/yaac.exe \
//        daemon=dist/release/yaacd.mjs daemon-mcp=dist/release/yaacd-mcp.mjs \
//        android=dist/release/yaac-android-0.1.0-build.1005.apk \
//        hub-linux-x86_64=dist/release/yaac-hub-linux-x86_64
//
// Each artefact is `<component>=<path>`; its name in the release is the file's base name, its
// download URL https://github.com/<repo>/releases/download/<tag>/<name>. `--released` (an ISO
// time) defaults to now. Run by .github/workflows/release.yml; tested by make-manifest.test.mjs.

import { createHash } from "node:crypto";
import { readFileSync, statSync, writeFileSync } from "node:fs";
import { basename } from "node:path";
import { fileURLToPath } from "node:url";

/** The manifest format. A reader accepts only the schema it knows; fields are only added. */
export const SCHEMA = 1;

/** The components a release may hold. */
export const COMPONENTS = ["installer", "app", "daemon", "daemon-mcp", "android", "hub-linux-x86_64"];

/** A release asset name GitHub keeps as it is (no renaming, nothing to escape in a URL). */
const SAFE_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

/**
 * The manifest for these files. `files` is a list of `{ component, path }`.
 * Pure apart from reading the files.
 */
export function makeManifest({ version, build, tag, repo, released, files }) {
  if (!/^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$/.test(version ?? "")) throw new Error(`--version: not a version: ${version}`);
  const n = Number(build);
  if (!Number.isSafeInteger(n) || n <= 0 || String(n) !== String(build).trim()) throw new Error(`--build: not a build number: ${build}`);
  if (!tag) throw new Error("--tag is required");
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo ?? "")) throw new Error(`--repo: not owner/name: ${repo}`);
  const time = released ?? new Date().toISOString();
  if (Number.isNaN(Date.parse(time))) throw new Error(`--released: not a time: ${time}`);
  if (!files?.length) throw new Error("no artefacts given");
  const seen = new Set();
  const artifacts = files.map(({ component, path }) => {
    if (!COMPONENTS.includes(component)) throw new Error(`unknown component ${component} (one of ${COMPONENTS.join(", ")})`);
    if (seen.has(component)) throw new Error(`${component} given twice`);
    seen.add(component);
    const name = basename(path);
    if (!SAFE_NAME.test(name)) throw new Error(`${name}: a release asset name must be letters, digits, '.', '-' and '_'`);
    const bytes = readFileSync(path);
    return {
      name,
      component,
      // Every artefact of a build carries its build id after its own version (D135).
      version: `${version}+${n}`,
      size: statSync(path).size,
      sha256: createHash("sha256").update(bytes).digest("hex"),
      url: `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${name}`,
    };
  });
  if (!seen.has("installer")) throw new Error("a release needs its installer");
  return { schema: SCHEMA, version, build: n, tag, releasedAt: new Date(time).toISOString(), artifacts };
}

/** `--key value` options and `component=path` artefacts. */
export function parseArgs(argv) {
  const opts = { files: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const key = a.slice(2);
      const value = argv[++i];
      if (value === undefined) throw new Error(`${a} needs a value`);
      opts[key] = value;
    } else {
      const at = a.indexOf("=");
      if (at <= 0) throw new Error(`not component=path: ${a}`);
      opts.files.push({ component: a.slice(0, at), path: a.slice(at + 1) });
    }
  }
  return opts;
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  const manifest = makeManifest(opts);
  const text = `${JSON.stringify(manifest, null, 2)}\n`;
  if (opts.out) writeFileSync(opts.out, text);
  else process.stdout.write(text);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    main();
  } catch (e) {
    console.error(`make-manifest: ${e.message}`);
    process.exit(1);
  }
}
