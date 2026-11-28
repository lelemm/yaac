# 16 — External dialogs: a channel for integrator apps

**Proposed** (P417–P420). The owner's request, branch `external-dialogs`. A plugin system is not
needed yet; this is the first, narrow way for another program to reach the user through YAAC.

## What it is

An **integrator app** — any program on any target — connects to the `yaacd` of its own target and
registers a **channel**. A channel can open a **dialog** with a title and runtime layout. The
connected UIs (Windows, Android) show its content, inputs, and buttons while the integrator
waits for an answer.

```
integrator app → yaacd on its target → hub → every attached UI
integrator app ← yaacd on its target ← hub ← the UI that answered
```

## Connecting

Connect directly to the local daemon while it is running. Send one JSON-RPC object per line,
encoded as UTF-8. Each connection starts with `channel/register`.

| OS | Address |
| --- | --- |
| Linux, WSL, macOS | `~/.yaac/run/channel.sock` |
| Windows | `\\.\pipe\yaac-channel-<USERNAME>` |

On Unix the socket accepts only channel connections. It has mode `0600` and lives inside the
daemon's private run directory. A daemon configured with a different home places `run/channel.sock`
under that home. On Windows the plain pipe name uses the daemon's OS account name and stays the
same across daemon homes and builds. When the endpoint is already owned, a second daemon leaves
it with the owner.

