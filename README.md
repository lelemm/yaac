# YAAC — Yet Another ACP Controller

A fast, low-footprint desktop app for driving coding-agent harnesses (Codex, Claude Code, and
later others) on your local machine, over SSH, or inside WSL — from one window — and for
letting those agents **work together**: message each other, start each other, and wait for
each other, across harnesses and across machines.

> **Status: design phase.** No code yet. The design lives in [`docs/`](docs/README.md).
> Unresolved choices are tracked in [`docs/decisions.md`](docs/decisions.md).

## The idea in one diagram

```
 HOST (Windows / Linux)                     TARGET (local | SSH host | WSL distro)
┌──────────────────────────┐   exactly one   ┌──────────────────────────────────────────┐
│  YAAC UI   (Rust, egui)  │   connection    │  yaacd   (TypeScript, Node)              │
│                          │◄───────────────►│   ├─ codex provider  ─► Codex daemon     │
│  sidebar · threads ·     │   per target    │   ├─ claude provider ─► Claude Agent SDK │
│  composer · local cache  │                 │   └─ (future providers)                  │
└──────────────────────────┘                 └──────────────────────────────────────────┘
```

- **Agents can reach each other.** Every thread is given tools to list, message, spawn, and wait
  for other threads — a Codex thread can hand work to Claude workers on another server. A
  headless hub on the host carries messages between machines even with the window closed.
- **One connection per target.** Everything — every provider, every thread, every file upload —
  is multiplexed over a single byte stream. Never a second SSH session, never a second `wsl.exe`.
- **Thin host, smart target.** The UI is a renderer plus a cache. `yaacd` sits next to the
  harnesses and speaks their native interfaces, so the host needs no harness SDKs at all.
- **A thread is one provider conversation.** A chat is called a **thread** and maps one-to-one
  to a conversation in its provider. You can switch models inside a thread, never providers. To
  move to another provider you **fork**.
- **Nothing is lost while you are away.** Agents keep working when the app is closed or the
  network drops. `yaacd` journals everything — including questions an agent asks you — and
  delivers it when you reconnect.
- **Nothing about models is hardcoded.** Models, effort levels, and what each model can do come
  from the target at runtime.

## Documentation

| Doc | What it covers |
| --- | --- |
| [01 Overview](docs/01-overview.md) | Goals, non-goals, MVP scope, glossary |
| [02 Architecture](docs/02-architecture.md) | Topology, transports, bootstrap, daemon lifecycle, security, footprint |
| [03 Protocol](docs/03-protocol.md) | The UI ⇄ `yaacd` wire protocol |
| [04 Providers](docs/04-providers.md) | Provider contract, capabilities, Codex and Claude mappings |
| [05 Threads](docs/05-threads.md) | Thread identity, model switching, fork, grouping, local cache |
| [06 Attachments](docs/06-attachments.md) | Paste, upload, and why images are special |
| [07 UI](docs/07-ui.md) | Sidebar, thread view, composer, settings, rendering performance |
| [08 Roadmap](docs/08-roadmap.md) | Spikes, milestones, test strategy |
| [09 Compatibility](docs/09-compatibility.md) | How a newer YAAC works with an older daemon, and how the protocol evolves |
| [10 Agent mesh](docs/10-agent-mesh.md) | **The main purpose:** agents messaging, spawning, and waiting for each other across harnesses and machines |
| [Screens](docs/design/screens.md) | Every screen, with mockups and the design review |
| [Pending definitions](docs/pending.md) | Choices made without the owner, awaiting review |
| [Decisions](docs/decisions.md) | What is decided, what is proposed, what is still open |
| [Harness surfaces](docs/research/harness-surfaces.md) | Verified facts about the Codex and Claude interfaces |
| [Codex design review](docs/research/codex-review-2026-11-20.md) | An independent review of the first draft, kept as a record |

## Planned repository layout

```
yaac/
├── Cargo.toml            # Rust workspace
├── crates/
│   ├── yaac-app/         # eframe/egui binary: windows, views, input
│   ├── yaac-core/        # domain model, provider contract, capabilities, app state
│   ├── yaac-protocol/    # wire types + frame codec (source of truth for the protocol)
│   ├── yaac-transport/   # local / SSH / WSL connectors, bootstrap, reconnect
│   └── yaac-cache/       # SQLite cache
├── daemon/               # yaacd (TypeScript) — bundled to one file and embedded in the app
└── docs/
```

## Prerequisites (once code lands)

- Host build: Rust (stable, via `rustup`) and Node.js ≥ 20 for bundling `yaacd`. Neither is
  needed to *run* YAAC.
- Target runtime: only the harness CLIs you want to use (`codex`, `claude`), already signed in.
  `yaacd` is self-contained and installs itself on first connect.
