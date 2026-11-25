# YAAC documentation

Read in order the first time; each doc stands alone afterwards.

1. [Overview](01-overview.md) — what we are building and what we are not
2. [Architecture](02-architecture.md) — processes, connections, lifecycle
3. [Protocol](03-protocol.md) — the UI ⇄ `yaacd` wire protocol
4. [Providers](04-providers.md) — the provider contract and capability model
5. [Threads](05-threads.md) — identity, fork, grouping, cache
6. [Attachments](06-attachments.md) — files and images
7. [UI](07-ui.md) — the app surface and rendering budget
8. [Roadmap](08-roadmap.md) — spikes, milestones, testing
9. [Compatibility](09-compatibility.md) — protocol versions, dialects, upgrading a busy daemon
10. [Agent mesh](10-agent-mesh.md) — **the main purpose**: agents working together across
    harnesses and machines. Read this one even if you read nothing else.
11. [Web preview](11-preview.md) — see a dev server on a target live, through a tunnel on its one connection
12. [Files and diffs](12-files.md) — the worktree tree with git marks, file and diff tabs, Add to composer
13. [Terminal](13-terminal.md) — a fast terminal tab on the target: PowerShell on Windows, the login shell on Linux
11. [Installing on Windows](install.md) — the per-user setup: where files go, what runs at
    login, upgrade and uninstall

Reference:

- [Decisions](decisions.md) — the decision log. **Check here before changing a design choice.**
- [Pending definitions](pending.md) — choices made without the owner, awaiting review.
- [Screens](design/screens.md) — every screen, its mockups, and the design review.
- [Harness surfaces](research/harness-surfaces.md) — what Codex and Claude Code actually expose,
  verified against installed versions.
- [Codex design review](research/codex-review-2026-11-20.md) — an independent review of the
  first draft. Historical; the numbered docs are current.

## Conventions used in these docs

- **Decided** — settled; change only through the decision log.
- **Proposed** — the current recommendation, not yet confirmed by the project owner.
- **Open** — a question without an answer yet.
- **Verify** — an assumption about third-party behaviour that a spike must confirm before we
  build on it.

Method names, type names, and package versions quoted from Codex or Claude Code were read from
the installed tools on 2026-11-20 (see the research doc). Both harnesses move fast: re-check
before relying on a detail.
