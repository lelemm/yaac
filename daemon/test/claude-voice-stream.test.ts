import assert from "node:assert/strict";
import test from "node:test";
import { EventEmitter } from "node:events";
import WebSocket from "ws";
import {
  ClaudeVoiceStream,
  voiceClientPlatform,
  voiceKeyterms,
  voiceLanguage,
  voiceUserAgent,
} from "../src/providers/claude/voice-stream.js";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

test("Claude voice_stream sends the CLI headers, query, binary PCM and exact controls", async () => {
  const controls: string[] = [];
  const audio: Buffer[] = [];
  let requestUrl = "";
  let headers: Record<string, string> = {};
  class FakeSocket extends EventEmitter {
    readyState: number = WebSocket.OPEN;
    binaryType = "nodebuffer";
    constructor(url: string, options: { headers: Record<string, string> }) {
      super();
      requestUrl = url;
      headers = options.headers;
      setImmediate(() => {
        this.emit("open");
        this.emit("message", Buffer.from(JSON.stringify({ type: "TranscriptInterim", data: "olá" })), false);
        this.emit("message", Buffer.from(JSON.stringify({ type: "TranscriptText", data: "olá mundo" })), false);
      });
    }
    send(data: Buffer | string, options?: { binary?: boolean } | ((error?: Error) => void), callback?: (error?: Error) => void): void {
      const binary = typeof options === "object" && options.binary;
      if (binary) audio.push(Buffer.from(data));
      else {
        const text = data.toString();
        controls.push(text);
        if (text === '{"type":"CloseStream"}') {
          setImmediate(() => this.emit("message", Buffer.from(JSON.stringify({ type: "TranscriptEndpoint" })), false));
        }
      }
      (typeof options === "function" ? options : callback)?.();
    }
    close(): void { this.readyState = WebSocket.CLOSED; }
    terminate(): void { this.readyState = WebSocket.CLOSED; }
  }

  const transcripts: Array<[string, boolean]> = [];
  const ended = deferred<string | undefined>();
  const dictation = new ClaudeVoiceStream({
    token: async () => "oauth-token-for-tests",
    version: () => "2.1.330",
    endpoint: "ws://fake.local/api/ws/speech_to_text/voice_stream",
    websocket: FakeSocket as unknown as typeof WebSocket,
    env: {
      CLAUDE_CODE_ENTRYPOINT: "sdk-ts",
      CLAUDE_AGENT_SDK_VERSION: "0.3.329",
      CLAUDE_AGENT_SDK_CLIENT_APP: "yaac",
      CLAUDE_CODE_VOICE_FORWARD_INTERIMS_TYPED: "1",
    },
    keyterms: ["alpha,beta", "alpha beta", "café"],
  }, { language: "pt-BR" }, {
    transcript: (text, final) => transcripts.push([text, final]),
    ended: (error) => ended.resolve(error),
  });
  await dictation.accepted;
  await dictation.audio(Buffer.from([1, 0, 2, 0]));
  dictation.stop();
  assert.equal(await ended.promise, undefined);
  const query = new URL(requestUrl).searchParams;
  assert.deepEqual(Object.fromEntries(query), {
    encoding: "linear16", sample_rate: "16000", channels: "1", endpointing_ms: "300",
    utterance_end_ms: "1000", language: "pt", use_conversation_engine: "true", forward_interims: "typed",
  });
  assert.equal(headers.Authorization, "Bearer oauth-token-for-tests");
  assert.equal(headers["x-app"], "cli");
  assert.equal(headers["anthropic-client-platform"], "claude_code_sdk");
  assert.equal(headers["User-Agent"], "claude-cli/2.1.330 (external, sdk-ts, agent-sdk/0.3.329, client-app/yaac)");
  assert.equal(headers["x-config-keyterms"], "alpha beta,caf");
  assert.deepEqual(controls, ['{"type":"KeepAlive"}', '{"type":"CloseStream"}']);
  assert.deepEqual(audio, [Buffer.from([1, 0, 2, 0])]);
  assert.deepEqual(transcripts, [["olá", false], ["olá mundo", false], ["olá mundo", true]]);
});

test("Claude voice header helpers match the CLI mapping and bounds", () => {
  assert.equal(voiceLanguage(" Português "), "pt");
  assert.equal(voiceLanguage("pt-BR"), "pt");
  assert.equal(voiceLanguage("unsupported"), "en");
  assert.equal(voiceClientPlatform("claude-vscode"), "claude_code_vscode");
  assert.equal(voiceClientPlatform("remote_mobile"), "claude_code_remote");
  assert.equal(voiceClientPlatform("mcp"), "claude_code_mcp");
  assert.equal(voiceClientPlatform(undefined), "claude_code_cli");
  assert.equal(voiceUserAgent("2.1.330", {}), "claude-cli/2.1.330 (external, cli)");
  assert.equal(voiceUserAgent("2.1.330", {}, "local-agent", "voice"), "claude-cli/2.1.330 (external, local-agent, workload/voice)");
  assert.equal(voiceKeyterms(["a,b", "a b", "é c"]), "a b,c");
  assert.equal(voiceKeyterms(["x".repeat(1025), "later"]), null, "the CLI stops at the first term that exceeds 1024");
});
