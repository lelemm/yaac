# Decision log

The single place that says what is settled. Before changing a design choice, find it here.

- **Decided** — settled by the project owner. Change only by adding a new entry that supersedes it.
- **Proposed** — the current recommendation; the docs are written as if it holds. Not yet confirmed.
- **Open** — needs an answer from the project owner.

## Decided

All dated entries are from 2026-11-20.

| # | Decision | Source |
| --- | --- | --- |
| D1 | The host app is Rust with egui, GPU-accelerated, optimised for low binary and memory footprint. | Brief |
| D2 | The target side is one TypeScript daemon, `yaacd`, so both harness SDKs can be imported in one process. | Brief |
| D3 | Exactly one connection from host to each target. Everything is multiplexed over it. | Brief |
| D4 | Targets are local (Windows or Linux), SSH (many), and WSL (many). WSL is off by default. SSH connections can be added, removed, edited, enabled, and disabled. | Brief |
| D5 | A chat is a **thread**. A thread maps one-to-one to a provider conversation. Models may change within a thread; the provider may not. | Brief |
| D6 | **Fork** creates a new thread from an existing one, with a newly chosen provider and model. | Brief |
| D7 | Each provider prefers its harness daemon and falls back to its SDK. Codex and Claude are the first two; more will follow. *(For Codex, superseded by D31: daemon only. For Claude, see D18.)* | Brief |
| D8 | The provider contract in Rust carries an array of capabilities. Image reading and reasoning are among them. | Brief |
| D9 | Images are routed by capability: sent as image input when the model can read images, never treated as an ordinary attachment in that case. | Brief |
| D10 | Pasted files are uploaded to a folder on the target, and the message references them there. | Brief |
| D11 | The sidebar groups threads origin → project → worktree. Each level can be toggled at any time. | Brief |
| D12 | A local cache on the host makes startup independent of any connection. | Brief |
| D13 | A target may have several provider instances of the same kind (for example several Codex daemons on different sockets). An instance has an optional name and a configurable socket location, both editable in the UI. | Owner |
| D14 | No hardcoded model lists. Models come from the target. The host may cache them for a fast start, but the cache is invalidated and refreshed from the target. | Owner |
| D15 | The protocol version is exchanged in the connection handshake, not repeated on every packet. *(The original "must match on both sides" is superseded by D32: the two sides negotiate, and the newer adapts.)* | Owner |
| D16 | A running turn survives the UI closing or the connection dropping. `yaacd` is persistent and keeps collecting everything the host needs, delivering it when the host reconnects. | Owner |
| D17 | `yaacd` is self-contained: a target needs no pre-installed JavaScript runtime. | Owner |
| D18 | Claude runs on the Agent SDK only in the MVP. The daemon backend stays an empty slot in the contract. | Owner |
| D19 | The sidebar shows every conversation the harness knows about; automated ones are hidden by default behind a filter. | Owner |
| D20 | Codex backend order: the running daemon → **start the daemon**. *(The original third step, the SDK, is removed by D31.)* | Owner |
| D21 | The UI ⇄ `yaacd` protocol is YAAC's own. ACP comes in later as a provider. | Owner |
| D22 | A fork to a different provider carries the conversation as a **transcript file**. | Owner |
| D23 | The composer styles markdown **as you type**, with the syntax left visible and editable. | Owner |
| D24 | SSH uses the **system OpenSSH client**. | Owner |
| D25 | The private runtime is **fetched on demand**; the installer ships only the host's own platform. | Owner |
| D26 | A thread that is **in use elsewhere** is shown live but read-only: the composer is disabled and the user cannot interact, only watch. | Owner |
| D27 | SSH targets in the MVP are Linux and other POSIX machines. Windows is supported locally and through WSL, not as an SSH target. | Owner |
| D28 | Asking the user for information is a **provider capability**. Both harnesses' question mechanisms are supported, and a question is never lost. | Owner |
| D29 | An SSH connection is set up through a guided flow: host, port (and user) → **Setup** tries password-less login → if that fails, ask for a password or a key file → **Connect** logs in and installs password-less access for that user from this machine. **Revoke** removes it, and is disabled when password-less access was already in place. | Owner |
| D30 | A WSL connection has an **optional user**, since a distro can have several. | Owner |
| D31 | **No Codex SDK fallback.** Codex is driven through its daemon only; YAAC requires a Codex version that has it. | Owner |
| D32 | **Backward compatibility.** A newer UI never replaces a running older daemon: it keeps working against it, warns that the target is on an older version and will be updated when idle, and offers to take over immediately. Compatibility must not be a copy of the protocol per version, nor `if version == …` checks; it is object-oriented, with a method split into steps and only the changed step overridden. | Owner |
| D33 | Protocol dialects inherit **newest-as-base**: the current protocol is one complete class, and each older version is a small subclass holding only what differed. | Owner |
| D34 | The compatibility window is the **last 3 protocol versions**. | Owner |
| D35 | **No WSL keep-alive.** When a distro is down, the host notifies the user and offers to start it or to disable its threads. | Owner |
| D36 | A conversation that is open elsewhere but idle still counts as **in use**, and is read-only in YAAC. | Owner |
| D37 | Pasted files live in the target's **system temp folder**. | Owner |
| D38 | A new thread starts in the **harness's own default** permission mode. | Owner |
| D39 | In SSH setup, a key file (PEM) is **used once**; YAAC then installs its own dedicated key. | Owner |
| D40 | The SSH key YAAC generates is stored as a **user-only file**. | Owner |
| D41 | macOS as a host is **best effort**: kept building, not packaged or tested. | Owner |
| D42 | Claude Code has **one instance per target** in the MVP. | Owner |
| D43 | **The main purpose.** Harnesses **know** they can communicate with each other; `yaacd` provides the bridge as an **MCP server** offered to each harness. | Owner |
| D44 | Asked of a Codex thread, "send a message to the YAAC thread running on Claude" works. | Owner |
| D45 | A thread can spawn threads on **any provider and model** — for example Opus workers from a Codex thread. | Owner |
| D46 | A thread can message a thread on **another target** — for example to tell a waiting worker on another server to resume. | Owner |
| D47 | An orchestrator on one machine can orchestrate threads on other targets. | Owner |
| D48 | `yaacd` delivers messages between threads on its own target; for other targets **the host is the bridge**. | Owner |
| D49 | The thread view has a sidebar showing the **sub-threads** of that agent; the chat shows messages sent to another worker (naming the destination) and messages received from another worker. | Owner |
| D50 | The host has its own **daemon**, so communication works without the UI. Later the bridge can move to a server, or a server can act as one. | Owner |
| D51 | Whether a chat's agent may work with other agents is a **toggle in that chat's composer**: *allow without asking*, or *ask for permission*. A new chat starts on **ask**. The permission prompt is where the **budget** is set. | Owner |
| D52 | A message from another agent **wakes an idle thread**: it starts a turn on its own. | Owner |
| D53 | The hub runs **always, started at login**. | Owner |
| D54 | The sidebar has a **Needs you** list. The other additions from the design review are **dropped**: the "Since you left" page, the warning marks that climb to parent threads, and the Agents tab in settings. | Owner |
| D55 | **Mute**: a thread can be muted (right-click) to take it out of the main list while it keeps working. A muted thread that becomes blocked on the user appears under *Needs you* until answered. | Owner |
| D56 | **Archive**: a thread can be archived — off the list entirely, kept for later. | Owner |
| D57 | **The sidebar is a flat list under sticky section headers, not a tree.** The tree felt polluted. | Owner |
| D58 | **Controls are not outlined.** A border around every control adds density for no reason. | Owner |
| D60 | **The agents panel has three states**: a floating vertical pill of harness icons with up and down arrows while data is being sent or received; on hover, a floating card adding each agent's name and its last action; and docked to the side as a panel, in the muted style. | Owner |
| D61 | **Pinned**: a thread can be pinned; the sidebar has a *Pinned* section. | Owner |
| D62 | **Projects are created by the user.** A project is a folder the user selects, with a name the user gives it — it does not depend on the folder's name. | Owner |
| D63 | A discovered thread whose folder is in no known project is shown under a **temporary project** named after its folder, marked so it is clear the project does not exist yet. Right-clicking it offers to create the project, through the normal create-project flow with the folder prefilled and editable (for example trimmed up to a parent). | Owner |
| D64 | A thread in a folder **inside** a project belongs to that project. Where projects are nested, the **deepest** project containing the thread's folder wins: with projects at `~` and `~/projects/abc`, a thread in `abc/123` belongs to *abc*, and one in `~/projects` belongs to *~*. | Owner |
| D65 | A thread created from YAAC starts in the **root of its project's folder**. | Owner |
| D66 | **Sidebar rows are more informative: more than one line**, with room for the **git branch** and, very compactly, **how many files differ from origin** for that branch. | Owner |
| D68 | The git changes in a sidebar row use **real icons, not Unicode characters**, and the **usual git colours** (green, red, …). | Owner |
| D95 | Attachments can be added by an **attachment button that opens the file picker**, by **dragging and dropping files onto the chat**, and by **pasting files or clipboard images**. | Owner |
| D94 | **The sidebar is resizable and collapses into a vertical strip** showing the YAAC title, a + button, the history buttons, each project's name written vertically, and each thread's state icon; hovering an icon shows the full row; the mouse wheel scrolls it. | Owner |
| D93 | **Archiving a thread removes its pin** (and its mute), and the sidebar has a collapsed **Archived** section under *Muted*. | Owner |
| D92 | **Mute, pin, and dismiss are view preferences of the host, not facts about the thread.** The hub stores them on the user's computer, alongside drafts and window state; the daemon never hears of them, and a timed mute needs no timer — it is "hidden until this moment", checked when drawn. A second computer does not share them. *(Supersedes the "stored on the target" part of N32, N35, N40, and the daemon side of D90.)* | Owner |
| D91 | **Times are always shown and entered in the local time zone of the UI host**, never the target's. | Owner |
| D90 | **Mute opens a dialog** where it can be made timed: unlimited, a duration in minutes / hours / days (shown as the date it converts to), or a date picker with an editable time of day. | Owner |
| D89 | **The composer is two lines tall by default**, Shift+Enter breaks a line, it grows with its text **up to five lines**, and beyond that it scrolls with a scrollbar. | Owner |
| D88 | The back and forward buttons each have a **dropdown of history entries**, to move several steps at once. | Owner |
| D87 | **Navigation history**: back and forward buttons beside the YAAC title in the sidebar, working like a browser's — following a link to another chat and pressing back returns to the previous one, and forward is then enabled. The mouse's back and forward buttons work too. | Owner |
| D86 | **File changes appear as diffs**: one collapsed line with added/removed counts that expands in place to a unified diff, and a per-turn summary line. | Owner |
| D85 | **Code blocks have a copy button.** | Owner |
| D84 | **Text is selectable and copyable everywhere**, including a selection that spans several pieces of a chat. | Owner |
| D83 | **No blue accent — no accent hue at all.** Selection is a neutral surface, the primary button is solid near-white / near-black, links are uncoloured and slightly bolder, and *running* is drawn without colour. Colour is kept for needs-you, failed, done, and git changes. | Owner |
| D82 | The agents pill's hover card **must not flicker** as the pointer moves. | Owner (bug report) |
| D81 | A message to a worker can be **queued**, or **force-sent**: on Claude by interrupting the worker, on Codex by steering it. This is available **from the mesh tools and from the chat composer**. | Owner |
| D80 | An orchestrator can **stop a worker** through the mesh tools. The improvements proposed for it are approved: the whole subtree, stop-and-hold, a real outcome, and `stop_all`. | Owner |
| D79 | A discovered thread can be **dismissed**: it is simply hidden in the interface. | Owner |
| D78 | **Thread discovery has a hard cap on quantity and on age**, so a long-used harness does not flood the UI. | Owner |
| D77 | The mesh block added to each harness's instructions is **appended after the harness's own system prompt, which is kept intact**, and its text is **fixed** — there is no setting for the user's own instructions. | Owner |
| D76 | Mesh listings and searches are **paged**: a small page per call, with a cursor for the next. | Owner |
| D75 | The mesh has a **search by thread id**, and a **fuzzy finder by title**, both **across all targets**. | Owner |
| D74 | **The mesh tools never list every running agent.** With a fleet of ten targets that would be far too much; listings are scoped and bounded. | Owner |
| D73 | **If the hub is unavailable for any reason, threads on the same target can still communicate** — their daemon is local. | Owner |
| D72 | **When it cannot be told whether a thread is in use elsewhere, YAAC takes it over.** Only positive evidence makes a thread read-only. A terminal that has merely opened a thread YAAC holds going undetected until it acts is acceptable. *(Supersedes the "when unsure, treat as in use" half of P14, and limits D36 to harnesses that can report an idle holder.)* | Owner |
| D71 | A mode that **bypasses all permission checks is offered, behind a confirmation dialog**. | Owner |
| D70 | **Every icon in the app comes from one embedded icon set**, never from Unicode symbols. The **git colours are softer than the state colours**, because green, yellow, and red already mean done, needs-you, and failed for an agent's state. | Owner (agreed) |
| D69 | **Links are the accent colour with no underline.** Blue underlined links are ugly. | Owner |
| D67 | The hub stores its cache, drafts, and relay queue in **SQLite, compiled into the binary** (confirmed after the reasons were explained; was P17). | Owner |
| D59 | **In a chat, what the user and the agent say to each other stands out most.** Tool calls, conversation between workers, and the like are muted and collapsed by default into a short description. | Owner |

