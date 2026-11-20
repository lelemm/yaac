# 08 — Roadmap

The order is chosen to retire the riskiest unknowns first and to have something usable on screen
as early as possible.

## Spikes — before committing to the design

Each spike is a throwaway experiment with a yes/no answer and a number. None is product code.

| # | Question | Done when |
| --- | --- | --- |
| **S1** | Does egui meet the footprint budget, and with which backend? | A window with a 5 000-row virtualised list and a markdown stream at model speed, including one 20 MB item, a resize during streaming, several open threads, high DPI, and thumbnail churn. Binary size, idle memory of the whole process tree, and frame time recorded for `glow` and `wgpu` on Windows and Linux. |
| **S2** | Is the stream binary-clean through `ssh -T` and through `wsl.exe --exec`, and does control traffic stay responsive under load? | 100 MB of random bytes round-trip unchanged in both directions on each. With an upload and a history download running on a throttled link, 95th-percentile latency of a Stop and of an approval answer is recorded against the budget. |
| **S3** | Can the Claude Agent SDK and a Codex app-server client be bundled into one `yaacd` file that uses the target's own `codex` and `claude`, and run on a private runtime we ship? Does stage 0 work? | One bundled file runs a turn against each harness on the candidate runtimes (glibc, musl, Windows), with artefact sizes recorded. Stage 0 installs runtime and daemon over SSH on `dash`, `bash`, and `busybox` — with a noisy login script, a non-login `PATH`, an interrupted install, and two simultaneous installs. Both the target-download and the host-upload path are exercised. |
| **S4** | Can we read a *file list* from the clipboard? | Ctrl+V of files copied in Explorer, and in a Linux file manager under X11 and Wayland, yields paths. |
| **S5** | Does the Codex app-server behave as its schema says, over the daemon's socket? | WebSocket over the Unix socket; `initialize` + `initialized`; `model/list` across pages; `thread/list` with explicit source kinds; a streamed turn with an approval and a `requestUserInput` question; `thread/fork`; starting the daemon when absent; the oldest Codex version that has the daemon identified, as the minimum YAAC supports. Run against a daemon newer than the installed CLI. |
| **S6** | What does a Claude thread cost on the target, and how do questions and effort behave? | Memory per idle and active `query()`; resume latency after eviction; an `AskUserQuestion` round-trip through `canUseTool`; whether `applyFlagSettings({ effortLevel })` takes effect on a live query. |
| **S7** | Can we reliably tell that a thread is in use elsewhere, and still watch it? | For each harness: a session open in a terminal, idle and mid-turn, is detected within a few seconds and released when it ends; a second client on the same Codex daemon is detected; the view updates while read-only. False "free" results: zero. |
| **S8** | Does `yaacd` really outlive its connection on every platform? | With a turn running: the UI exits, the SSH session drops, the laptop sleeps, the Windows user stays logged in, the WSL distro has no attached session. For each, whether `yaacd` and the turn survive. For WSL: exactly when a distro is shut down, and that its state can be read without starting it. |
| **S9** | Can the SSH setup flow be driven entirely through the system `ssh`, with no terminal? | On Windows and Linux: host-key confirmation, a password login, and a passphrase-protected key file all answered through askpass; a YAAC key installed and verified; the key removed again; a server that refuses key logins detected and cleaned up. |
| **S10** | Can each harness be given the `yaac` MCP server per thread, and what does it cost? | Codex honours an MCP server set in per-thread configuration and an identity block in `developerInstructions`; Claude takes an in-process server. A Codex thread calls a tool and `yaacd` knows which thread called. Memory per bridge process recorded; whether one shared bridge can identify its caller. An agent, asked in plain words, messages another thread without being told how. |

A failed spike changes the design before any code depends on it. S2, S3, S7, and S8 are the ones
that could force real changes.

## Milestones

### M1 — Walking skeleton (local, Codex)

**The hub and the window as two modes of one binary**, attached over a local socket.
Workspace, protocol crate with generated TypeScript, frame codec with control-before-bulk
scheduling, `initialize` with version negotiation, and the **dialect layer** in both languages
(one dialect so far, but nothing outside it can see a version). Local target only. `yaacd` with the Codex
daemon backend and the fake provider. One window: start a thread, send a message, watch
markdown stream, stop it.

*Proves:* the two halves talk, end to end, with real types.

### M2 — Second provider, real capabilities

Claude provider on the SDK backend. Effective capabilities computed in `yaacd` and driving the
controls under the composer: model, effort, mode. Models from the target, cached with
revalidation. **Approval and question cards**, with every request state. Model switching inside
a thread.

*Proves:* the contract is provider-neutral. If adding Claude forces a UI branch on provider
kind, the contract is wrong — fix it here.

### M3 — Threads at scale

Thread index and incremental sync. Sidebar with three-level grouping and toggles. SQLite cache;
startup from cache; durable draft store. Thread history paging. Search. **In-use-elsewhere
detection and the read-only view.**

*Proves:* instant startup, the grouping model, and the single-writer rule.

### M4 — Remote targets and persistence

SSH and WSL connectors. Stage-0 bootstrap with on-demand runtime and in-band daemon install.
Persistent daemon with `yaacd connect`, one serving daemon per user, and hand-over at idle
("Update now" included). WSL distro-down notification.
On-disk journal; reconnect with lossless replay; operation ids with `outcome_unknown`.
Settings: targets. The SSH setup flow — password-less probe, password or key-file login, key
install and verify, Revoke — and WSL connections with an optional user.

