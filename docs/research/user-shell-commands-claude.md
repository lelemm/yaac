# User shell commands (`!`) in Claude Code

How Claude Code's bash mode (`! <command>` in the prompt) works, what a program can reach of
it, and how YAAC can offer the same for Claude Code threads. Observed **2026-11-25**.
**Verified** marks what was read from the installed binaries or seen in an experiment.
**Inference** marks a reading that was not tested.

| Component | Version examined |
| --- | --- |
| `claude` CLI (`~/.local/bin/claude --version`) | 2.1.330 |
| `@anthropic-ai/claude-agent-sdk` (in `daemon/node_modules`) | 0.3.329 |
| CLI bundled with that SDK (`claude-agent-sdk-linux-x64/claude --version`) | 2.1.329 |

YAAC drives Claude Code with the SDK's streaming input. `query({ prompt: AsyncIterable<SDKUserMessage> })`
is in `daemon/src/providers/claude/sdk.ts`, and `live.input.push(message)` in `index.ts` sends
each message. Messages from peers already go through it with `priority: "next"`.

## Summary

- **The TUI's `!` runs the command through Claude Code's own Bash tool.** It skips the
  permission check and the sandbox. Two plain user records are written: `<bash-input>cmd</bash-input>`
  and `<bash-stdout>…</bash-stdout><bash-stderr>…</bash-stderr>`.
- **On 2.1.330, `!` starts a model turn by default.** The setting `respondToBashCommands`
  (default `true`) controls this. With `false`, Claude Code first writes an `isMeta`
  `<local-command-caveat>` record and does not start a turn. The model then sees the records
  with the next prompt.
- **A program cannot trigger the TUI path.** A stream-json or SDK user message that starts with
  `!` is an ordinary prompt: it is not run and it goes to the model as the text `!cmd`. There is
  also an `@internal` stdin frame, `{"type":"bash_command"}`. It is not in `sdk.d.ts`. On a local
  host it runs `/bin/sh -c` *beside* the conversation: the output goes back on stdout only, and
  is neither written to the session file nor shown to the model.
- **What does work, and is documented, is a user message with `shouldQuery: false`.** The
  record is added to the transcript without starting a turn. The SDK's own comment names this
  path: "plain (non-synthetic) `shouldQuery: false` transcript appends (e.g. CCD bash mode)".
  The model sees the content merged into the next prompt. `--resume` renders it as a native
  `! cmd ⎿ output` entry, provided input and output are **separate text blocks**.
- **While a turn is running, `priority` matters.** With no `priority`, or `"next"`, a
  `shouldQuery: false` message is injected *into the running turn* as "The user sent a new
  message while you were working: … Address the message above". With `priority: "later"` it
  waits until the turn ends and is then added as context only. That matches the TUI, which
  queues a `!` typed during a turn until the turn ends.

