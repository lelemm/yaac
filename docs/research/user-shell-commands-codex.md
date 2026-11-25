# Codex composer shell commands

Verified on WSL Ubuntu, 2026-11-25. Typing `! printf tui-marker` and pressing Enter in the installed Codex 0.170.0 TUI sent the following request to a private 0.172.0 app-server. The TUI displayed “You ran printf tui-marker” and the output underneath.

```json
{
  "id": 6,
  "method": "thread/shellCommand",
  "params": {
    "threadId": "01a210da-49d9-77c0-a61d-80aca79789de",
    "command": "printf tui-marker",
    "timeoutMs": null
  }
}
```

**Recommendation for YAAC.** Expose a provider capability for user shell commands. On explicit composer submission, the Codex adapter should send the command string after the leading bang to `thread/shellCommand` on the loaded thread over the existing daemon connection. Preserve quoting and shell operators. Surface that the command has full access on the target. Journal normal item events, render `source: "userShell"` as user-initiated, and let Codex maintain model history. The empty acknowledgement carries no item ID. Correlate notifications by thread, turn and item ID. Apply YAAC's normal writer ownership and uncertain-outcome rules. A lost acknowledgement can follow a command that already ran.

## Observed behavior

| Question | Installed schema or runtime observation |
| --- | --- |
| Availability | Both the PATH CLI (0.170.0) and managed binary (0.172.0, matching the version reported by the running daemon) generated the method in default TS bindings. The JSON Schema requires string `threadId` and `command`. Optional nullable int64 `timeoutMs` defaults to one hour. Zero requests immediate timeout. Reply type is an empty object. A client initialized without experimental capabilities successfully called it. |
| Execution | The thread's configured shell evaluates the string in the thread cwd on the target. On this Linux fixture the item showed `/bin/bash -lc`; a probe printed the thread directory, `/bin/bash` and a variable inherited from the server. With `shell_environment_policy.inherit="none"` and an explicit `set` variable, the inherited marker disappeared and the set variable appeared. There are no per-call cwd, shell, environment, or sandbox fields in the schema. |
| Permissions | The schema explicitly specifies unsandboxed full access. With thread sandbox `read-only` and approval policy `on-request`, a shell command wrote a marker outside the repo inside the test directory. No approval request arrived. |
| Idle thread | Acknowledgement, active thread status, `turn/started`, `item/started`, zero or more `item/commandExecution/outputDelta`, `item/completed`, idle status, `turn/completed`. Four shell-only turns made zero requests to the local model endpoint. A subsequent ordinary user turn included their results. |
| Running turn | While the local model endpoint held a response open, a shell command was acknowledged and finished under the **same turn ID**. It produced its own command item, with no additional turn-start notification. After the endpoint completed the outstanding response, Codex made another Responses request containing the shell result within that turn. An already transmitted request cannot contain the later result. |
| Streams and status | The two deltas for the short fixture contained `one\n` (stdout) and `two\n` (stderr). The wire delta has a single text field and no stream label. Watch the completed item's `exitCode` and `status`. Exit 7 failed the item while its shell-only turn completed normally. |
| Timeout and validation | `sleep 10` with an 80 ms timeout received an acknowledgement and then a failed item with `exitCode: -1`, `durationMs: 0`, and execution-error text containing the underlying timeout and internal exit code 124. Blank command and missing loaded thread returned -32600 (“command must not be empty” / “thread not found: …”). Malformed UUID returned -32600. Negative timeout returned -32602. |
| Large output | A 192,001-character ASCII line was delivered in full by deltas and live completion. On `thread/read` the completed item's aggregate had 65,536 characters. The model-facing text retained 10,000 original characters around a middle omission, with `Warning: truncated output (original token count: 48001)`, `Total output lines: 1`, and `…182001 chars truncated…`. In a larger ASCII probe the deltas totaled 6,754,388 characters while the live completion held 1,048,576. Treat streamed, replayed, and model-visible outputs as separate representations. These are measured lengths for these inputs. |

## Captured wire and history

For a second command, these are the actual request, acknowledgement, first delta and completed notification. The start notification carried the same command item ID, `status: "inProgress"` and null output, exit code and duration.

```json
{
  "method": "thread/shellCommand",
  "params": {
    "threadId": "01a210d9-a266-7651-80d9-7ddd73f80c3f",
    "command": "printf 'one\\n'; sleep .2; printf 'two\\n' >&2; exit 7"
  },
  "id": 3
}
```

```json
{
  "id": 3,
  "result": {}
}
```

