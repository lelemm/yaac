# YAAC — agent guide

YAAC is a Rust/egui desktop app that drives coding-agent harnesses (Codex, Claude Code) on
local, SSH, and WSL targets through one TypeScript daemon per target (`yaacd`).

**The project is in the design phase. `docs/` is the specification.** Read
`docs/README.md` first, and `docs/decisions.md` before changing any design choice.

The point of the product is the **agent mesh** (`docs/10-agent-mesh.md`): agents in different
harnesses and on different machines messaging, spawning, and waiting for each other.

## Invariants — do not break these

1. **One connection per target.** Never open a second SSH session, `wsl.exe`, or side channel
   to a target, for any reason. Everything is multiplexed over the one stream. (A target is a
   user on a machine. SSH Setup and Revoke use short sessions only while no live connection
   exists.)
2. **A thread is one provider conversation.** Models may change within a thread; the provider
   instance never does. Changing provider means forking. The key is the harness's own
   conversation id (for Codex: `Thread.id`, never `sessionId`).
3. **Nothing about models is hardcoded.** No model ids, names, effort levels, or per-model
   capability tables in source. They come from the target at runtime; the host cache is only a
   speed-up and is refreshed on every connect.
4. **Branch on capabilities, never on provider kind.** UI code must not contain
   `if kind == "codex"`. If a capability is missing, add one.
5. **The protocol version is negotiated once, in the `initialize` handshake.** Do not add
   version fields to frames or messages. The newer side adapts to the older one through the
   dialect layer, and **nothing outside the dialect module may read a version number** — no
   `if version …`, no copied message sets. Follow `docs/09-compatibility.md` exactly when making
   an incompatible change: split the method, override the step.
6. **Images are not ordinary attachments.** Route by the thread's effective `ImageInput`
   capability (`docs/06-attachments.md`).
7. **The UI thread never blocks** and never repaints without cause. Footprint budgets are in
   `docs/02-architecture.md`.
8. **Nothing is lost while the host is away.** Turns keep running; `yaacd` journals every event
   on disk. A question or approval from an agent is journaled before anything else happens to
   it and is never dropped, auto-answered, or timed out by YAAC — except a tool approval in a
   thread the user put in the provider's never-ask mode (bypass, full access): that one is
   approved by YAAC, recorded as such, and shown (P342). Questions, forms and logins always
   reach the user.
9. **One writer per thread.** A thread that is known to be in use elsewhere is shown live and
   read-only until the user takes it over (**Take over**, D140). When it cannot be told whether
   a thread is in use, YAAC takes it over — only positive evidence of another user makes a
   thread read-only.
10. **No blind retries.** State-changing requests carry an operation id; an uncertain outcome
    is shown to the user, never re-sent automatically.
11. **`yaacd` may start a Codex daemon, never stop, restart, or update one.** Codex is driven
    through its daemon only; there is no SDK fallback.
13. **Agent messages are durable, attributed, and visible.** Each hop writes to disk before
    acknowledging. `yaacd` stamps the sender; an agent cannot forge it. Every message appears in
    both transcripts. A message from an agent is never presented to another agent as coming
    from the user, and never answers an approval or question meant for the user.
14. **The hub owns the target connections; the window owns none.** They talk only through the
    attach protocol and the cache file.
12. **Never store a password.** SSH passwords are used once, in memory. The only key YAAC keeps
    is one it generated itself for a connection it set up; it never copies, installs, or
    removes the user's own keys, and Revoke removes only what YAAC added.

## Working here

- Keep the docs true. A change to behaviour updates the matching doc in the same change.
- New design choices go in `docs/decisions.md` as *Proposed* until the owner confirms them.
- Facts about Codex or Claude Code interfaces must be verified against the installed tools, not
  recalled — see `docs/research/harness-surfaces.md` for how. Both change often.
- Rust types in `yaac-protocol` are the source of truth for the wire protocol; TypeScript
  bindings are generated, never hand-edited.
