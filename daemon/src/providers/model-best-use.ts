// Public model guidance is independent of the harness catalog and its capabilities.
import * as path from "node:path";
import type { ModelBestUse, ModelInfo } from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import { readJson, sha256Hex, writeJsonAtomic } from "../util/fsx.js";

const GUIDES = new Map([
  ["codex", "https://developers.openai.com/api/docs/guides/model-selection.md"],
  ["claude", "https://platform.claude.com/docs/en/about-claude/models/choosing-a-model.md"],
]);
const MAX_GUIDE_BYTES = 512 * 1024;
const FETCH_RETRY_MS = 15 * 60_000;

export function hasModelBestUse(kind: string): boolean {
  return GUIDES.has(kind);
}

/** The catalog filter applies only when reporting models to agents. */
export function listForAgents(kind: string, model: ModelInfo): boolean {
  if (kind !== "codex") return true;
  const major = /(?:^|\/)gpt[-\s\u2010-\u2015]?(\d+)/i.exec(model.id)?.[1]
    ?? /^gpt[-\s\u2010-\u2015]?(\d+)/i.exec(model.displayName)?.[1];
  return major === undefined || Number(major) >= 6;
}

/** Prefer the specific model when the harness exposes it behind a mutable alias. */
export function modelUseId(model: ModelInfo): string {
  return model.resolvedModel ?? model.id;
}

