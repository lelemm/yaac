// Helpers for the agent-mesh tests: transcripts, tool outputs, and threads by title.

import type { Item } from "../src/protocol/generated/types.js";
import type { Daemon } from "../src/session/daemon.js";
import { readAll, startThread, until, type TestClient } from "./helpers.js";

/** A thread's items, oldest first. */
export async function itemsOf(c: TestClient, threadId: string): Promise<Item[]> {
  const pages = await readAll(c, threadId);
  return pages.flatMap((p) => p.items).reverse();
}

export type AnyItem = Item & Record<string, any>;

/** The outputs of a thread's `yaac.<tool>` calls (fake provider), oldest first. */
export async function toolOutputs(c: TestClient, threadId: string, tool?: string): Promise<Array<{ tool: string; output: string; failed: boolean; input: any }>> {
  return (await itemsOf(c, threadId))
    .map((i) => i as AnyItem)
    .filter((i) => i.kind === "toolCall" && typeof i.title === "string" && i.title.startsWith("yaac.") && (!tool || i.title === `yaac.${tool}`) && i.status !== "inProgress")
    .map((i) => ({ tool: i.title.slice(5), output: String(i.output ?? ""), failed: i.status === "failed", input: i.input }));
}

export async function kinds(c: TestClient, threadId: string, kind: string): Promise<AnyItem[]> {
  return (await itemsOf(c, threadId)).filter((i) => i.kind === kind) as AnyItem[];
}

/** Waits until a thread has `n` completed calls of a tool; returns them. */
export async function untilTool(c: TestClient, threadId: string, tool: string, n = 1, timeoutMs = 15000) {
  return until(async () => {
    const out = await toolOutputs(c, threadId, tool);
    return out.length >= n ? out : null;
  }, `${n} ${tool} call(s) in ${threadId}`, timeoutMs);
}

/** Starts a fake thread whose chat allows agents (no permission prompt). */
export async function startAllowed(c: TestClient, message: string, extra: Record<string, unknown> = {}) {
  return startThread(c, message, { agents: "allow", ...extra });
}

export function addressOf(d: Daemon, threadId: string): string {
  return d.mesh.addressOf(threadId);
}

/** The thread spawned by `parent` (the first, or the n-th). */
export async function childOf(d: Daemon, parentId: string, n = 0, timeoutMs = 15000): Promise<string> {
  return until(() => {
    const kids = d.mesh.store.peekThread(parentId)?.children ?? [];
    const a = kids[n];
    return a ? a.slice(a.indexOf("/") + 1) : null;
  }, `child ${n} of ${parentId}`, timeoutMs);
}

export function stateOf(d: Daemon, threadId: string): string {
  return d.core.summaryOf(d.core.thread(threadId)!).state;
}

export async function untilIdle(d: Daemon, threadId: string, timeoutMs = 15000): Promise<void> {
  await until(() => {
    const rt = d.core.thread(threadId);
    return rt && !rt.activeTurn && !rt.dispatching ? true : null;
  }, `${threadId} idle`, timeoutMs);
}
