# Voice input — contract and work split

The owner's decision (P382): voice-to-text in the composer for **both** harnesses, using each
harness's own voice surface — Codex's experimental app-server `thread/realtime/*`, Claude
Code's internal `dictate` control request (the one the VS Code extension uses). Research:
[research/voice-input.md](../research/voice-input.md). User-facing behaviour:
[07-ui.md](../07-ui.md#voice-input). This file is the **shared contract**; the Rust types in
`crates/yaac-protocol/src/types/voice.rs` are its source of truth, and the TypeScript in
`daemon/src/protocol/generated/` is generated from them (`cargo run -p yaac-protocol --bin
yaac-protocol-gen`), never hand-edited.

## Rules that do not bend

- **Audio is never journaled or written to disk** — not by the client, the hub, or `yaacd`.
  It lives in memory only while it is being forwarded. Log byte counts, never bytes.
- **The transcript lands only in the composer draft.** It is never sent automatically and is
  never shown as the user's message until the user sends it. It is not a journal event.
- **One connection** (invariant 1): the audio is data frames on the target's existing stream;
  nothing new is opened by the hub. (What `yaacd` does towards the harness — the Claude CLI's
  own stdin, Codex's daemon socket — is that provider's existing channel.)
- **Capabilities, not kinds** (invariant 4): the microphone shows when the thread's effective
  capabilities contain `voiceInput`. The format comes from the target. No sample rate, model,
  or voice is hardcoded in a client or the hub (invariant 3).
- **No blind retries** (invariant 10): `voice/start` carries an `opId`; an uncertain start is
  shown, not re-sent.

## Capability and feature

| Where | Name | Meaning |
| --- | --- | --- |
| `initialize` → `features` | `voiceInput` | This `yaacd` implements `voice/*`. Without it the hub's dialect folds `voiceInput` out of every capability set, so no mic appears. |
| `ProviderDescriptor.capabilities` | `voiceInput` | The instance has a usable voice surface (signed in, allowed, harness new enough). |
| `ThreadEffective.capabilities` | `voiceInput` | This thread can dictate **now**. The mic is shown iff this is present. |
| `ThreadEffective.voiceInput` | `VoiceFormat` | `{ format: "pcm16le", sampleRate, channels, maxSeconds? }` — what to record. Set together with the capability. |

`VoiceFormat.format` is an open enum (`AudioEncoding`); only `pcm16le` exists. A client that
cannot produce the format hides the mic.

## Methods (yaacd ⇄ host, additive feature `voiceInput`)

| Method | Direction | Params → Result |
| --- | --- | --- |
| `voice/start` (op) | host → daemon | `VoiceStartParams { opId, threadId, language? }` → `VoiceStartResult { stream, initialCredit, format }` |
| data frames on `stream` | host → daemon | Raw audio in `format`, interactive lane, under credit — exactly like terminal input. The daemon grants more with `stream/credit { stream, bytes }`. No daemon → host data on this stream. |
| `voice/stop` | host → daemon | `VoiceStopParams { stream, bytes }` → `{}`. `bytes` = total audio bytes sent; data and control use different lanes, so the daemon finishes once it has received `bytes` (grace ~3 s, then finish with what it has). |
| `voice/cancel` | host → daemon | `VoiceCancelParams { stream }` → `{}`. Words are dropped. |
| `voice/transcript` | daemon → host | `VoiceTranscriptParams { stream, text, final }`. `text` is the **whole** transcript so far and **replaces** the previous one (interim guesses may be revised). `final: true` is sent at most once, last. |
| `voice/ended` | daemon → host | `VoiceEndedParams { stream, error? }`. Always the last message for a started stream (stopped, cancelled, timed out at `maxSeconds`, or failed). `error` is words for the user; absent after a normal stop or a cancel. |

`voice/start` errors: `capability_missing` (no `voiceInput` on the thread), `needs_auth` (the
harness wants a sign-in; message says which), `provider_unavailable` (the harness refused —
the message carries its reason, e.g. Claude's `policy`, `not_signed_in`), `thread_not_found`,
`limit_exceeded` (a dictation is already running on this thread — one at a time per thread).

The daemon ends a dictation by itself (with `voice/ended`) when its connection drops, when
`maxSeconds` is reached (after a final transcript), or when the harness fails.

## Attach protocol (window ⇄ hub)

Same shape as `hub/terminal/*`: the target-side params flattened beside `target`
(`TargetVoice<P>` = `{ target, ...P }`), and the stream renumbered to a **hub-local** one per
window.

| Name | Direction | Payload |
| --- | --- | --- |
| `hub/voice/start` | window → hub | `{ target, opId, threadId, language? }` → `{ stream (hub-local), initialCredit, format }` |
| data frames on the hub-local stream | window → hub | audio; the hub checks credit and forwards byte-for-byte |
| `stream/credit` | hub → window | more audio credit on the hub-local stream |
| `hub/voice/stop` | window → hub | `{ target, stream, bytes }` → `{}` |
| `hub/voice/cancel` | window → hub | `{ target, stream }` → `{}` |
| `hub/voice/transcript` | hub → the starting window only | `{ target, stream, text, final }` |
| `hub/voice/ended` | hub → the starting window only | `{ target, stream, error? }` |

The hub keeps no audio and no transcript: nothing is cached or written. If the window
disconnects the hub sends `voice/cancel`. If the target connection drops the hub sends the
window `hub/voice/ended` with an error. A window that overruns its credit has its dictation
cancelled. `voice/*` is refused through `hub/request` ("use the hub voice methods").

## Work split

| Worker | Scope |
| --- | --- |
| **V1** (contract + hub) | `yaac-protocol` types, feature, generated TS; hub relay (`crates/yaac-hub/src/core/voice.rs`) and tests; `yaac-core` client calls and dictation state (no UI); docs 03/04/07/decisions. |
| **V2** (yaacd, Claude) | `daemon/src/providers/claude`: report `voiceInput` (format 16 kHz mono pcm16le, `maxSeconds: 120`) when the `dictate` surface answers; drive `dictate start/audio/stop/cancel` (≤ 2 s of audio per `audio`, a request at least every 15 s); map `interim`/`text` to `voice/transcript`; refusals to the errors above. |
| **V3** (yaacd, Codex) | `daemon/src/providers/codex`: report `voiceInput` from the app-server's experimental realtime surface; `thread/realtime/start` (`outputModality: "text"`, `clientManagedHandoffs: true`, no `model`), `appendAudio` (base64 chunks), `stop`; collect the user-role `transcript/delta`/`done` into `voice/transcript`; never let realtime start a turn. Shared `voice/*` handling in `yaacd` (stream registry, credit, `bytes` wait, `voice/ended`, advertising `features: ["voiceInput"]`) is V3's unless V2 lands it first — coordinate. |
| *Landed (V2)* | The shared `voice/*` handling is `daemon/src/voice/` (`VoiceSessions`: stream registry, credit, the `bytes` wait with a 3 s grace, one dictation per thread, `voice/ended` last) and `features: ["voiceInput"]`. A provider implements `HarnessProvider.startDictation` returning a `Dictation`; an optional `accepted` promise makes `voice/start` answer only once the harness accepted, so a refusal (`DictationRefused`) is the start's error. |
| *Claude fallback (P390)* | After the `dictate` probe fails, the direct `voice_stream` path resolves Claude Code's current access token dynamically from the CLI's own source. YAAC has no token setting or persisted copy. `daemon/test/claude-voice-stream.test.ts` checks the request shape and transcript lifecycle with a fake local WebSocket server. |
| **V4** (clients) | Desktop, Android, controller: mic button per [07-ui.md](../07-ui.md#voice-input), recording at the reported format, streaming under credit, interim text in place, insertion at the caret. Uses the `yaac-core` calls V1 adds. |