```json
{
  "method": "item/commandExecution/outputDelta",
  "params": {
    "threadId": "01a210d9-a266-7651-80d9-7ddd73f80c3f",
    "turnId": "01a210d9-a26f-7c63-b736-75c5d0f63c71",
    "itemId": "322f7126-d52c-45e8-b998-f9fe7b6721bd",
    "delta": "one\n"
  },
  "emittedAtMs": 1795579028224
}
```

```json
{
  "method": "item/completed",
  "params": {
    "item": {
      "type": "commandExecution",
      "id": "322f7126-d52c-45e8-b998-f9fe7b6721bd",
      "pluginId": null,
      "scriptPath": null,
      "command": "/bin/bash -lc \"printf 'one\\\\n'; sleep .2; printf 'two\\\\n' >&2; exit 7\"",
      "cwd": "/tmp/yaac-shell.HlR9H5o1/repo",
      "processId": null,
      "source": "userShell",
      "status": "failed",
      "commandActions": [
        {
          "type": "unknown",
          "command": "printf 'one\\n'; sleep .2; printf 'two\\n' >&2; exit 7"
        }
      ],
      "aggregatedOutput": "one\ntwo\n",
      "exitCode": 7,
      "durationMs": 204,
      "startedAtMs": 1795579028220,
      "completedAtMs": 1795579028424
    },
    "threadId": "01a210d9-a266-7651-80d9-7ddd73f80c3f",
    "turnId": "01a210d9-a26f-7c63-b736-75c5d0f63c71",
    "contentTokenCount": null,
    "completedAtMs": 1795579028424
  },
  "emittedAtMs": 1795579028426
}
```

The private rollout contains `event_msg/task_started`, `event_msg/item_completed`, the following `response_item`, then `event_msg/task_complete`. The structured command item supplies the transcript entry. The separate user-role message supplies model context.

```json
{
  "timestamp": "2026-11-25T03:57:08.426Z",
  "ordinal": 3,
  "type": "response_item",
  "payload": {
    "type": "message",
    "id": "msg_01a210d9-a3ca-7680-b0fc-fd091d4f8e74",
    "role": "user",
    "content": [
      {
        "type": "input_text",
        "text": "<user_shell_command>\n<command>\nprintf 'one\\n'; sleep .2; printf 'two\\n' >&2; exit 7\n</command>\n<result>\nExit code: 7\nDuration: 0.2042 seconds\nOutput:\none\ntwo\n\n</result>\n</user_shell_command>"
      }
    ],
    "internal_chat_message_metadata_passthrough": {
      "turn_id": "01a210d9-a26f-7c63-b736-75c5d0f63c71",
      "create_time": 1795579028.4264596,
      "content_item_kinds": [
        "shell.user_command"
      ],
      "content_item_metadata": [
        {
          "provenance": {
            "type": "harness",
            "hook": false
          }
        }
      ]
    }
  },
  "metadata": {
    "history_sequence": 1,
    "sampled_through_sequence": 0,
    "client_authored": false,
    "mcp_attribution": {
      "status": "none"
    },
    "model_search_state": "unused"
  }
}
```

This is the actual input element from the local Responses endpoint. JSON newline escapes decode to line breaks. The command and result text matched the rollout exactly, including the blank line after the trailing output newline.

```json
{
  "type": "message",
  "id": "msg_01a210d9-a3ca-7680-b0fc-fd091d4f8e74",
  "role": "user",
  "content": [
    {
      "type": "input_text",
      "text": "<user_shell_command>\n<command>\nprintf 'one\\n'; sleep .2; printf 'two\\n' >&2; exit 7\n</command>\n<result>\nExit code: 7\nDuration: 0.2042 seconds\nOutput:\none\ntwo\n\n</result>\n</user_shell_command>"
    }
  ]
}
```

## Scope and open questions

The installed standalone package contains native executables and resources. No matching Rust TUI source was located in the installed packages or home-tree filename search. The TUI request above was observed across the transport. The timeout error log identifies the core module as `codex_core::tasks::user_shell`. The internal Rust Op variant name was not established. Windows shell selection, multibyte output boundaries, exact truncation algorithms across model configurations, and races with interrupt or simultaneous shell requests remain unverified.

For a daemon lacking this method, the proposed compatibility behavior is to leave the capability unavailable. Running a shell in yaacd and injecting markup would require recreating shell selection, environment, timeout, streaming, turn membership, replay and harness provenance. Sending that markup via `turn/start` would additionally invoke the model. Neither emulation is needed on the two examined versions.

## How to re-verify

