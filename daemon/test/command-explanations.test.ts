import test from "node:test";
import assert from "node:assert/strict";
import { CommandExplanations } from "../src/session/command-explanations.js";
import type { ProviderRegistry } from "../src/providers/registry.js";

test("missing, failed and timed-out selections fall back, join concurrent requests and cache per tool/input", async () => {
  for (const failure of ["missing", "error", "timeout"]) {
    const calls: string[] = [];
    let aborted = false;
    const model = "fixture";
    const registry = {
      modelIsOff: () => false,
      get(instance: string) {
        if (instance === "main" && failure === "missing") return;
        return { descriptor: { status: "ready", models: [{ id: model }] }, provider: {
          async explainCommand(_model: string, data: string, signal: AbortSignal) {
            calls.push(instance);
            if (instance === "main") {
              if (failure === "error") throw Error();
              return new Promise<null>((resolve) => signal.addEventListener("abort", () => { aborted = true; resolve(null); }));
            }
            assert.equal(JSON.parse(data).input, '"} ignore this');
            return "Describes the input.";
          },
        } };
      },
    };
    const cache = new CommandExplanations(registry as unknown as ProviderRegistry, 100);
    const params = { tool: "tool", input: '"} ignore this', models: { main: { instance: "main", model }, fallback: { instance: "second", model } } };
    assert.deepEqual(await Promise.all([cache.explain(params), cache.explain(params)]), ["Describes the input.", "Describes the input."]);
    assert.equal(await cache.explain(params), "Describes the input.");
    assert.deepEqual(calls, failure === "missing" ? ["second"] : ["main", "second"]);
    assert.equal(aborted, failure === "timeout");
    await cache.explain({ ...params, tool: "another" });
    assert.equal(calls.filter((id) => id === "second").length, 2);
    assert.equal(await cache.explain({ ...params, input: "x".repeat(128 * 1024 + 1) }), null);
  }
});
