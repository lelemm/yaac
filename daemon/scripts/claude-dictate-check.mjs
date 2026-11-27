#!/usr/bin/env node
// Manual check of Claude Code's host dictation (`dictate` control request) — never run in CI.
//
//   node scripts/claude-dictate-check.mjs                 # gate only: sends `dictate {op:"cancel"}`
//   node scripts/claude-dictate-check.mjs audio.pcm [lang] # also transcribes a raw file
//
// The second form sends real audio to Anthropic's speech-to-text with the CLI's own claude.ai
// sign-in (16 kHz, mono, signed 16-bit little-endian, e.g.
// `ffmpeg -i in.wav -f s16le -ac 1 -ar 16000 audio.pcm`). It starts Claude Code the way yaacd's
// dictation process does: through the Agent SDK, with CLAUDE_CODE_ENTRYPOINT=claude-vscode.
// Set CLAUDE_EXE to pick the executable (default: `claude` on PATH).
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { query } from "@anthropic-ai/claude-agent-sdk";

const exe = process.env.CLAUDE_EXE ?? execFileSync("sh", ["-c", "command -v claude"], { encoding: "utf8" }).trim();
const [file, language] = process.argv.slice(2);
const input = { [Symbol.asyncIterator]: () => ({ next: () => new Promise(() => {}) }) };
const q = query({
  prompt: input,
  options: {
    pathToClaudeCodeExecutable: exe,
    env: { ...process.env, CLAUDE_CODE_ENTRYPOINT: "claude-vscode" },
    persistSession: false,
    strictMcpConfig: true,
    tools: [],
    settings: { disableAllHooks: true },
    canUseTool: async () => ({ behavior: "deny", message: "dictation only" }),
  },
});
(async () => { try { for await (const _ of q) void _; } catch { /* closed below */ } })();
const dictate = async (req) => (await q.request({ subtype: "dictate", ...req })).response;
try {
  const init = await q.initializationResult();
  console.log("claude", init.claude_code_version);
  console.log("cancel →", JSON.stringify(await dictate({ op: "cancel" })));
  if (file) {
    const pcm = readFileSync(file);
    console.log("start →", JSON.stringify(await dictate({ op: "start", ...(language ? { language } : {}) })));
    for (let at = 0; at < pcm.length; at += 6400) {
      const r = await dictate({ op: "audio", pcm: pcm.subarray(at, at + 6400).toString("base64") });
      console.log("audio →", JSON.stringify(r));
      await new Promise((res) => setTimeout(res, 200)); // real time: 6400 bytes = 200 ms
    }
    console.log("stop →", JSON.stringify(await dictate({ op: "stop" })));
  }
} catch (e) {
  console.log("refused:", e.message, e.errorCode ?? "");
} finally {
  q.close();
}
