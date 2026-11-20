# Implementation plan

How the design in `docs/` becomes code. Work is split into **work packages** small enough for
one worker to finish and prove, with file ownership that lets several run at once without
touching each other's files.

The aim of the first pass is a **vertical slice that demonstrates the agent mesh end to end**
on real code paths — not every feature of the MVP. Each package lists what "done" means; a
package that does not meet it is reported as not done.

## Ground rules for every package

- **The docs are the specification.** `docs/decisions.md` and the invariants in `CLAUDE.md` win
  over anything a package brief says. If the brief and a doc disagree, stop and report it.
- **Own only your paths.** Do not edit files outside the paths the package owns. If another
  package's interface is wrong or missing something, report it; do not patch it.
- **No hardcoded model ids, model names, or effort levels**, including in tests of real
  providers. Fake providers may invent their own.
- **No version checks outside the dialect module**; no version fields on frames.
- **Never branch on provider kind in UI code.** Branch on capabilities.
- **Prove it.** Done means it builds with no warnings, its tests pass, and the commands that
  show that are in the report. Do not report something as working that was not run.
- **Do not commit.** The orchestrator handles version control.
- **Several workers share this workspace at once.** Build and test only your own crates
  (`cargo build -p <crate>`), with your own `CARGO_TARGET_DIR` (for example `target/wp3`), so
  another package that is mid-edit cannot break or block you. Declare dependencies in your own
  crate's `Cargo.toml`, not in the workspace root.
- Toolchain: Rust stable at `~/.cargo/bin` (not on `PATH` by default — use the full path or
  `. "$HOME/.cargo/env"`); Node.js 22; npm.

## Layout

```
Cargo.toml                 workspace
crates/
  yaac-protocol/           wire types, frame codec, scheduling, dialects, TS export, schema snapshot
  yaac-core/               domain model and app state — pure, no I/O, no UI
  yaac-transport/          connectors (local, ssh, wsl) and the bootstrap
  yaac-hub/                the hub: connections, mesh router, relay queue, cache, attach server
  yaac-app/                the `yaac` binary: the window, and `yaac hub`
daemon/                    yaacd (TypeScript)
  src/protocol/generated/  generated from yaac-protocol — never hand-edited
  src/session/             framing, server, journal, index, idempotency
  src/mesh/                MCP server, mailbox, outbox
  src/providers/{fake,codex,claude}/
protocol/schema/           frozen schema snapshots per protocol version
```

## Work packages

