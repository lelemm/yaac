# Voice input through the harnesses

Can YAAC send recorded audio to Codex or Claude Code and get back text, using the voice feature
that each harness already has, so that we do not build our own speech-to-text? Checked on
**2026-11-27**. Everything under "Verified" was read from the installed tools on this machine
(WSL Ubuntu). Nothing was called at runtime: no realtime session was opened and no `dictate`
request was sent. The research was read-only.

| Component | Version examined | Where |
| --- | --- | --- |
| `codex` CLI | 0.172.0 | `~/.local/bin/codex` → `~/.codex/packages/standalone/current/bin/codex` |
| Codex app-server daemon | 0.172.0 (`codex app-server daemon version`) | `~/.codex/packages/app-server-daemon/current/bin/codex` |
| `claude` CLI | 2.1.330 | `~/.local/bin/claude` → `~/.local/share/claude/versions/2.1.330` |
| `@anthropic-ai/claude-agent-sdk` (+ bundled CLI) | 0.3.329 (CLI 2.1.329) | `daemon/node_modules/@anthropic-ai/` |

**Short answer.**

- **Codex:** yes, but only experimentally. The app-server has a `thread/realtime/*` family that
  takes PCM audio and streams back transcripts. YAAC already opts into experimental methods.
  This feature is a voice *conversation* with a realtime model, though, not plain dictation.
- **Claude Code:** the CLI has a real dictation request, control subtype `dictate`. It is
  marked `@internal` and is gated to first-party hosts ("in a public build only for the VS Code
  extension, behind a flag"). The Agent SDK does not expose it. **YAAC should not depend on it.**
- **Recommendation:** use the platform's own speech recognizer in the client (Android
  `SpeechRecognizer`, Windows speech). Treat Codex realtime as an optional extra that the
  target reports as a capability.

## Codex

### Verified

**Feature flags.** Output of `codex features list` (0.172.0):

```
in_app_dictation        stable             true
in_app_voice            stable             true
realtime_conversation   removed            true      # graduated: no longer a flag
realtime_frames_local   under development  false
file_backed_tool_audio  under development  false
```

The `in_app_*` flags appear next to `InAppBrowser`, `InAppChat` and `InAppUpdates` in the
binary's feature enum. The binary also contains the analytics strings
`codex_app_used_dictation` and a turn field `used_dictation`.

**TUI.** The binary strings include "use **/voice** to start or stop a voice conversation, or
**/voice settings** to choose a voice", "Mute or unmute the voice microphone", and the error
"voice audio devices could not open; check microphone and speaker setup". The `[realtime]`
config table (`RealtimeToml`) has the fields `voice`, `microphone`, `speaker` and
`microphone_channel`. The enum `RealtimeWsMode` has the values `conversational` and
`transcription`, and `RealtimeTransport` includes `webrtc`.

**App-server protocol.** I generated it with
`codex app-server generate-ts --experimental --out …` and with
`generate-json-schema --experimental`. Without `--experimental`, the `thread/realtime/*`
client requests do not appear in `ClientRequest.json` at all. Only the notifications do.

Client → server requests (all marked "EXPERIMENTAL"):

| Method | Params (abridged) |
| --- | --- |
| `thread/realtime/start` | `ThreadRealtimeStartParams`. Fields: `threadId`, `outputModality: "text" \| "audio"`, `transport?: {type:"websocket"} \| {type:"webrtc", sdp} \| {type:"existingCall", callId}`, `model?` ("Overrides the configured realtime model for this session only"), `version?: "v1"\|"v2"\|"v3"`, `voice?`, `clientManagedHandoffs?`, `includeStartupContext?`, `prompt?`, `voiceSessionId?`, … The response is empty. |
| `thread/realtime/appendAudio` | `{ threadId, audio: ThreadRealtimeAudioChunk }`. The chunk is `{ data: string, sampleRate, numChannels, samplesPerChannel \| null, itemId \| null }`. |
| `thread/realtime/appendText` | Appends text input. |
| `thread/realtime/appendSpeech` | `{ threadId, text }`: "speakable text". |
| `thread/realtime/stop` | `{ threadId }` |
| `thread/realtime/list` | Paged history of realtime events for a thread. |
| `thread/realtime/listVoices` | `{ voices: RealtimeVoicesList }` |

