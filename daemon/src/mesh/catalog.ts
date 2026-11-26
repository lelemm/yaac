// Catalog files are fetched only when a thread activates a skill.
import { createHash } from "node:crypto";
import { promises as fs } from "node:fs";
import path from "node:path";
import type { CatalogSkill, CatalogSkillInfo, CatalogSkillsResult } from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import { ensurePrivateDir } from "../session/uploads.js";
import { catalogSkillId } from "./tools.js";

type Fetch = (id?: string, timeoutMs?: number) => Promise<CatalogSkillsResult>;
const MAX_BYTES = 512 * 1024;

interface TurnSkills {
  explicit: Set<string>;
  loaded: Map<string, Promise<string>>;
}

export class Catalog {
  private readonly turns = new Map<string, TurnSkills>();

  constructor(private readonly fetch: Fetch) {}

  private turn(thread: string): TurnSkills {
    let entry = this.turns.get(thread);
    if (!entry) {
      entry = { explicit: new Set(), loaded: new Map() };
      this.turns.set(thread, entry);
    }
    return entry;
  }

  done(thread: string): void {
    this.turns.delete(thread);
  }

  async instructions(thread: string, text: string, user: boolean, continuation = false): Promise<string> {
    if (!continuation) this.done(thread);
    const mentioned = new Set(text.match(/(?<![\w.-])yaac\.[a-z0-9]+(?:-[a-z0-9]+)*(?![\w-]|\.[a-z0-9])/g) ?? []);
    if (user && mentioned.size) {
      const explicit = this.turn(thread).explicit;
      for (const id of mentioned) explicit.add(id);
    }
    const explicit = this.turns.get(thread)?.explicit;
    // This metadata query is best-effort. An old or disconnected hub never blocks a turn.
    const info = (await this.fetch(undefined, 600).catch(() => ({} as CatalogSkillsResult))).skills ?? [];
    const selected = info.filter(skill => skill.automatic || explicit?.has(skill.id));
    if (!selected.length) return "";
    return "\n\n<yaac_skill_catalog>\n"
      + "The following names and descriptions are discovery data for this turn. "
      + "For a relevant auto skill or one the user explicitly requested, call yaac.load_skill with its exact ID "
      + "before carrying out the work. Read its SKILL.md from the returned local path and resolve bundled files beside it.\n"
      + JSON.stringify(selected.map(({ id, description, automatic }) => ({ id, description, automatic })))
        .replaceAll("<", "\\u003c").replaceAll(">", "\\u003e")
      + "\n</yaac_skill_catalog>";
  }

  async list(): Promise<CatalogSkillInfo[]> {
    return (await this.fetch()).skills ?? [];
  }

  async load(thread: string, directory: string, id: string, stillActive: () => boolean): Promise<string> {
    if (!catalogSkillId(id)) {
      throw new RpcError("invalid_params", "Use the exact yaac.<skill-name> ID from list_skills.");
    }
    const turn = this.turn(thread);
    let load = turn.loaded.get(id);
    if (!load) {
      load = this.fetch(id).then(({ skill }) => {
        if (!skill || skill.info.id !== id) throw new RpcError("invalid_params", "That skill was not found.");
        if (!skill.info.automatic && !turn.explicit.has(id)) {
          throw new RpcError("policy_denied", "This skill is manual. The user can request it by its exact catalog ID.");
        }
        return materialize(directory, skill, stillActive);
      });
      turn.loaded.set(id, load);
    }
    try {
      return await load;
    } catch (error) {
      if (turn.loaded.get(id) === load) turn.loaded.delete(id);
      throw error;
    }
  }
}

async function materialize(directory: string, skill: CatalogSkill, stillActive: () => boolean): Promise<string> {
  if (!stillActive()) throw new RpcError("thread_not_found", "The thread was removed while loading the skill.");
  const files = skill.files;
  if (!Array.isArray(files) || !files.length || files.length > 64
      || !files.some(file => file.path === "SKILL.md")) {
    throw new RpcError("invalid_params", "Invalid skill bundle.");
  }
  const total = files.reduce((sum, file) => sum + file.dataBase64.length, 0);
  if (total > (Math.ceil(MAX_BYTES / 3) + 64) * 4) {
    throw new RpcError("limit_exceeded", "Skill files exceed 512 KiB.");
  }
  const revision = createHash("sha256").update(skill.info.id + "\0" + skill.info.revision).digest("hex");
  // Prefix skill directories so they cannot collide with uploaded file names.
  const parent = path.join(directory, "yaac-skills");
  ensurePrivateDir(parent);
  const final = path.join(parent, revision);
  const slug = skill.info.id.slice("yaac.".length);
  const entrypoint = path.join(final, slug, "SKILL.md");
  try {
    if ((await fs.lstat(entrypoint)).isFile()) return entrypoint;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }

  const staging = await fs.mkdtemp(path.join(parent, ".extract-"));
  try {
    const root = path.join(staging, slug);
    let bytes = 0;
    for (const file of files) {
      const parts = file.path.split("/");
      if (parts.some(part => !part || part === "." || part === ".." || /[\\\\:\x00-\x1f<>|"*?]/.test(part)
          || /[ .]$/.test(part) || /^(?:con|prn|aux|nul|com[1-9¹²³]|lpt[1-9¹²³])(?:\.|$)/i.test(part))) {
        throw new RpcError("invalid_params", "Invalid skill path.");
      }
      const content = Buffer.from(file.dataBase64, "base64");
      bytes += content.length;
      if (bytes > MAX_BYTES) throw new RpcError("limit_exceeded", "Skill files exceed 512 KiB.");
      const filePath = path.join(root, ...parts);
      await fs.mkdir(path.dirname(filePath), { recursive: true, mode: 0o700 });
      await fs.writeFile(filePath, content, { flag: "wx", mode: file.executable ? 0o700 : 0o600 });
    }
    if (!stillActive()) throw new RpcError("thread_not_found", "The thread was removed while loading the skill.");
    try {
      await fs.rename(staging, final);
    } catch (error) {
      if (!(await fs.lstat(entrypoint).catch(() => null))?.isFile()) throw error;
    }
    return entrypoint;
  } finally {
    await fs.rm(staging, { recursive: true, force: true }).catch(() => {});
  }
}
