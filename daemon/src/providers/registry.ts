// Provider kinds and instances (docs/04-providers.md#kinds-instances-backends).
//
// A kind is registered once by its package (`registerKind`). Instances are configured on the
// target and stored under the yaacd home in `instances.json`; every kind whose factory says
// so also has an implicit default instance. The registry owns each instance's provider
// object and its descriptor, whose `revision` only ever increases.

import * as fs from "node:fs";
import * as path from "node:path";
import type {
  Capability,
  ProviderDescriptor,
  ProviderInstanceAddParams,
  ProviderInstanceConfig,
  ProviderInstanceUpdateParams,
} from "../protocol/generated/types.js";
import { canonical } from "../protocol/json.js";
import { RpcError } from "../protocol/rpc.js";
import { ensureDir, randomId, readJson, writeJsonAtomic } from "../util/fsx.js";
import { hasModelBestUse, ModelBestUses, modelUseId } from "./model-best-use.js";
import type { StoreToolImage } from "./tool-images.js";
import type { HarnessHome, HarnessProvider, MeshHooks, ProbeResult, ProviderEvent, ProviderFactory } from "./types.js";

export interface Instance {
  config: ProviderInstanceConfig;
  provider: HarnessProvider | null;
  descriptor: ProviderDescriptor;
  /** Unsubscribes from the provider's events. */
  off?: () => void;
}

interface InstancesFile {
  instances: ProviderInstanceConfig[];
  revisions?: Record<string, number>;
}

export function implicitId(kind: string): string {
  return `pi_${kind}`;
}

function kindDisplayName(kind: string): string {
  return kind.length > 0 ? kind[0]!.toUpperCase() + kind.slice(1) : kind;
}

export class ProviderRegistry {
  private factories = new Map<string, ProviderFactory>();
  private byId = new Map<string, Instance>();
  private stored: ProviderInstanceConfig[] = [];
  private revisions: Record<string, number> = {};
  private eventListeners = new Set<(instance: string, event: ProviderEvent) => void>();
  private descriptorListeners = new Set<(descriptor: ProviderDescriptor) => void>();
  private readonly file: string;
  private readonly bestUses: ModelBestUses;
  /** The agent mesh's hooks, per instance id (WP7); offered to every provider that takes them. */
  private meshHooks: ((instanceId: string) => MeshHooks) | null = null;

  constructor(
    private readonly home: string,
    private readonly env: NodeJS.ProcessEnv,
    private readonly log: (line: string) => void,
    private readonly providerRemoved?: (id: string) => void,
    private readonly storeToolImage?: StoreToolImage,
  ) {
    this.file = path.join(home, "instances.json");
    this.bestUses = new ModelBestUses(home, (kind) => {
      for (const instance of this.byId.values()) {
        if (instance.config.kind === kind) this.setDescriptor(instance, instance.descriptor, false);
      }
    }, log);
  }

  /** The agent mesh: offer the `yaac` tools to every provider instance, now and when created. */
  setMeshHooks(hooks: ((instanceId: string) => MeshHooks) | null): void {
    this.meshHooks = hooks;
    for (const i of this.byId.values()) i.provider?.attachMesh?.(hooks ? hooks(i.config.id) : null);
  }

  /** Registers a provider kind. Call before `load`. */
  registerKind(factory: ProviderFactory): void {
    this.factories.set(factory.kind, factory);
  }

  kinds(): string[] {
    return [...this.factories.keys()];
  }

  onEvent(listener: (instance: string, event: ProviderEvent) => void): () => void {
    this.eventListeners.add(listener);
    return () => this.eventListeners.delete(listener);
  }

  onDescriptor(listener: (descriptor: ProviderDescriptor) => void): () => void {
    this.descriptorListeners.add(listener);
    return () => this.descriptorListeners.delete(listener);
  }

  /** Reads the stored configuration and creates every instance (not yet probed). */
  load(): void {
    const file = readJson<InstancesFile>(this.file) ?? { instances: [] };
    this.stored = file.instances ?? [];
    this.revisions = file.revisions ?? {};
    for (const config of this.configs()) this.materialise(config);
  }

  /** Stored configurations plus implicit defaults, as `providerInstance/list` shows them. */
  configs(): ProviderInstanceConfig[] {
    const out: ProviderInstanceConfig[] = [];
    for (const f of this.factories.values()) {
      if (!f.implicit(this.env)) continue;
      const id = implicitId(f.kind);
      const stored = this.stored.find((c) => c.id === id);
      out.push(stored ?? { id, kind: f.kind, enabled: true, backend: "auto", implicit: true });
    }
    for (const c of this.stored) if (!c.implicit) out.push(c);
    return out;
  }

  private save(): void {
    writeJsonAtomic(this.file, { instances: this.stored, revisions: this.revisions } satisfies InstancesFile);
  }

