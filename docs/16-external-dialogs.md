# 16 — External dialogs: a channel for integrator apps

**Proposed** (P417–P420). The owner's request, branch `external-dialogs`. A plugin system is not
needed yet; this is the first, narrow way for another program to reach the user through YAAC.

## What it is

An **integrator app** — any program on any target — connects to the `yaacd` of its own target and
registers a **channel**. The first and only operation of a channel is a **dialog**: a title, text,
fields, options and buttons, shown to the user in the connected UIs (Windows, Android), that waits
for one answer.

```
integrator app → yaacd on its target → hub → every attached UI
integrator app ← yaacd on its target ← hub ← the UI that answered
```

## The integrator's side (P417)

- **Transport.** The integrator speaks newline-delimited JSON-RPC 2.0 over **`yaacd channel
  --name <app>`** (stdio), a bridge like `yaacd connect` that attaches to the target's running
  `yaacd` (starting it if none serves) and registers the channel. On Linux an integrator may also
  connect to `yaacd`'s Unix socket directly and send `channel/register` itself. No authentication of
  its own in v1: the socket and the bridge are the user's own (user-only permissions), as the owner
  asked.
- **`channel/register { name, description? }`** → `{ channelId }`. The name is shown to the user as
  the dialog's source ("From <name> on <target>"). One channel per connection; it ends when the
  connection closes.
- **`dialog/open { requestId, title, layout, threadId? }`** →
  `{ accepted: true }` once `yaacd` has journaled it (invariant 8). The integrator supplies
  `requestId` as the operation ID, unique within its channel. A repeated ID is refused with
  `duplicate_request`. A known `threadId` places the dialog in that thread and in Needs you.
  Otherwise the dialog appears under the target in Needs you.
- **Notification `dialog/result { requestId, outcome, action?, values? }`** to the channel:
  `outcome` ∈ `answered` (with `action` = the button id and `values` = input id → JSON value) | `cancelled` (the user closed it) | `unknown` (see below).
- **`dialog/close { requestId }`**: the integrator withdraws a dialog it no longer needs; the UIs
  drop it and the result is `cancelled` (once).

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

## How it travels (P418)

- **`yaacd`** turns `dialog/open` into an ordinary **request** with a new body kind **`dialog`**
  (`RequestBody::Dialog`), raised on the thread when `threadId` is given and known, else on a
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
- **One result per request.** Answers carry an operation id; the first one wins, later taps on any
  UI see "already answered". A dropped connection while answering shows **outcome unknown** to the
  user (invariant 10); `yaacd` sends the integrator `dialog/result {outcome:"unknown"}` only if it
  cannot tell, and the real result if it can.
- **Integrator gone.** If the channel closes while a dialog is open, the dialog stays visible,
  marked "<name> is no longer connected"; answering it is refused with that reason; it can be
  dismissed. Nothing is delivered later to a different connection.

## Contracts (names; types in `yaac-protocol`)

Integrator ⇄ `yaacd` (a channel connection): `channel/register`, `dialog/open`, `dialog/close`,
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