**Recommendation for YAAC**: see [How YAAC should implement it](#how-yaac-should-implement-it).
YAAC runs the command in `yaacd`, or takes it from its own PTY. It then pushes **one**
`SDKUserMessage` with `shouldQuery: false` and `priority: "later"`. Its content is an array of
text blocks: `<bash-input>`, then `<bash-stdout>…<bash-stderr>…<bash-exit-code>`.

## 1. What the CLI does with `!` (TUI)

The handler is `processBashCommand`, in the 2.1.330 binary. It is called from prompt
processing when `mode === "bash"`. The core, de-minified:

```js
// respond = settings.respondToBashCommands ?? true
input  = userMessage(`<bash-input>${cmd}</bash-input>`)          // command NOT escaped
result = await BashTool.call({ command: cmd, dangerouslyDisableSandbox: true }, ctx, …,
                             { userTypedShellDispatch: true })    // no permission check
out    = mapToolResultToToolResultBlockParam(result) → large-output persistence (qAe)
query  = respond && !interrupted && !backgrounded && !aborted
return { messages: [ ...(query ? [] : [caveat()]), input,
                     userMessage(`<bash-stdout>${esc(stdout)}</bash-stdout><bash-stderr>${esc(stderr)}</bash-stderr>`) ],
         shouldQuery: query }
// caveat() = userMessage("<local-command-caveat>The command below was run directly in Claude
//            Code, not sent to you as a request, and its output goes straight to the user.
//            It's recorded here as context for later messages.</local-command-caveat>", isMeta: true)
```

### Behaviour

| Question | Answer | How known |
| --- | --- | --- |
| Which shell | The Bash tool's shell: the user's bash, or PowerShell when `defaultShell: "powershell"` (setting: "Default shell for input-box ! commands") | binary; verified with bash |
| Working directory | The session's working directory, as for the model's Bash calls. A `cd` outside the project is undone, and Claude Code appends `\nShell cwd was reset to <dir>` to `<bash-stderr>` | verified (`cd /tmp && pwd`) |
| Sandbox / permissions | None: `dangerouslyDisableSandbox: true` and no permission prompt (the `bash_command` schema says "Trust model matches the local TUI `!cmd` path (no sandbox, no per-command prompt)") | binary; verified, no prompt shown |
| Timeout | The Bash tool's default, `BASH_DEFAULT_TIMEOUT_MS` (120 000 ms). On timeout the command **moves to the background**; it is not killed. `<bash-stdout>` then holds the "Command did not complete within its 3s timeout and was moved to the background (ID: …)" text. Completion later arrives as a `<task-notification>` (origin `task-notification`), which **started a model turn** | verified with `BASH_DEFAULT_TIMEOUT_MS=3000` |
| stdout and stderr | Merged by the Bash tool: `echo out; echo err >&2` gave `<bash-stdout>out\nerr…</bash-stdout><bash-stderr></bash-stderr>`. On a non-zero exit the output landed in `<bash-stderr>` (`<bash-stdout></bash-stdout><bash-stderr>partial\n</bash-stderr>`) | verified |
| Exit code | **Not written** by the TUI path. `<bash-exit-code>` comes only from the headless `bash_command` runner | verified |
| Escaping | `<bash-input>` is written as typed, **not** escaped. Output is XML-escaped: `<b>&x` → `&lt;b&gt;&amp;x` | verified |
| ANSI colour codes | Kept in the record (`\u001b[31mred\u001b[0m`) | verified |
| Output size | Inline up to `bashOutputMaxChars` (default 30 000, clamped 4 000–128 000). Above that, `<bash-stdout><persisted-output>\nOutput too large (575.1KB). Full output saved to: <config>/projects/<proj>/<session>/tool-results/<id>.txt\n\nPreview (first 2KB):\n…` | verified (`seq 1 100000`) |
| Starts a turn? | **Yes by default** (`respondToBashCommands` defaults to `true` on 2.1.330): no caveat, and an API request follows at once. With `false`: caveat first, no turn | verified both |
| While a turn runs | Queued (`queue-operation enqueue`), then run **after** the turn ends. It is not injected into the turn | verified |
| Ctrl+B / backgrounded, interrupted | Backgrounded or aborted → `shouldQuery: false`. Interrupted → the `[Request interrupted by user]` marker | binary |

### Session JSONL written by the TUI (2.1.330, `respondToBashCommands: false`)

Copied from `runs/tui2` of the experiment. `cwd`, `version`, `gitBranch`, `sessionId`,
`userType`, `entrypoint` and `isSidechain` were removed.

```json
{"parentUuid": null, "promptId": "f560b203-…", "type": "user", "message": {"role": "user", "content": "<local-command-caveat>The command below was run directly in Claude Code, not sent to you as a request, and its output goes straight to the user. It's recorded here as context for later messages.</local-command-caveat>"}, "isMeta": true, "uuid": "fe8eb0ef-…", "timestamp": "2026-11-25T04:07:57.366Z"}
{"parentUuid": "fe8eb0ef-…", "promptId": "f560b203-…", "type": "user", "message": {"role": "user", "content": "<bash-input>echo out; echo err >&2; printf '<b>&x\\n'</bash-input>"}, "uuid": "ece3a09c-…", "timestamp": "2026-11-25T04:07:57.276Z"}
{"parentUuid": "ece3a09c-…", "promptId": "f560b203-…", "type": "user", "message": {"role": "user", "content": "<bash-stdout>out\nerr\n&lt;b&gt;&amp;x</bash-stdout><bash-stderr></bash-stderr>"}, "uuid": "7ad45052-…", "timestamp": "2026-11-25T04:07:57.366Z"}
```

With the default (`respondToBashCommands` true) there is no caveat record. The output record
carries `"turnOrigin": "human", "turnPosition": {…}`, and an `api-request` follows. A
`{"type":"tools-used","tools":["Bash"]}` record and a `last-prompt` of `"! <cmd>"` are also
written. None of the three records has an `origin` (as in harness-surfaces.md: "no origin").

### What the model sees next turn (verified: 2.1.330 writes `api-request-blob` records into the JSONL)

Each record becomes **its own text block** of the next user message. A `\n` is appended to
each, and the caveat stays in even though it is `isMeta`:

```
"<local-command-caveat>The command below was run directly in Claude Code, … context for later messages.</local-command-caveat>\n"
"<bash-input>echo out; echo err >&2; printf '<b>&x\\n'</bash-input>\n"
"<bash-stdout>out\nerr\n&lt;b&gt;&amp;x</bash-stdout><bash-stderr></bash-stderr>\n"
"hello"
```

## 2. What a program can use

| Path | Runs the command? | In the session file? | Model sees it? | Starts a turn? | Verified |
| --- | --- | --- | --- | --- | --- |
| User message `"!echo x"` (stream-json / SDK) | **No** | as the prompt `!echo x` | as the text `!echo x` | yes (an ordinary prompt) | yes, 2.1.329 against a fake API |
| `{"type":"bash_command","command":…,"cwd"?,"uuid"?}` on stdin (`@internal`, not in `sdk.d.ts`) | yes, `/bin/sh -c` (or `pwsh -NoProfile -Command`), no permissions or sandbox, own env, 10 min timeout, 262 144-char cap per stream | **no** | **no** | no | yes, 2.1.329 |
| User message, `shouldQuery: false` | no (the host supplies the text) | yes, a plain user record | yes, merged into the next prompt | no (emits a `result` with `num_turns: 0`) | yes, 2.1.329 |
| Setting `respondToBashCommands` | applies only to the TUI's `!` | — | — | — | binary |
| Hooks, control requests | nothing runs a user command | — | — | — | not found in `sdk.d.ts` |

### `bash_command` (internal) in detail

The output from a local run (2.1.329, stream-json) was emitted as two replay rows and
nothing more:

```json
{"type":"user","message":{"role":"user","content":"<bash-input>echo side-out; echo side-err &gt;&amp;2; pwd; exit 3</bash-input>"},"isReplay":true,…}
{"type":"user","message":{"role":"user","content":"<bash-stdout>side-out\n/tmp/bashexp/work</bash-stdout><bash-stderr>side-err</bash-stderr><bash-exit-code>3</bash-exit-code>"},"isReplay":true,…}
```

The schema's own description: "By default it runs in a one-shot `/bin/sh -c` … beside the
conversation, which never sees it". It runs as a real `!` turn only "on a cloud worker with
tengu_cloud_session_shell on". That is a server-side flag, and it applies only when the CLI
reads a cloud session stream. The CLI also refuses a `bash_command` in several cases: in a
background session (`CLAUDE_CODE_SESSION_KIND` set), when the command is older than 90 s, has
control characters, or is longer than 65 536 characters, or while another one is running.
Because it is internal, unversioned and kept out of the model's view, **YAAC should not use it**.

### `shouldQuery: false` appends in detail (2.1.329 via stream-json; the SDK sends the same frames)

The `SDKUserMessage` documentation says: "When false, the message is appended to the
transcript without triggering an assistant turn. It will be merged into the next user message
that does query."

**Stored record** (verified). It is a plain user entry: no `isMeta`, no `origin`, and
`promptSource: "sdk"` with `queueTranscriptOnly: true`:

```json
{"parentUuid": null, "promptId": "7f6ed628-…", "type": "user", "message": {"role": "user", "content": "<bash-input>echo appended</bash-input>"}, "uuid": "22222222-2222-4222-8222-222222222222", "timestamp": "2026-11-25T04:01:47.767Z", "permissionMode": "default", "promptSource": "sdk", "queueTranscriptOnly": true, "turnAttributionKey": "22222222-…"}
```

The first append of a session also triggered the turn-start attachment pass. That pass writes
`environment`, `model`, `deferred_tools_delta`, `agent_listing_delta` and `skill_listing`
attachments.

**Merging** (verified). Several `shouldQuery: false` messages that are queued together are
merged into **one** record (`mergedSendUuids: [...]`):

- string contents are joined with `\n`;
- array contents are concatenated block by block, with `\n` appended to the earlier blocks.

**Rendering on `claude --resume`** (verified with the 2.1.330 TUI):

| Stored content | Rendered |
| --- | --- |
| Two separate records (input, then output) | `! echo sep-one ⎿ sep-one` (native) |
| One record whose content is an **array** `[{text:"<bash-input>…"},{text:"<bash-stdout>…"}]` | `! echo blocks-two ⎿ blocks-two` (native) |
| Two array records merged during a turn (5 blocks, a caveat block between) | both commands native; the caveat block hidden |
| One **string** `"<bash-input>…</bash-input>\n<bash-stdout>…"` (two string messages merged) | **raw markup** shown as a user prompt |

So array content is robust against merging, and string content is not.

**`isSynthetic: true`** (verified). The record is stored with `isMeta: true` and
`origin: {kind: "unclassified"}`, and the text is **prefixed** with
`[MESSAGE FROM NON-USER SOURCE - NOT USER INPUT]\n`. That is wrong for a command the user ran,
so do not use it.

**While a turn is running** (verified, with a fake turn that ran a 6 s Bash tool call):

| `priority` | What happened |
| --- | --- |
| absent (or `"next"`; **inference:** absent means `next`) | Delivered **inside the running turn**, at the next tool boundary, as `queued_command` attachments. The model received, as a system-role message, `"The user sent a new message while you were working:\n<bash-input>echo default-prio</bash-input>\n\nThis is how Claude Code surfaces messages the user sends mid-turn … Address the message above as you continue this turn."` `shouldQuery: false` does **not** prevent this. |
| `"later"` | Held until the turn ended, then appended as a plain record (`result` with `num_turns: 0`). The model saw it at the start of the next prompt. This matches the TUI. |
| `"now"` | not tested (**inference:** it interrupts or joins the running turn; never use it for this) |

## 3. How YAAC should implement it

**There is no programmatic way to make Claude Code run a `!` command so that the model sees
it.** The faithful alternative is to reproduce the records:

1. Run the command in `yaacd`, in the thread's working directory. For the owner's
   PTY-terminal feature ("send every command run here to the agent"), YAAC does not run
   anything: it takes the command, output and exit code that shell integration marked.
2. Push **one** message on the thread's existing SDK input stream:

   ```ts
   {
     type: "user",
     message: { role: "user", content: [
       { type: "text", text: `<bash-input>${command}</bash-input>` },
       { type: "text", text: `<bash-stdout>${xmlEscape(stdout)}</bash-stdout><bash-stderr>${xmlEscape(stderr)}</bash-stderr><bash-exit-code>${code}</bash-exit-code>` },
     ] },
     parent_tool_use_id: null,
     uuid: opId,            // the operation id (no blind retries); dedups on the CLI side
     shouldQuery: false,    // context only: no turn
     priority: "later",     // never injected into a running turn
   }
   ```

   - **Command text**: raw, as Claude Code writes it.
   - **Output**: XML-escape `&`, `<` and `>`, as Claude Code does. Remove ANSI escape
     sequences and handle carriage returns from a PTY. Claude Code keeps SGR codes, but in PTY
     output they are noise. This is YAAC's choice.
   - **Truncation**: cap the inline output near Claude Code's default, 30 000 characters. Use
     the marker the CLI's own headless runner uses: `\n[output truncated: N more characters not shown]`.
     Claude Code's `<persisted-output>` file belongs in its tool-results directory, which YAAC
     should not write to.
   - **stdout and stderr**: a PTY merges them anyway. Put everything in `<bash-stdout>` and
     leave `<bash-stderr></bash-stderr>` empty. That is what the TUI itself produces.
   - **`<bash-exit-code>`**: the TUI does not write it, but Claude Code's own headless runner
     does, YAAC's markup table already maps it (`markup.ts`: "Shell exit code"), and it tells
     the model whether the command failed. Include it.
   - **Caveat**: optional. To match the TUI's context-only (`respondToBashCommands: false`)
     shape, put the caveat text as the first block. It reaches the model and is hidden on
     `--resume`, but it is stored inside a non-`isMeta` record. YAAC's history then shows it as
     the "Note from Claude Code" context part. Wording that fits a terminal command better
     would be a YAAC decision (*Proposed*).
3. **To make the model respond** (like the TUI's default): send the same record, then a
   normal prompt. Alternatively, send the record itself with `shouldQuery` left on. **Untested:**
   whether a querying message consisting only of bash markup behaves well; it would also be
   delivered mid-turn if a turn is running.

**What this loses compared with a real `!`:**

- The records are user entries with `promptSource: "sdk"` and `queueTranscriptOnly: true`, not
  the TUI's bare ones. There is no `isMeta` caveat record, because the SDK input cannot set
  `isMeta` except through `isSynthetic`, which adds the "NON-USER SOURCE" prefix. `--resume`
  still renders them natively, and the model's view is the same blocks.
- A command YAAC runs does not share the Bash tool's shell session: no
  `Shell cwd was reset` handling, no Claude Code env snapshot, no auto-background at 120 s, no
  `<persisted-output>`. With `priority: "later"` YAAC must also decide what its own UI shows
  while the record is queued.
- `respondToBashCommands` and `defaultShell` are the user's Claude Code settings. Honouring
  them would mean reading the user's settings, which YAAC does not do today.

**YAAC-side consequences** (inference, to be checked when implementing):

- YAAC history has to treat these records as context and must not start a new turn for them.
  They are not `isMeta`. Check whether `getSessionMessages` passes `queueTranscriptOnly`
  through. If it does not, key on "the message is only bash markup", as `markup.ts` already
  recognises `<bash-input>` at the start.
- Each append produces a `result` with `num_turns: 0` and `command_lifecycle` frames. The
  provider must not report these as a finished turn.
- The append is a state-changing request. It carries the operation id as `uuid`
  (invariant 10), and the CLI skips duplicate uuids.

## Open questions

- Is `priority` absent the same as `"next"`? And what does `"now"` do with `shouldQuery: false`?
- Does a PreToolUse or PostToolUse hook on `Bash` fire for a TUI `!` command? The call passes
  `userTypedShellDispatch: true`, so possibly not.
- Does the model see ANSI codes from the TUI's records? They are stored; whether the API
  request strips them was not checked.
- How does the CCD (desktop app) bash mode build its appends? The SDK comment names it as a
  `shouldQuery: false` user. Its exact content (string or blocks, caveat, exit code) is
  unknown.
- In the TUI, `respondToBashCommands` defaulted to `true` here. Earlier versions behaved
  differently (context only). Versions were not bisected.

## How to re-verify

Everything below uses a throwaway config directory, a dummy API key, and a local fake
Messages API, so **no real API usage**. The user's sessions and settings are not touched.
Scripts used: `/tmp/bashexp/scripts/{fake.py,drive.py,tui.py,show.py}`. They are not kept in
the repo; they are easy to recreate:

- `fake.py`: an HTTP server that saves each `POST` body and answers `/v1/messages` with a
  streamed "FAKE-REPLY". It answers with a `Bash` tool_use when the prompt says `TOOLTURN`, and
  sleeps when it says `SLOW`.
- `drive.py`: runs `claude -p --input-format stream-json --output-format stream-json --verbose --replay-user-messages`
  and writes frames to it.
- `tui.py`: runs the interactive CLI in a pty and types into it.

```sh
claude --version
node_modules/@anthropic-ai/claude-agent-sdk-linux-x64/claude --version

# Types: shouldQuery, priority, respondToBashCommands, defaultShell, bashOutputMaxChars
grep -n "shouldQuery\|priority?:\|respondToBashCommands\|defaultShell\|bashOutputMaxChars\|CCD bash mode" \
  daemon/node_modules/@anthropic-ai/claude-agent-sdk/sdk.d.ts

# The TUI handler and the internal bash_command frame (minified JS inside the native binary)
python3 - <<'EOF'
d=open('/home/<user>/.local/share/claude/versions/<ver>','rb').read()
for k in [b'export{he as processBashCommand}', b'type:E("bash_command")', b'function NDe(){', b'runHeadlessBashCommand']:
    i=d.find(k); print(k, d[max(0,i-3000):i+500].decode('utf8','replace')[:3500] if i>=0 else 'missing')
EOF

# Isolated run: separate config, dummy key, local fake API
export CLAUDE_CONFIG_DIR=/tmp/x/cfg ANTHROPIC_API_KEY=sk-ant-dummy-not-a-key \
       ANTHROPIC_BASE_URL=http://127.0.0.1:<port> CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
unset $(env | grep -o '^CLAUDE[A-Z_]*' | grep -v CLAUDE_CONFIG_DIR)   # do not inherit a parent session
#  - TUI: in <cfg>/.claude.json set hasCompletedOnboarding, project trust, and
#    customApiKeyResponses.approved = [last 20 chars of the key]; type "!" then a command.
#  - SDK: write {"type":"user",...,"shouldQuery":false,"priority":"later"} frames to stream-json stdin.
#  - Read <cfg>/projects/<cwd-slug>/<session>.jsonl. 2.1.330 also writes api-request-blob
#    records there: the exact user content the model was sent.
#  - Check rendering with: claude --resume <session> (in the pty).
```
