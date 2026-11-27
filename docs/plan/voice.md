# Voice input — contract and work split

The owner's decision (P382): voice-to-text in the composer for **both** harnesses, using each
harness's own voice surface — Codex's experimental app-server `thread/realtime/*`, Claude
Code's internal `dictate` control request (the one the VS Code extension uses). Research:
[research/voice-input.md](../research/voice-input.md). The owner's later decision (P392):
transcription is a **service of an instance**, not a thread capability — any instance that can
transcribe, on any connected target, serves any composer (a thread's of any harness, or a
new-thread draft's); the hub chooses which. Voice calls stay bound to the thread's own harness
([voice-call.md](voice-call.md), unchanged). User-facing behaviour:
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
- **Capabilities, not kinds** (invariant 4): the microphone is enabled when some connected
  target has an instance with the `transcription` capability (and a format); the choice reads
  only descriptors. The format comes from the target. No sample rate, model, or voice is
  hardcoded in a client or the hub (invariant 3).
- **Nothing new is opened** when the transcriber is on another target than the composer: its
  audio travels over that target's existing connection.
- **No blind retries** (invariant 10): `voice/start` carries an `opId`; an uncertain start is
  shown, not re-sent.

## Capability and feature

| Where | Name | Meaning |
| --- | --- | --- |
| `initialize` → `features` | `transcription` | This `yaacd` implements the instance-addressed `voice/*`. Without it the hub's dialect folds `transcription` out of every capability set, so none of its instances is chosen. |
| `ProviderDescriptor.capabilities` | `transcription` | The instance can transcribe **now** (signed in, allowed, harness new enough). |
| `ProviderDescriptor.transcription` | `TranscriptionOffer` | `{ format? , unavailable? }`. `format` (`{ format: "pcm16le", sampleRate, channels, maxSeconds? }`, what to record) with the capability; `unavailable` (why not, for the user) when the instance has a voice surface that is not ready. Absent: no voice surface. |

There is **no thread capability**: `ThreadEffective` carries nothing about dictation, and a
thread's harness does not decide whether its composer can dictate.

`VoiceFormat.format` is an open enum (`AudioEncoding`); only `pcm16le` exists. A format a
client cannot produce counts as unavailable.

**Feature name.** The first contract (P382) used the feature and capability `voiceInput` with a
thread-addressed `voice/start { threadId }`. The semantics changed (an instance is addressed, the
thread is optional), so the feature has a new name, `transcription`, following
[09-compatibility](../09-compatibility.md): a `yaacd` that only advertises `voiceInput` simply
has no transcriber for a newer hub. Neither shape was released, so `voiceInput` is gone from
the types rather than kept beside it.

## Choosing the transcriber

`yaac_protocol::transcription::choose` — one pure function, called by the hub when a window
starts a dictation and by the window beforehand, to show the mic and say who will transcribe.
Over the connected targets' descriptors (capabilities already folded), first match wins:

1. the composer's own instance, on its own target, when it can transcribe;
2. another ready instance on the composer's target;
3. a ready instance on another connected target.

Ties break by target id, then instance id, so the choice never depends on arrival order. A
cached descriptor of an offline target never counts. With nothing ready the result is the
nearest reason in the same order (`"No transcriber available. Codex on devbox: …"`), or that no
machine is connected. The label is `"{instance display name} on {target name}"`. A user
preference can be added later; there is none now.

## Language

**P401.** A dictation listens for the same language a call speaks
([voice-call.md § Language](voice-call.md#language)): the renderer's `language` in
`composer.voiceStart` (Android: the phone's setting or locale), else the window's *Voice call
language*, else the computer's display language. The window passes it as `language` on
`hub/voice/start` → `voice/start`. What each transcriber does with it (verified):

- **Claude `dictate`** (2.1.330): `start` takes `language`, "the language spoken, as a code
  (en, pt-BR) or a name" — passed as is.
- **Claude `voice_stream` fallback** (P390): the `language` query parameter, which takes a
  two-letter code; yaacd maps the tag to its base language (`pt-BR` → `pt`) and sends `en` for
  one it does not list (`providers/claude/voice-stream.ts` `voiceLanguage`).
- **Codex transcription** (0.172.0): no field. The v2 transcription session (`type:
  "transcription"`) Codex builds carries no language, and `thread/realtime/start` has none, so
  the language is ignored there and Codex's transcriber detects it.

## Methods (yaacd ⇄ host, additive feature `transcription`)

| Method | Direction | Params → Result |
| --- | --- | --- |
| `voice/start` (op) | host → daemon | `VoiceStartParams { opId, instance, threadId?, language? }` → `VoiceStartResult { stream, initialCredit, format }`. `instance` transcribes; `threadId` is context only (the per-thread limit) and the hub sends it only to the composer's own target. |
| data frames on `stream` | host → daemon | Raw audio in `format`, interactive lane, under credit — exactly like terminal input. The daemon grants more with `stream/credit { stream, bytes }`. No daemon → host data on this stream. |
| `voice/stop` | host → daemon | `VoiceStopParams { stream, bytes }` → `{}`. `bytes` = total audio bytes sent; data and control use different lanes, so the daemon finishes once it has received `bytes` (grace ~3 s, then finish with what it has). |
| `voice/cancel` | host → daemon | `VoiceCancelParams { stream }` → `{}`. Words are dropped. |
| `voice/transcript` | daemon → host | `VoiceTranscriptParams { stream, text, final }`. `text` is the **whole** transcript so far and **replaces** the previous one (interim guesses may be revised). `final: true` is sent at most once, last. |
| `voice/ended` | daemon → host | `VoiceEndedParams { stream, error? }`. Always the last message for a started stream (stopped, cancelled, timed out at `maxSeconds`, or failed). `error` is words for the user; absent after a normal stop or a cancel. |

`voice/start` errors: `capability_missing` (the instance does not transcribe now; the message
is its `unavailable` reason), `invalid_params` (no such instance), `needs_auth` (the harness
wants a sign-in; message says which), `provider_unavailable` (the harness refused — the message
carries its reason, e.g. Claude's `policy`, `not_signed_in`), `thread_not_found` (a `threadId`
that is not on this target), `limit_exceeded` (a dictation is already running for this thread —
one at a time per thread; dictations without a thread are not limited here).

The daemon ends a dictation by itself (with `voice/ended`) when its connection drops, when
`maxSeconds` is reached (after a final transcript), or when the harness fails.

## Attach protocol (window ⇄ hub)

Same shape as `hub/terminal/*`: the target-side params flattened beside `target`
(`TargetVoice<P>` = `{ target, ...P }`), and the stream renumbered to a **hub-local** one per
window.

| Name | Direction | Payload |
| --- | --- | --- |
| `hub/voice/start` | window → hub | `HubVoiceStartParams { opId, target?, instance?, threadId?, language? }` — the composer, not the transcriber → `HubVoiceStartResult { target, instance, label, stream (hub-local), initialCredit, format }`: the hub [chooses](#choosing-the-transcriber) and sends `voice/start` to the chosen target. Nothing ready: `capability_missing` with the reason, and nothing is sent anywhere. |
| data frames on the hub-local stream | window → hub | audio; the hub checks credit and forwards byte-for-byte |
| `stream/credit` | hub → window | more audio credit on the hub-local stream |
| `hub/voice/stop` | window → hub | `{ target, stream, bytes }` → `{}` (`target`: the transcriber's, from the start's result) |
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
| **V2** (yaacd, Claude) | `daemon/src/providers/claude`: report `voiceInput` (now `transcription` on the instance, P392) (format 16 kHz mono pcm16le, `maxSeconds: 120`) when the `dictate` surface answers; drive `dictate start/audio/stop/cancel` (≤ 2 s of audio per `audio`, a request at least every 15 s); map `interim`/`text` to `voice/transcript`; refusals to the errors above. |
| **V3** (yaacd, Codex) | `daemon/src/providers/codex`: report `voiceInput` (now `transcription` on the instance, P392) from the app-server's experimental realtime surface; `thread/realtime/start` (`outputModality: "text"`, `clientManagedHandoffs: true`, no `model`), `appendAudio` (base64 chunks), `stop`; collect the user-role `transcript/delta`/`done` into `voice/transcript`; never let realtime start a turn. Shared `voice/*` handling in `yaacd` (stream registry, credit, `bytes` wait, `voice/ended`, advertising `features: ["voiceInput"]`) is V3's unless V2 lands it first — coordinate. |
| *Landed (V2)* | The shared `voice/*` handling is `daemon/src/voice/` (`VoiceSessions`: stream registry, credit, the `bytes` wait with a 3 s grace, one dictation per thread, `voice/ended` last) and `features: ["voiceInput"]` (now `["transcription"]`, P392). A provider implements `HarnessProvider.startDictation` returning a `Dictation`; an optional `accepted` promise makes `voice/start` answer only once the harness accepted, so a refusal (`DictationRefused`) is the start's error. |
| *Transcription service (P392)* | Protocol: `transcription` feature/capability, `TranscriptionOffer` on descriptors, instance-addressed `voice/start`, `HubVoiceStart*`, `transcription::choose` (unit tests). Hub: per-connection transcriber list from `provider/list`/`provider/updated`, choice on `hub/voice/start` (`crates/yaac-hub/tests/voice.rs`: own target preferred, another target's transcriber, none → reason). `yaacd`: descriptors from each provider's probe, `startDictation(opts, events)` with no thread, the fake's `FAKE_TRANSCRIBE` (`daemon/test/transcription.test.ts`). `yaac-core`: `voice::transcriber`, dictations keyed by composer (`DraftRef`). Controller: the `voice` object on every composer, `transcriber` and `unavailable` fields, the status "Dictating with …" (`wpf::composer` tests: a Codex thread through Claude, a draft). |
| *Claude fallback (P390)* | After the `dictate` probe fails, the direct `voice_stream` path resolves Claude Code's current access token dynamically from the CLI's own source. YAAC has no token setting or persisted copy. `daemon/test/claude-voice-stream.test.ts` checks the request shape and transcript lifecycle with a fake local WebSocket server. |
| **V4** (clients) | Desktop, Android, controller: mic button per [07-ui.md](../07-ui.md#voice-input), recording at the reported format, streaming under credit, interim text in place, insertion at the caret. Uses the `yaac-core` calls V1 adds. |