function plain(text: string): string {
  return text.replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
    .replace(/<[^>]*>/g, " ")
    .replace(/[*`_~]/g, "")
    .replace(/[\p{Cc}\p{Cf}]/gu, " ")
    .trim().replace(/\s+/g, " ");
}

function key(text: string): string {
  return text.normalize("NFKD").toLowerCase().replace(/[^a-z0-9]/g, "");
}

function gist(text: string): string | undefined {
  const sentence = plain(text).replace(/^(?:for|to)\s+/i, "").replace(/[.;,:]+$/, "");
  const words = sentence.split(" ").filter(Boolean).slice(0, 15);
  if (!words.length) return undefined;
  const summary = words.join(" ").replace(/[.;,:]+$/, "");
  return summary ? summary[0]!.toUpperCase() + summary.slice(1) : undefined;
}

/** Extract the guide's use-case recommendations by the models it explicitly names. */
export function guideRecommendations(markdown: string): ReadonlyMap<string, string> {
  const entries = new Map<string, { priority: number; summary: string }>();
  const add = (name: string, value: string, priority: number) => {
    const id = key(plain(name));
    const summary = gist(value);
    if (id && summary && (entries.get(id)?.priority ?? -1) < priority) {
      entries.set(id, { priority, summary });
    }
  };
  const mentioned = (text: string): string[] =>
    [...text.matchAll(/\*\*([^*]+)\*\*/g)].map((match) => match[1]!);
  const lines = markdown.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!.trim();
    if (line.startsWith("|")) {
      const cells = line.split("|").slice(1, -1).map((cell) => cell.trim());
      // The selection matrix names a single model in the middle column and examples after it.
      if (cells.length !== 3 || !cells[2] || !/^(?:Claude|GPT)[\s-]/i.test(plain(cells[1] ?? ""))) continue;
      const candidates = mentioned(cells[1]!);
      if (candidates.length > 1) continue;
      const name = candidates[0] ?? cells[1]!;
      add(name, cells[2], 3);
      continue;
    }
    const rec = /^[-*]\s+(?:(For .+?),\s+)?(?:start with|choose|use|consider)\s+\*\*([^*]+)\*\*(.*)$/i.exec(line);
    if (rec) {
      const tail = rec[3]!.trim();
      const task = rec[1] ?? (/^for\s|^to\s/i.test(tail) ? tail : undefined);
      if (task) add(rec[2]!, task, rec[1] ? 1 : 4);
      continue;
    }
    // OpenAI's detailed model-and-effort examples introduce the use case on the next line.
    const example = /^\d+\.\s+\*\*([^*·]+)\s+·\s+[^*]+\*\*/.exec(line);
    if (example) {
      const next = lines.slice(i + 1).find((candidate) => candidate.trim())?.trim();
      if (next && !/^[#|*\d]/.test(next)) add(example[1]!, next, 2);
      continue;
    }
    // Claude's prose also describes models that are not in the selection matrix.
    const prose = /^\*\*([^*]+)\*\*\s*(?:\([^)]*\)\s*)?(?:is built for|is designed for|excel[s]? at|offers)\s+([^.!?]+)/i.exec(line);
    if (prose) add(prose[1]!, prose[2]!, 1);
  }
  return new Map([...entries].map(([id, data]) => [id, data.summary]));
}

interface CatalogState {
  fingerprint: string;
  seen: string[];
  generated: [string, string][];
  custom: [string, string][];
}

interface Catalog {
  models: Map<string, ModelInfo>;
  fetched: Set<string>;
  fingerprint: string;
  generated: Map<string, string>;
  custom: Map<string, string>;
  running: Promise<void> | null;
  retryAt: number;
}

async function downloadGuide(url: string): Promise<string> {
  const response = await fetch(url, {
    headers: { accept: "text/markdown" }, redirect: "error", signal: AbortSignal.timeout(8_000),
  });
  if (!response.ok || !response.body) throw new Error(`guide returned HTTP ${response.status}`);
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let bytes = 0;
  let text = "";
  try {
    for (;;) {
      const chunk = await reader.read();
      if (chunk.done) return text + decoder.decode();
      bytes += chunk.value.byteLength;
      if (bytes > MAX_GUIDE_BYTES) throw new Error("guide exceeded the download limit");
      text += decoder.decode(chunk.value, { stream: true });
    }
  } finally {
    await reader.cancel().catch(() => {});
  }
}

export class ModelBestUses {
  private readonly file: string;
  private readonly catalogs = new Map<string, Catalog>();
  private readonly saved: Record<string, CatalogState>;

  constructor(
    home: string,
    private readonly changed: (kind: string) => void,
    private readonly log: (message: string) => void,
    private readonly loadGuide = downloadGuide,
  ) {
    this.file = path.join(home, "model-best-use.json");
    try {
      this.saved = readJson<Record<string, CatalogState>>(this.file) ?? {};
    } catch (error) {
      this.log(`Could not read model guidance: ${String(error)}`);
      this.saved = {};
    }
  }

  private catalog(kind: string): Catalog {
    const prior = this.catalogs.get(kind);
    if (prior) return prior;
    const saved = this.saved[kind];
    const catalog: Catalog = {
      models: new Map(),
      fetched: new Set(saved?.seen), fingerprint: saved?.fingerprint ?? "",
      generated: new Map(saved?.generated), custom: new Map(saved?.custom), running: null, retryAt: 0,
    };
    this.catalogs.set(kind, catalog);
    return catalog;
  }

  private save(kind: string, catalog: Catalog): void {
    const next: CatalogState = {
      fingerprint: catalog.fingerprint, seen: [...catalog.fetched].sort(),
      generated: [...catalog.generated], custom: [...catalog.custom],
    };
    const saved = { ...this.saved, [kind]: next };
    writeJsonAtomic(this.file, saved);
    this.saved[kind] = next;
  }

  model(kind: string, id: string): ModelBestUse | undefined {
    if (!hasModelBestUse(kind)) return undefined;
    const catalog = this.catalog(kind);
    const custom = catalog.custom.get(id);
    if (custom !== undefined) return { summary: custom, custom: true };
    const summary = catalog.generated.get(id);
    return summary === undefined ? undefined : { summary, custom: false };
  }

  set(kind: string, id: string, value: unknown): void {
    const catalog = this.catalog(kind);
    const before = catalog.custom.get(id);
    if (value === null) {
      if (before === undefined) return;
      catalog.custom.delete(id);
    } else {
      if (typeof value !== "string") throw new RpcError("invalid_params", "summary must be text or null");
      const summary = value.replace(/[\p{Cc}\p{Cf}]/gu, " ").trim().replace(/\s+/g, " ");
      if (!summary) throw new RpcError("invalid_params", "Enter a model recommendation or reset it to generated guidance");
      if (summary === before) return;
      catalog.custom.set(id, summary);
    }
    try {
      this.save(kind, catalog);
    } catch (error) {
      if (before === undefined) catalog.custom.delete(id);
      else catalog.custom.set(id, before);
      throw error;
    }
    this.changed(kind);
  }

  observe(kind: string, models: readonly ModelInfo[]): void {
    if (!hasModelBestUse(kind) || !models.length) return;
    const catalog = this.catalog(kind);
    for (const model of models) catalog.models.set(modelUseId(model), model);
    this.refreshIfNeeded(kind, catalog);
  }

  private refreshIfNeeded(kind: string, catalog: Catalog): void {
    const url = GUIDES.get(kind);
    if (!url || !catalog.models.size || catalog.running || Date.now() < catalog.retryAt) return;
    const known = [...new Set([...catalog.fetched, ...catalog.models.keys()])].sort();
    const fingerprint = sha256Hex(JSON.stringify(known));
    if (fingerprint === catalog.fingerprint) return;
    catalog.running = this.refresh(kind, url, catalog).finally(() => {
      catalog.running = null;
    });
  }

  /** A tool asked while a new catalog is loading can use the pending refresh. */
  async waitForCurrent(): Promise<void> {
    await Promise.all([...this.catalogs].map(([kind, catalog]) => {
      this.refreshIfNeeded(kind, catalog);
      return catalog.running;
    }));
  }

  private async refresh(kind: string, url: string, catalog: Catalog): Promise<void> {
    try {
      const entries = guideRecommendations(await this.loadGuide(url));
      if (!entries.size) throw new Error("the guide did not contain recognized recommendations");
      const next = new Map(catalog.generated);
      const fetched = new Set(catalog.fetched);
      for (const [id, model] of catalog.models) {
        const resolved = key(id).replace(/20\d{6}$/, "");
        const label = key(model.displayName);
        const recommendation = entries.get(resolved) ?? entries.get(label)
          ?? entries.get(`claude${label}`) ?? entries.get(key(model.id));
        if (recommendation && !catalog.custom.has(id)) next.set(id, recommendation);
        fetched.add(id);
      }
      const fingerprint = sha256Hex(JSON.stringify([...fetched].sort()));
      this.save(kind, { ...catalog, generated: next, fetched, fingerprint });
      const changed = [...next].some(([id, summary]) => catalog.generated.get(id) !== summary);
      catalog.generated = next;
      catalog.fetched = fetched;
      catalog.fingerprint = fingerprint;
      if (changed) this.changed(kind);
    } catch (error) {
      catalog.retryAt = Date.now() + FETCH_RETRY_MS;
      this.log(`Model guidance for ${kind}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
}
