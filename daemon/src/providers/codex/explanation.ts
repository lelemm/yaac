import { COMMAND_EXPLANATION_INSTRUCTIONS } from "../command-explanation.js";
import type { CodexConnection } from "./rpc.js";
import type { Thread, ThreadItem, Turn } from "./wire.js";

export const EXPLANATION_SOURCE = "yaac.command-explanation";

export async function explainCommand(
  conn: CodexConnection,
  cwd: string,
  model: string,
  command: string,
  remember: (id: string) => void,
  signal?: AbortSignal,
): Promise<string | null> {
  const { config } = await conn.request("config/read", { cwd }, 4_000) as {
    config: { mcp_servers?: Record<string, unknown> };
  };
  const disabledServers = Object.fromEntries(Object.keys(config.mcp_servers ?? {})
    .map((name) => [`mcp_servers.${name}.enabled`, false]));
  if (signal?.aborted) return null;
  const { thread } = await conn.request("thread/start", {
    cwd,
    model,
    ephemeral: true,
    threadSource: EXPLANATION_SOURCE,
    approvalPolicy: "never",
    sandbox: "read-only",
    baseInstructions: COMMAND_EXPLANATION_INSTRUCTIONS,
    developerInstructions: COMMAND_EXPLANATION_INSTRUCTIONS,
    config: {
      ...disabledServers,
      "features.shell_tool": false,
      "features.unified_exec": false,
      "features.hooks": false,
      "features.apps": false,
      "features.plugins": false,
      "features.multi_agent": false,
      "features.multi_agent_v2": false,
      "features.skill_search": false,
      "features.view_image": false,
      "features.goals": false,
      "features.request_permissions_tool": false,
      "project_doc_max_bytes": 0,
      web_search: "disabled",
    },
  }, 8_000) as { thread: Thread };
  const threadId = thread.id;
  remember(threadId);
  let turnId: string | undefined;
  let finished = false;
  let text: string | null = null;
  let unexpectedItem = false;
  let settle!: (value: string | null) => void;
  const completion = new Promise<string | null>((resolve) => { settle = resolve; });
  const offClose = conn.closed.on(() => settle(null));
  const offNotification = conn.notifications.on(({ method, params }) => {
    const event = params as { threadId?: string; turn?: Turn; item?: ThreadItem } | null;
    if (event?.threadId !== threadId) return;
    if (method === "turn/started") turnId = event.turn?.id;
    if ((method === "item/started" || method === "item/completed") && event.item) {
      const { type, text: body } = event.item;
      if (type === "agentMessage" && method === "item/completed" && typeof body === "string") text = body;
      if (type !== "agentMessage" && type !== "userMessage" && type !== "reasoning") {
        unexpectedItem = true;
        settle(null);
      }
    }
    if (method === "turn/completed") {
      finished = true;
      settle(event.turn?.status === "completed" && !unexpectedItem ? text : null);
    }
  });
  const abort = () => settle(null);
  signal?.addEventListener("abort", abort, { once: true });
  const timeout = setTimeout(() => settle(null), 20_000).unref();
  try {
    // Verify the server honored the option before sharing the command with it.
    if (thread.ephemeral !== true || signal?.aborted) return null;
    const { turn } = await conn.request("turn/start", {
      threadId,
      input: [{ type: "text", text: command, text_elements: [] }],
    }, 8_000) as { turn: Turn };
    turnId ??= turn.id;
    return await completion;
  } finally {
    signal?.removeEventListener("abort", abort);
    clearTimeout(timeout);
    offNotification();
    offClose();
    if (turnId && !finished) await conn.request("turn/interrupt", { threadId, turnId }, 2_000).catch(() => {});
    await conn.request("thread/unsubscribe", { threadId }, 2_000).catch(() => {});
  }
}
