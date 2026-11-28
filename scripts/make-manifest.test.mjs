// node --test scripts/make-manifest.test.mjs   (also run by .github/workflows/release.yml)
import { strict as assert } from "node:assert";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { SCHEMA, makeManifest, parseArgs } from "./make-manifest.mjs";

const here = dirname(fileURLToPath(import.meta.url));

function files() {
  const dir = mkdtempSync(join(tmpdir(), "yaac-manifest-"));
  const put = (name, text) => {
    const path = join(dir, name);
    writeFileSync(path, text);
    return path;
  };
  return {
    dir,
    list: [
      { component: "installer", path: put("YAAC-Setup-0.1.0-build.1005.exe", "setup bytes") },
      { component: "app", path: put("yaac.exe", "yaacexe") },
      { component: "daemon", path: put("yaacd.mjs", "daemon") },
      { component: "daemon-mcp", path: put("yaacd-mcp.mjs", "mcp") },
    ],
  };
}

const base = { version: "0.1.0", build: "1005", tag: "v0.1.0", repo: "lelemm/yaac", released: "2026-11-21T12:00:00Z" };

test("every artefact is listed with its version, size, hash and URL", () => {
  const m = makeManifest({ ...base, files: files().list });
  assert.equal(m.schema, SCHEMA);
  assert.equal(m.build, 1005);
  assert.equal(m.version, "0.1.0");
  assert.equal(m.releasedAt, "2026-11-21T12:00:00.000Z");
  assert.deepEqual(m.artifacts.map((a) => a.component), ["installer", "app", "daemon", "daemon-mcp"]);
  const setup = m.artifacts[0];
  assert.equal(setup.version, "0.1.0+1005");
  assert.equal(setup.size, "setup bytes".length);
  assert.equal(setup.sha256, createHash("sha256").update("setup bytes").digest("hex"));
  assert.equal(setup.url, "https://github.com/lelemm/yaac/releases/download/v0.1.0/YAAC-Setup-0.1.0-build.1005.exe");
});

test("the format matches the sample the Rust updater is tested against", () => {
  const sample = JSON.parse(readFileSync(join(here, "testdata", "manifest.sample.json"), "utf8"));
  const m = makeManifest({ ...base, files: files().list });
  assert.deepEqual(Object.keys(m), Object.keys(sample));
  assert.deepEqual(Object.keys(m.artifacts[0]), Object.keys(sample.artifacts[0]));
  // The same names, components, versions and URLs; only the bytes (size, hash) differ.
  const strip = (a) => ({ ...a, size: 0, sha256: "" });
  assert.deepEqual(m.artifacts.map(strip), sample.artifacts.map(strip));
});

test("the Android app is an artefact of its own", () => {
  const { dir, list } = files();
  const apk = join(dir, "yaac-android-0.1.0-build.1005.apk");
  writeFileSync(apk, "apk bytes");
  const m = makeManifest({ ...base, files: [...list, { component: "android", path: apk }] });
  const a = m.artifacts.find((x) => x.component === "android");
  assert.equal(a.name, "yaac-android-0.1.0-build.1005.apk");
  assert.equal(a.version, "0.1.0+1005");
  assert.equal(a.sha256, createHash("sha256").update("apk bytes").digest("hex"));
  assert.equal(a.url, "https://github.com/lelemm/yaac/releases/download/v0.1.0/yaac-android-0.1.0-build.1005.apk");
});

test("the Linux hub has a downloadable and verifiable entry", () => {
  const { dir, list } = files();
  const path = join(dir, "yaac-hub-linux-x86_64");
  writeFileSync(path, "hub bytes");
  const m = makeManifest({ ...base, files: [...list, { component: "hub-linux-x86_64", path }] });
  const hub = m.artifacts.find((a) => a.name === "yaac-hub-linux-x86_64");
  assert.equal(hub.size, "hub bytes".length);
  assert.equal(hub.sha256, createHash("sha256").update("hub bytes").digest("hex"));
  assert.equal(hub.url, "https://github.com/lelemm/yaac/releases/download/v0.1.0/yaac-hub-linux-x86_64");
});

test("bad input is refused", () => {
  const { list } = files();
  assert.throws(() => makeManifest({ ...base, build: "12a", files: list }), /build number/);
  assert.throws(() => makeManifest({ ...base, build: "0", files: list }), /build number/);
  assert.throws(() => makeManifest({ ...base, version: "one", files: list }), /version/);
  assert.throws(() => makeManifest({ ...base, files: list.slice(1) }), /installer/);
  assert.throws(() => makeManifest({ ...base, files: [...list, list[0]] }), /twice/);
  assert.throws(() => makeManifest({ ...base, files: [{ component: "other", path: list[0].path }] }), /unknown component/);
  const plus = join(files().dir, "YAAC-Setup-0.1.0+1005.exe");
  writeFileSync(plus, "x");
  assert.throws(() => makeManifest({ ...base, files: [{ component: "installer", path: plus }] }), /asset name/);
});

test("the command line writes the file", () => {
  const { dir, list } = files();
  const out = join(dir, "manifest.json");
  const args = ["--version", "0.1.0", "--build", "7", "--tag", "v0.1.0-build.7", "--repo", "lelemm/yaac", "--out", out, ...list.map((f) => `${f.component}=${f.path}`)];
  assert.equal(parseArgs(args).files.length, 4);
  execFileSync(process.execPath, [join(here, "make-manifest.mjs"), ...args]);
  const m = JSON.parse(readFileSync(out, "utf8"));
  assert.equal(m.build, 7);
  assert.equal(m.tag, "v0.1.0-build.7");
  assert.match(m.artifacts[1].url, /\/download\/v0\.1\.0-build\.7\/yaac\.exe$/);
});