  private displayName(config: ProviderInstanceConfig, base?: string): string {
    const kind = base ?? kindDisplayName(config.kind);
    return config.name ? `${kind} · ${config.name}` : kind;
  }

  // ── The harness home (P300) ────────────────────────────────────────────────────────────

  /** The kind's harness home, when it has one the user may set. */
  private homeOf(kind: string): HarnessHome | undefined {
    return this.factories.get(kind)?.home;
  }

  /** Capabilities that come from the kind and yaacd, not from a probe. */
  private ownCapabilities(kind: string): Capability[] {
    const caps: Capability[] = [];
    if (this.homeOf(kind)) caps.push("configurableHome");
    if (hasModelBestUse(kind)) caps.push("modelBestUse");
    return caps;
  }

  /**
   * The home an instance uses with this configuration (P300), or null for a kind without one.
   * Precedence: the instance's `home`; then the kind's variable in the instance's `env`; then
   * in the daemon's environment (on WSL that carries P297's `.bashrc` exports); then the
   * harness default.
   */
  configuredHome(config: ProviderInstanceConfig): string | null {
    const h = this.homeOf(config.kind);
    if (!h) return null;
    const fromEnv = (v: string | undefined) => (v && v.length > 0 ? v : null);
    return config.home ?? fromEnv(config.env?.[h.variable]) ?? fromEnv(this.env[h.variable]) ?? h.defaultHome(this.env);
  }

  /** The configuration the provider applies: `home` put in `env` under the kind's variable. */
  private applied(config: ProviderInstanceConfig): ProviderInstanceConfig {
    const h = this.homeOf(config.kind);
    if (!h || !config.home) return config;
    return { ...config, env: { ...(config.env ?? {}), [h.variable]: config.home } };
  }

  /** A home as typed: trimmed, `~` expanded against the daemon's home; an existing folder. */
  private checkHome(kind: string, raw: unknown): string {
    if (!this.homeOf(kind)) throw new RpcError("capability_missing", `a ${kind} instance has no harness home to set`);
    if (typeof raw !== "string") throw new RpcError("invalid_params", "home must be a path");
    let p = raw.trim();
    const userHome = this.env.HOME ?? this.env.USERPROFILE;
    if (userHome && (p === "~" || p.startsWith("~/"))) p = path.join(userHome, p.slice(1));
    if (!path.isAbsolute(p)) throw new RpcError("invalid_params", `the harness home must be an absolute path: ${raw}`);
    p = path.normalize(p);
    if (p.length > 1) p = p.replace(/[\\/]+$/, "") || p;
    let dir = false;
    try {
      dir = fs.statSync(p).isDirectory();
    } catch {
      dir = false;
    }
    if (!dir) throw new RpcError("invalid_params", `there is no folder at ${p} on this target`);
    return p;
  }

  /**
   * Whether two homes of one kind lead to the same store: the same store key, as the kind's
   * probe would report it. Codex's key is the real path, so a symlink to the same folder is the
   * same store; Claude Code's is the resolved path, so it is not, and its threads would not
   * follow — the user is asked.
   */
  private sameStore(kind: string, a: string, b: string): boolean {
    const h = this.homeOf(kind);
    return !h || h.storeKey(a) === h.storeKey(b);
  }

  private placeholder(config: ProviderInstanceConfig, status: ProviderDescriptor["status"], message?: string): ProviderDescriptor {
    const home = this.configuredHome(config);
    return {
      instance: config.id,
      kind: config.kind,
      name: config.name ?? null,
      displayName: this.displayName(config),
      backend: "sdk",
      store: home ? { key: null, home } : { key: null },
      status,
      statusMessage: message ?? null,
      capabilities: this.ownCapabilities(config.kind),
      models: [],
      modelsComplete: false,
      modes: [],
      revision: this.revisions[config.id] ?? 0,
    };
  }

  private materialise(config: ProviderInstanceConfig): Instance {
    const factory = this.factories.get(config.kind);
    let provider: HarnessProvider | null = null;
    let descriptor: ProviderDescriptor;
    if (!factory) {
      descriptor = this.placeholder(config, "notInstalled", `this yaacd has no provider of kind "${config.kind}"`);
    } else if (!config.enabled) {
      descriptor = this.placeholder(config, "disabled");
    } else {
      const stateDir = path.join(this.home, "providers", config.id);
      ensureDir(stateDir);
      provider = factory.create({
        config: this.applied(config),
        stateDir,
        home: this.home,
        env: this.env,
        log: (l) => this.log(`[${config.id}] ${l}`),
        storeToolImage: this.storeToolImage,
      });
      if (this.meshHooks) provider.attachMesh?.(this.meshHooks(config.id));
      descriptor = this.placeholder(config, "probing");
    }
    const instance: Instance = { config, provider, descriptor };
    if (provider) {
      instance.off = provider.events.on((event) => {
        if (event.type === "descriptorChanged") void this.probe(config.id);
        for (const l of [...this.eventListeners]) l(config.id, event);
      });
    }
    this.byId.set(config.id, instance);
    return instance;
  }

