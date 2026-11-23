// One Claude query's shell metadata. Only the live task snapshot establishes membership.
import { randomUUID } from "node:crypto";
import { constants, promises as fs } from "node:fs";
import * as path from "node:path";
import type { CommandLogsResult, RunningCommand } from "../../protocol/generated/types.js";

interface Shell {
  view: RunningCommand;
  run: unknown;
  taskId: string;
  parent: string | undefined;
  toolId?: string;
  outputPath?: string;
  announced: boolean;
}

export class ClaudeCommands {
  active = new Map<string, Shell>();
  private tools = new Map<string, string>();
  private taskTools = new Map<string, string>();
  private results = new Map<string, { toolId: string; outputPath?: string }>();

  private remember<T>(map: Map<string, T>, id: string, value: T) {
    if (map.size >= 256 && !map.has(id)) map.delete(map.keys().next().value!);
    map.set(id, value);
  }

  /** Called before routing a message to the parent or a subagent's transcript. */
  message(m: { type: string; subtype?: string; session_id?: string; message?: unknown; tool_use_result?: unknown; task_id?: unknown; tool_use_id?: unknown }): boolean {
    if (m.type === "system" && m.subtype === "task_started" && typeof m.task_id === "string" && typeof m.tool_use_id === "string") {
      this.remember(this.taskTools, m.task_id, m.tool_use_id);
      return this.active.has(m.task_id);
    }
    const content = (m.message as { content?: unknown } | undefined)?.content;
    if (!Array.isArray(content)) return false;
    if (m.type === "assistant") {
      let changed = false;
      for (const b of content) {
        if (b?.type !== "tool_use" || b.name !== "Bash" || typeof b.id !== "string" || typeof b.input?.command !== "string") continue;
        this.remember(this.tools, b.id, b.input.command);
        changed ||= [...this.active.values()].some((shell) => this.toolId(shell) === b.id);
      }
      return changed;
    }
    if (m.type !== "user") return false;
    const result = m.tool_use_result as { backgroundTaskId?: unknown; backgroundOutputPath?: unknown } | undefined;
    const taskId = result?.backgroundTaskId;
    if (typeof taskId !== "string") return false;
    const matches = content.filter((b) => b?.type === "tool_result" && b.is_error !== true && this.tools.has(b.tool_use_id));
    if (matches.length !== 1) return false;
    const block = matches[0]!;
    const rendered = Array.isArray(block.content) ? block.content[0]?.text : block.content;
    // Older SDK messages carry the path in Claude's background-launch tool result.
    const fallback = typeof rendered === "string" && rendered.includes(taskId)
      ? /Output is being written to: ([^\r\n]+?\.output)(?:[.\s]|$)/.exec(rendered)?.[1]
      : undefined;
    const candidate = typeof result?.backgroundOutputPath === "string" ? result.backgroundOutputPath
      : fallback && path.basename(path.dirname(path.dirname(fallback))) === m.session_id ? fallback : undefined;
    const outputPath = candidate && path.isAbsolute(candidate) && path.basename(candidate) === `${taskId}.output`
      && path.basename(path.dirname(candidate)) === "tasks" ? candidate : undefined;
    this.remember(this.results, taskId, { toolId: block.tool_use_id, outputPath });
    return this.active.has(taskId);
  }

  replace(tasks: unknown[], canStop: boolean) {
    const current = new Map<string, Shell>();
    for (const task of tasks) {
      const t = task as Record<string, unknown> | null;
      if (t?.task_type !== "local_bash" || typeof t.task_id !== "string" || t.ambient === true) continue;
      const prev = this.active.get(t.task_id);
      const parent = typeof t.parent_task_id === "string" ? t.parent_task_id : undefined;
      let shell: Shell;
      if (prev && prev.run === t.run_id) {
        prev.parent = parent ?? prev.parent;
        shell = prev;
      } else {
        shell = {
          taskId: t.task_id, run: t.run_id, parent, announced: false,
          view: { id: randomUUID(), command: typeof t.description === "string" && t.description || "Shell command", observedAt: new Date().toISOString(), canStop },
        };
      }
      current.set(t.task_id, shell);
    }
    this.active = current;
  }

  private toolId(shell: Shell): string | undefined {
    return this.taskTools.get(shell.taskId) ?? this.results.get(shell.taskId)?.toolId;
  }

  enrich(shell: Shell): string | undefined {
    shell.toolId ??= this.toolId(shell);
    const result = this.results.get(shell.taskId);
    if (result?.toolId === shell.toolId) shell.outputPath ??= result?.outputPath;
    const command = shell.toolId && this.tools.get(shell.toolId);
    if (command && command !== shell.view.command) shell.view = { ...shell.view, command };
    if (shell.announced || !command) return undefined;
    shell.announced = true;
    return command;
  }
}

/** Use only a task's harness-reported file, and read a bounded tail without following a leaf symlink. */
export async function claudeCommandLogs(file: string | undefined): Promise<CommandLogsResult> {
  if (!file) return { truncated: false };
  let handle;
  try {
    handle = await fs.open(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
    const stat = await handle.stat();
    if (!stat.isFile()) return { truncated: false };
    const position = Math.max(0, stat.size - 32 * 1024);
    const buffer = Buffer.alloc(stat.size - position);
    const { bytesRead } = await handle.read(buffer, 0, buffer.length, position);
    return { output: buffer.subarray(0, bytesRead).toString("utf8"), truncated: position > 0 };
  } catch {
    return { truncated: false };
  } finally {
    await handle?.close().catch(() => {});
  }
}
