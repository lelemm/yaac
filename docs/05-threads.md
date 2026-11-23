# 05 — Threads

## Identity

**Decided.** A thread is exactly one conversation in exactly one provider instance.

```
thread  ≡  ( target , provider instance , native conversation id )
```

The **native conversation id** is whatever the harness itself uses to resume that exact
conversation. It is not always the field called "session id":

| Harness | Native conversation id | Not this |
| --- | --- | --- |
| Codex | `Thread.id` | `Thread.sessionId` — shared by every thread in a session tree |
| Claude Code | the session id | — |

YAAC does not mint its own conversation ids. The thread id on the wire is an opaque string that
`yaacd` builds from the instance, that instance's store binding, and the native id — so the
same native id under a different Codex home is a different thread id. Because the identity *is* the harness
conversation, the one-to-one rule cannot be broken by a bookkeeping bug.

What follows from that:

- **The provider of a thread never changes.** There is no operation that could do it.
- **The model can change**, as long as it belongs to the same provider instance and the thread's
  effective capabilities include `ModelSwitch`. The session is untouched.
- **Effort can change** the same way, under `EffortSwitch`.
- **To change provider, fork.**

A thread that has not sent its first message is a **draft**. Drafts exist only on the host.
The thread is created together with its first message (`thread/start`), so an abandoned draft
leaves nothing behind in the harness. Until then, its project and provider instance are still
editable; after it, they are fixed. A new thread is started in the root of its project's
folder — the user picks a project, not a path.

### Binding a thread to an instance

With several instances of one kind on a target, "which instance does this thread belong to?"
needs a real answer, because two instances may share one conversation store.

- `yaacd` records an **immutable binding** for every thread it creates: the instance, and that
  instance's [store identity](04-providers.md#an-instance-has-a-store-identity) at the time.
- A thread discovered through listing is bound to the instance that reported it. If two
  instances report the same conversation (same kind, same store), it appears **once**; the
  other instance is recorded as also able to see it.
- *Seeing* a thread and *driving* it are tracked separately. Only one process can be a
  conversation's writer at a time.
- If an instance's endpoint is edited and now leads to a different store, its threads are not
  re-pointed. They show as unavailable until the user confirms the change.

## Thread summary

What the sidebar needs, and what the cache stores per thread:

```rust
pub struct ThreadSummary {
    pub id: ThreadId,
    pub instance: InstanceId,
    pub title: String,               // harness title, or first prompt, or user override
    pub preview: String,             // short excerpt
    pub cwd: TargetPath,             // opaque; interpreted only on the target
    pub project: ProjectRef,         // a project the user created, or a temporary one; see below
    pub worktree: Option<WorktreeRef>,
    pub git: Option<GitStatus>,      // branch; files added / modified / deleted vs origin; commits ahead / behind; None outside git
    pub model: Option<String>,
    pub state: ThreadState,          // Idle | Running | NeedsAttention | Failed
    pub control: Control,            // Free | Mine | OtherClient | External
    pub forked_from: Option<ThreadId>,
    pub spawned_by: Option<Address>, // the agent that started this thread, possibly on another target
    pub children: u32,               // threads this one spawned; listed on demand
    pub peers: u32,                  // threads it has exchanged messages with but did not spawn
    pub root: Option<Address>,       // the orchestration this thread belongs to, if any
    pub origin_kind: OriginKind,     // Interactive | Yaac | Programmatic — drives the sidebar filter
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub archived: bool,              // off the list entirely; kept, and found by search
    pub native_id: Option<String>,   // the harness's own id, as shown in a terminal
    pub repo: Option<RepoLocation>,  // repository, path within the checkout, worktree name
    pub agents: AgentsPolicy,        // Ask | Allow — may this chat's agent work with others unasked
}
```

`NeedsAttention` means a `request` item is pending: an approval or a question. It is the most
important state to surface in the sidebar, because an agent is blocked on the user.

## Pull requests

A thread in a git checkout has the pull requests of its **branch** (D119). They are worked
with **on the target**, by `yaacd` (`daemon/src/forge/`), using the target's own command-line
tools and their logins — the host never talks to a forge and holds no forge credentials.

- **Which forge.** From the checkout's `origin` URL: GitHub (`github.com`, and any GitHub
  Enterprise host the target's `gh auth status` lists) in its HTTPS, SSH and scp forms; Azure
  DevOps as `dev.azure.com/{org}/{project}/_git/{repo}`, `{org}.visualstudio.com/…`,
  `ssh.dev.azure.com:v3/…` or `vs-ssh.visualstudio.com:v3/…`. Anything else: no badge. A GitHub
  fork whose `upstream` remote is its parent works against the upstream repository (P47).
