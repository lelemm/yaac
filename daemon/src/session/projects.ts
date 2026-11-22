// Projects the user created (docs/05-threads.md#projects), stored on the target in
// `projects.json`, and the rule that puts a thread in a project.

import * as path from "node:path";
import type { Project, ProjectRef, RepoLocation } from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import { nowIso, randomId, readJson, writeJsonAtomic } from "../util/fsx.js";

/** Where a thread runs, as the matching rule sees it. */
export interface Location {
  folder: string;
  repo?: RepoLocation | null;
}

/** Path components, by `/` or `\`, ignoring empty and `.` components. */
export function components(p: string): string[] {
  return p.split(/[\\/]+/).filter((c) => c.length > 0 && c !== ".");
}

/** Whether `inner` is `outer` or inside it, by whole components. */
export function contains(outer: string, inner: string): boolean {
  const o = components(outer);
  const i = components(inner);
  return o.length <= i.length && o.every((c, k) => c === i[k]);
}

/**
 * The project a thread belongs to, or null (a temporary project). First the repository rule:
 * the same repository, and the thread's path relative to its checkout's root inside the
 * project's — whichever checkout it is physically in; the deepest such project wins. Then the
 * folder rule: the deepest project whose folder contains the thread's folder.
 */
export function matchProject<P extends { folder: string; repo?: RepoLocation | null }>(projects: readonly P[], loc: Location): P | null {
  const repo = loc.repo;
  if (repo) {
    let best: P | null = null;
    let depth = -1;
    for (const p of projects) {
      if (!p.repo || p.repo.repo !== repo.repo || !contains(p.repo.relative, repo.relative)) continue;
      const d = components(p.repo.relative).length;
      if (d > depth) {
        best = p;
        depth = d;
      }
    }
    if (best) return best;
  }
  let best: P | null = null;
  let depth = -1;
  for (const p of projects) {
    if (!contains(p.folder, loc.folder)) continue;
    const d = components(p.folder).length;
    if (d > depth) {
      best = p;
      depth = d;
    }
  }
  return best;
}

/** The `ProjectRef` a thread's summary carries: its user project, or a temporary one. */
export function projectRefFor(projects: readonly Project[], loc: Location): ProjectRef {
  const p = matchProject(projects, loc);
  if (!p) return { kind: "temporary", path: loc.folder, displayName: loc.folder };
  return userProjectRef(p);
}

/** The `ProjectRef` of one user project. */
export function userProjectRef(p: Project): ProjectRef {
  const ref: ProjectRef = { kind: "user", id: p.id, path: p.folder, displayName: p.name };
  if (p.repo) ref.repo = p.repo;
  if (p.crossTargetKey) ref.crossTargetKey = p.crossTargetKey;
  return ref;
}

interface StoreFile {
  projects: Project[];
}

export class ProjectStore {
  private data: StoreFile;

  constructor(private readonly file: string) {
    let loaded: StoreFile | undefined;
    try {
      loaded = readJson<StoreFile>(file);
    } catch {
      loaded = undefined;
    }
    this.data = loaded && Array.isArray(loaded.projects) ? loaded : { projects: [] };
  }

  list(): Project[] {
    return [...this.data.projects].sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
  }

  get(id: unknown): Project | undefined {
    return typeof id === "string" ? this.data.projects.find((p) => p.id === id) : undefined;
  }

  require(id: unknown): Project {
    const p = this.get(id);
    if (!p) throw new RpcError("invalid_params", `no project ${String(id)} on this target`);
    return p;
  }

  create(folder: string, name: string, repo: RepoLocation | null, crossTargetKey: string | null): Project {
    const p: Project = { id: randomId("pj_", 9), name, folder: path.resolve(folder), createdAt: nowIso() };
    if (repo) p.repo = repo;
    if (crossTargetKey) p.crossTargetKey = crossTargetKey;
    this.data.projects.push(p);
    this.save();
    return p;
  }

  rename(id: string, name: string): Project {
    const p = this.require(id);
    p.name = name;
    this.save();
    return p;
  }

  setRepo(id: string, repo: RepoLocation | null): boolean {
    const p = this.get(id);
    if (!p) return false;
    const before = JSON.stringify(p.repo ?? null);
    if (repo) p.repo = repo;
    else delete p.repo;
    if (JSON.stringify(p.repo ?? null) === before) return false;
    this.save();
    return true;
  }

  delete(id: string): void {
    this.require(id);
    this.data.projects = this.data.projects.filter((p) => p.id !== id);
    this.save();
  }

  private save(): void {
    writeJsonAtomic(this.file, this.data);
  }
}
