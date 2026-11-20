// The fake provider's script language: directives in the user's message text, one per line,
// each starting with `/fake`. Documented in README.md next to this file.

export type Step =
  | { k: "say"; text: string }
  | { k: "reason"; text: string }
  | { k: "tool"; command: string; output: string; exitCode: number }
  | { k: "bigOutput"; bytes: number }
  | { k: "approve"; command: string }
  | { k: "ask"; text: string; options: string[]; multi: boolean; secret: boolean }
  | { k: "plan"; steps: string[] }
  | { k: "stall" }
  | { k: "sleep"; ms: number }
  | { k: "pace"; ms: number }
  | { k: "fail"; message: string }
  | { k: "inUse"; holder: string; ms: number | null }
  | { k: "externalTurn"; ms: number; text: string }
  | { k: "outsideThread"; title: string }
  | { k: "usage" }
  | { k: "crash" }
  | { k: "crashOnAnswer" }
  | { k: "resolveElsewhere"; ms: number }
  | { k: "expire"; ms: number };

export interface Script {
  steps: Step[];
  /** No directive at all: the fake echoes the message. */
  plain: boolean;
}

function num(s: string | undefined, fallback: number): number {
  const n = Number.parseInt(s ?? "", 10);
  return Number.isFinite(n) && n >= 0 ? n : fallback;
}

export function parseScript(text: string): Script {
  const steps: Step[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line.startsWith("/fake ") && line !== "/fake") continue;
    const body = line.slice(5).trim();
    const sp = body.indexOf(" ");
    const cmd = (sp < 0 ? body : body.slice(0, sp)).toLowerCase();
    const rest = sp < 0 ? "" : body.slice(sp + 1).trim();
    const words = rest.split(/\s+/).filter(Boolean);
    switch (cmd) {
      case "say":
        steps.push({ k: "say", text: rest || "Hello from the fake provider." });
        break;
      case "reason":
        steps.push({ k: "reason", text: rest || "Thinking about it." });
        break;
      case "tool": {
        const [command, output] = rest.split("=>").map((s) => s.trim());
        steps.push({ k: "tool", command: command || "echo hello", output: output ?? `ran ${command || "echo hello"}`, exitCode: 0 });
        break;
      }
      case "big-output":
        steps.push({ k: "bigOutput", bytes: num(words[0], 2 * 1024 * 1024) });
        break;
      case "approve":
        steps.push({ k: "approve", command: rest || "rm -rf build" });
        break;
      case "ask":
      case "ask-multi":
      case "ask-secret": {
        const [q, ...opts] = rest.split("|").map((s) => s.trim());
        steps.push({
          k: "ask",
          text: q || (cmd === "ask-secret" ? "Enter the token" : "Which approach do you want?"),
          options: cmd === "ask-secret" ? [] : opts.filter(Boolean),
          multi: cmd === "ask-multi",
          secret: cmd === "ask-secret",
        });
        break;
      }
      case "plan":
        steps.push({ k: "plan", steps: rest.split("|").map((s) => s.trim()).filter(Boolean) });
        break;
      case "stall":
        steps.push({ k: "stall" });
        break;
      case "sleep":
        steps.push({ k: "sleep", ms: num(words[0], 1000) });
        break;
      case "pace":
        steps.push({ k: "pace", ms: num(words[0], 20) });
        break;
      case "fail":
        steps.push({ k: "fail", message: rest || "the fake provider failed this turn" });
        break;
      case "in-use": {
        const last = words.at(-1);
        const ms = last !== undefined && /^\d+$/.test(last) ? num(last, 0) : null;
        const holder = (ms === null ? rest : words.slice(0, -1).join(" ")) || "a terminal";
        steps.push({ k: "inUse", holder, ms });
        break;
      }
      case "external-turn": {
        const ms = /^\d+$/.test(words[0] ?? "") ? num(words[0], 2000) : 2000;
        const text = (/^\d+$/.test(words[0] ?? "") ? words.slice(1) : words).join(" ");
        steps.push({ k: "externalTurn", ms, text: text || "Working on it from the terminal." });
        break;
      }
      case "outside-thread":
        steps.push({ k: "outsideThread", title: rest || "A conversation started in a terminal" });
        break;
      case "usage":
        steps.push({ k: "usage" });
        break;
      case "crash":
        steps.push({ k: "crash" });
        break;
      case "crash-on-answer":
        steps.push({ k: "crashOnAnswer" });
        break;
      case "resolve-elsewhere":
        steps.push({ k: "resolveElsewhere", ms: num(words[0], 200) });
        break;
      case "expire":
        steps.push({ k: "expire", ms: num(words[0], 200) });
        break;
      default:
        steps.push({ k: "say", text: `Unknown fake directive: ${cmd}` });
    }
  }
  return { steps, plain: steps.length === 0 };
}

/** Splits text into at least `min` pieces (for streaming in several deltas). */
export function chunks(text: string, min = 4): string[] {
  if (text.length === 0) return [];
  const words = text.match(/\S+\s*|\s+/g) ?? [text];
  if (words.length >= min) return words;
  const size = Math.max(1, Math.ceil(text.length / min));
  const out: string[] = [];
  for (let i = 0; i < text.length; i += size) out.push(text.slice(i, i + size));
  return out;
}
