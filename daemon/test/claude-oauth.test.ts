import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import test from "node:test";
import { claudeAccessToken } from "../src/providers/claude/oauth.js";

test("Claude voice resolves the CLI token dynamically without a YAAC copy", async (t) => {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "yaac-claude-oauth-"));
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  const config = path.join(home, ".claude");
  await fs.mkdir(config);
  const credentials = path.join(config, ".credentials.json");
  await fs.writeFile(credentials, JSON.stringify({ claudeAiOauth: { accessToken: "first", refreshToken: "not-read" } }), { mode: 0o600 });
  assert.equal(await claudeAccessToken({ HOME: home }), "first");

  await fs.writeFile(credentials, JSON.stringify({ unrelated: true, claudeAiOauth: { accessToken: "rotated" } }), { mode: 0o600 });
  assert.equal(await claudeAccessToken({ HOME: home }), "rotated", "each connection rereads Claude Code's source");
  assert.equal(await claudeAccessToken({ HOME: home, CLAUDE_CODE_OAUTH_TOKEN: " injected " }), "injected");
});

test("Claude secure-storage directory follows the CLI override semantics", async (t) => {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "yaac-claude-secure-"));
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  const configured = path.join(home, "configured");
  const secure = path.join(home, "secure");
  await fs.mkdir(configured);
  await fs.mkdir(secure);
  await fs.writeFile(path.join(configured, ".credentials.json"), '{"claudeAiOauth":{"accessToken":"config"}}');
  await fs.writeFile(path.join(secure, ".credentials.json"), '{"claudeAiOauth":{"accessToken":"secure"}}');
  assert.equal(await claudeAccessToken({ HOME: home, CLAUDE_CONFIG_DIR: configured }), "config");
  assert.equal(await claudeAccessToken({ HOME: home, CLAUDE_CONFIG_DIR: configured, CLAUDE_SECURESTORAGE_CONFIG_DIR: secure }), "secure");
  assert.equal(await claudeAccessToken({ HOME: home, CLAUDE_CONFIG_DIR: configured, CLAUDE_SECURESTORAGE_CONFIG_DIR: "" }), null);
});
