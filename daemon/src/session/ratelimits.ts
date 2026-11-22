// Rate limits (D161): `provider/rateLimits` and `provider/rateLimits/updated`.
//
// Each provider instance with the `rateLimits` capability keeps its own last report (from the
// harness's notifications and stream events, and a read at probe time); this service answers
// for all of them and passes each change on to every client. A `refresh` asks the providers to
// read again — at most every five minutes per instance, and only because a window shows the
// panel (the hub forwards the window's ask). It never polls on its own.

import type { InstanceRateLimits, ProviderRateLimitsResult } from "../protocol/generated/types.js";
import type { ProviderRegistry } from "../providers/registry.js";
import type { RateLimitReport } from "../providers/ratelimits.js";

export const MIN_REFRESH_MS = 5 * 60_000;
/** How long an ask waits for one provider's answer before answering with what is known. */
const ANSWER_MS = 5_000;

export interface RateLimitsOptions {
  broadcast: (method: string, params: unknown) => void;
  log: (line: string) => void;
  now?: () => number;
  minRefreshMs?: number;
}

export class RateLimits {
  private readonly last = new Map<string, InstanceRateLimits>();
  private readonly refreshed = new Map<string, number>();
  private readonly now: () => number;
  private readonly off: () => void;

  constructor(
    private readonly registry: ProviderRegistry,
    private readonly opts: RateLimitsOptions,
  ) {
    this.now = opts.now ?? Date.now;
    this.off = registry.onEvent((instance, ev) => {
      if (ev.type === "rateLimitsChanged") this.record(instance, ev.report);
    });
  }

  methods(): Map<string, (client: unknown, p: Record<string, unknown>) => Promise<unknown>> {
    return new Map([["provider/rateLimits", (_c, p) => this.list(p.refresh === true)]]);
  }

  close(): void {
    this.off();
  }

  /** Every reporting instance's last report; `refresh` reads those that are due again. */
  async list(refresh: boolean): Promise<ProviderRateLimitsResult> {
    const now = this.now();
    const asks = this.registry
      .all()
      .filter((i) => i.provider?.rateLimits && (i.descriptor.capabilities ?? []).includes("rateLimits"))
      .map(async (i) => {
        const id = i.config.id;
        const due = refresh && now - (this.refreshed.get(id) ?? 0) >= (this.opts.minRefreshMs ?? MIN_REFRESH_MS);
        if (due) this.refreshed.set(id, now);
        const read = i.provider!.rateLimits!({ refresh: due }).then(
          (r) => r && this.record(id, r),
          (e: Error) => this.opts.log(`rate limits of ${id} could not be read: ${e.message}`),
        );
        // A refresh can take a while (a harness read): answer with what is known, and let
        // the read's result follow as a notification.
        await Promise.race([read, new Promise((r) => setTimeout(r, due ? 0 : ANSWER_MS).unref())]);
      });
    await Promise.all(asks);
    return { instances: [...this.last.values()].filter((e) => this.registry.get(e.instance)) };
  }

  private record(instance: string, report: RateLimitReport): void {
    const inst = this.registry.get(instance);
    if (!inst) return;
    const entry: InstanceRateLimits = { instance, kind: inst.config.kind, ...(report.account ? { account: report.account } : {}), windows: report.windows, at: report.at };
    const before = this.last.get(instance);
    this.last.set(instance, entry);
    if (JSON.stringify(before) !== JSON.stringify(entry)) this.opts.broadcast("provider/rateLimits/updated", entry);
  }
}
