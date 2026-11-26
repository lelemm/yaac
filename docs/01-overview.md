# 01 — Overview

## What YAAC is

YAAC (**Y**et **A**nother **A**CP **C**ontroller) is a desktop chat application that wraps
coding-agent harnesses. It does not implement an agent. It gives you one window to start, watch,
steer, and organise agent conversations that run wherever your code lives: on the machine in
front of you, on a server over SSH, or inside a WSL distro.

## Why it exists

**So that agents can work together across harnesses and across machines.** A Codex thread can
message a Claude thread. A thread can start workers on another provider and model. A worker on
one server can tell a thread on another that it is done. One orchestrator can run threads on
many machines. And each agent *knows* it can, because YAAC gives every thread the tools and
tells it so. This is the [agent mesh](10-agent-mesh.md); the rest of YAAC is what makes it
dependable.

## Goals

0. **Agents that can reach each other.** Messaging, spawning, and waiting, between any two
   threads YAAC can see, on any provider, on any target, with or without the window open.
1. **Fast and small.** GPU-accelerated UI in Rust with egui. Low binary size, low idle memory,
   no CPU use while idle, instant startup from a local cache. Budgets are in
   [02 Architecture](02-architecture.md#footprint-budget).
2. **Run harnesses anywhere.** A harness may live on the local machine (Windows or Linux), on any
   number of SSH hosts, or in any number of WSL distros.
3. **One connection per target.** The host opens a single connection to each target and
   multiplexes everything over it. Extra connections add traffic and, for WSL, can break interop.
4. **Several harnesses, one interface.** Codex and Claude Code first. Adding a provider must not
   require touching the UI beyond what its capabilities unlock.
5. **Honest about differences.** Providers differ. The UI adapts to a declared capability set
   instead of pretending every harness can do everything.
6. **Nothing hardcoded about models.** Model lists, effort levels, and per-model abilities always
   come from the target at runtime. See [the rule](04-providers.md#models-are-never-hardcoded).

## Vocabulary

| Term | Meaning |
| --- | --- |
| **Host** | The machine running the YAAC UI. |
| **Target** | Where harnesses run: a user on a machine — `local`, a user on an SSH host, or a user in a WSL distro. Also called the *origin* of a thread in the sidebar. |
| **`yaacd`** | The YAAC daemon. One per target. TypeScript on Node. The only thing the UI talks to. |
| **Harness** | An agent runtime such as Codex or Claude Code. |
| **Provider kind** | A harness type YAAC knows how to drive: `codex`, `claude`. |
| **Provider instance** | One configured endpoint of a provider kind on a target, with an optional display name. A target can have several of the same kind — for example two Codex daemons on different sockets. |
| **Backend** | How an instance reaches its harness: an existing harness daemon, or the harness SDK. |
| **Thread** | One chat. Maps one-to-one to a conversation inside one provider instance. |
| **Turn** | One user message and everything the agent does in response. |
| **Item** | One unit inside a turn: a message, a reasoning block, a tool call, an approval request. |
| **Capability** | A declared ability of a provider instance or a model, such as reading images. |
| **Fork** | A new thread that starts from an existing thread's history, optionally on a different provider instance and model. |
| **Request** | Something an agent is waiting on the user for: an approval or a question. |
| **Hub** | The headless part of YAAC on the host. Holds the connection to every target and routes between them; keeps running when the window is closed. |
| **Mesh** | Agents reaching each other through YAAC: sending messages, spawning threads, waiting for results. |
| **Worker / sub-thread** | A thread started by another agent rather than by the user. |
| **Journal** | The on-target log of a thread's events, which lets a reconnecting host catch up losslessly. |
| **Project / worktree** | The repository a thread works in, and the specific git worktree within it. |

## MVP scope

In scope for the first usable version:

- Host on Windows and Linux.
- Targets: local, SSH (many; Linux and other POSIX machines), WSL (many, off by default,
  Windows hosts only).
- Settings to add, remove, edit, enable, and disable SSH connections, with a guided setup that
  establishes password-less access and a Revoke that undoes it; to enable WSL and add distros,
  each with an optional user; and to add, name, and configure provider instances per target.
- Providers: Codex (through its daemon, which YAAC starts if needed) and Claude Code (through
  its SDK; a daemon backend is reserved for later).
- A newer YAAC keeps working with an older daemon that is still busy, and updates it once idle.
- Sidebar of threads grouped origin → project → worktree, with every level independently
  toggleable.
- A local cache so the sidebar and recently opened threads appear before any connection is up.
- New thread: choose target, working directory, and model (models grouped by provider instance).
- Thread view with streamed markdown, reasoning, tool calls, approval prompts, and the
  questions an agent asks the user — which are never lost, even across a disconnect.
- Running turns continue when the UI is closed; everything missed is delivered on reconnect.
- Threads that are in use elsewhere (for example in a terminal) are shown live but read-only.
- Composer with markdown, pasted files and images, and selectors underneath for model, effort,
  and the other per-thread settings a provider exposes.
- Switch models within a thread (same provider instance only).
- Fork a thread onto any provider instance and model.
- **The agent mesh:** every thread gets tools to list, message, spawn, and wait for other
  threads — on the same target or another — with messages shown in both transcripts, a panel
  of each agent's workers, limits on runaway spawning, and a headless hub so it all keeps
  working with the window closed.

Explicitly **not** in the MVP (the design must not preclude them):

- An embedded terminal, file tree, or diff editor.
- Logging in to a harness from the UI. Authenticate with the harness CLI on the target first.
- Managing MCP servers, plugins, skills, or harness configuration.
- Creating git worktrees from the UI (existing worktrees are discovered and grouped).
- Moving a thread between targets.
- Windows machines as SSH targets (Windows is supported locally and through WSL).
- An ACP provider (planned as the first provider after the MVP).
- Auto-update, telemetry, multi-window, mobile (a post-MVP Android design is proposed in
  [14 Android](14-android.md)). macOS is kept building on a best-effort basis
  but is not packaged or tested.
- Cloud-hosted agent tasks.
- Running the hub on a server, or a target acting as a hub (the design allows both).
- Shared memory between agents, broadcast messages, and file transfer between targets.

## Why the daemon is TypeScript

The harness integrations are TypeScript-first: Claude Code is driven through
`@anthropic-ai/claude-agent-sdk`, and Codex publishes its app-server protocol as TypeScript
bindings. Putting one TypeScript process on the target lets it import the SDK and speak to the
Codex daemon over a local socket. A Rust daemon would need a second Node helper for the SDK
path, which means two moving parts per target. One process is simpler to install, version, and
supervise — and the next provider's SDK will most likely be TypeScript too.

The price is a JavaScript runtime on each target. YAAC brings its own: `yaacd` is
self-contained and installs a private runtime under `~/.yaac/`, so a target needs nothing but
the harnesses themselves ([bootstrap](02-architecture.md#bootstrap)).
