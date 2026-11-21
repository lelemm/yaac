# The fake provider

Kind `fake`. A harness simulator: it streams, reasons, runs tools, asks for approvals and
questions, stalls, fails, and pretends to be in use elsewhere — with no harness installed and no
tokens spent. Its models (`fake-large`, `fake-small`, `fake-mystery`), efforts (`fake-low`,
`fake-high`), and modes (`fake-ask`, `fake-auto`) are invented.

## Enabling it

- `YAAC_FAKE_PROVIDER=1` in yaacd's environment gives an implicit instance `pi_fake`; or
- `providerInstance/add { kind: "fake", … }` stores an explicit one.

Instance `env` options: `FAKE_STORE=<dir>` (conversation store; two instances with the same
store share conversations and allow native fork), `FAKE_SEED=0` (no pre-existing "outside"
threads), `FAKE_NATIVE_FORK=0` (no `threadFork`), `FAKE_PACE_MS=<n>` (delay between deltas,
default 5), `FAKE_STATUS=<providerStatus>` (e.g. `needsAuth`, to test status handling),
`FAKE_STEER=0` (no `steer`: a harness that cannot steer), `FAKE_FORCE_STOP=0` (no `forceStop`:
a shared harness process yaacd cannot end).

A new store is seeded with two conversations "created outside YAAC": one `interactive`
("Refactor the parser (started in a terminal)") and one `programmatic` ("Nightly lint
(background job)").

## Directives

Put directives in the user's message, one per line, each starting with `/fake`. Other lines
are ignored. A message with no directive is echoed back (`Echo: <text>`) in several deltas.
Steps run in order.

| Directive | Effect |
| --- | --- |
| `/fake say <text>` | An agent message, streamed in several deltas. |
| `/fake reason <text>` | A reasoning item, streamed. |
| `/fake tool <command> => <output>` | A command tool call with streamed output, exit 0. |
| `/fake big-output <bytes>` | A tool call whose output is `<bytes>` long (default 2 MiB), in 64 KiB deltas. |
| `/fake plan a \| b \| c` | A plan item. |
| `/fake edit <path>` | A file change (`toolCall`, tool `fileChange`, title "Edited <path>") whose `output` is a small unified diff of that file, as every provider reports file changes (D86). |
| `/fake deaf` | The rest of the turn ignores `interrupt` (a harness that does not comply); only `forceStop` ends it. |
| `/fake approve <command>` | Asks for approval (decisions `allow`, `allowSession`, `deny` with reason). Allowed: the tool runs; denied: the tool call is `declined`. |
| `/fake ask <question> \| <opt1> \| <opt2>` | A structured question (single choice, free text allowed). The reply echoes the choice. |
| `/fake ask-multi <question> \| <opt1> \| …` | The same, multi-select. |
| `/fake ask-secret <question>` | A secret free-text question. The reply gives only the answer's length. |
| `/fake resolve-elsewhere <ms>` | The *next* request is answered "in a terminal" after `<ms>` (resolved elsewhere). |
| `/fake expire <ms>` | The *next* request is dropped by the harness after `<ms>` (expired). |
| `/fake crash-on-answer` | When the *next* request is answered, the yaacd process is killed (SIGKILL) before the harness confirms — the request becomes `unknown`. |
| `/fake crash` | The yaacd process is killed (SIGKILL) right after the harness accepted the turn — a retried `opId` yields `outcome_unknown`. |
| `/fake stall` | The turn hangs until interrupted. |
| `/fake sleep <ms>` | Pause. |
| `/fake pace <ms>` | Delay between deltas for the rest of the turn. |
| `/fake fail <message>` | The turn fails with that error. |
| `/fake usage` | A `usageUpdated` event. |
| `/fake inputs` | An agent message listing what the provider was handed: `image <path> <mediaType>` per image input and `file <path> <name>` per file part (or `Inputs: none`). |
| `/fake in-use [holder] [ms]` | After the turn, the thread is held by another program (`external`), released after `ms` if given. |
| `/fake external-turn [ms] [text]` | After the turn, another program takes the thread, runs a turn (origin `external`) that says `text`, and lets go `ms` later (default 2000). |
| `/fake outside-thread <title>` | A new conversation "created outside YAAC" appears in the listing. |

`turn/steer` input is answered at the next step boundary as `Steered: <text>`.

### Agent-mesh directives

Each is one call of a `yaac` mesh tool (docs/10-agent-mesh.md#the-tools), made through the
thread's attachment ticket exactly as a harness would (with `_meta.threadId` set to the fake's
own conversation id). The call appears in the transcript as a `toolCall` item titled
`yaac.<tool>` whose output is the tool's text result (`failed` when the tool returned an error).
A message delivered to a fake thread is framed as from an agent; `/fake` lines in its body run,
so one fake agent can script another.

| Directive | Tool call |
| --- | --- |
| `/fake mesh <tool> <json>` | Any tool with JSON arguments, e.g. `/fake mesh list_threads {"scope":"project"}`. |
| `/fake mesh-send <address> \| <text>` | `send_message { to, text }`. |
| `/fake mesh-spawn [key=value …] \| <text>` | `spawn_thread { text, … }`; keys `target`, `instance`, `model`, `effort`, `cwd`, `title`, `detached=true`. `\n` in the text is a line break, so the worker can be given several directives. |
| `/fake mesh-wait [address …] [timeoutSecs=n] [messages=false]` | `wait`; `$all` stands for every thread this one has spawned. |
| `/fake mesh-list [scope] [key=value …]` | `list_threads`; keys `target`, `query`, `state`, `includeInactive=true`, `limit`, `cursor`. |
| `/fake mesh-find <query>` | `find_thread { query }`. |
| `/fake mesh-read <address>` | `read_thread { thread }`. |
| `/fake mesh-notify-when-done <address>` | `notify_when_done { to }`. |
| `/fake mesh-stop <address>` | `stop_thread { thread }`. |
| `/fake mesh-whoami`, `/fake mesh-targets`, `/fake mesh-models [target]` | `whoami`, `list_targets`, `list_models`. |

In the arguments, `$spawned` is the address of the last thread this one spawned (`$spawned1`,
`$spawned2`, … the first, second, …), and `$from` the agent that last messaged it (from the
turn's input or a `wait` result).

## Programmatic control (tests)

`FakeProvider` (from `daemon.core.registry.get(id).provider`) exposes: `setExternal(native,
control)`, `runExternalTurn(native, text, holdMs, extraSteps)`, `resolveElsewhere(native,
requestId)`, `expireRequest(native, requestId)`, `addOutsideThread(title)`, `failNextScan`,
`afterAccept` (a hook between the harness accepting a turn and yaacd recording it),
`addOldThreads(n, daysOld, prefix)` (many old outside conversations, no events), `scannedCount`,
`lookedUp`, `forceStopped`,
`meshAttachment(native)` (the thread's mesh attachment, to call tools as that thread),
`received` (every input handed over), `answers` (every answer, secrets included), `turnLog`,
and `nativeIds`.
