// Which mode a spawned worker runs in (D146; docs/10-agent-mesh.md#spawning): the mode that
// corresponds to its parent's effective mode, never more permissive. Modes are compared only
// by what their providers report — `skipsApprovals` and `autonomy` — never by id or by
// provider kind (except that, on the parent's own instance, the same id is the same mode).

import type { ModeAutonomy, ModeId, ModeInfo } from "../protocol/generated/types.js";

/** The scale, least first (the order of `ModeAutonomy` in yaac-protocol). */
const SCALE: readonly ModeAutonomy[] = ["readOnly", "ask", "edits", "workspace", "judged", "full"];

/** A mode's place on the scale; `null` when its provider did not say (never compared). */
export function rankOf(m: ModeInfo | null | undefined): number | null {
  if (!m) return null;
  if (m.skipsApprovals) return SCALE.length - 1;
  const i = m.autonomy ? SCALE.indexOf(m.autonomy) : -1;
  return i >= 0 ? i : null;
}

export interface ParentMode {
  /** The parent's provider instance, when it is on this target (same instance: same ids). */
  instance: string | null;
  /** The mode id the parent's harness reports it is using (null: its harness's default). */
  id: ModeId | null;
  /** That mode as the parent's provider describes it, when known. */
  info: ModeInfo | null;
}

export interface WorkerProvider {
  instance: string;
  displayName: string;
  modes: ModeInfo[];
}

export interface ModeChoice {
  /** The mode to start the worker in; null: send none (the harness's default). */
  mode: ModeId | null;
  /** One sentence for the spawn result and the worker's transcript. */
  note: string;
}

export class ModeRefused extends Error {}

function label(m: ModeInfo): string {
  return m.displayName || m.id;
}

function defaultNote(w: WorkerProvider, why: string): ModeChoice {
  const d = w.modes.find((m) => m.isDefault);
  return { mode: null, note: `Runs in ${d ? `${label(d)}, ${w.displayName}'s default mode` : `${w.displayName}'s default mode`}: ${why}.` };
}

function listing(modes: ModeInfo[]): string {
  return modes.map((m) => `${m.id} ("${label(m)}")`).join(", ") || "none";
}

/** A mode as an agent named it: its id, else its display name (any case). */
function resolve(w: WorkerProvider, wanted: string): ModeInfo {
  const exact = w.modes.find((m) => m.id === wanted);
  if (exact) return exact;
  const w2 = wanted.toLowerCase();
  const byName = w.modes.filter((m) => m.id.toLowerCase() === w2 || label(m).toLowerCase() === w2);
  if (byName.length === 1) return byName[0]!;
  if (w.modes.length === 0) throw new ModeRefused(`${w.displayName} offers no modes to choose from; leave mode out.`);
  throw new ModeRefused(`${w.displayName} has no mode "${wanted}". Its modes: ${listing(w.modes)}.`);
}

/**
 * The worker's mode. `requested`: `spawn_thread`'s `mode`, allowed only when it is no more
 * permissive than the parent's (or is the parent's own mode on the same instance); otherwise
 * `ModeRefused`. Without it, the parent's mode is inherited: on the same instance, the same id;
 * elsewhere, the worker provider's most permissive mode that is no more permissive than the
 * parent's — preferring one with the parent's id, then the harness default, then the first
 * listed. When none can be chosen, no mode is sent and the note says why.
 */
export function chooseWorkerMode(parent: ParentMode, worker: WorkerProvider, requested: string | null): ModeChoice {
  const sameInstance = parent.instance !== null && parent.instance === worker.instance;
  const parentName = parent.info ? label(parent.info) : parent.id ? `"${parent.id}"` : "its harness's default mode";
  const parentRank = rankOf(parent.info);
  if (requested) {
    const m = resolve(worker, requested);
    if (sameInstance && m.id === parent.id) return { mode: m.id, note: `Runs in ${label(m)}, as asked (the same mode as its parent).` };
    const r = rankOf(m);
    if (parentRank === null) {
      throw new ModeRefused(`Your own mode (${parentName}) cannot be compared with ${label(m)}, so a worker cannot be given it: a worker never gets more freedom than its parent. Leave mode out to inherit yours.`);
    }
    if (r === null) {
      throw new ModeRefused(`${worker.displayName} does not say how permissive ${label(m)} is, so it cannot be checked against your own mode (${parentName}). Leave mode out to inherit yours.`);
    }
    if (r > parentRank) {
      const allowed = worker.modes.filter((x) => {
        const xr = rankOf(x);
        return xr !== null && xr <= parentRank;
      });
      throw new ModeRefused(`${label(m)} is more permissive than your own mode (${parentName}); a worker never gets more freedom than its parent. Allowed: ${listing(allowed)}.`);
    }
    return { mode: m.id, note: `Runs in ${label(m)}, as asked (no more permissive than its parent's ${parentName}).` };
  }
  if (sameInstance) {
    if (!parent.id) return { mode: null, note: `Runs in ${worker.displayName}'s default mode, as its parent.` };
    const own = worker.modes.find((m) => m.id === parent.id);
    return { mode: parent.id, note: `Runs in ${own ? label(own) : parentName}, as its parent.` };
  }
  if (worker.modes.length === 0) return defaultNote(worker, `${worker.displayName} offers no modes to choose from`);
  if (parentRank === null) {
    return defaultNote(worker, parent.info || parent.id ? `its parent's mode (${parentName}) cannot be compared with ${worker.displayName}'s modes` : "its parent's mode is not known");
  }
  let best: ModeInfo | null = null;
  let bestRank = -1;
  const score = (m: ModeInfo) => (m.id === parent.id ? 2 : m.isDefault ? 1 : 0);
  for (const m of worker.modes) {
    const r = rankOf(m);
    if (r === null || r > parentRank) continue;
    if (r > bestRank || (r === bestRank && best && score(m) > score(best))) {
      best = m;
      bestRank = r;
    }
  }
  if (!best) return defaultNote(worker, `${worker.displayName} has no mode as restrictive as its parent's (${parentName})`);
  if (bestRank === parentRank) {
    const same = parent.info && label(best) === label(parent.info);
    return { mode: best.id, note: same ? `Runs in ${label(best)}, as its parent.` : `Runs in ${label(best)}, as its parent (which runs in ${parentName}).` };
  }
  return { mode: best.id, note: `Runs in ${label(best)}: the closest ${worker.displayName} mode to its parent's ${parentName} that is no more permissive.` };
}