Server → client notifications: `thread/realtime/started` (`realtimeSessionId`, `version`),
`thread/realtime/transcript/delta` (`{threadId, role, delta}`), `thread/realtime/transcript/done`
(`{threadId, role, text}`), `thread/realtime/item/{started,completed}`,
`thread/realtime/item/transcript/delta`, `thread/realtime/itemAdded`,
`thread/realtime/outputAudio/delta`, `thread/realtime/sdp`, `thread/realtime/error`, and
`thread/realtime/closed`.

**Opting in.** `InitializeCapabilities.experimentalApi` controls these methods ("Opt into
receiving experimental API methods and fields"). YAAC already sends
`{ experimentalApi: true }` (`daemon/src/providers/codex/provider.ts:420`).

**Audio as a turn input.** `UserInput` (v2) also has `{type:"audio", url|fileId}` and
`{type:"localAudio", path}`. `Model.inputModalities` can contain `"audio"`. None of the models
in `~/.codex/models_cache.json` lists `audio` today; all list `['text','image']`.

**Backend.** The binary strings show:

- websocket path `/v1/realtime`, and `realtime/calls` (WebRTC call creation)
- `chatgpt.com/backend-api` base URLs and the header `x-oai-attestation`
- OpenAI Realtime event names, such as `input_audio_buffer.append`,
  `conversation.item.input_audio_transcription.delta` and
  `SessionInputAudioTranscription`
- the model strings `gpt-realtime-1.5` and `gpt-4o-mini-transcribe`

`~/.codex/auth.json` has `auth_mode: "chatgpt"` on this machine.

### Inference (not verified at runtime)

- **Where the features live.** `in_app_dictation` (press to dictate into the composer) is a
  feature of the **Codex desktop app**. The CLI and app-server carry only the flag and the
  analytics strings. I found no dictation-only RPC. The TUI's `/voice` is a realtime *voice
  conversation*: a realtime model listens, talks back, and delegates work to Codex. One of its
  tool descriptions reads: "Send a user request to the background agent… pass along the user's
  own words."
- **How it transcribes.** Transcription happens in the backend, not on the device. Audio goes
  to OpenAI's Realtime service, through the ChatGPT backend for ChatGPT sign-ins (or
  `api.openai.com` for API keys), and uses whatever auth the thread's model provider uses.
  `gpt-4o-mini-transcribe` looks like the default model for transcribing input audio. That is
  fine for Codex, but YAAC must not copy it, because of invariant 3.
- **Using realtime as dictation.** YAAC could get dictation out of `thread/realtime/*` like
  this:
  1. `start` with `outputModality:"text"` and `clientManagedHandoffs:true`, so that Codex
     does not start turns on its own.
  2. Stream `appendAudio`.
  3. Collect `transcript/delta` and `transcript/done` where `role` is the user's.
  4. `stop`.

  Caveats:
  - It is still a conversation with a realtime model. That model may answer, which costs
    usage, and it is scoped to one thread.
  - `RealtimeWsMode = transcription` suggests a transcription-only mode, but it is set in
    config, not per request.
  - The whole family is marked EXPERIMENTAL and changes between releases. The parameter set
    already has v1, v2 and v3 protocol knobs and rollout TODOs.
  - The format of `data` (base64 PCM, presumably) is not documented in the schema. It needs a
    live test.
- Under invariant 11 (one connection, driven through the daemon) this needs nothing new: it is
  more JSON-RPC on the connection that already exists.

### Follow-up: a transcription-only session exists (2026-11-27)