1. Run `codex --version` and `codex app-server daemon version`. Record both versions and `managedCodexPath`. For each executable generate `app-server generate-ts --out DIR` and `app-server generate-json-schema --experimental --out OTHER_DIR` in separate temporary directories. Search `ClientRequest.ts`, `v2/ThreadShellCommandParams`, and `v2/ThreadShellCommandResponse`.
2. Make a fresh temp directory with an empty OS home, CODEX_HOME and git repo. Configure a fixture model with a custom provider using `wire_api="responses"` and a loopback HTTP base URL. Use an isolated environment with PATH, SHELL, HOME and CODEX_HOME set explicitly. Launch the managed binary as a child with `app-server --listen stdio://` in the test repo. All runtime observations here used private processes and synthetic prompts.
3. Send one JSON object per line. Send `{"id":1,"method":"initialize","params":{"clientInfo":{"name":"probe","version":"1"}}}`, then `{"method":"initialized","params":{}}`, then `{"id":2,"method":"thread/start","params":{"approvalPolicy":"on-request","sandbox":"read-only"}}`. Substitute its returned `result.thread.id` in the example requests. Record notifications through item/turn completion. Check the resulting JSONL under the private home's `sessions/`. Compare live output with `thread/read` using `includeTurns:true`.
4. To reproduce the model and concurrency check, record the fixture provider's POST bodies. Start a normal `turn/start` with a single text input after the shell-only turns. Hold the first POST open, issue a new shell command, and wait for its item completion. Respond with HTTP 200 `text/event-stream` containing `response.created` and `response.completed` events (same response ID, empty output). Capture the following POST too. Inspect `input[]` entries with role `user` and content beginning with `<user_shell_command>`.
5. To check the composer itself, connect the installed TUI using `codex --remote ws://127.0.0.1:PORT` from an isolated terminal and home. In this test a loopback WebSocket recorder forwarded text frames to the private stdio process and replies back to the TUI. Type the bang command, press Enter and inspect the outgoing frame. Close only the private clients and processes afterward.

## Commands already executed in a YAAC terminal

**Use `thread/inject_items` to append the PTY result.** This spelling, including the underscore, appears in the default generated `ClientRequest.ts` for both 0.170.0 and 0.172.0. The 0.172.0 private server accepted it with ordinary client capabilities. Camel-case `thread/injectItems` returned -32600 (unknown method). The params take `threadId` and `items: Array<JsonValue>`, documented as raw Responses API items for model-visible history. The method returned `{"id":4,"result":{}}`.

### Captured idle injection and subsequent model input

This fixture supplied already-known command text, duration, exit code and PTY output. The named marker file remained absent. Sending this request while idle produced no shell execution, turn lifecycle events, command-item events, or model traffic.

```json
{
  "method": "thread/inject_items",
  "params": {
    "threadId": "01a210df-3f3e-7941-a1e5-6bef6d5a3c71",
    "items": [
      {
        "type": "message",
        "role": "user",
        "content": [
          {
            "type": "input_text",
            "text": "<user_shell_command>\n<command>\ntouch SHOULD_NOT_EXIST_IDLE\n</command>\n<result>\nExit code: 0\nDuration: 0.1250 seconds\nOutput:\nPTY-IDLE\n\n</result>\n</user_shell_command>"
          }
        ]
      }
    ]
  },
  "id": 4
}
```

After closing the private process, starting a fresh one on the same temporary home, calling `thread/resume`, and sending an ordinary user prompt, the first HTTP request to the mock Responses endpoint included this separate input element before the new prompt.

```json
{
  "type": "message",
  "id": "msg_01a210df-3f76-7123-b2d9-52f8a1a5ec2e",
  "role": "user",
  "content": [
    {
      "type": "input_text",
      "text": "<user_shell_command>\n<command>\ntouch SHOULD_NOT_EXIST_IDLE\n</command>\n<result>\nExit code: 0\nDuration: 0.1250 seconds\nOutput:\nPTY-IDLE\n\n</result>\n</user_shell_command>"
    }
  ]
}
```

Codex supplied the message ID. The role, content type, delimiters, and literal text match the model-facing format of the native example above. Format measured duration in seconds with four fractional digits. Preserve output newlines: the formatter adds a newline before `</result>`, so output that already ends in a newline leaves a blank line. The PTY supplies its combined output stream. YAAC owns capture and truncation for this path. The small fixture verifies text fidelity, not automatic replication of the native shell formatter's output budget.