| # | Package | Owns | Needs | Done when |
| --- | --- | --- | --- | --- |
| **WP1** | **Protocol crate.** Every wire type in [03](../03-protocol.md) and the mesh additions in [10](../10-agent-mesh.md); frame codec; two-class scheduler with credit; the dialect table with one dialect; `initialize` negotiation; TypeScript generation; schema snapshot v1. | `Cargo.toml`, `crates/yaac-protocol/`, `daemon/src/protocol/generated/`, `protocol/schema/` | — | Round-trip and property tests pass for the codec and every message; a control frame is never delayed by more than one bulk chunk (tested); generated TypeScript is committed to its folder and a test fails if it is stale; nothing outside the dialect module can name the negotiated version. |
| **WP2** | **`yaacd` session layer.** `serve` and `connect`; framing; `initialize`; journal with epochs, compaction, replay, gap-free reset; thread index and sync tokens; operation ids; thread control; **fake provider** that can stream, ask for approval, ask a question, stall, fail, and be "in use elsewhere". | `daemon/` except `src/protocol/generated/`, `src/mesh/`, `src/providers/{codex,claude}/` | WP1 | Every case in the "session layer", "requests", and "thread control" rows of the [test strategy](../08-roadmap.md#testing-strategy) is a passing test against the fake provider; `yaacd connect` over stdio completes a turn end to end in an integration test. |
| **WP3** | **Hub and local transport.** Local connector spawning `yaacd`; reconnect with replay; the attach server and its protocol; SQLite cache with atomic items-plus-position commits; durable draft store; the `yaac hub` mode. | `crates/yaac-transport/`, `crates/yaac-hub/` | WP1 | An integration test starts a hub, which starts a real `yaacd` on the fake provider, runs a turn, kills the connection mid-stream, and shows nothing lost after reconnect; the cache survives a hub restart; a second attach sees the same state. |
| **WP4** (split into 4a and 4b, see Status) | **The window.** Theme with the dark and light tokens from [design/screens.md](../design/screens.md); sidebar with three-level grouping and toggles; virtualised transcript with streamed markdown; approval and question cards with every request state; composer with markdown styled in place; control row drawn from effective capabilities; the agents panel and the four mesh items; read-only "in use elsewhere" state; notices. | `crates/yaac-app/`, `crates/yaac-core/` | WP1 | Runs against a scripted in-memory hub with no daemon; headless rendering tests produce an image of each screen in both themes; UI state is tested without a window; no code path branches on provider kind. |
| **WP5** | **Codex provider.** WebSocket over the daemon's Unix socket; `initialize` + `initialized`; start the daemon when absent; models (all pages); thread index with explicit source kinds; history; turns with turn ids; approvals and `requestUserInput`; store identity; in-use detection. | `daemon/src/providers/codex/` | WP2 | Unit tests replay recorded app-server traffic into the adapter and assert the normalised items; a live smoke test against the running daemon lists models and threads and is skipped, not failed, when no daemon is present. No model id appears in source. |
| **WP6** | **Claude provider.** One `query()` per live thread; models from `supportedModels()`; sessions; history; approvals through `canUseTool`; `AskUserQuestion`; effort and model switching; idle eviction and resume; in-use detection. | `daemon/src/providers/claude/` | WP2 | Unit tests against a stubbed SDK assert the normalised items for a streamed turn, an approval, and a question; a live smoke test is skipped when `claude` is absent. No model id appears in source. |
| **WP7** | **The mesh.** The `yaac` MCP server and its ten tools; identity block; mailbox and outbox with hop-by-hop acknowledgement; delivery rules for every recipient state; spawn with parent and child records; `wait`; `notify_when_done`; limits and policy; hub relay queue and `mesh/query` routing; attachment to the fake, Codex, and Claude providers. | `daemon/src/mesh/`, the mesh module of `crates/yaac-hub/`, mesh hooks in the three providers | WP2, WP3, WP5, WP6 | Every case in the "agent mesh" row of the test strategy passes with two fake targets and a real hub; a scripted fake agent on one target spawns a worker on another, waits, and notifies a third thread, with the window never started. |
| **WP8** | **Remote targets.** SSH and WSL connectors; stage 0; on-demand runtime; SSH setup, verify, and revoke; WSL optional user and distro-down notice; settings screens for targets. | `crates/yaac-transport/` (ssh, wsl, bootstrap), settings in `crates/yaac-app/` | WP3, WP4 | The SSH setup-and-revoke test matrix passes against a local `sshd`; stage 0 passes under `dash` and `bash` with a noisy profile. |
| **WP9** | **Attachments and fork.** Clipboard and drop; upload protocol; temp-folder store; image routing by effective support; native and handoff fork with preview. | upload and fork modules across `daemon/`, `yaac-hub`, `yaac-app` | WP2–WP6 | Table-driven routing tests for supported, unsupported, and unknown; a handoff fork from a thread the fake provider "created elsewhere" works. |

## Order

```
WP1 ──► WP2 ──► WP5 ─┐
   │        └─► WP6 ─┼─► WP7 ──► (WP8, WP9)
   ├──► WP3 ─────────┘
   └──► WP4
```

WP1 goes first, alone: everything else compiles against it. Then WP2, WP3, and WP4 run
together. WP5 and WP6 follow WP2. WP7 is the goal of the first pass. WP8 and WP9 are attempted
only if the first seven are solid.

## Status

Kept current by the orchestrator. "Verified" means the orchestrator ran the package's tests
itself, not that the worker said so.

| # | Status | Verified by orchestrator | Notes |
| --- | --- | --- | --- |
| WP1 | **done** | **yes** — build, 73 tests, clippy `-D warnings`, generated-file staleness check, all re-run by the orchestrator | 9,400 lines. API and rules in `crates/yaac-protocol/README.md`. |
| WP2 | **done** | **yes** — typecheck, 42 tests, bundle build, and the version-ban test re-run by the orchestrator | 192 KB single-file bundle, no runtime dependencies. Fake provider directives in `daemon/src/providers/fake/README.md`. Runs Node with `--no-turbofan` because Node 22.22.3 crashed intermittently in its optimising compiler on this machine — to investigate before release. |
| WP3 | **done** | **yes** — build, 26 tests including one against the real daemon bundle, clippy `-D warnings`, re-run by the orchestrator | Mesh relay built (store-and-forward, dedupe, receipts, roster, query routing). Later added: `mesh/usage` forwarding, the 15-minute undelivered notice, paged thread search. |
| WP4a | **done** | **yes** — 52 tests and clippy `-D warnings` re-run by the orchestrator; no provider-kind branches and no model names outside the demo module; screenshots inspected | Stripped release binary 8.4 MiB (budget 20). Idle CPU 0. Idle memory measured only under software rendering (123 MB) — to re-measure on a GPU driver. Runs on scripted demo data; not yet attached to the real hub. Real screenshots in `docs/design/screenshots/`. Twelve protocol gaps reported, listed in the worker's notes below. |
| WP4b | **done** | **yes** — 79 tests (including 4 that start the real `yaac hub` in a temporary profile and attach to it) and clippy `-D warnings` re-run by the orchestrator; no provider-kind branches, no model names outside the demo; screenshots inspected | `yaac hub` runs headless; the window attaches to it or starts it; settings, first run, fork, create project, the bypass confirmation, the narrow layout. Stripped release binary 11.5 MiB (budget 20). `yaac hub` idle: 6 MB, no graphics libraries loaded. 28 screens in `docs/design/screenshots/`. Remaining seams: the real SSH backend (needs `hub/ssh/*`), SSH setup records, the outside-tools switch, the WSL master switch and distro list, "found running / started by YAAC" for Codex, a connection-step log, unarchive. |
| WP5 | **done** | **yes** — 21 Codex tests (replay, under the session layer, and live against the real daemon) re-run by the orchestrator; only `daemon start` is ever invoked; no model names in source; fixtures use placeholder paths and model ids | Registered in the daemon. One real turn and one fork were run by hand to record fixtures, then deleted. Modes: read-only, auto, full access (flagged `skipsApprovals`). |
| WP6 | **done** | **yes** — 16 unit tests and 1 live test re-run by the orchestrator; no model names in source; fixture free of personal data | Registered in the daemon. Offers the bypass-permissions mode flagged `skipsApprovals`. Bundles into one file with no build change; the SDK adds about 2 MB to `yaacd` (estimate until built). One real turn was run by hand to record a fixture, then deleted. An idle Claude thread costs about 229 MB on the target. |
| WP7 | not started | — | |
| WP8 | not started | — | |
| WP9 | not started | — | |

## Follow-up: protocol and daemon additions — done

**Done and verified** (114 Rust tests, 93 daemon tests, bindings and the frozen baseline check,
all re-run by the orchestrator). The exact names are in [wire-additions.md](wire-additions.md).
Also in this pass: uncertain control is treated as free (D72); a thread keeps one attachment
folder for life; `ProviderContext` carries the daemon's environment; the hub forwards
`mesh/usage`, sends the 15-minute undelivered notice, and answers `hub/threads/find`. The
original list follows for the record.

### The original list

Owner decisions made after the protocol crate and daemon were finished, plus gaps the window
worker found. To be done as one pass once the provider workers land:

- `ThreadSummary`: `muted`, `pinned`, git status (`branch`, `added`, `modified`, `deleted`,
  `ahead`, `behind`), and thread states for *waiting* and *done*.
- User-created projects: registry on the target, create/rename/delete methods, the matching
  rule (deepest folder; repository rule first), temporary projects.
- A per-chat agents policy (ask / allow) and a permission request variant carrying the budget.
- Item timestamps (for "Thought for 4s"); a file name on input parts; context usage in the
  subscribe result; the window's own client id in the hub's hello; `thread/fork` without a
  first message; `enabled` in target-state notifications.
- `item/read` on the wire; `mesh/usage` and budget forwarding in the hub.

## Follow-up: after the mesh lands

- **Discovery caps (D78, N39):** apply the age and number caps in `yaacd`'s thread index while
  scanning (both providers list newest first — stop early), with the never-cut rules; on-demand
  lookup by id at the harness; "Show older" / "Search older" paging from a target; the
  per-target settings; the two sidebar affordances.
- Unarchive on the wire; the Codex "found running / started by YAAC" fact in the descriptor;
  a connection-step log for diagnostics; `hub/ssh/*` and the real SSH backend; WSL distro
  listing; the outside-tools switch.