  all(): Instance[] {
    return [...this.byId.values()];
  }

  get(id: string): Instance | undefined {
    return this.byId.get(id);
  }

  require(id: string): Instance {
    const i = this.byId.get(id);
    if (!i) throw new RpcError("instance_not_found", `no provider instance ${id} on this target`);
    return i;
  }

  /** The instance's provider, or the reason it cannot be used. */
  usable(id: string): { instance: Instance; provider: HarnessProvider } {
    const instance = this.require(id);
    const d = instance.descriptor;
    if (!instance.provider || d.status === "disabled" || d.status === "notInstalled") {
      throw new RpcError("provider_unavailable", d.statusMessage ?? `${d.displayName} is ${d.status}`);
    }
    if (d.status === "needsAuth") throw new RpcError("needs_auth", d.statusMessage ?? `${d.displayName} is not signed in`);
    if (d.status !== "ready" && d.status !== "probing") {
      throw new RpcError("provider_unavailable", d.statusMessage ?? `${d.displayName} is ${d.status}`);
    }
    return { instance, provider: instance.provider };
  }

  /** Re-probes one instance or all; pushes descriptor changes with a higher revision. */
  async probe(id?: string): Promise<void> {
    const targets = id ? [this.require(id)] : this.all();
    await Promise.all(targets.map((i) => this.probeOne(i)));
  }

  private async probeOne(instance: Instance): Promise<void> {
    if (!instance.provider) return;
    let result: ProbeResult;
    try {
      result = await instance.provider.probe();
    } catch (e) {
      this.log(`probe of ${instance.config.id} failed: ${(e as Error).message}`);
      this.setDescriptor(instance, {
        ...instance.descriptor,
        status: "unreachable",
        statusMessage: `probe failed: ${(e as Error).message}`,
      });
      return;
    }
    this.setDescriptor(instance, {
      instance: instance.config.id,
      kind: instance.config.kind,
      name: instance.config.name ?? null,
      displayName: this.displayName(instance.config, result.displayName),
      backend: result.backend,
      harnessVersion: result.harnessVersion ?? null,
      // A harness that did not say where its home is (not reachable, not signed in): the one
      // it is configured to use, so Settings can show it (P300).
      store: result.store.home || !this.homeOf(instance.config.kind) ? result.store : { ...result.store, home: this.configuredHome(instance.config) },
      status: result.status,
      statusMessage: result.statusMessage ?? null,
      // Thread placement and retaining fork child links are yaacd capabilities;
      // `configurableHome` the kind's (P300), whatever the probe found.
      capabilities: [...result.capabilities, ...["threadMove" as const, "forkKeepChildren" as const, ...this.ownCapabilities(instance.config.kind)].filter((c) => !result.capabilities.includes(c))],
      models: result.models,
      modelsComplete: result.modelsComplete,
      modes: result.modes,
      revision: instance.descriptor.revision,
      // When the model list was last read from the harness (a probe that got none keeps the last time).
      ...(result.models.length > 0 || result.modelsComplete ? { modelsReadAt: result.modelsReadAt ?? new Date().toISOString() } : instance.descriptor.modelsReadAt ? { modelsReadAt: instance.descriptor.modelsReadAt } : {}),
      ...(result.backendOrigin ? { backendOrigin: result.backendOrigin } : {}),
    });
  }

  private setDescriptor(instance: Instance, next: ProviderDescriptor, observe = true): void {
    const kind = instance.config.kind;
    if (hasModelBestUse(kind) && (next.models?.length ?? 0) > 0) {
      if (observe) this.bestUses.observe(kind, next.models ?? []);
      next = { ...next, models: next.models?.map(({ bestUse: _bestUse, ...model }) => {
        const bestUse = this.bestUses.model(kind, modelUseId(model));
        return bestUse ? { ...model, bestUse } : model;
      }) };
    }
    const same = canonical({ ...next, revision: 0 }) === canonical({ ...instance.descriptor, revision: 0 });
    if (same && instance.descriptor.revision > 0) return;
    const revision = (this.revisions[instance.config.id] ?? 0) + 1;
    this.revisions[instance.config.id] = revision;
    this.save();
    instance.descriptor = { ...next, revision };
    for (const l of [...this.descriptorListeners]) l(instance.descriptor);
  }

  waitForModelBestUses(): Promise<void> {
    return this.bestUses.waitForCurrent();
  }