The tagged source (`rust-v0.172.0`) settles the open question above: Codex has a real
**transcription** session type, with no tools, no responses, and nothing added to the thread.
It is chosen by the thread's config `realtime.type`, not by a `start` parameter, and works only
on the v2 websocket wire, which needs API-key auth. `clientManagedHandoffs` does **not** stop a
conversational session from starting turns. YAAC therefore runs each dictation on a private
ephemeral helper thread started with that override. Details:
[harness surfaces](harness-surfaces.md#realtime-voice-verified-2026-11-27) and
[04](../04-providers.md#voice-input-codex).

## Claude Code

### Verified

**User-facing feature.** `/voice` is push-to-talk dictation in the TUI. These strings come from
`claude` 2.1.330:

- "Use /voice for push-to-talk dictation"
- "hold space to speak"
- the settings schema `voice: { enabled, mode: "hold" | "tap", autoSubmit }` ("'hold' (default):
  hold to talk. 'tap': tap to start, tap to stop+submit.")
- the legacy setting `voiceEnabled` ("Enable voice mode (hold-to-talk dictation)")
- `language` ("Preferred language for Claude responses and voice dictation")

The Agent SDK types (`sdk.d.ts`, lines ~9504, 9977 and 10237) declare these as *settings*
fields only.

**Backend.** The CLI opens a WebSocket to
`${BASE_API_URL → wss}/api/ws/speech_to_text/voice_stream`. The query parameters are
`encoding=linear16`, `sample_rate=16000`, `channels=1`, `endpointing_ms=300`,
`utterance_end_ms=1000`, `language`, `use_conversation_engine=true` and
`forward_interims=typed`. The headers are `Authorization: Bearer <OAuth accessToken>`,
`x-app: cli`, `anthropic-client-platform` and `x-config-keyterms`. The CLI sends KeepAlive and
CloseStream frames and receives `TranscriptInterim` and `TranscriptText` messages. When there
is no OAuth token it logs `[voice_stream] No OAuth token available`. Availability is checked in
this order:

1. Not a first-party provider → `provider`.
2. Nonessential traffic is disabled → `nonessential_traffic`.
3. → `not_available_here`.
4. Policy `allow_voice_mode` is off, or `voice.enabled=false` → `policy`.
5. A launcher started the CLI → `launcher_started`.
6. No claude.ai sign-in → `not_signed_in`.

**Host dictation control request.** The CLI's control protocol has subtype **`dictate`**:

```
{ subtype: "dictate", op: "start" | "audio" | "stop" | "cancel",
  language?: string,   // "With start: the language spoken, as a code (en, pt-BR) or a name"
  pcm?: string }       // "With audio: base64 of 16 kHz mono 16-bit little-endian samples, two seconds at most"
reply: { status:"ok", text, interim, done }
     | { status:"unavailable", reason: "started_by_session"|"provider"|"nonessential_traffic"|
         "not_available_here"|"policy"|"launcher_started"|"not_signed_in"|"not_started"|
         "connection_failed", text }
```

Its description, verbatim: "**@internal** Turns speech that the host recorded into text, with
the service and the sign-in of /voice. … A dictation ends after two minutes of audio, and is
dropped with its words after 15 seconds with no request. Answered only for the program that
started the CLI on the same machine, over its stdin; **in a public build only for the VS Code
extension, behind a flag.**"

The handler in 2.1.330 runs three checks:

1. A feature gate. If it fails, the request is refused as unknown.
2. `hostOwnedLocalStdin && !transportPersists && !diskless/hostedWorker`. If it fails: "dictate
   is not available on this connection".
3. A spare-session check.

Cloud-hosted sessions reject it ("dictate is not supported in a cloud-hosted session"). The
SDK's bundled CLI 2.1.329 contains the same code; `strings` finds `dictate`, `HostDictation`
and `voice_stream`.

**The Agent SDK exposes nothing.** `sdk.d.ts` 0.3.329 has no `dictate` method or type and no
voice API. The only matches are the settings fields listed above.

### Inference

- **How it transcribes.** Transcription runs in Anthropic's backend at `api.anthropic.com`. It
  is a streaming speech-to-text service that appears to wrap a third-party engine: the frame
  vocabulary (KeepAlive, CloseStream, linear16, endpointing, utterance_end) is Deepgram-style.
  Only a claude.ai subscription sign-in can use it. It does not work with an API key, Bedrock,
  Vertex or a gateway.
- **Could YAAC send `dictate`?** In principle `yaacd` launches the CLI over stdin, so the
  "launched the CLI" condition might hold. But the request is `@internal`, it is flag-gated to
  first-party hosts in public builds, and it is not part of the SDK's public surface. Sending
  it means writing raw control requests past the SDK and hoping the gate is open. It could
### Follow-up: the gate, verified (2026-11-27)

The owner chose to use `dictate` anyway (P382). The "flag" turned out to be two things: the
VS Code host entrypoint (`CLAUDE_CODE_ENTRYPOINT=claude-vscode`) and a server-side GrowthBook
feature, `tengu_binary_reef`, that defaults to off and has no local override in the public
build. The feature is off for this machine's account, so a probe is refused today. `yaacd`
therefore offers Claude voice input only when a network-free `dictate {op:"cancel"}` is
answered, and runs it in a separate dictation-only process so that no thread gets the VS Code
entrypoint. Details: [harness surfaces](harness-surfaces.md#claude-code-dictation-verified-2026-11-27)
and [04](../04-providers.md#voice-input-claude).

  disappear in any release. **Not acceptable as a dependency.**

## Reusing harness credentials directly

Two shortcuts are possible:

- Read `~/.codex/auth.json` or the Claude OAuth token and call `voice_stream`, `realtime/calls`
  or the ChatGPT backend ourselves.
- Impersonate `x-app: cli`.

Either one would use private endpoints with another product's credentials and client identity.
That is likely outside both providers' terms. It breaks the spirit of invariant 12 (YAAC does
not take over the user's secrets) and breaks without warning. **Do not do this.** The Codex
`thread/realtime/*` route is different: it is a published app-server method that Codex itself
carries out with its own credentials, so YAAC never touches a token.

## Alternatives

| Option | Pros | Cons | Fit with YAAC |
| --- | --- | --- | --- |
| **Android `SpeechRecognizer`** (on-device where the device offers it, `EXTRA_PREFER_OFFLINE`; or the system IME's voice key) | No key, no cost, low latency, works with any harness and target. The IME route needs zero code. | Quality and languages vary by device. Some devices send audio to Google. | Best fit. The text is produced on the client and sent as an ordinary prompt; nothing about models touches the target. |
| **Windows speech** (`Windows.Media.SpeechRecognition` WinRT, or Win+H voice typing in any text box) | Built in. Win+H already works in the WPF composer at no cost. | WinRT dictation needs online speech enabled for good quality. The API surface is old. | Good fit. It is client-side only. |
| **Codex `thread/realtime/*`** | Uses the user's own Codex sign-in through the daemon. Gives a transcript and could also give a "talk to the agent" mode. | EXPERIMENTAL and churning. It is a conversation, not dictation. Codex threads only. Costs usage. | Acceptable as an **optional, capability-gated** feature: the target advertises e.g. `VoiceInput: realtime`, and the UI branches on the capability, never on `kind`. Leave `model` unset so Codex picks it (invariant 3). Requests go through the dialect layer. |
| **OpenAI / Anthropic public transcription APIs with the user's own key** | Good quality, documented, stable. | Needs a key YAAC would have to store, which is in tension with "never store a password" (and a key is close to one). Costs money. Model ids must not be hardcoded. | Possible later, as a user-configured provider. List the models from the API at runtime, never from source. Low priority. |
| Claude `dictate` control request | Same service as `/voice`. | `@internal` and flag-gated. | Do not use. |

## How to re-verify

```sh
codex --version; codex app-server daemon version; codex features list | grep -iE 'voice|dictat|realtime'
codex app-server generate-ts --experimental --out /tmp/cx && ls /tmp/cx/v2 | grep -i realtime
grep -o '"thread/realtime/[a-zA-Z]*"' /tmp/cx/ClientRequest.ts | sort -u
claude --version
strings -n 5 "$(readlink -f ~/.local/bin/claude)" | grep -E 'subtype:E\("dictate"\)|speech_to_text/voice_stream'
grep -n -iE 'dictat|voice' daemon/node_modules/@anthropic-ai/claude-agent-sdk/sdk.d.ts
```

Use `python3 -I` rather than `grep -oE '.{0,600}…'` for context windows inside the 380 MB Claude
binary, because the grep regex times out.