*Proves:* the single-connection rule in the environments it was written for, and that nothing
is lost while the host is away — including a question asked while disconnected.

### M5 — Attachments and fork

Clipboard and drag-and-drop. Upload protocol with the content-addressed store. Image routing by
effective support, including the `Unknown` case. Native fork and handoff fork, including from
threads created outside YAAC. Provider-instance management in settings, including several Codex
daemons on one target and store-identity checks.

### M5b — The agent mesh

This is the point of the product; it comes last only because it stands on everything above.
Its foundations are laid earlier — the hub in M1, relationships in M3, the journal in M4.

The `yaac` MCP server in `yaacd` and its attachment to both harnesses. Mailbox and outbox with
hop-by-hop acknowledgement. Same-target delivery; cross-target delivery through the hub's relay
queue. The delivery rules for every recipient state. Spawning with parent and child records.
`wait` and `notify_when_done`. Limits and the mesh policy. Mesh items in the transcript and the
agents panel.

*Proves:* a Codex thread, asked in plain words, starts Claude workers on another target, waits
for them, and tells a third thread it is done — with the window closed for part of it.

### M6 — Hardening

Error surfaces for every provider status, including a Codex too old to have the daemon. Eviction of idle harness
processes. Diagnostics view. Accessibility pass. Footprint pass against the budget. Packaging
for Windows and Linux.

*Exit:* the MVP scope in [01 Overview](01-overview.md#mvp-scope) is met and the budgets hold.

## Testing strategy

| Layer | How it is tested |
| --- | --- |
| Frame codec and protocol types | Unit and property tests in Rust: round-trip, split and coalesced reads, oversized frames, garbage input. A golden-file test asserts the Rust and TypeScript encodings of every message agree. |
| **Compatibility** | Schema snapshots per supported version, with a CI diff that fails on an incompatible change without a bump. Recorded conversations per version replayed against each side's dialect. The current UI protocol layer run end to end against the **real released `yaacd`** of each supported version, and old recorded clients against the current daemon. Negotiation: overlap, no overlap in each direction, hand-over at idle and "now". A build check that no code outside the dialect module can read the negotiated version. ([09](09-compatibility.md#tests)) |
| Scheduling | A control message is never delayed by more than one bulk chunk, in either direction, under a saturated upload and a large history page. |
| `yaacd` session layer | Against the **fake provider**: a full turn with no client attached is replayed intact; replay across a compacted range; reset when the position predates the journal or the epoch changed; no gap and no duplicate between `thread/read` and live events; journal survives a daemon restart; backpressure collapse with `throughSeq`; a retried `opId` acts once; a crash between send and record yields `outcome_unknown`; upload hash mismatch. |
| **Requests** | A question and an approval each asked while no client is attached are delivered on reconnect and answerable; resolved-elsewhere, expired, and unknown each render correctly; a secret answer is never written to the host cache. |
| **Agent mesh** | With fake providers on two fake targets and a real hub: a message to an idle thread starts a turn; to a waiting thread it returns from `wait`; to a busy thread it queues; to a thread in use elsewhere it queues and says so; to an offline target it waits in the hub and arrives on reconnect; with the hub stopped, a same-target message still arrives and a cross-target one waits in the outbox. A duplicated message is delivered once. The sender cannot be forged. Each limit trips with a readable reason. A message never resolves a pending request. A worker's result returns to its parent; `notify_when_done` fires even when the turn is interrupted. |
| **Thread control** | With a thread in use elsewhere, `turn/start`, `turn/steer`, and `request/respond` are rejected; the view keeps updating; control returns when the other party finishes. |
| Provider adapters | Recorded harness event streams replayed into each adapter, asserting the normalised items — including each harness's question format. Recordings are refreshed when a harness version changes behaviour. |
| Capability rules | Table-driven: for a given instance, model, and thread state, the effective capabilities; which composer controls exist; how an image is routed for supported, unsupported, and unknown. |
| Thread identity | Two instances sharing a store list a thread once; two with separate stores never merge; an endpoint edit that changes the store does not re-point threads. |
| Transports | An integration suite that runs the bootstrap against a local `sshd` container (glibc and musl) and, on Windows CI, a WSL distro — as the default user and as a named user. |
| **SSH setup and revoke** | Against `sshd` containers: access already in place (Revoke disabled); password login then key install; key-file login then key install; wrong password; key logins refused by the server (the added line is removed again); an existing `authorized_keys` is left byte-for-byte intact apart from the one line; Revoke removes exactly that line; Revoke with the server unreachable keeps the local key. The password never appears in settings, cache, or logs. |
| UI state | The application state is a plain data structure updated by events; it is tested without a window. Rendering is checked with egui's headless test harness for the layouts that matter (sidebar grouping, composer controls, request cards, read-only banner). |
| Footprint | A benchmark that loads a synthetic cache of 5 000 threads and plays a recorded stream, failing CI when binary size, idle memory, or frame time exceeds the budget. |

The fake provider is the most valuable piece of test infrastructure: it lets the UI, the
protocol, and the daemon be developed and tested with no harness installed and no tokens spent.
It can ask questions, request approvals, stall, fail, and pretend to be in use elsewhere. It is
built in M1.
