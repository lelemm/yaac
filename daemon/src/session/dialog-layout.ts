// Runtime validation for the channel protocol. All persisted nodes pass through these schemas,
// which strip properties outside the contract. In particular, secret inputs have no defaults.

import { z } from "zod";
import type { DialogAnswer, DialogNode, DialogOpenParams } from "../protocol/generated/types.js";
import { RpcError } from "../protocol/rpc.js";

const KiB = 1024;
export const DIALOG_BYTES = 256 * KiB;
const IMAGE_BYTES = 1024 * KiB;
export const CHANNEL_LINE_BYTES = DIALOG_BYTES + IMAGE_BYTES + 4 * KiB;
const MAX_NODES = 500;
const MAX_DEPTH = 12;

const nonblank = (value: string) => value.trim().length > 0;
const id = z.string().max(128).refine(nonblank);
const name = z.string().max(256).refine(nonblank);
const title = z.string().max(DIALOG_BYTES).refine(nonblank);
const boolean = z.boolean().optional();
const number = z.number().optional();
const text = z.string().optional();
const input = { id, label: text, help: text, required: boolean, disabled: boolean };
const bounds = { default: number, min: number, max: number, step: z.number().positive().optional() };
const option = z.object({ value: z.string(), label: z.string(), detail: text });
const select = { ...input, options: z.array(option), default: text, placeholder: text };
const children = z.array(z.unknown()).max(MAX_NODES);
const schemas = {
  column: z.object({ children, gap: z.number().nonnegative().optional() }),
  row: z.object({ children, gap: z.number().nonnegative().optional() }),
  group: z.object({ children, label: text }),
  divider: z.object({}),
  text: z.object({ text: z.string(), style: text, tone: text }),
  markdown: z.object({ text: z.string() }),
  image: z.object({ src: z.string(), alt: z.string(), maxHeight: z.number().positive().optional() }),
  link: z.object({ label: z.string(), url: z.string() }),
  list: z.object({ items: z.array(z.string()) }),
  table: z.object({ columns: z.array(z.string()), rows: z.array(z.array(z.string())) }),
  progress: z.object({ value: z.number().min(0).max(1).optional() }),
  textInput: z.object({ ...input, default: text, placeholder: text, multiline: boolean, secret: boolean, maxLength: z.number().int().nonnegative().max(DIALOG_BYTES).optional() }),
  number: z.object({ ...input, ...bounds }),
  checkbox: z.object({ ...input, default: boolean }),
  toggle: z.object({ ...input, default: boolean }),
  select: z.object(select),
  radio: z.object(select),
  multiSelect: z.object({ ...input, options: z.array(option), default: z.array(z.string()).optional() }),
  date: z.object({ ...input, default: text }),
  slider: z.object({ ...input, ...bounds, min: z.number(), max: z.number() }),
  button: z.object({ id, label: z.string(), kind: text, submit: boolean, requires: z.array(id).optional() }),
};

type Kind = keyof typeof schemas;
type KnownNode = {
  [K in Kind]: K extends "divider" ? { type: K } : { type: K } & z.infer<(typeof schemas)[K]>;
}[Kind];
type InputNode = Extract<Exclude<KnownNode, { type: "button" }>, { id: string }> & { unknown?: false };
type UnsupportedInput = { id: string; required?: boolean; disabled?: boolean; unknown: true };
type AnyInput = InputNode | UnsupportedInput;
type Button = Extract<KnownNode, { type: "button" }>;

export const registration = z.object({ name, description: z.string().max(1024).optional() });
export const withdrawal = z.object({ requestId: id });
const conversationId = z.string().max(256).refine(nonblank).nullish();
const open = z.object({ requestId: id, title, layout: z.unknown(), threadId: conversationId, harnessSessionId: conversationId });
const scalar = z.union([z.string(), z.number(), z.boolean(), z.array(z.string())]);
const answer = z.object({
  requestKind: z.literal("dialog"),
  action: id.optional(),
  cancelled: z.boolean().default(false),
  values: z.record(z.string(), scalar).default({}),
});

export function invalid(message: string): never {
  throw new RpcError("invalid_params", message);
}

function limit(message: string): never {
  throw new RpcError("limit_exceeded", message);
}

/** Give the caller the rejected field path without echoing input values. */
export function parse<S extends z.ZodType>(schema: S, value: unknown, at = "params"): z.output<S> {
  const p = schema.safeParse(value);
  if (p.success) return p.data;
  const field = p.error.issues[0]?.path.map(String).join(".");
  return invalid(`Invalid ${at}${field ? `.${field}` : ""}.`);
}

function urlHasProtocol(src: string, ...protocols: string[]): boolean {
  try {
    return protocols.includes(new URL(src).protocol);
  } catch {
    return false;
  }
}

function validValue(node: InputNode, value: unknown): boolean {
  switch (node.type) {
    case "textInput":
      return typeof value === "string" && (node.maxLength === undefined || [...value].length <= node.maxLength);
    case "number":
    case "slider":
      return typeof value === "number" && Number.isFinite(value)
        && (node.min === undefined || value >= node.min) && (node.max === undefined || value <= node.max);
    case "checkbox":
    case "toggle":
      return typeof value === "boolean";
    case "select":
    case "radio":
      return node.options.some((option) => option.value === value);
    case "multiSelect":
      return Array.isArray(value) && new Set(value).size === value.length
        && value.every((entry) => node.options.some((option) => option.value === entry));
    case "date": {
      if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
      const ms = Date.parse(value);
      return Number.isFinite(ms) && new Date(ms).toISOString().slice(0, 10) === value;
    }
    default:
      return false;
  }
}

