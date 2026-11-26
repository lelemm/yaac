import test from "node:test";
import assert from "node:assert/strict";
import { promises as fs } from "node:fs";
import os from "node:os";
import path from "node:path";
import { Catalog } from "../src/mesh/catalog.js";

test("catalog discovers only permitted skills and stages an immutable local revision once per turn", async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "yaac-catalog-test-"));
  try {
    const automatic = { id: "yaac.theme", description: "Create a theme", revision: "test-1", automatic: true, bundled: false };
    const manual = { id: "yaac.deploy", description: "Deploy a project", revision: "test-1", automatic: false, bundled: false };
    const calls: string[] = [];
    const catalog = new Catalog(async id => {
      if (!id) return { skills: [automatic, manual] };
      calls.push(id);
      return { skill: {
        info: id === automatic.id ? automatic : manual,
        files: [
          { path: "SKILL.md", dataBase64: Buffer.from("Instructions").toString("base64") },
          { path: "references/template.md", dataBase64: Buffer.from(automatic.revision).toString("base64") },
        ],
      } };
    });
    const hint = await catalog.instructions("thread", "Choose colors", true);
    assert.match(hint, /yaac\.theme/);
    assert.doesNotMatch(hint, /yaac\.deploy/);
    assert.deepEqual(await fs.readdir(dir), []);
    await assert.rejects(catalog.load("thread", dir, manual.id, () => true), { code: "policy_denied" });
    await catalog.instructions("thread", "Use yaac.deploy", false);
    await assert.rejects(catalog.load("thread", dir, manual.id, () => true), { code: "policy_denied" });
    await catalog.instructions("thread", "Use yaac.deploy.", true);
    await catalog.load("thread", dir, manual.id, () => true);
    const [first, again] = await Promise.all([
      catalog.load("thread", dir, automatic.id, () => true),
      catalog.load("thread", dir, automatic.id, () => true),
    ]);
    assert.equal(first, again);
    assert.equal(await fs.readFile(first, "utf8"), "Instructions");
    const template = (entry: string) => fs.readFile(path.join(path.dirname(entry), "references/template.md"), "utf8");
    automatic.revision = "test-2";
    assert.equal(await template(await catalog.load("thread", dir, automatic.id, () => true)), "test-1");
    assert.equal(calls.filter(id => id === automatic.id).length, 1);
    catalog.done("thread");
    const next = await catalog.load("thread", dir, automatic.id, () => true);
    assert.notEqual(next, first);
    assert.equal(await template(next), "test-2");
    assert.equal(await template(first), "test-1");
    await assert.rejects(catalog.load("thread", dir, manual.id, () => true), { code: "policy_denied" });
  } finally {
    await fs.rm(dir, { recursive: true, force: true });
  }
});
