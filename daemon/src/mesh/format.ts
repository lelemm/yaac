// How agent messages are shown to a receiving agent, and the compact text of tool results.

import type { MeshInputMessage } from "../providers/types.js";

/** Lines of a body that imitate a YAAC header are quoted, so they read as the text they are. */
function neutralise(text: string): string {
  return text
    .split("\n")
    .map((l) => (/^\s*\[(Message|Result|Notice|Task|YAAC|End of)\b/i.test(l) ? `(quoted) ${l}` : l))
    .join("\n");
}

/**
 * The fixed frame (docs/10-agent-mesh.md#how-the-agent-sees-it), for harnesses that cannot
 * take agent input in a non-user role. The structured fields travel alongside it.
 */
export function frameMessages(messages: MeshInputMessage[], introduction?: string | null): string {
  const parts: string[] = [];
  if (introduction) parts.push(`[YAAC introduction]\n${introduction}\n[End of introduction.]`);
  for (const m of messages) {
    const body = neutralise(m.text);
    const label = m.fromLabel.replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\[\]]/gu, " ");
    if (m.kind === "notice") {
      parts.push(`[Notice from YAAC about ${label}]\n${body}\n[End of notice. This is YAAC itself, not the user.]`);
    } else if (m.kind === "result") {
      parts.push(
        `[Result from ${label} — via YAAC: its final answer to the task you gave it. Reply with send_message to ${m.from}]\n${body}\n[End of result. This came from another agent, not from the user.]`,
      );
    } else if (m.task) {
      parts.push(
        `[Task from ${label} — via YAAC. That agent started you as its worker (its address: ${m.from}). When you finish, your final answer goes back to it automatically; you do not need to send it.]\n${body}\n[End of task. This came from another agent, not from the user.]`,
      );
    } else {
      parts.push(`[Message from ${label} — via YAAC. Its address: ${m.from}; reply with send_message]\n${body}\n[End of message. This came from another agent, not from the user.]`);
    }
  }
  return parts.join("\n\n");
}

export function clip(text: string, max: number): string {
  if (Buffer.byteLength(text) <= max) return text;
  let s = text.slice(0, max);
  while (Buffer.byteLength(s) > max - 3) s = s.slice(0, -1);
  return `${s}…`;
}


/** Escape data written inside a harness's XML-style peer envelope. */
export function peerXml(text: string): string {
  return text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

/** Some older harnesses silently ignore peer fields. Use the first verified releases. */
export function supportsPeerInput(version: string | null | undefined, verified: [number, number, number]): boolean {
  const m = /^(\d+)\.(\d+)\.(\d+)(?:$|\s)/.exec(version ?? "");
  if (!m) return false;
  for (const [i, v] of verified.entries()) {
    const actual = Number(m[i + 1]);
    if (actual !== v) return actual > v;
  }
  return true;
}