| Property | Observed with private 0.172.0 app-server |
| --- | --- |
| Rollout | A durable `response_item` with a user-role message and generated ID. Its internal `content_item_kinds` was `["unknown"]`. Native execution recorded `["shell.user_command"]` with harness provenance and a separate completed command event. Omit internal metadata from the client request. |
| Transcript | Before any ordinary turn, `thread/read(includeTurns:true)` returned an empty turns list even though the injected message had been persisted. After two ordinary turns the only items in those turns were their user prompts. Injection created no renderable `commandExecution` item. |
| Active turn | An injection acknowledged while the fixture held an outstanding model request open appeared in a follow-up request **within that same turn**, then again on the next ordinary turn. For strictly next-turn delivery, YAAC must buffer while the model is active and coordinate the append with the next turn submission. |
| Compaction | On this short resumed thread, explicit `thread/compact/start` sent both injected messages to the fixture summarizer. The fixture returned “fixture summary”, omitting both markers. The next normal model input contained the summary and neither original shell record. This establishes eligibility for compaction, not a retention guarantee across providers or context lengths. |

**Proposed YAAC behavior.** On shell-integration command completion in an opted-in terminal, journal the terminal record with its destination thread and submit one user-role input message as above over the existing daemon connection. Serialize completed records in terminal order and coordinate with ordinary sends. Keep the journal for display and reconnect, since the Codex turn view lacks the PTY entries. An empty acknowledgement has no record ID or idempotency key. Treat a lost reply as an uncertain write. Attribute the command to the terminal in YAAC's UI. The native execution method remains appropriate only for a command that still needs to run.

**Fallback if a target lacks injection (design inference).** YAAC could hold records in its own durable queue and prefix the next explicit user submission with their formatted text. Until that submission, Codex resume, other clients and compaction cannot see them. Afterward they share the ordinary prompt's message/turn attribution, and compaction can summarize them. The direct append preserves independent model-visible messages and their order before the prompt.

**Re-verify** using the private setup above. Search for the literal `thread/inject_items` in each default binding. Submit the captured request with a fresh thread ID and a marker filename absent from its cwd. Check marker absence, rollout, emitted events, and the turn list. Close only the private process, resume on a fresh private server with the same home, and capture the next POST. Repeat with a second marker while that POST is held open. For the compaction check, return a synthetic assistant summary from the loopback provider during `thread/compact/start`, wait for completion, and compare the compaction input with the following ordinary POST.

## Making the model answer a `!` command (P344)

Verified on WSL Ubuntu, 2026-11-25, against a private `codex app-server --listen stdio://`
(managed binary, codex-cli 0.172.0) with a temporary `CODEX_HOME`, an empty OS home, and a
loopback fixture Responses provider that answered every POST with one assistant message.
The user's own daemon and sessions were not used.

`thread/shellCommand` alone never reaches the model: its shell-only turn completes with zero
model requests (above). The faithful way to have the agent answer it is **`turn/start` with
`input: []`, sent after the shell turn's `turn/completed`**:

| Observation | Result |
| --- | --- |
| Request | `{"method":"turn/start","params":{"threadId":T,"input":[]}}` — no text, no `clientUserMessageId` |
| Accepted? | Yes: `{"turn":{"id":T2,"status":"inProgress","items":[],…}}`, a new turn id (the shell turn T1 had already ended) |
| Model input (thread with an earlier ordinary turn) | the history ends with the `<user_shell_command>` user message exactly as Codex recorded it (`…Exit code: 3\nDuration: 0.0043 seconds\nOutput:\nhello\n\n</result>\n</user_shell_command>`); **no extra user message** is added by the empty input |
| Model input (first model call of a fresh thread) | Codex adds its usual turn-start context (developer instructions, `<environment_context>`) after the record, as for any first turn |
| `thread/read` afterwards | T1: `[commandExecution source=userShell]`; T2: `[agentMessage]` — no `userMessage` item in T2 |

So the conversation the harness stores is the `!` record followed by the model's answer, with
nothing invented by YAAC. Sending the same `turn/start` while T1 is still running would join
T1 (see "Running turn" above) and could sample before the command finished, so YAAC waits for
T1's `turn/completed` first. YAAC shows T1 and T2 as **one** turn (the YAAC turn id is T1's):
the provider holds T1's completion, starts T2 at once, and reports T2's items and completion
under T1. If the command was stopped (`turn/interrupt` on T1 → `interrupted`), or T1 failed,
no answer turn is started.

Re-verify: the probe in "How to re-verify" above with, after the shell turn's
`turn/completed`, `{"id":4,"method":"turn/start","params":{"threadId":T,"input":[]}}`, then
read the fixture's last POST body (`input[]`) and `thread/read` with `includeTurns:true`. Do it
once on a fresh thread and once after an ordinary `turn/start` with text.

**Stop** (same private setup): `turn/interrupt { threadId, turnId: T1 }` while `sleep 30` ran
returned `{}`, and within 10 ms the command item completed with `status: "failed"`,
`exitCode: -1`, `aggregatedOutput: "command aborted by user"`, and T1 completed
`interrupted`. So Stop on a running `!` command is the ordinary turn interrupt of its turn.
