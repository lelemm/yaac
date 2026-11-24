import type { BrowserAction, BrowserCallParams, BrowserCallResult } from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";
import type { MeshToolResult } from "../providers/types.js";
import type { ToolDef } from "./tools.js";

const tab = { type: "string", minLength: 1, description: "Tab id from browser_list or browser_open." };
const cursor = { type: "integer", minimum: 0, description: "Cursor from the previous read." };
const url = { type: "string", minLength: 1, description: "A URL, host:port, or a bare port (5173 means http://localhost:5173/). localhost is this machine." };
const ref = { type: "string", minLength: 1, description: "Element reference from browser_snapshot." };
const selector = { type: "string", minLength: 1, description: "CSS selector; the first match is used." };
const point = (axis: string) => ({ type: "integer", description: `${axis} position in CSS pixels from the page's top left, with ${axis === "Horizontal" ? "y" : "x"}.` });

function tool(kind: BrowserAction["kind"], description: string, properties: Record<string, unknown>, required = "tab" in properties ? ["tab"] : []): ToolDef {
  return {
    name: "browser_" + kind,
    description: description + " Tabs belong to your thread and see the network from this machine.",
    inputSchema: { type: "object", properties, required, additionalProperties: false },
  };
}

export const BROWSER_TOOLS = [
  tool("list", "List the thread's open tabs.", {}),
  tool("open", "Open a tab in the thread's browser area without switching the user's view; waits (bounded) for the page to load and returns the tab.", { url }, ["url"]),
  tool("close", "Close a tab.", { tab }),
  tool("navigate", "Load a URL in a tab, or go back, forward, or reload; waits (bounded) for the load and returns the tab (loading true if it had not finished).",
    { tab, url, history: { type: "string", enum: ["back", "forward", "reload"] } }),
  tool("snapshot", "Read a tab's accessible text tree, with element references for click, type and press.", { tab }),
  tool("screenshot", "Capture a PNG of the tab or a matching element.", { tab, selector: { type: "string" } }),
  tool("click", "Click an element (by ref or selector) or a point (x and y).", { tab, ref, selector, x: point("Horizontal"), y: point("Vertical") }),
  tool("type", "Type text into an element (by ref or selector), or into the focused element.",
    { tab, text: { type: "string" }, ref, selector, clear: { type: "boolean", description: "Replace the current value instead of appending." } }, ["tab", "text"]),
  tool("press", "Press a key, with modifiers joined by + (Enter, Tab, Escape, ArrowDown, Control+a); focuses an element first when ref or selector is given.",
    { tab, key: { type: "string", minLength: 1 }, ref, selector }, ["tab", "key"]),
  tool("eval", "Run a JavaScript expression in the page (a promise is awaited) and return its JSON value.", { tab, expression: { type: "string", minLength: 1 } }, ["tab", "expression"]),
  tool("console", "Read console messages and uncaught exceptions.", { tab, cursor }),
  tool("network", "Read recent request metadata and failures.", { tab, cursor }),
];

function invalid(message: string): never {
  throw new RpcError("invalid_params", message);
}
function text(a: Record<string, unknown>, key: string, required = false): string | undefined {
  const v = a[key];
  if (v === undefined && !required) return undefined;
  if (typeof v !== "string" || (!v && key !== "text")) invalid(`Expected a string ${key}.`);
  return v;
}
function element(a: Record<string, unknown>): { ref?: string; selector?: string } {
  const r = text(a, "ref"), selector = text(a, "selector");
  if (r !== undefined && selector !== undefined) invalid("Give ref or selector, not both.");
  return { ...(r !== undefined ? { ref: r } : {}), ...(selector !== undefined ? { selector } : {}) };
}