Node 22.22.3's bundled libuv creates the Windows pipe with the default Windows security
descriptor. Ordinary users of other accounts can have read access under that descriptor,
but cannot open the pipe for read/write or send `channel/register`. The creator, LocalSystem,
and administrators have full control. Each connection receives results only for dialogs on
the channel it registers or resumes. [Node's libuv source](https://github.com/nodejs/node/blob/v22.22.3/deps/uv/src/win/pipe.c#L505-L513),
[Microsoft's named-pipe access rights](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights).

See [the example integrator](../examples/dialog-demo.mjs) for a runtime-defined dropdown.

## The integrator's side (P417)

- **`channel/register { name, description?, resume? }`** → `{ channelId }`. Save the returned
  ID and pass it as `resume` to reattach on a later connection. The original name identifies the
  dialog source. Each channel can have one live connection. Resuming a channel that is still
  attached returns `channel_busy`.
- **`dialog/open { requestId, title, layout, threadId?, nativeThreadId? }`** →
  `{ accepted: true, placement: "thread" | "inbox", threadId? }` after journaling (invariant 8).
  The integrator supplies `requestId` as the operation ID, unique within its channel. A repeated
  ID is refused with `duplicate_request`. Supply either `threadId` or `nativeThreadId`, or
  omit both. If the daemon resolves a thread, the result gives its YAAC thread ID and the
  dialog appears in that thread and in Needs you. Otherwise it appears under the target in
  Needs you and the result has `placement: "inbox"`. An ambiguous native ID goes to the inbox
  just like an unknown one.
- **Notification `dialog/result { requestId, outcome, action?, values? }`** to the channel:
  `answered` carries the button ID in `action` and the input values keyed by ID in `values`.
  The other outcomes are `cancelled` and `unknown` (see below).
  Each button yields `answered` even when its label is Cancel or it sets `submit: false`.
  Closing the dialog (×, Escape, or Back) yields `cancelled`.
- **`dialog/status { requestId }`** returns `{ state, action?, values? }`, where `state` is
  `pending`, `answered`, `cancelled`, or `unknown`. Use the integrator's request ID on
  the same channel.
- **`dialog/close { requestId }`**: the integrator withdraws a dialog it no longer needs; the UIs
  drop it and the result is `cancelled` (once).

### From a harness hook

For a Claude Code command hook, read the JSON on stdin and pass its `session_id` as
`nativeThreadId` (verified against the installed Claude Code 2.1.332). For Codex pass the
conversation's `Thread.id`. Codex's `sessionId` identifies a session tree. If the integrator
already has the YAAC thread ID, use `threadId` by itself. A native ID is used only when it
identifies exactly one YAAC thread.
[Claude Code hook input fields](https://code.claude.com/docs/en/hooks#common-input-fields).

An integrator can also learn `yaacThreadId` from YAAC's own launch hook. Use `thread.yaacThreadId`
for `thread.beforeFirstPrompt` or `child.yaacThreadId` for `worker.beforeFirstPrompt`.
Enable `thread.beforeFirstPrompt` in the launch config to receive the thread event.
A dialog opened before the new thread is registered follows the usual inbox placement.
[YAAC launch hook](10-agent-mesh.md#launch-hook).

## Runtime layout (P420)

`layout` is a tree of objects, each with a `type`. The integrator defines the contents,
input choices, and button placement when it opens the dialog.

| Types | Properties |
| --- | --- |
| `column`, `row` | `children`, `gap?` |
| `group` | `label?`, `children` |
| `divider` | |
| `text` | `text`, `style?` (body, heading, caption, code), `tone?` (normal, muted, warning, error, success) |
| `markdown` | `text` |
| `image` | `src` (HTTPS or raster data URL), `alt`, `maxHeight?` |
| `link` | `label`, `url` |
| `list` | `items` (strings) |
| `table` | `columns`, `rows` (strings) |
| `progress` | `value?` (0–1, indeterminate when absent) |
| `textInput` | `default?`, `placeholder?`, `multiline?`, `secret?`, `maxLength?` |
| `number` | `default?`, `min?`, `max?`, `step?` |
| `checkbox`, `toggle`, `date` | `default?` (boolean or YYYY-MM-DD for dates) |
| `select`, `radio` | `options`, `default?` (choice value), `placeholder?` |
| `multiSelect` | `options`, `default?` (array of choice values) |
| `slider` | `min`, `max`, `step?`, `default?` |
| `button` | `id`, `label`, `kind?` (primary, danger, normal), `submit?`, `requires?` (input IDs) |

Every input carries `id`, `label?`, `help?`, `required?`, and `disabled?`. Options carry
`value`, `label`, and optional `detail`. A submit button validates enabled required inputs.
A button with `submit: false` skips those requirements. Both validate any supplied values and
the button's own `requires` list. Unknown nodes render a muted “This part of the dialog needs
a newer YAAC” line. An enabled unknown required input prevents submission. Secret inputs
cannot have defaults and their answers stay out of journals and caches.

A layout can have at most 500 nodes across 12 levels. Dialog content and answers each have a
256 KiB limit. Inline raster image data URLs have a separate 1 MiB budget across the dialog.
An oversized or invalid dialog is refused before it reaches the user. The target can have up
to 256 connected channels and 16 MiB of pending dialog records, which keeps full snapshots
within the protocol's message limit.

## How it travels (P418)

- **`yaacd`** turns `dialog/open` into an ordinary **request** with a new body kind **`dialog`**
  (`RequestBody::Dialog`), raised in the thread when the supplied ID resolves uniquely, or in a
  target-level **inbox** (a pseudo-scope the hub lists under Needs you). It is journaled before it
  is acknowledged, survives the host being away, and is never auto-answered or timed out
  (invariant 8). The request carries `source: { channelId, name }` so every UI says where it comes
  from; it is never shown as coming from an agent or from the user (invariant 13).
- **The hub** keeps the channel registry (`channelId` → `targetId`, name, connected) from `yaacd`'s
  `channel/registered|closed` notifications, and routes the answer back over that target's one
  connection (invariant 1) — the answer path of every other request, so it reaches the `yaacd` that
  raised it. No new connection, no side channel.
- **The UIs** render the dialog with the existing request cards / form renderer (egui, WPF,
  Android), plus a notification on the phone like any request. Secret fields are never cached on the
  host (as secret questions today).
- **One decision per request.** Answers carry an operation ID. The first accepted answer
  records the final decision. A later answer gets `request_not_pending`. A dropped hub
  connection can leave the UI unsure whether its operation took effect, so it never retries
  automatically (invariant 10).
- **Delivery is separate.** A disconnected dialog stays answerable and says “<name> is not
  connected · your answer will be delivered when it reconnects”. The daemon retains an open
  dialog until answered, withdrawn, or dismissed. It journals each decision before sending
  its result. On resume it emits queued results once per connection. A successful socket write
  retires the queued result. An interrupted write can cause a result to appear on a later
  connection, so integrators deduplicate by `requestId`. The redacted decision remains
  available to `dialog/status`.
- **Secret values have one delivery attempt.** Secrets can be returned through the first
  `dialog/result` or `dialog/status` that releases them. The daemon discards them before
  attempting that write and keeps any waiting values only in memory. After a daemon restart,
  a queued result or status can still report the decision and non-secret values. A later
  notification or status reply omits secrets that were released earlier.
- **Restart and deletion.** A disconnected channel can be resumed after a daemon restart.
  Deleting a thread moves its pending dialogs into the target inbox.

Channel errors use JSON-RPC 2.0 integer codes. `error.data.reason` gives the stable YAAC reason
such as `duplicate_request`, `channel_busy`, or `request_not_found`.

| Code | Meaning |
| --- | --- |
| `-32700` | Invalid JSON |
| `-32600` | Invalid JSON-RPC request |
| `-32601` | Unknown method |
| `-32602` | Invalid method parameters |
| `-32000` | Application error |

## Contracts (names; types in `yaac-protocol`)

Integrator ⇄ `yaacd` (a channel connection): `channel/register`, `dialog/open`, `dialog/status`, `dialog/close`,
notification `dialog/result`.

`yaacd` ⇄ hub (daemon protocol): capability `channels`; notifications `channel/registered
{channelId, name, description?}`, `channel/closed {channelId}`; dialogs ride the existing request
events with `RequestBody::Dialog(DialogRequest { title, layout, source, channelOpen })`
and `RequestResponse::Dialog(DialogAnswer { action?, values, cancelled })`. The daemon assigns
a distinct `requestId` to the request the hub sees. The integrator's own ID is used on its
channel for open, close, and result.

A dialog in a thread uses the existing `thread/event` and `request/respond` paths. For dialogs
without a thread, `inbox/list {}` returns `{ requests, channels }` and hydrates the current
target-level inbox and channel registry on attach. `inbox/updated { request }` upserts a pending
`RequestItem` or removes a terminal one. The hub sends `inbox/respond { opId, requestId, response }`
for answers and dismissals, using the same request and answer shapes without a thread ID.

Hub ⇄ UI (attach): the target inbox requests in the index/needs-you payload.
