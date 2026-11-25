import { test } from "node:test";
import assert from "node:assert/strict";
import { parseUmask, restoreUserUmask } from "../src/util/umask.js";

test("parseUmask reads what `umask` prints and refuses anything else", () => {
  assert.equal(parseUmask("0022"), 0o022);
  assert.equal(parseUmask("022"), 0o022);
  assert.equal(parseUmask("0077\n"), 0o077);
  assert.equal(parseUmask("0002"), 0o002);
  assert.equal(parseUmask(" 22 "), 0o022, "surrounding whitespace is ignored");
  for (const bad of [undefined, "", "abc", "0088", "-022", "07777", "u=rwx"]) {
    assert.equal(parseUmask(bad), undefined, String(bad));
  }
});

test("restoreUserUmask applies YAAC_USER_UMASK and leaves the umask alone without it", { skip: process.platform === "win32" }, () => {
  const before = process.umask();
  try {
    process.umask(0o077);
    assert.equal(restoreUserUmask({ YAAC_USER_UMASK: "0022" }), 0o022);
    assert.equal(process.umask(), 0o022);
    process.umask(0o077);
    assert.equal(restoreUserUmask({}), undefined);
    assert.equal(process.umask(), 0o077, "no variable: unchanged");
    assert.equal(restoreUserUmask({ YAAC_USER_UMASK: "garbage" }), undefined);
    assert.equal(process.umask(), 0o077, "invalid: unchanged");
  } finally {
    process.umask(before);
  }
});
