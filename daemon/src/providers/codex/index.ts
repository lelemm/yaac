// The Codex provider kind (`codex`). See provider.ts.

import type { ProviderContext, ProviderFactory } from "../types.js";
import { CodexProvider } from "./provider.js";

export { CodexProvider, type CodexThreadSetup, type CodexThreadSetupHook, type CodexThreadSetupRequest, type CodexOptions } from "./provider.js";

export const CODEX_KIND = "codex";

/**
 * The factory. Every target has an implicit default Codex instance (the standard
 * `~/.codex` daemon) unless `YAAC_CODEX_PROVIDER=0` — tests set that so they never probe, or
 * start, a real Codex.
 */
export const codexProviderFactory: ProviderFactory = {
  kind: CODEX_KIND,
  implicit: (env) => env.YAAC_CODEX_PROVIDER !== "0",
  create(ctx: ProviderContext): CodexProvider {
    const env = ctx.config.env ?? {};
    return new CodexProvider(ctx, {
      socket: ctx.config.endpoint?.socket ?? null,
      env,
    });
  },
};