/** Builds the wire action; the thread id is never taken from the arguments. */
export function parseAction(def: ToolDef, a: Record<string, unknown>): BrowserAction {
  if (Object.keys(a).some(k => !Object.hasOwn(def.inputSchema.properties, k))) {
    invalid("Unexpected browser tool argument.");
  }
  const kind = def.name.slice("browser_".length) as BrowserAction["kind"];
  if (kind === "list") return { kind };
  if (kind === "open") return { kind, url: text(a, "url", true)! };
  if (typeof a.tab !== "string" || !a.tab) invalid("A tab id is required.");
  const tab = a.tab;
  switch (kind) {
    case "close":
    case "snapshot":
      return { kind, tab };
    case "screenshot":
      if (a.selector !== undefined && typeof a.selector !== "string") invalid("Expected a CSS selector.");
      return { kind, tab, selector: a.selector as string | undefined };
    case "navigate": {
      const to = text(a, "url"), history = a.history;
      if (history !== undefined && history !== "back" && history !== "forward" && history !== "reload") invalid("history is back, forward or reload.");
      if ((to === undefined) === (history === undefined)) invalid("Give exactly one of url and history.");
      return to !== undefined ? { kind, tab, url: to } : { kind, tab, history: history as "back" | "forward" | "reload" };
    }
    case "click": {
      const target = element(a);
      const xy = [a.x, a.y];
      if (xy.some(v => v !== undefined && (typeof v !== "number" || !Number.isInteger(v)))) invalid("x and y are integers.");
      const atPoint = a.x !== undefined || a.y !== undefined;
      if (atPoint && (a.x === undefined || a.y === undefined)) invalid("Give both x and y.");
      if (atPoint === (Object.keys(target).length > 0)) invalid("Give one of ref, selector, or x and y.");
      return atPoint ? { kind, tab, x: a.x as number, y: a.y as number } : { kind, tab, ...target };
    }
    case "type":
      if (a.clear !== undefined && typeof a.clear !== "boolean") invalid("clear is a boolean.");
      return { kind, tab, text: text(a, "text", true)!, ...element(a), ...(a.clear !== undefined ? { clear: a.clear as boolean } : {}) };
    case "press":
      return { kind, tab, key: text(a, "key", true)!, ...element(a) };
    case "eval":
      return { kind, tab, expression: text(a, "expression", true)! };
    case "console":
    case "network":
      if (a.cursor !== undefined && (typeof a.cursor !== "number" || !Number.isSafeInteger(a.cursor) || a.cursor < 0)) {
        invalid("Expected a nonnegative integer cursor.");
      }
      return { kind, tab, cursor: a.cursor as number | undefined };
  }
  invalid("Unknown browser tool.");
}

export async function inspectBrowser(
  def: ToolDef, args: Record<string, unknown>, threadId: string,
  request: ((p: BrowserCallParams) => Promise<unknown>) | null, acceptsImage: () => Promise<boolean>,
): Promise<MeshToolResult> {
  if (!request) throw new RpcError("no_window", "No YAAC window is open; the browser needs one.");
  const action = parseAction(def, args);
  if (action.kind === "screenshot" && !await acceptsImage()) {
    throw new RpcError("capability_missing", "This model has no confirmed image input support. Use browser_snapshot for text.");
  }
  const reply = await request({ threadId, action }) as BrowserCallResult;
  if (reply?.kind !== action.kind) throw new Error("The window returned a mismatched browser result.");
  if (reply.kind === "screenshot") {
    const bytes = typeof reply.data === "string" ? Buffer.from(reply.data, "base64") : Buffer.alloc(0);
    if (!bytes.subarray(0, 8).equals(Buffer.from("89504e470d0a1a0a", "hex")) || bytes.toString("base64") !== reply.data) {
      throw new Error("The window returned an invalid PNG image block.");
    }
    return { isError: false, text: "", image: { type: "image", mimeType: "image/png", data: reply.data } };
  }
  if (reply.kind === "snapshot") return { isError: false, text: reply.tree };
  if (reply.kind === "eval") return { isError: false, text: JSON.stringify(reply.value ?? null) };
  if (reply.kind === "close" || reply.kind === "click" || reply.kind === "type" || reply.kind === "press") {
    return { isError: false, text: "Done." };
  }
  return { isError: false, text: JSON.stringify(reply) };
}