function checkInput(node: InputNode): void {
  if (node.type === "textInput" && node.secret && node.default !== undefined) {
    invalid(`Secret input "${node.id}" cannot have a default.`);
  }
  if ((node.type === "number" || node.type === "slider") && node.min !== undefined && node.max !== undefined && node.min > node.max) {
    invalid(`Input "${node.id}" has a minimum above its maximum.`);
  }
  if ("options" in node && new Set(node.options.map((option) => option.value)).size !== node.options.length) {
    invalid(`Input "${node.id}" has duplicate option values.`);
  }
  if (node.default !== undefined && !validValue(node, node.default)) invalid(`Input "${node.id}" has an invalid default.`);
}

type Layout = { node: DialogNode; inputs: Map<string, AnyInput>; buttons: Map<string, Button>; imageBytes: number };

function readLayout(raw: unknown): Layout {
  const ids = new Set<string>();
  const inputs = new Map<string, AnyInput>();
  const buttons = new Map<string, Button>();
  let count = 0;
  let imageBytes = 0;
  function takeId(value: string) {
    if (ids.has(value)) invalid(`Duplicate dialog node ID "${value}".`);
    ids.add(value);
  }
  function walk(rawNode: unknown, depth: number, at: string): DialogNode {
    if (++count > MAX_NODES || depth > MAX_DEPTH) limit("A dialog may have at most 500 nodes and 12 levels.");
    const { type } = parse(z.object({ type: id }), rawNode, at);
    if (!Object.hasOwn(schemas, type)) {
      // Older renderers need the unknown type and an optional input marker. Its payload is
      // opaque to this daemon and could include a default whose secrecy it cannot determine.
      const unknown = parse(z.object({ id: id.optional(), required: boolean, disabled: boolean }), rawNode, at);
      if (unknown.id) {
        takeId(unknown.id);
        inputs.set(unknown.id, { id: unknown.id, required: unknown.required, disabled: unknown.disabled, unknown: true });
      } else if (unknown.required && !unknown.disabled) {
        invalid(`Required input at ${at} must carry an ID.`);
      }
      return { type, ...unknown } as DialogNode;
    }
    const kind = type as Kind;
    const node = { type: kind, ...parse(schemas[kind], rawNode, at) } as KnownNode;
    if ("children" in node) {
      return { ...node, children: node.children.map((child, i) => walk(child, depth + 1, `${at}.children.${i}`)) };
    }
    if (node.type === "image") {
      if (node.src.startsWith("data:")) {
        const bytes = Buffer.byteLength(node.src);
        imageBytes += bytes;
        if (imageBytes > IMAGE_BYTES) limit("Inline dialog images may use at most 1 MiB in total.");
        if (!/^data:image\/(?:png|jpeg|gif|webp);base64,[A-Za-z0-9+/]+={0,2}$/.test(node.src)) invalid(`Invalid raster data URL at ${at}.src.`);
      } else if (!urlHasProtocol(node.src, "https:")) invalid(`Expected an HTTPS image at ${at}.src.`);
    }
    if (node.type === "link" && !urlHasProtocol(node.url, "http:", "https:")) invalid(`Expected an HTTP or HTTPS link at ${at}.url.`);
    if ("id" in node) {
      takeId(node.id);
      if (node.type === "button") buttons.set(node.id, node);
      else {
        checkInput(node);
        inputs.set(node.id, node);
      }
    }
    return node as DialogNode;
  }

  const node = walk(raw, 1, "layout");
  for (const button of buttons.values()) {
    for (const id of button.requires ?? []) {
      const target = inputs.get(id);
      if (!target || target.disabled) invalid(`Button "${button.id}" requires an unavailable input "${id}".`);
    }
  }
  return { node, inputs, buttons, imageBytes };
}

export function parseOpen(raw: unknown): DialogOpenParams {
  const params = parse(open, raw);
  if (params.threadId && params.harnessSessionId) invalid("Provide either threadId or harnessSessionId.");
  const layout = readLayout(params.layout);
  const result = { ...params, layout: layout.node };
  if (Buffer.byteLength(JSON.stringify(result)) - layout.imageBytes > DIALOG_BYTES) limit("Dialog content may use at most 256 KiB excluding inline images.");
  return result;
}

export function parseAnswer(raw: unknown, node: DialogNode): { answer: DialogAnswer; recorded: DialogAnswer } {
  const accepted: DialogAnswer = parse(answer, raw);
  const values = accepted.values ?? {};
  if (accepted.cancelled) {
    if (accepted.action !== undefined || Object.keys(values).length) invalid("A cancellation cannot carry an action or values.");
    return { answer: accepted, recorded: accepted };
  }
  const { inputs, buttons } = readLayout(node);
  const button = accepted.action ? buttons.get(accepted.action) : undefined;
  if (!button) return invalid("An answer needs a button ID from this dialog.");
  const required = new Set(button.requires ?? []);
  if (button.submit !== false) {
    for (const [id, field] of inputs) if (field.required && !field.disabled) required.add(id);
  }
  for (const id of required) {
    const value = Object.hasOwn(values, id) ? values[id] : undefined;
    if (value === undefined || value === "" || (Array.isArray(value) && value.length === 0)) {
      invalid(`Input "${id}" is required.`);
    }
  }
  const publicValues = [];
  for (const [id, value] of Object.entries(values)) {
    const field = inputs.get(id);
    if (!field || field.disabled || field.unknown || !validValue(field, value)) invalid(`Input "${id}" has an invalid value.`);
    if (!(field.type === "textInput" && field.secret)) publicValues.push([id, value] as const);
  }
  if (Buffer.byteLength(JSON.stringify(values)) > DIALOG_BYTES) limit("Dialog answers may use at most 256 KiB.");
  return { answer: accepted, recorded: { ...accepted, values: Object.fromEntries(publicValues) } };
}
