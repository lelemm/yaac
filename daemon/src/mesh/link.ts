// What the `yaacd mcp` bridge and the daemon share, kept free of any dependency so the bridge's
// own bundle stays small (docs/10-agent-mesh.md#attaching-the-mcp-server).

/** The 4-byte magic a bridge writes first on the daemon's socket (never the start of a YAAC frame). */
export const BRIDGE_MAGIC = Buffer.from("YMCP", "ascii");

/** An error as one readable line for an agent: never a stack trace. */
export function readable(e: unknown): string {
  const m = (e as { message?: unknown })?.message;
  return typeof m === "string" && m ? m : "the yaac tool failed";
}
