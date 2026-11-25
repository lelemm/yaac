import type { ApprovalExplainParams } from "../protocol/generated/types.js";
import type { ProviderRegistry } from "../providers/registry.js";
import { sha256Hex } from "../util/fsx.js";

/** Each target daemon owns its own bounded, in-memory cache. */
export class CommandExplanations {
  private readonly cached = new Map<string, string>();
  private readonly running = new Map<string, Promise<string | null>>();

  constructor(private readonly registry: ProviderRegistry, private readonly timeoutMs = 10_000) {}

  async explain({ tool, input, models }: ApprovalExplainParams): Promise<string | null> {
    if (typeof tool !== "string" || !tool || tool.length > 256 || typeof input !== "string" ||
        !input.trim() || Buffer.byteLength(input) > 128 * 1024 || !models?.main) return null;
    const data = JSON.stringify({ tool, input });
    const key = sha256Hex(data);
    const hit = this.cached.get(key) ?? this.running.get(key);
    if (hit) return hit;
    if (this.running.size >= 4) return null;
    const run = this.run(models, data).then((text) => {
      if (text) {
        this.cached.set(key, text);
        if (this.cached.size > 64) this.cached.delete(this.cached.keys().next().value!);
      }
      return text;
    }).finally(() => this.running.delete(key));
    this.running.set(key, run);
    return run;
  }

  private async run(models: ApprovalExplainParams["models"], data: string): Promise<string | null> {
    const deadline = Date.now() + this.timeoutMs;
    for (const [index, choice] of [models.main, models.fallback].entries()) {
      if (!choice) continue;
      const instance = this.registry.get(choice.instance);
      if (!instance?.provider?.explainCommand || instance.descriptor.status !== "ready" ||
          !instance.descriptor.models?.some((m) => m.id === choice.model && !m.hidden) ||
          this.registry.modelIsOff(instance, choice.model)) continue;
      const budget = index === 0 && models.fallback ? this.timeoutMs / 2 : deadline - Date.now();
      if (budget <= 0) break;
      const controller = new AbortController();
      let timer: NodeJS.Timeout | undefined;
      try {
        const output = await Promise.race([
          instance.provider.explainCommand(choice.model, data, controller.signal),
          new Promise<null>((resolve) => { timer = setTimeout(() => { controller.abort(); resolve(null); }, budget); }),
        ]);
        const text = output?.replace(/\s+/g, " ").trim();
        if (text && text.length <= 600) return text;
      } catch { /* Continue with the configured fallback. */ }
      finally { clearTimeout(timer); }
    }
    return null;
  }
}
