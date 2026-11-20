// Canonical JSON (docs/03-protocol.md#messages; frozen core).
//
// Compact; every object has its keys sorted by code point; `undefined` members are omitted.
// The envelope order (`id`, `method`, then `params`/`result`/`error`) is applied by rpc.ts.
// This matches the Rust side byte for byte, which the frozen-core fixture test proves.

function compareCodePoints(a: string, b: string): number {
  const ia = a[Symbol.iterator]();
  const ib = b[Symbol.iterator]();
  for (;;) {
    const x = ia.next();
    const y = ib.next();
    if (x.done) return y.done ? 0 : -1;
    if (y.done) return 1;
    const cx = x.value.codePointAt(0)!;
    const cy = y.value.codePointAt(0)!;
    if (cx !== cy) return cx < cy ? -1 : 1;
  }
}

/** Encodes one JSON value canonically. */
export function canonical(value: unknown): string {
  if (value === null || value === undefined) return "null";
  switch (typeof value) {
    case "string":
      return JSON.stringify(value);
    case "number":
      return Number.isFinite(value) ? JSON.stringify(value) : "null";
    case "boolean":
      return value ? "true" : "false";
    case "bigint":
      return value.toString();
    case "object": {
      if (Array.isArray(value)) {
        return `[${value.map((v) => (v === undefined || typeof v === "function" ? "null" : canonical(v))).join(",")}]`;
      }
      const maybe = value as { toJSON?: () => unknown };
      if (typeof maybe.toJSON === "function") return canonical(maybe.toJSON());
      const entries = Object.keys(value)
        .filter((k) => {
          const v = (value as Record<string, unknown>)[k];
          return v !== undefined && typeof v !== "function";
        })
        .sort(compareCodePoints);
      return `{${entries.map((k) => `${JSON.stringify(k)}:${canonical((value as Record<string, unknown>)[k])}`).join(",")}}`;
    }
    default:
      return "null";
  }
}

/** Removes `undefined` members recursively (for values written to disk). */
export function stripUndefined<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}