## Proposed

How the decisions above are carried out, plus choices nobody has been asked about yet. The docs
assume these.

| # | Proposal | Reasoning | Doc |
| --- | --- | --- | --- |
| P1 | D16 is a per-user daemon (`yaacd serve`) plus a stdio bridge (`yaacd connect`), with an on-disk per-thread **journal** — positions are `(epoch, seq)`; compacted and trimmed. | Lossless across disconnects and daemon restarts, with bounded disk use. | [02](02-architecture.md#daemon-lifecycle), [03](03-protocol.md#journal-and-replay) |
| P2 | At most **one serving `yaacd` per user per target**, and daemons only move forward: when none is serving, the newest installed build starts, and it adapts to an older UI. Hand-over is requested through `daemon/handover` (idle or now). Protocol version, build, and state format are versioned separately; state migrates forward only. | Carries out D32 without two daemons owning the same journal and sessions. | [02](02-architecture.md#one-serving-daemon-per-user), [09](09-compatibility.md#upgrading-a-daemon-without-interrupting-it) |
| P3 | `yaacd` is installed in-band by a minimal stage-0 shell handshake that hands over to the runtime as early as possible; installs are atomic and locked. | Keeps the single-connection rule during install; survives noisy shells and interrupted installs. | [02](02-architecture.md#stage-0) |
| P4 | D17 and D25 as two artefacts: a private pinned runtime (once per target) and a small daemon bundle (each release), installed alongside older builds rather than replacing them. The target downloads the runtime itself against a pinned checksum; otherwise the host fetches it with the system `curl` and uploads it in-band. A system Node is never used. | Small updates; every target runs the runtime we tested; no HTTP stack in the host binary. | [02](02-architecture.md#bootstrap) |
| P5 | Approvals and questions are thread **items** with explicit states (pending, submitted, resolved, expired, unknown), answered with `request/respond`. | Carries out D28: they survive reconnects, show in the sidebar, and can be resolved elsewhere without confusion. | [03](03-protocol.md#requests-approvals-and-questions) |
| P6 | One stream, two priorities: control traffic goes ahead of bulk in both directions; small adaptive chunks; credit-based flow control; large things paged by bytes. | D3 on a slow link would otherwise delay Stop and approvals by seconds. | [03](03-protocol.md#scheduling-and-flow-control) |
| P7 | Every state-changing request carries an operation id recorded on the target; an uncertain result is `outcome_unknown`, never an automatic resend. | A dropped connection must not duplicate a thread or a paid turn. | [03](03-protocol.md#idempotency) |
| P8 | Creating a thread and sending its first message are one operation (`thread/start`). | Some backends assign the id only when the first turn starts; drafts leave no trace. | [03](03-protocol.md#threads) |
| P9 | Rust types are the protocol's source of truth; TypeScript is generated. | One definition, checked in CI. | [03](03-protocol.md#design-rules) |
| P10 | Provider-instance configuration is stored on the target, and each instance records a **store identity**. Threads are bound immutably to an instance; an endpoint edit that changes the store does not re-point them. | D13 with shared or separate Codex homes would otherwise merge or misroute conversations. | [04](04-providers.md#an-instance-has-a-store-identity), [05](05-threads.md#binding-a-thread-to-an-instance) |
| P11 | The thread key is the harness's own conversation id — for Codex `Thread.id`, not `sessionId`. | `sessionId` is shared across a session tree. | [05](05-threads.md#identity) |
| P12 | **Effective capabilities** of a thread are computed by `yaacd`, with model abilities as Supported / Unsupported / Unknown, and the UI shows what the harness actually applied. | Three of the four inputs exist only on the target; "unknown" must not be treated as "yes". | [04](04-providers.md#effective-capabilities-are-computed-on-the-target) |
| P13 | `yaacd` starts a Codex daemon (D20) but never stops, restarts, or updates one. An instance with an explicit socket is connect-only. A Codex too old to have the daemon is reported as such. | Those actions can interrupt work that is not YAAC's. | [04](04-providers.md#backend-selection) |
| P14 | "In use elsewhere" (D26) also applies between two YAAC clients, per turn: whoever started the running turn holds the thread until it ends. When detection is unsure, the thread is treated as in use. | One consistent rule; a wrongly enabled composer can corrupt a conversation. | [05](05-threads.md#in-use-elsewhere) |
| P15 | Fork stays on the same target, starts from the last completed turn, is native only within one conversation store, and a handoff shows what will cross to the other provider and asks for confirmation. | The working directory must exist; a handoff moves content to another vendor. | [05](05-threads.md#fork) |
| P16 | *(Superseded by D62–D65: projects are defined by the user, not inferred from git.)* Git is still used for one thing — putting a thread that runs in another **worktree** of a project's repository under that project. | A worktree usually lives outside the project's folder, so the folder rule alone would lose it. | [05](05-threads.md#projects) |
| P17 | The cache is one SQLite file, dropped and rebuilt on schema change. **Drafts are separate, durable, and migrated.** | The cache is disposable; a half-written message is not. | [05](05-threads.md#local-storage-on-the-host) |
| P18 | Within the temp folder (D37): a user-only directory `yaacd` creates and verifies, blobs by full digest, per-thread links, copied even on the local target, readable to the harness by a narrow explicit grant. Attachments that have been cleaned away show as *no longer on the target*. Fork handoff transcripts live in `~/.yaac/` instead, because they must survive a reboot. | Temp is shared and volatile; the design has to be safe on a multi-user machine and honest when files vanish. | [06](06-attachments.md#the-upload-folder) |
| P19 | The egui backend (`glow` or `wgpu`) is picked by measurement in spike S1. | Both are GPU-accelerated; memory differs. | [02](02-architecture.md#footprint-budget) |
| P20 | Target paths are opaque values interpreted only on the target. | Windows, WSL, and remote paths do not share rules. | [02](02-architecture.md#paths) |
| P21 | D29 installs a **dedicated key YAAC generates per connection**, never the user's own keys (D39, D40); the password is held in memory once and never stored; a final key-only login proves the setup, and a failed proof removes the line again. Revoke matches the public key itself and keeps the local key if the server is unreachable. | Revoke then has exactly one thing to remove, and the connection never depends on a file the user might move. | [02](02-architecture.md#setting-up-an-ssh-connection) |
| P23 | The compatibility design in full: a **frozen core** (framing, `initialize` with a version range, the error shape, `daemon/status`, `daemon/handover`); the negotiated version private to the dialect module; in Rust a dialect is a table of functions built with struct-update syntax, in TypeScript a class chain; a fixed seven-step procedure for an incompatible change; schema snapshots, per-version fixtures, and CI runs against the real released daemons of each supported version. | D32 asked for this to be very well designed; these are the parts that keep it from rotting. | [09](09-compatibility.md) |
| P24 | WSL (D35): the distro's state is read with `wsl.exe -l -v`, which starts nothing; YAAC reconnects on its own only when the distro is already running. | Running `wsl.exe -d` would boot the distro behind the user's back. | [02](02-architecture.md#wsl) |
| P25 | The agent mesh in full ([10](10-agent-mesh.md)) — tools, addressing, delivery rules, receipts, hop-by-hop durability, limits, policy, the hub. Each choice the owner has not seen is itemised in [pending.md](pending.md). | Carries out D43–D50. | [10](10-agent-mesh.md) |
| P22 | A **target is a user on a machine**. The same host or distro with a different user is a separate target with its own connection and `yaacd`. Setup and Revoke sessions are short, sequential, and never open alongside a target's live connection. | D30 and D29 without bending D3. | [02](02-architecture.md#what-a-target-is) |

## Open

Nothing currently needs an answer from the project owner before work can continue.

Two choices are deliberately left to measurement rather than opinion:

- **Which private runtime** — Node.js or Bun — by spike S3 (size, memory, musl support, SDK
  compatibility).
- **Which egui backend** — `glow` or `wgpu` — by spike S1.

Choices made without the owner while work continued are listed in
[pending.md](pending.md), each with the option taken, for review.

## Resolved questions

Kept so earlier references still make sense.

| Was | Question | Outcome |
| --- | --- | --- |
| Q1 | Should a turn survive the UI closing? | D16 |
| Q2 | How do targets get a JavaScript runtime? | D17, D25 |
| Q3 | What does "use the Claude daemon" mean? | D18 |
| Q4 | Which sessions appear in the sidebar? | D19 |
| Q5 | How does a cross-provider fork carry the conversation? | D22 |
| Q6 | What does "render markdown in the composer" mean? | D23 |
| Q7 | System OpenSSH or an embedded library? | D24 |
| Q8 | Should the wire protocol be ACP? | D21 |
| Q9 | Codex fallback when no daemon runs? | D20 |
| Q10 | Two UIs on one daemon; other small items | D26 with P14; rest in O5 |
| Q11 | How is the private runtime delivered? | D25 |
| O1 | History on a backend that cannot read it back? | Moot: D31 removes the Codex SDK fallback |
| O2 | A newer YAAC meets a busy older daemon? | D32, D33, D34 |
| O3 | WSL keep-alive? | D35 |
| O4 | Is an idle-but-open session "in use"? | D36 |
| O5 | Upload location; default mode; macOS; Claude instances; SSH key storage; key file handling | D37, D38, D41, D42, D40, D39 |

## Design review

The first draft was reviewed by Codex on 2026-11-20; the full text is in
[research/codex-review-2026-11-20.md](research/codex-review-2026-11-20.md). Its factual claims
were re-tested before use.

**Adopted** into the docs: the Codex conversation key (P11); store identity and immutable
bindings (P10); request states and "resolved elsewhere" (P5); turn ids on interrupt and steer;
journal epochs, gap-free reset, and atomic client commit (P1); operation ids and
`outcome_unknown` (P7); one serving daemon and hand-over (P2); control-before-bulk scheduling
(P6); stage-0 hardening (P3); effective capabilities with an explicit unknown (P12); the
upload-store redesign (P18); fork by source/destination pair with a preview (P15); opaque
target paths (P20); durable drafts (P17); bounded rendering of giant items and a worker pool;
the diagnostics method; accessibility and input-method handling; extra spikes S7 and S8.

**Corrected** because the draft was wrong: the Codex control socket speaks WebSocket, not JSON
lines; `sessionId` is not a conversation id; the daemon can be newer than the CLI; the Codex SDK
only learns a thread's id after its first turn starts.

**Not adopted:** requiring Node.js on targets (superseded by D17); starting a private Codex
app-server instead of the shared daemon (superseded by D20); replacing an older daemon with
an exact-match protocol check (superseded by D32).
