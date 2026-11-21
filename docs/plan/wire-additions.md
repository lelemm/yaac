# Wire additions after v1 was drafted

Everything here was added to the protocol **additively** — no version bump, the frozen v1
baseline untouched — to carry the owner's later decisions. It is the reference for removing the
`SEAM` placeholders in the window crates and for the mesh package. The Rust types in
`crates/yaac-protocol` remain the source of truth; if this list and the code disagree, the code
is right.

## `ThreadSummary`

- `nativeId?` — the harness's own conversation id (what a user copies from a terminal).
- `muted`, `pinned` — booleans.
- `agents` — `ask` | `allow` (open enum, default `ask`).
- `repo?` — `{ repo, relative, root, worktreeName? }`: which git repository the thread's folder
  is in, where it sits relative to the checkout root, and the worktree's name when it is a
  linked worktree.
- `git?` — `{ branch?, base?, added, modified, deleted, ahead, behind, removed, at? }`.
  Counts compare the working tree (including untracked files) with the merge-base of the
  branch and its upstream; `removed` means the checkout no longer exists.
- `project` is always set; `worktree` and `branch` are filled from `repo` and `git`.
- `ThreadState` gains `waiting` (waiting on another agent) and `done` (a spawned thread whose
  work completed).

## Projects

- `ProjectRef` gains `kind?` (`user` | `temporary`), `id?`, `repo?`. A temporary project's
  path and name are the thread's folder.
- `Project { id, name, folder, repo?, crossTargetKey?, createdAt }`.
- Methods: `project/list`, `project/create { opId, folder, name }`,
  `project/rename { opId, projectId, name }`, `project/delete { opId, projectId }`.
- Notification `project/updated` carrying the full project list.
- `thread/start` accepts `project?` (the thread starts in that project's folder) and `agents?`.

## Threads and items

- `thread/update { opId, threadId, muted?, pinned?, agents? }` → `{ thread }` — a patch; also
  pushed as `thread/updated`.
- `thread/forkOnly { opId, source, instance, model?, effort?, mode?, atTurn? }` →
  `{ threadId, forkKind }` — a **native** fork with no first message; a handoff is refused
  (`capability_missing`) and goes through `thread/fork`. (`thread/fork` is unchanged: its
  first message could not be made optional additively.)
- `item/read { threadId, itemId, offset, maxBytes }` →
  `{ present, data, offset, nextOffset, totalBytes, more }`.
- `Item.startedAt?`, `Item.completedAt?`.
- Image and file input parts: `name?`, `mediaType?`.
- Subscribe result: `context?`, `tokens?`. Read result: `context?` on the first page.
- `ModeInfo.skipsApprovals` — a mode in which the agent acts without asking.

## Requests

- New variant `meshPermission`. Request:
  `{ actions: [{ action: "startThreads" | "message", summary, threads, recipients }], budget: { agentTurns, threadsSpawned, wallClockSecs }, root? }`.
  Answer: `{ decision: "allow" | "deny", budget?, alwaysAllow }`.

## Mesh

- `mesh/usage { root, source, agentTurns, threadsSpawned }` → `{ exhausted, used? }`,
  target → hub → the root's target. Counts are cumulative, so a repeat is harmless.
- Notice kind `undelivered`: a relayed message still queued after 15 minutes earns its sender
  one passive notice.

## Window ⇄ hub

- `hub/hello` result: `clientId?`.
- Target-state notifications: `enabled`.
- `hub/threads/find { query, target?, state?, limit?, cursor? }` →
  `{ results: [{ target, address, matched, exact, score, thread }], total, ambiguous, cursor? }`
  — by id (full or a prefix of four or more characters) or fuzzy title, ten per page.
- `hub/projects/list`, and the notification `hub/projects/changed { target }`.

## Added with the mesh

- `MeshQuery` gains `findThreads` and `listTargets`, with their replies (answered by the hub
  from its own index); `SpawnThreadQuery.depth`; `MeshThreadFilter` gains `includeInactive`,
  `state`, `cursor`, and a provider filter; `ListThreadsReply` gains `total`, `cursor`.
- Item ids: `sent:<msgId>` (updated in place as its receipt changes), `recv:<msgId>`,
  `spawn:<turnId>` (one live item), and `meshWait`. Mesh items made outside a turn have an
  empty `turnId`.
- Notice kinds `budgetExhausted`, `meshLimit`.
- `mesh/budget { action: "continue", opId, root }` is the Continue action.

## Pull requests (D119–D121)

- `forge/prs { threadId, refresh? }` → `ForgePrsResult { root, branch?, access, defaultBranch?, prs, at?, error? }`.
- `forge/prepare { threadId, base? }` → `ForgePrepareResult`.
- `forge/pr/create { opId, threadId, base, title, body, draft }` → `{ pr, pushed }` (state-changing).
- `forge/pr/get { threadId, number }` → `{ pr, body, reviewers, checks }`.
- Notification `forge/prs/changed` (a `ForgePrsResult`).
- Open enums `ForgeKind`, `ForgeCapability`, `PrState`, `ChecksState`, `CheckState`.

## Not on the wire yet

- Unarchive (`thread/update` does not take `archived`).
- (Mute, pin, and dismiss are host preferences kept by the hub — D92 — and are being taken off
  `ThreadSummary` and `thread/update`.)
- A way and a way to ask a target for older threads than the
  discovery caps list (D78).
