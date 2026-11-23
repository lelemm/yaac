// Skills (`skills/list`): what one provider instance offers for a working directory.
//
// The provider reads its own harness (Codex asks its app-server, Claude Code is read from the
// folders it uses); this service only routes the ask to the instance, checks the capability, and
// bounds the answer. Read-only: it changes nothing on the target.

import * as path from "node:path";
import { RpcError } from "../protocol/rpc.js";
import type { ProviderRegistry } from "../providers/registry.js";
import type { SkillsListResult } from "../protocol/generated/types.js";

export const SKILLS_MAX = 500;

export class Skills {
  constructor(private readonly registry: ProviderRegistry) {}

  methods(): Map<string, (client: unknown, p: Record<string, unknown>) => Promise<unknown>> {
    return new Map([["skills/list", (_c, p) => this.list(p)]]);
  }

  async list(p: Record<string, unknown>): Promise<SkillsListResult> {
    if (typeof p.instance !== "string") throw new RpcError("invalid_params", "instance is required");
    if (typeof p.cwd !== "string" || !path.isAbsolute(p.cwd)) throw new RpcError("invalid_params", "cwd must be an absolute path on this target");
    const inst = this.registry.get(p.instance);
    if (!inst) throw new RpcError("not_found", `no provider instance ${p.instance}`);
    if (!inst.provider?.skills || !(inst.descriptor.capabilities ?? []).includes("skills")) {
      throw new RpcError("capability_missing", `${inst.descriptor.displayName ?? p.instance} does not list skills`);
    }
    const res = await inst.provider.skills(p.cwd, { forceReload: p.forceReload === true });
    return { skills: res.skills.slice(0, SKILLS_MAX), ...(res.errors && res.errors.length > 0 ? { errors: res.errors.slice(0, 20) } : {}) };
  }
}
