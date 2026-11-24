import assert from "node:assert/strict";
import { rmSync } from "node:fs";
import { test } from "node:test";
import { guideRecommendations, listForAgents, ModelBestUses, modelUseId } from "../src/providers/model-best-use.js";
import type { ModelInfo } from "../src/protocol/generated/types.js";
import { tmpHome } from "./helpers.js";

const model = (id: string, displayName: string, resolvedModel?: string): ModelInfo =>
  ({ id, displayName, ...(resolvedModel ? { resolvedModel } : {}) });

test("model guidance extracts named recommendations and caps them at 15 words", () => {
  const guide = guideRecommendations(`
- For everyday API work, start with **GPT-7 Cedar** at medium effort.
- Choose **GPT-7 Birch** for demanding technical analysis and debugging.
1. **GPT-7 Cedar · Medium**
   Fixes small bugs and writes tests for existing behavior.
| When you need | Consider starting with | Examples |
| --- | --- | --- |
| Fast results | Claude Pine 8.1 | Quickly summarize ordinary reports, draft simple responses, and classify large batches of support tickets using clear rules and labels |
`);
  assert.equal(guide.get("gpt7cedar"), "Fixes small bugs and writes tests for existing behavior");
  assert.equal(guide.get("gpt7birch"), "Demanding technical analysis and debugging");
  assert.equal(guide.get("claudepine81")?.split(" ").length, 15);
  assert.equal(guide.get("gpt7spruce"), undefined);
});

test("the agent listing filters retired GPT and Claude releases", () => {
  for (const id of ["gpt-5", "gpt-5.5-codex", "gpt-4o", "gpt-3.5-turbo", "openai/gpt-5.2"]) {
    assert.equal(listForAgents("codex", model(id, id)), false, id);
    assert.equal(listForAgents("claude", model(id, id)), true, id);
  }
  for (const id of ["gpt-6", "gpt-6.2-sol", "gpt-10", "o3"]) {
    assert.equal(listForAgents("codex", model(id, id)), true, id);
  }
  for (const retired of [
    model("claude-sonnet-5", "Sonnet 5"),
    model("claude-opus-5-20261120", "Opus 5"),
    model("fable", "Fable", "claude-fable-5"),
    model("default", "Default (Opus 4.8)"),
    model("claude-haiku-4-5", "Haiku 4.5"),
    model("claude-sonnet-4", "Sonnet 4"),
  ]) {
    assert.equal(listForAgents("claude", retired), false, retired.id);
  }
  for (const available of [
    model("sonnet", "Sonnet 5", "claude-sonnet-5-5"),
    model("claude-fable-5-5-20261120", "Fable 5.5"),
    model("opus", "Opus 5.5"),
    model("claude-haiku-5", "Haiku 5"),
    model("future-alias", "Future model"),
  ]) {
    assert.equal(listForAgents("claude", available), true, available.id);
  }
});

test("new catalog identities refresh guidance and preserve overrides across restart", async () => {
  const home = tmpHome("model-best-use-");
  const pine = model("pine", "Pine 8.1", "claude-pine-8-1");
  const maple = model("maple", "Maple 8.2", "claude-maple-8-2");
  const newPine = model("pine", "Pine 8.2", "claude-pine-8-2");
  let requests = 0;
  let guidance = "| Quick work | Claude Pine 8.1 | Short coding tasks |\n| Large jobs | Claude Maple 8.2 | Systems engineering |";
  let pending: Promise<void> | undefined;
  const open = () => new ModelBestUses(home, () => {}, () => {}, async () => {
    requests++;
    await pending;
    return guidance;
  });
  try {
    let use = open();
    use.observe("claude", [pine, maple]);
    await use.waitForCurrent();
    assert.equal(requests, 1);
    assert.deepEqual(use.model("claude", modelUseId(pine)), { summary: "Short coding tasks", custom: false });
    use.observe("claude", [maple, pine]);
    await use.waitForCurrent();
    assert.equal(requests, 1, "reordering the list does not download the guide");

    let finish!: () => void;
    pending = new Promise<void>((resolve) => { finish = resolve; });
    use.observe("claude", [newPine, maple]);
    use.set("claude", modelUseId(pine), "My quick worker");
    guidance = "| Quick work | Claude Pine 8.1 | Documentation edits |\n| Complex tasks | Claude Maple 8.2 | Complex refactoring |\n| Fast analysis | Claude Pine 8.2 | Focused review |";
    finish();
    await use.waitForCurrent();
    assert.equal(requests, 2);
    assert.deepEqual(use.model("claude", modelUseId(pine)), { summary: "My quick worker", custom: true });
    assert.deepEqual(use.model("claude", modelUseId(newPine)), { summary: "Focused review", custom: false });
    assert.equal(use.model("claude", modelUseId(maple))?.summary, "Complex refactoring");

    use = open();
    use.observe("claude", [newPine, maple]);
    await use.waitForCurrent();
    assert.equal(requests, 2);
    assert.deepEqual(use.model("claude", modelUseId(pine)), { summary: "My quick worker", custom: true });
    use.set("claude", modelUseId(pine), null);
    assert.deepEqual(use.model("claude", modelUseId(pine)), { summary: "Short coding tasks", custom: false });
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});