  setModelBestUse(instanceId: string, modelId: string, expectedId: string, summary: unknown): void {
    const instance = this.require(instanceId);
    if (!hasModelBestUse(instance.config.kind)) throw new RpcError("capability_missing", "This provider has no model guidance");
    const model = instance.descriptor.models?.find((entry) => entry.id === modelId);
    if (!model || modelUseId(model) !== expectedId) {
      throw new RpcError("invalid_params", "The model changed while you were editing. Reopen its current guidance.");
    }
    this.bestUses.set(instance.config.kind, modelUseId(model), summary);
  }

  // ── providerInstance/* ─────────────────────────────────────────────────────────────────

  async add(params: ProviderInstanceAddParams): Promise<ProviderInstanceConfig> {
    if (typeof params.kind !== "string" || !this.factories.has(params.kind)) {
      throw new RpcError("invalid_params", `this yaacd has no provider of kind "${String(params.kind)}"`);
    }
    const config: ProviderInstanceConfig = {
      id: randomId("pi_", 9),
      kind: params.kind,
      enabled: params.enabled !== false,
      backend: params.backend ?? "auto",
    };
    if (params.name) config.name = params.name;
    if (params.endpoint) config.endpoint = params.endpoint;
    if (params.env && Object.keys(params.env).length > 0) config.env = params.env;
    if (params.home !== undefined && params.home !== null && params.home !== "") config.home = this.checkHome(params.kind, params.home);
    this.stored.push(config);
    this.save();
    const instance = this.materialise(config);
    for (const l of [...this.descriptorListeners]) l(instance.descriptor);
    await this.probeOne(instance);
    return config;
  }

  async update(params: ProviderInstanceUpdateParams): Promise<ProviderInstanceConfig> {
    const instance = this.require(params.id);
    const config: ProviderInstanceConfig = { ...instance.config };
    if (params.backend !== undefined && params.backend !== null) config.backend = params.backend;
    if (params.enabled !== undefined && params.enabled !== null) config.enabled = params.enabled;
    if (params.name !== undefined) {
      if (params.name === null || params.name === "") delete config.name;
      else config.name = params.name;
    }
    if (params.endpoint !== undefined) {
      if (params.endpoint === null) delete config.endpoint;
      else config.endpoint = params.endpoint;
    }
    if (params.env !== undefined && params.env !== null) config.env = params.env;
    if (params.home !== undefined) {
      if (params.home === null || params.home === "") delete config.home;
      else config.home = this.checkHome(config.kind, params.home);
    }
    // Another store (P10, P300): asked for only once the user confirmed it; the instance's
    // threads stay bound to the store they belong to and show unavailable meanwhile.
    if ((params.home !== undefined || (params.env !== undefined && params.env !== null)) && params.confirmStoreChange !== true) {
      const next = this.configuredHome(config);
      const current = instance.descriptor.store.home ?? this.configuredHome(instance.config);
      if (next && current && !this.sameStore(config.kind, current, next)) {
        throw new RpcError("store_change", `${instance.descriptor.displayName} would use another store: ${next} instead of ${current}. Its threads stay with ${current} and show unavailable until it is changed back.`, { current, next });
      }
    }
    // "Trust folders YAAC works in" (D147): absent in the stored configuration means on.
    if (params.trustFolders === true) delete config.trustFolders;
    else if (params.trustFolders === false) config.trustFolders = false;
    const idx = this.stored.findIndex((c) => c.id === config.id);
    if (idx >= 0) this.stored[idx] = config;
    else this.stored.push(config);
    this.save();
    // Only yaacd's own switch changed: the provider and its connection are kept.
    if (params.trustFolders !== undefined && params.trustFolders !== null && canonical({ ...config, trustFolders: null }) === canonical({ ...instance.config, trustFolders: null })) {
      instance.config = config;
      return config;
    }
    instance.off?.();
    this.providerRemoved?.(instance.config.id);
    await instance.provider?.dispose().catch(() => {});
    const next = this.materialise(config);
    next.descriptor = { ...next.descriptor, revision: instance.descriptor.revision };
    await this.probeOne(next);
    if (!next.provider) this.setDescriptor(next, { ...next.descriptor });
    return config;
  }

  async remove(id: string): Promise<void> {
    const instance = this.require(id);
    if (instance.config.implicit) {
      throw new RpcError("invalid_params", "an implicit default instance cannot be removed; disable it instead");
    }
    instance.off?.();
    this.providerRemoved?.(instance.config.id);
    await instance.provider?.dispose().catch(() => {});
    this.byId.delete(id);
    this.stored = this.stored.filter((c) => c.id !== id);
    this.save();
  }

  async dispose(): Promise<void> {
    for (const i of this.byId.values()) {
      i.off?.();
      await i.provider?.dispose().catch(() => {});
    }
  }
}