- **GitHub** is driven through `gh` with `-R HOST/OWNER/REPO`, JSON output, prompts disabled.
  **Azure DevOps** through `az` for a token and the REST API (P46), with the **tenant that can
  reach the organisation discovered** and remembered in `~/.yaac/remotes.json` — a generic
  per-remote record (an open `data` map keyed by the normalised remote URL), not a forge
  schema.
- **Missing or signed out** is not an error: the capabilities are empty and a reason says what
  to do, on which machine ("gh is not logged in on devbox · run `gh auth login` there").
- **Caching and refresh** (P49): one entry per checkout and branch, fresh for 2 minutes;
  refreshed when the checkout's git status changes (a commit, a push — the same queue as the
  [sidebar's git line](07-ui.md#sidebar)) for branches someone asked about recently; asked again
  by the window only for rows it draws, at most every 5 minutes; never a busy poll; a
  rate-limit answer pauses that host.
- **Creating** pushes first when needed, carries an operation id, and never re-sends an
  uncertain create (P51). Agents have the same abilities through the `yaac` MCP server's PR
  tools ([10](10-agent-mesh.md#pull-requests)).

## Threads that belong together

A thread can be started by another agent ([10 Agent mesh](10-agent-mesh.md#spawning)). That
gives threads a second kind of relationship besides fork lineage:

| Relationship | Meaning | Recorded on |
| --- | --- | --- |
| `forked_from` | Began with another thread's history. | The fork. |
| `spawned_by` / children | Was started by another agent to do part of its work. | Both: the child knows its parent, the parent lists its children — even across targets. |
| peers | Have exchanged messages, with neither having spawned the other. | Both, derived from the messages. |

A spawned thread is a full thread that the user can open, steer, or continue. It runs in the permission mode that
corresponds to its parent's, never a more permissive one, and keeps it when the parent's mode
changes later ([D146](10-agent-mesh.md#the-workers-permission-mode)). While its parent is in
the index, the worker appears under **Workers** in the parent's agents panel and contributes to
the parent's sidebar worker count (D171). A worker whose parent is missing can still appear in
the main list. A blocked worker is reachable through **Needs you**.

The [agents panel](07-ui.md#agents-in-the-thread-view) in the thread view shows the same
relationships as a tree, from the point of view of the open thread.

### Harness sub-agents

**Decided (D151):** every subagent is visible in YAAC, the harnesses' own included. A
sub-agent a harness runs inside its own process — Claude Code's `Agent` tool, its background
and workflow agents; Codex's `spawn_agent` — is a thread of its own in YAAC, linked to the
thread that started it. *Proposed* (P190–P194):

- **What it is.** Its summary carries `subagent { agentId, agentType, status, readOnly }`, and
  `spawnedBy` names its parent — the same relation a mesh worker has, always on the same target.
  The parent counts it among its `children`. Instances that list them declare
  `subagentThreads`. `subagent` is the flag the window branches on ("Claude subagent ·
  Explore" against "YAAC worker"), never the provider kind.
- **Where it shows.** Under its parent only: in the parent's agents pill, card and docked panel
  (Workers), and in the parent's worker count in the sidebar. It is never a sidebar row of its
  own while its parent is listed. It does not use up the discovery number cap: it is indexed
  while its parent is and it is itself within the age cap, at most `maxThreads` per parent,
  newest first; a running one is never cut.
- **Live.** It appears as soon as the harness reports it (Claude Code: the `task_started` of
  the parent's query; Codex: `thread/started`), runs a turn while it works, and its state
  follows the harness (running, done, failed, stopped). Its transcript streams while YAAC runs
  the parent (Claude Code forwards the subagent's own messages); otherwise it is read again
  from the harness each time it is opened, like a thread in use elsewhere.
- **What the user can do.** Open and read it, always. It is **read-only**: neither harness
  offers a supported way to continue its sub-agent from outside without making a second
  writer (invariant 9). It is held by "the agent that started it" with the harness's reason
  (`takeOverBlocked`): no turns, no rename, archive, delete, move or fork, no **Take over**. Its
  banner names the parent and offers **Open parent**, the agent to message instead.
- **What it asks is the user's** (D162; Proposed P240–P242). An approval or question a
  sub-agent asks for is raised on **its own thread**, in its running turn — never on the
  parent, and it never opens or holds a turn there: an orchestrator whose background workers
  wait for approval still takes the user's next message. It shows in the sub-agent's
  transcript, in **Needs you**, and as the needs-you mark on that worker in the parent's agents
  pill, and it is answered there: the read-only hold does not cover answers, which follow the
  parent's writer (a parent in use elsewhere is answered there). It ends when the harness
  withdraws it or the sub-agent's run ends, not with a parent turn. A request whose asker
  cannot be told stays on the parent, bound to no turn, and the composer stays usable.
- **Its address** is an ordinary thread address, `<target id>/<thread id>`, where the thread
  id is `yaacd`'s usual one for `(instance, native id)`. Native ids: Claude Code
  `<sessionId>/agent-<agentId>`, Codex the sub-agent's thread id. An agent-call pill finds it
  with `yaac_core::agentcall::resolve_subagent(caller, agentId)`: the thread beneath the caller
  whose `subagent.agentId` (or native id) is the id the call names.

## Keeping the list small: pin, mute, and archive

**Decided.** A fleet produces more threads than anyone can watch. Three actions, all in a
thread's right-click menu. **Pin and mute — like dismiss below — are preferences of the host,
not facts about the thread** (D92): the hub keeps them on the user's computer, the daemon
never hears of them, and another computer does not share them. Archive is different: it
archives the conversation in the harness, so it does go to the target. **Pin** keeps a
thread in a *Pinned* section near the top of the sidebar, whatever its project. The other two
take threads out of the way:

| | Mute | Archive |
| --- | --- | --- |
| For | A thread that is still working but that you do not need to watch. | A thread you are done with but may want again. |
| The thread | **Keeps working**, receives messages, and can be woken, exactly as before. | Is left as it is. |
| In the sidebar | Moves to a collapsed **Muted (n)** section at the foot of the list. | Disappears; shown by a "Show archived" switch, and found by search. |
| If it becomes blocked on you | **Comes back to the top of its group** (with the needs-you mark, D116) until answered, then goes back to muted. An agent is never silently stuck because it was muted. | Not applicable — an archived thread is not running. |
| Undo | Unmute — or it ends by itself: a mute can be given an end, a duration or a date and time, and the thread returns to the list when it passes. | Unarchive. |

Muting is about the user's attention, not the agent's behaviour: nothing about the thread
changes for the agents that work with it. Archive uses the harness's own archive where it has
one, and asks for confirmation first (D111).

### Unseen: a turn that finished while nobody looked

**Decided (D114).** When a thread's turn ends — its state moves from running, needs you, or
waiting to anything else — and **no attached window reports it as shown**, the hub marks it
**unseen**. Each window tells the hub what it shows (`hub/view/showing`, the open thread while
the thread view is on screen); with no window attached, every finished turn is unseen. The
mark is a view preference beside pin, mute and dismiss (D92): kept in the hub's durable store,
sent to every window in `hub/view/changed`, read back by `hub/view/get` after a restart. The
window clears it (`hub/view/set { unseen: false }`) when the thread is opened or is on screen
when the mark arrives. A thread gone from its target loses it. It is not *Needs you*.

### Delete

**Decided (D110).** **Delete…** in the thread menu asks first. `thread/delete` carries an
operation id and `alsoInHarness`: `yaacd` always removes its own record of the thread and its
journal; with `alsoInHarness` — offered only under the `threadDelete` capability, ticked by
default — it first asks the harness to delete its session (if that fails, nothing is removed).
Without it the harness keeps its session and `yaacd` remembers to leave it out of discovery;
asking for it by id (`thread/lookup`) brings it back. A lost answer is an unknown outcome,
shown and never re-sent. On `thread/removed` the window drops the thread from the list and the
history, and the hub drops its pin, mute and unseen mark.

## In use elsewhere

**Decided:** when a thread is being used somewhere else, YAAC lets the user **watch** it but
not interact with it. The composer is disabled, with a line saying where the thread is in use;
approvals and questions are shown but cannot be answered from YAAC.

**Decided:** this includes a conversation that is merely *open* somewhere else but idle — a
`codex` or `claude` session sitting at its prompt in a terminal. As long as the harness reports
another holder, the thread is read-only in YAAC: that holder can start a turn at any moment.

"Elsewhere" covers two cases ([control states](03-protocol.md#thread-control-who-may-act)):

- **Another program on the target** — a `codex` or `claude` session open in a terminal, or
  another app attached to the same harness daemon.
- **Another YAAC client** attached to the same `yaacd` — a second computer. Here the rule is
  per turn: whoever started the running turn controls the thread until that turn ends; then it
  is free again.

Why read-only rather than "first answer wins": a harness has one writer per conversation, and a
Codex daemon really does hand the same approval to every attached client. Letting two places
answer produces cards that go stale, messages that collide, and a user who cannot tell what the
agent was actually told.

### Detecting it

Each provider reports whether a thread is being driven by someone else. This is a
harness-specific job and gets its own spike (S7):

| Harness | Signal |
| --- | --- |
| Codex (daemon) | The thread's status and loaded state on the daemon, and whether a turn is running that `yaacd` did not start; a writer conflict when trying to resume. |
| Claude Code | `claude agents --json` lists live interactive and background sessions with their session ids and state. |

**Decided: when unsure, take over.** A thread is read-only only on **positive evidence** that
someone else is using it — a turn running that `yaacd` did not start, or a harness that
reports another live holder. When `yaacd` simply cannot tell, the thread is treated as free
and YAAC takes it.

Two consequences, both accepted by the owner:

- After `yaacd` restarts, the Codex threads it was holding are still loaded on the Codex
  daemon and `yaacd` cannot prove they are its own. They are usable at once, rather than
  read-only until Codex happens to unload them.
- On Codex, a terminal that has merely *opened* a thread and is sitting idle cannot be told
  apart from that case, and is not detected until it acts. So "open elsewhere but idle is in
  use" holds where the harness can report it (Claude Code lists its live sessions) and not
  where it cannot (Codex).

### Watching it

How live the view is depends on what the harness offers (`FollowExternal`):

- Where the harness streams a thread's events to any attached client (a thread running on the
  Codex daemon `yaacd` is connected to), the view is fully live.
- Otherwise `yaacd` re-reads the stored transcript periodically while the thread is open. The
  view then updates message by message rather than token by token.

A thread in use elsewhere can still be **forked** — that is the way to act on it without
disturbing whoever has it.

### Taking it over

**Decided (D140):** the user can also **take over** a thread another program holds — usually
the owner moving from one client to another. One click, no confirmation; the other program is
not stopped and will not see what is sent from YAAC from then on. The thread is writable here
at once.

*Proposed (P110, P111):* `thread/takeOver` (an operation, under `threadTakeOver`) records on
the thread record which holders it was taken from, and journals "Taken over from <where> · the
other program is still open and will not see these messages". While the harness reports only
those holders, the thread stays writable; a *different* holder makes it read-only again (with a
notice); the holders going away simply ends the takeover. It survives a `yaacd` restart.

| Harness | Holder named by | Refused when |
| --- | --- | --- |
| Codex (daemon) | The last turn another client started (Codex cannot name clients), so a new turn from another client is a new holder. | That client's turn is running: input would join it. A second client can rejoin a loaded thread and start turns in it; the other client, if subscribed, does receive them. |
| Claude Code | The process of an interactive session; the id of a background one. | The holder is a **background** session: Claude Code refuses to resume it (`session_held_by_background`) until it is stopped there, which YAAC never does. Also while the interactive session is mid-turn. An idle interactive `claude` does not stop a resume and does not see what is sent. |

After a takeover, approvals and questions the harness re-surfaces to `yaacd` (Codex re-sends a
pending request to a client that subscribes) are answerable here; those that stayed with the
other program are not.

## Discovery

`yaacd` builds the thread index by scanning each instance's conversations (`ThreadList`).

**Decided:** show every conversation the harness knows about, not only those created in YAAC. A
conversation started in a terminal on the server appears in the sidebar and can be continued.
Conversations that were not started by a person — SDK, sub-agent, and background-worker
sessions — are hidden by default behind a sidebar filter.

Each summary carries an `origin_kind`, so the filter is a view over the cached index:

- `Interactive` — started by a person in the harness's own interface.
- `Yaac` — started from YAAC. **Always shown.** These are technically SDK or app-server
  sessions, and must never be swept up by the "hide automated" filter. `yaacd` knows them from
  its own registry, not from what the harness labels them.
- `Programmatic` — everything else. Hidden by default.

The index is synchronised incrementally ([protocol](03-protocol.md#the-thread-index)): a
reconnect after a week costs one small exchange, not a full listing.

**Archived conversations are discovered too, marked archived** (P131). The scan lists what the
harness has archived as well as what it has not — Codex: a second `thread/list` with
`archived: true` after the active listing; Claude Code: `listSessions()` returns archived
sessions with `archived: true` — so a conversation archived from YAAC, or in the harness itself,
stays in the index and shows in the sidebar's **Archived** section, where **Unarchive** calls the
harness's own unarchive (`thread/unarchive`, `unarchiveSession()`). One archived from YAAC is
never cut by the caps below; one archived elsewhere is subject to them like any other.

**An instance that cannot list lists nothing.** Discovery runs only for an instance that is
`ready` with `ThreadList`; one that is *not installed*, *unreachable* or *could not start* shows
no conversations, and its status says why. `yaacd` is often started by a non-login shell
(`wsl.exe`, `ssh host cmd`) whose `PATH` leaves out `~/.local/bin`, so it looks for `codex`
there and in Codex's own package folder as well as on `PATH`, as it does for `claude` (P130).

### Discovery is capped by age and by number

**Decided.** A harness that has been used for months holds hundreds of old conversations.
Showing them all would flood the sidebar the first time a machine is connected. So a
conversation that YAAC did not create is brought in only if it is **recent enough** and
**among the most recent**:

| Cap | Default | Applies to |
| --- | --- | --- |
| Age | last active within **30 days** | each discovered conversation |
| Number | the **50 most recent** | per provider instance, per target |

Both must hold. The caps are applied **on the target, while scanning**: `yaacd` asks the
harness for conversations newest first and stops at the first one that is too old or once it
has enough — so an old machine costs a short listing, not a full one.

**Never cut by the caps**, however old or however many:

- threads started from YAAC, and threads an agent started through the mesh;
- anything pinned, muted, or archived by the user — the user has touched it;
- anything running, waiting, or needing the user right now;
- any thread that belongs to an orchestration still in progress.

### Dismissing a discovered thread

**Decided.** A thread YAAC discovered and the user does not care about can be **dismissed**
from its right-click menu. It disappears from the interface and nothing else happens: the
harness is not told, the conversation is not archived or changed, and it can still be opened
in a terminal as before.

- The choice is remembered by the hub on this computer (D92), so the thread stays hidden
  after the next scan; the daemon still knows the thread and is not told.
- It is for discovered threads. A thread started from YAAC has **Archive** instead — the
  difference is that Archive also archives the conversation in the harness.
- A dismissed thread still turns up when looked for by id, and a "Show dismissed" switch in
  the same menu as "Show archived" lists them, where each can be restored.
- If a dismissed thread ever needs the user — an agent messaged it and it is now blocked on an
  approval — it is listed again at the top of its group, like a muted one. It is never silently
  stuck.
- A temporary project's header offers **Dismiss all**, for a folder full of old conversations.

**Nothing is out of reach.** The caps decide what is *listed*, not what exists:

- **By id.** Looking a thread up by its id — in the sidebar search, or an agent's
  `find_thread` — asks the harness directly when the index does not have it, and brings that
  one thread in.
- **"Show older"** at the foot of a project section, and "Search older threads on *devbox*"
  under the search results, fetch the next batch from that target on demand.
- A thread brought in either way stays for as long as it is open, pinned, or active, and is
  dropped from the index again afterwards.

The caps are settings per target (Advanced), and can be raised or switched off.

## Sidebar grouping

Three levels — machine (origin), project, worktree — each independently switchable at any
time. They are drawn as **flat sections under sticky headers, not as a tree** (decided): the
levels that are switched on make up each section's header.

```
yaac · feature/protocol                      devbox     ← one header per group, sticky
  ▶ Fix frame decoder                            2m
  ○ Explain the handshake                        2h
api · main                                    local
  ▶ Migrate auth module — orchestrator          3 👥
```

Turning a level off removes that level and merges its children upward:

| Origin | Project | Worktree | Result |
| --- | --- | --- | --- |
| on | on | on | One section per worktree on each machine: `project · worktree` — `machine` |
| off | on | on | One section per worktree, the same project on different machines merged |
| on | off | off | One section per machine |
| off | off | off | No headers: one list, newest first |

Within any group, threads sort by last activity. Threads needing attention float to the top of
their group. Grouping is a view over the same rows, so toggling is instant and needs no
round-trip.

### Projects

**Decided: a project is something the user creates.** It is a folder on a machine, chosen by
the user, with a name the user gives it. The name is free — "Billing API" rather than
`billing-api-v2-final` — and is what the sidebar shows.

Projects are stored on the target, like the rest of the thread registry, so every host sees
the same ones.

#### Which project a thread belongs to

1. **The deepest project whose folder contains the thread's folder.** A thread anywhere inside
   a project's folder belongs to that project.
2. Projects can be nested, and then the most specific one wins:

   | Projects defined | Thread's folder | Belongs to |
   | --- | --- | --- |
   | `~` and `~/projects/abc` | `~/projects/abc/123` | *abc* — the deeper project contains it |
   | `~` and `~/projects/abc` | `~/projects` | *~* — only the outer project contains it |
   | `~/projects/abc` | `~/projects/abc/123/src` | *abc* |
   | `~/projects/abc` | `~/other` | none — a temporary project (below) |

3. **Otherwise the thread has no project yet.**

#### Worktrees that live somewhere else

*Proposed — the owner asked for a suggestion; not yet confirmed.*

A git worktree is a second checkout of the same repository, and it is usually **not** inside
the project's folder: a sibling directory, or a harness-managed location such as
`~/.codex/worktrees/…`. By the folder rule alone, a thread running there would look like it
belongs to no project. The suggestion: **apply the folder rule inside the repository instead
of on the disk.**

- When a project is created, `yaacd` notes whether its folder is in a git repository, and if
  so which repository (its shared git directory) and where the folder sits **relative to the
  checkout's root** — `.` for the root itself, `services/billing` for a subfolder.
- A discovered thread's folder is resolved the same way: which repository, and where relative
  to *its* checkout's root.
- **Same repository, and the thread's relative path is inside the project's relative path →
  the thread belongs to that project**, whichever checkout it is physically in. The deepest
  such project wins, exactly as with folders.
- This is tried **before** the plain folder rule, because it is the more specific fact.

| Projects | Thread's folder | Belongs to |
| --- | --- | --- |
| *yaac* at `~/code/yaac` (a repository) | `~/code/yaac-wt/feature-x` (a worktree of it) | *yaac*, worktree `feature-x` |
| *yaac* at `~/code/yaac` | `~/.codex/worktrees/a1b2/yaac/crates` | *yaac*, that worktree |
| *Billing* at `~/code/mono/services/billing` | `~/wt/hotfix/services/billing/src` | *Billing*, worktree `hotfix` |
| *Billing* at `~/code/mono/services/billing` | `~/wt/hotfix/services/search` | none — same repository, but outside Billing's part of it |
| *Home* at `~` and *yaac* at `~/code/yaac` | `~/code/yaac-wt/feature-x` | *yaac* — the repository match beats the outer folder |

The section header then shows which checkout: `yaac · feature-x`. The main checkout shows its
own branch the same way.

What follows from it:

- **Nothing to configure.** The user never lists worktrees; they are recognised.
- **Starting a thread in a worktree.** A new thread starts in the project's root (D65). When
  the project's repository has other checkouts, the project picker offers them as a second,
  optional choice — "worktree: main ▾" — and the thread then starts at the same relative place
  in that checkout. Creating a worktree from YAAC stays out of the MVP.
- **A worktree that was deleted.** Its threads keep the project and the worktree name they had,
  marked as removed.
- **A folder that is not in a repository** has no worktree; only the folder rule applies.
- The lookup is one `git` query per distinct folder, cached.

This matching matters for **discovered** threads — ones started in a terminal, in whatever
folder the user happened to be in. A thread created from YAAC has no such question: it is
started in **the root of its project's folder**.

#### Temporary projects

A discovered thread that matches no project is not hidden and not dumped in an "Other" pile.
It is listed under a **temporary project** named after its folder, drawn so it is plainly not
a real project yet — italic, with a dashed mark, and the folder path as its name.

Right-clicking a temporary project offers **Create project…**. That opens the ordinary
create-project flow with the folder already filled in — and editable, because the thread's
folder is often *inside* what the user thinks of as the project:

```
Folder   /home/user1/projects/abc/123/        ← prefilled from the thread
         /home/user1/projects/abc/            ← the user deletes "123": that is the project
Name     abc                                  ← prefilled from the last folder name; editable
```

Once created, every thread whose folder falls inside it moves under the new project at once —
including the ones that were sitting in other temporary projects beneath that folder.

#### Moving a thread to another project

**Decided (D144); the details are Proposed (P100–P105).** A thread can be listed under another
project — and, optionally, one of that project's worktrees — **on the same machine**. This is
YAAC's own **placement**, kept by `yaacd` in the thread's record on the target (so every host
sees it) and apart from the harness's working directory:

- `thread/move { opId, threadId, project?, worktree? }` sets it; without `project` it is
  forgotten and the folder rules above decide again. While it is set, the summary's `project`
  and `worktree` follow it (`ThreadSummary.placement`), whatever the folder rules would say.
  Deleting the project forgets it. An archived thread, and one in use elsewhere, cannot be
  moved; a running one can, and the change applies from its next turn.
- When the thread's provider has **`threadMoveWorkingDirectory`**, the move also changes where
  the agent works: the project's folder, or the same place in the chosen checkout (the
  checkout's root plus the project's path inside its repository). It must exist. `yaacd` hands
  that folder to every later turn (Codex: `cwd` on `turn/start` and on `thread/resume`), and the
  first turn after the move begins with a short fixed note, seen only by the harness:

  ```text
  [YAAC] Your working directory is now /home/dev/web. It was /home/dev/api; paths earlier in this conversation refer to that folder.

  ---

  <the turn's input>
  ```

  Without the capability (Claude Code: its sessions are stored per project folder and it has
  no way to change one's folder) only the placement changes.
- Each move is recorded in the transcript as a notice: "Moved to web · working directory now
  /home/dev/web" or "Moved to web in YAAC · working directory unchanged".
- The worktrees offered are the project's own checkout and the other checkouts of its
  repository that YAAC already knows on that machine — those its threads are in or were moved
  to. A placement in a project in git always names a checkout (the project's own when none was
  chosen), so the worktree level of the sidebar can group it.

#### Folder trust

**Decided (D147); the details are Proposed (P160–P165).** A harness's own interface asks
before it works in a folder it has not been told to trust (Codex: "Do you trust the files in
this folder?"). YAAC starts conversations through interfaces that do not ask, so a folder YAAC
worked in would still be asked about the first time the user opens it in the harness. `yaacd`
therefore tells the harness, where the provider has **`folderTrust`**
([04](04-providers.md#trusting-a-folder-is-a-capability)) and the instance's **"Trust folders
YAAC works in"** switch is on (the default):

- **When.** Before YAAC creates a conversation in a folder — a new thread, a fork (native or
  handoff), a worker an agent spawns — and when a move changes the folder the agent works in.
  Asked first, so the conversation itself already runs as it would after the user said yes.
- **Which folders.** Only a folder that belongs to a project the user created, by the rule
  that puts threads under projects ([above](#which-project-a-thread-belongs-to)) — never the
  folder of a thread YAAC discovered and did not start, and never an arbitrary path an agent
  passes to `spawn_thread` outside the user's projects; those keep the harness's own
  behaviour. The folder marked is the conversation's working directory itself.
- **Read first; never downgrade.** A folder the harness already trusts is left alone and
  nothing is said. A folder the harness already has another word on (the user marked it
  untrusted there) is left alone too. YAAC never removes or lowers trust.
- **Once per folder.** `yaacd` records the outcome per instance and harness store in
  `folder-trust.json` in its home, and a folder with an outcome is not asked about again — so a
  folder the user later untrusts in the harness stays untrusted.
- **Visible.** The first thread that marks a folder gets a notice (`folderTrust`): "Marked
  /home/dev/api as trusted in Codex". A refusal (the configuration file is locked, it changed
  between the read and the write, the harness is too old for the method) or no answer within
  10 s is one warning notice in that thread, saying YAAC will not try again; it is never
  retried (invariant 10). The thread starts either way.

#### Worktrees

The worktree level of the grouping is the git worktree a thread's folder is in — its branch
name is what the header shows. A thread whose folder is not in a git repository has no
worktree, and its section header is just the project.

## Fork

Fork creates a new thread that begins with the history of an existing one. The new thread may
use any provider instance and any model on the **same target**. The original thread is not
modified, and may even be in use elsewhere.

`yaacd` picks the mechanism from the **source and destination pair**:

### Native fork — within one conversation store

When the destination instance is the same kind as the source **and shares its store**, and the
source declares `ThreadFork`, the harness does it (Codex `thread/fork`, Claude `forkSession`).
The new conversation carries the full native context. This is lossless.

A different model can be chosen in the same step. Two Codex instances with separate homes do
**not** qualify: the destination could not even find the source conversation. They get a
handoff fork.

### Handoff fork — everything else

**Decided:** a fork that cannot be native carries the conversation as a transcript file.

1. `yaacd` reads the source thread's history **from the harness's own store** and renders it to
   a markdown transcript: user messages, agent messages, and a compact line per tool call (what
   ran, whether it succeeded). Reasoning and bulky tool output are left out.
2. It writes the transcript into `yaacd`'s own state directory on the target (`~/.yaac/`),
   pinned to the new thread. It is deliberately **not** in the upload folder: that lives in the
   system temp directory and can be wiped by a reboot, and the forked thread's inherited
   history must not disappear with it.
3. It creates the new thread in the same working directory.
4. It prefixes the user's first message with a short, fixed preamble pointing the agent at the
   transcript file and saying it is the prior conversation.

The new thread records `forked_from`. The UI shows the inherited history above a divider
(`handoff` item), read-only, rendered from that same frozen transcript.

**A handoff is created with its first message** (`thread/fork`), because the destination
conversation does not exist until the harness is sent one; `thread/forkOnly` (no first message)
is for native forks only and refuses a handoff (P33). Steps 1 and 2 happen before the destination
is asked; the operation id makes a resend return the recorded outcome, never a second thread.

#### The transcript

One renderer, shared by every provider, works on the items each provider's `readThread` returns
(P34):

```markdown
# Prior conversation

Forked from "Fix frame decoder" (Codex on devbox, thread t_3f…), through turn 2.
Hidden reasoning and full tool output are not included.

## Turn 1

**User:**

Why does decode() drop the last frame?

- Tool (command): `cargo test -p yaac-protocol` — succeeded (exit 0)
- Tool (command): `cargo clippy` — failed (exit 1): error: unused variable `n`

**Agent:**

The length prefix counts the kind byte, so the reader stops one byte early.
```

- User and agent messages are kept in full; agent-mesh messages keep their sender.
- One line per tool call, with its outcome: *succeeded*, *failed* (with the exit code and the
  first 160 characters of output on one line), *declined*, or *did not finish*. No other output.
- Approvals and questions are one line each; reasoning and notices are left out.
- An attachment still on the target is linked into the new thread's attachment folder and
  referenced by that path; one that is gone reads `[file omitted: log.txt — no longer on the
  target]`. Where the harness keeps no file (Claude stores image bytes inline), what `yaacd`
  journaled for that turn names the attachment.

The file is `~/.yaac/handoffs/<key>/transcript.md` (private to the user), keyed by the fork's
operation id since the new thread's id is not known yet. Its folder is made readable to the
harness for the thread's whole life (`ProviderInput.readable`), and is deleted with the thread.
The `handoff` item carries the same transcript, bounded to 48 KiB — its end, cut at a turn
heading, with a note naming the file (P35).

#### The preamble

Fixed text, in front of the user's first message (P36):

```text
[YAAC handoff] This conversation continues an earlier one, held with a different agent.
The prior conversation is in this file: /home/dev/.yaac/handoffs/3f9a…/transcript.md
Read it before you answer; it is the history of this conversation. It leaves out hidden reasoning and full tool output.
The earlier thread is t_3f… on devbox.
You can read it or message it with the yaac tools (read_thread, send_message; its address is tg_…/t_3f…).
The user's message follows.

---

<the user's message>
```

The line about the `yaac` tools is there **only when the new thread is offered them**
(`MeshTools`). The new thread's own transcript shows the user's message without the preamble;
only the harness receives it. Because the harness would title the conversation from that first
message, YAAC names the fork "<source title> (fork)" (P37).

#### Threads created outside YAAC fork the same way

Nothing in a handoff depends on YAAC having seen the conversation happen. The transcript is
built at fork time from what the harness stored, so a conversation that was started and run
entirely in a terminal has exactly the same material available as one started in YAAC.

What decides whether a thread can be forked is only this:

| Condition | Result |
| --- | --- |
| The source provider can read history (`ThreadRead`) — both MVP providers can | Fork available. |
| A future provider that cannot read history | Fork is **disabled, with the reason shown**. YAAC will not fork from a partial memory of the conversation. |
| The source thread is mid-turn (here or elsewhere) | Forks from the **last completed turn**. The dialog says which one. |
| The source contains images or attachments | Files still present on the target are referenced; ones that are gone (the temp folder was cleaned) are marked as omitted in the transcript. |

#### What a handoff does not carry, and what it may expose

- Not carried: hidden reasoning, cached context, the running state of tools, pending approvals.
- **A handoff moves conversation content to a different provider.** Tool arguments and outputs
  in the transcript may include things the user would not paste into another vendor's product.
  The fork dialog therefore shows what will be sent and where — destination instance and
  account, the turn it cuts off at, the transcript size, and a preview — and the fork happens
  on explicit confirmation.

A summary written by the source model, as an alternative to the full transcript, may be offered
later. It costs a turn and can distort; the transcript is the default.

### Fork point

The MVP forks from the last completed turn. Forking from an earlier message is a later feature,
gated on `ThreadForkAtTurn` for native forks and always possible for handoff forks.

### In the UI

The fork button sits with the other controls under the composer. It opens the same model picker
as a new thread — all instances on the target, grouped by provider instance — and labels each
choice as *native* or *handoff*.

The dialog asks `thread/forkPreview` for each destination chosen. It names the cut-off turn by
its position ("at the last completed turn (turn 12)"), never by its id.

- **Native:** the result line, what is carried, and **Fork** (Enter). The new thread opens idle;
  its first message goes with `turn/start`.
- **Handoff:** additionally the destination account, the transcript size, what is not carried
  (hidden reasoning, attachments no longer on the target), the preview (open), and a box for the
  **first message to the new agent**. The button is **Send and fork**: it waits for the preview,
  needs a message, and is not bound to Enter (P33).

The dialog's body scrolls; its title and buttons always show.

## Local storage on the host

### Cache — disposable

**Purpose:** the app is useful before any connection exists. Nothing in the cache is
authoritative, and deleting it loses nothing but speed.

Stored in one SQLite file in the host's user data directory:

| Table | Contents | Refreshed |
| --- | --- | --- |
| `targets` | Last known target facts and connection state | On connect |
| `providers` | One descriptor per instance, including models and capabilities, with its `revision` | On every connect, and on `provider/updated` |
| `threads` | One `ThreadSummary` per thread, plus the per-target sync token | Incrementally via `thread/sync` |
| `items` | Transcripts of threads the user has opened, with the journal position they are valid at | When the thread is opened or receives events |
| `ui_state` | Grouping toggles, collapsed groups, last-selected thread | On change |

Rules:

- **Stale-while-revalidate.** Show cached data immediately and unmistakably mark a target that
  is not yet connected. Replace it as fresh data arrives. Never block the first frame on I/O.
- **Transcripts are cached lazily and bounded.** Only opened threads are stored, with a size cap
  and least-recently-used eviction. The sidebar index is always complete.
- **Items and position commit together**, in one transaction
  ([protocol](03-protocol.md#journal-and-replay)).
- **Schema changes drop the cache.** There is no migration path to maintain; the cache carries
  a version and is rebuilt from the targets when it changes.
- **The cache is not a model authority.** See
  [models are never hardcoded](04-providers.md#models-are-never-hardcoded).
- **Secret answers are never cached.** An answer to a question marked `secret` is not written
  to the host.
- Removing a target deletes its rows.

### Drafts — durable

Unsent composer text, its attachments, and the receipts of operations still in flight are
**user data, not cache**. They live in a separate store with its own schema migrations. "Clear
cache" does not touch them, and a cache schema change does not drop them.
