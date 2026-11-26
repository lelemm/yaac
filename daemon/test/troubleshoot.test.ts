import test from "node:test";
import assert from "node:assert/strict";
import { redactTroubleshooting, Troubleshooter } from "../src/session/troubleshoot.js";
import type { ProviderRegistry } from "../src/providers/registry.js";

test("developer handoff retains source references and removes common private details", () => {
  const symptom = [
    "/home/alice/private.txt failed in crates/yaac-core/src/client.rs:71",
    '"C:\\Users\\Alice Person\\private.txt" while connecting to https://private.example/path',
    "alice@example.com at 192.168.4.7 and 2001:db8::1, token=private-secret",
    "yaac-12345678-1234-1234-1234-123456789abc",
  ].join("\n");
  assert.equal(redactTroubleshooting(symptom), [
    "[local path] failed in crates/yaac-core/src/client.rs:71",
    "[local path] while connecting to [URL]",
    "[email] at [IP] and [IP], [credential]",
    "[ID]",
  ].join("\n"));
});

test("diagnosis rejects an unavailable model or oversized symptom before preparing source", async () => {
  const registry = { get: () => undefined } as unknown as ProviderRegistry;
  const troubleshooter = new Troubleshooter(registry);
  const params = { instance: "fixture", model: "fixture", effort: "medium", symptom: "A symptom", build: "0.1.0+1042", environment: "WPF" };
  await assert.rejects(troubleshooter.run(params), { code: "capability_missing" });
  await assert.rejects(troubleshooter.run({ ...params, symptom: "👍".repeat(2001) }), { code: "invalid_params" });
});
