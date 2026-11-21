# Screens

The brief for YAAC's screens: what each one is for, what is on it, and what it must make easy.
Mockups generated from this brief are in [`mockups/`](mockups/); the review of them is in
[`review.md`](review.md). The behaviour behind each screen is specified in
[07 UI](../07-ui.md) and [10 Agent mesh](../10-agent-mesh.md).

This is revision 5 of the brief. Revisions 2 to 4 came out of the design review. **Revision 5
is the owner's**, after seeing the mockups: the sidebar tree is replaced by a calm flat list
under sticky headers; outlines around controls are removed; the "Since you left" page, the
warning marks on parent threads, and the Agents settings tab are removed; Mute, Archive, and
a per-chat agents toggle with a permission prompt are added.

## Visual direction

- A dense, calm **developer tool**. Closer to a good code editor than to a consumer chat app.
- Built with egui, so: flat, small corner radii (4–6 px), no drop shadows except on floating
  dialogs and menus, no gradients, no glass or blur.
- **Calm before complete.** The owner's reaction to the first mockups was that they felt
  polluted. Every mark on screen must earn its place; when in doubt, leave it out.
- **No outlines.** A border around a control adds a rectangle for the eye to parse and says
  nothing. Things are told apart by **space** first, then by a **faint fill**, and by a line
  only where two regions genuinely meet. Concretely:
  - **Text and prose have no box.** Agent prose sits directly on the window background. A user
    message is set apart by a faint fill and space — no outline.
  - **Collapsed reasoning and tool lines are plain lines of text** with a small mark, not
    boxed rows.
  - **Selectors are text with a chevron** — `gpt-6.2-sol ▾` — with no outline and no fill until
    hovered. The control row is a quiet line of words, not a row of boxes.
  - **One filled button per screen**: the primary action. Every other button is plain text,
    with no outline; it gains a faint fill on hover.
  - **Inputs** (search, composer, form fields) have a faint fill and no outline; the focused
    one gets the 2 px focus ring.
  - **Cards that need the user** (approval, question, permission) are the one thing worth
    marking: a faint fill and a 3 px coloured left edge. No outline around them either.
  - **Code blocks** are a faint fill, no outline.
  - **The only lines in the window** are one hairline between the sidebar and the thread, and
    one between the thread and the agents panel. Sections inside a panel are separated by
    space and a small heading, not by rules or boxes.
- **Icons are real icons.** Every icon comes from one small icon set embedded in the app, drawn
  as vector shapes at the text's size. The one exception is the **harness marks**: each
  provider is shown with its **real logo** (the Codex mark, the Claude mark), embedded as a
  vector asset and drawn **in its brand colour** — the Claude mark in its orange, the Codex
  mark in the theme's text colour (it has no colour of its own) — the one place colour appears
  that does not report a state; a
  provider with no logo gets its initial in a circle. Which logo belongs to which provider is a
  lookup table, not a branch in UI code. Unicode symbols and emoji are never used as icons —
  they render differently on every machine.
- **Fewer marks.** If an icon repeats on every row, it is decoration: remove it. No avatar
  beside each message, no icon before every tool line, no chevron on things that do not
  collapse, no folder icons.
- **Type:** a clean sans-serif for UI at 13–14 px; a monospace for code, commands, paths, and
  ids. Agent prose is the most readable thing on screen: 15 px, generous line height.
- **No accent colour.** The interface is monochrome; the owner found a blue accent grating.
  The only colours on screen are the ones that *mean* something: **needs you** (the red
  exclamation mark, D113; its words stay amber), **failed** (red), **done** (green), the git
  changes on a sidebar row, and **unseen** (cornflower blue, an outline only — D114, the one
  exception to D83). Everything else —
  selection, the primary button, the focus ring, links, the running mark — is drawn in
  neutrals.
- **Dark and light are equals.** Same layout, same content, same hierarchy, same contrast
  quality, and the same actions available. Neither is an inverted afterthought.

| Token | Dark | Light |
| --- | --- | --- |
| Window background | `#16181D` | `#FFFFFF` |
| Sidebar / panel background | `#1B1E24` | `#F5F6F8` |
| Raised surface (request cards, menus, dialogs) | `#22262E` | `#F7F8FA` |
| Faint fill (user message, input, code, hover) | `#1E2128` | `#F3F4F7` |
| Hairline between regions (the only border) | `#2E333D` | `#DADDE3` |
| Primary text | `#E6E8EC` | `#1A1D23` |
| Secondary text | `#9AA2AF` | `#5B6472` |
| Unavailable-control text | `#8891A0` | `#646E7C` |
| Primary button fill | `#E6E8EC` | `#1A1D23` |
| **Selected row fill** (neutral) | `#2A2E37` | `#E6E8EC` |
| Text on the primary button | `#16181D` | `#FFFFFF` |
| Focus ring | `#C9CED6` | `#3A4050` |
| Link text | primary text colour, semibold | primary text colour, semibold |
| Running | primary text colour — no hue | primary text colour — no hue |
| Needs you | `#F2B24C` | `#8F5600` |
| Failed (and the dangerous-action button) | `#FF7C87` | `#BF2636` |
| Done | `#4CC38A` | `#17754D` |
| Git: added | `#73C991` | `#166F31` |
| Git: modified | `#E2C08D` | `#855D00` |
| Git: deleted | `#F47C7C` | `#C22E29` |
| Unseen finished turn (an outline, never text — D114) | `#6495ED` | `#4A7FE0` |
| Modal scrim | black at 55 % | black at 35 % |

### Contrast is a requirement, not a hope

Every text colour above measures **at least 4.5 : 1** against all three backgrounds of its
theme (window, panel, raised surface). The values were computed, not eyeballed; a token that
drops below 4.5 : 1 is a bug. The **unseen** outline is a graphic, not text: it measures at
least **3 : 1** against every surface it is drawn on (light `#4A7FE0`: 3.2 : 1 on the selected
tint, 3.6 : 1 on the panel, 3.9 : 1 on white; dark `#6495ED`: 4.6 : 1 or more). The **fast**
yellow (D125: the fast-mode toggle when on, and the spinner of a turn running in fast mode) is
a graphic too, measured at the 87% icon opacity it is drawn with: light `#9A7000` at least
3.0 : 1 (3.0 on the selected tint, 3.3 on the panel, 3.55 on white; 3.65–4.5 at full opacity),
dark `#F5C518` 6.7 : 1 or more.

- **Anything the user must read is primary or secondary text** — including the *reason* a
  control is unavailable ("not signed in on local", "Native fork is only available within the
  same provider and store", "YAAC installed nothing on this server").
- **An unavailable control is never shown by faint text alone.** It has no fill, a dashed
  border, the unavailable-control text colour (still 4.5 : 1), and its reason beside it.
- The one filled button on a screen is solid in the primary-button colour — near-white on
  dark, near-black on light — with its label in the opposite. A dangerous action's filled
  button uses the failed colour instead.
- **Running has no colour.** It is the normal state, so its mark (the play triangle) is drawn
  in the primary text colour. Colour is kept for what wants attention or reports an outcome.
- **A selected row is a neutral tint** — the "selected row fill" token — and its text stays
  primary. Every label and state colour that can appear on a row measures at least 4.5 : 1
  against that tint too. Unavailable-control text is not used inside rows.

## Rules that hold on every screen

These exist because the first round of mockups broke each of them.

### State is a shape and a word, never a colour alone

| State | Mark | Word |
| --- | --- | --- |
| Running | play triangle (the symbol alone — no mark has a circle, D122) | running |
| **Needs you** | an exclamation mark in the failed colour (D113, D122) | needs you |
| Waiting on another agent | hourglass | waiting |
| Done | check | done |
| Failed | cross | failed |
| Idle | nothing | — (a relative time instead) |
| Offline / disabled | nothing (the row is dimmed) | offline, off |

Every mark must be recognisable in greyscale. In the sidebar list the shape alone carries the
state, to keep rows to one calm line; wherever there is room — the thread header, cards, the
agents panel — the word is shown beside it.

A finished turn nobody looked at (D114) is not a state: it is a **cornflower outline** around
the row (sidebar, docked panel, the strip's hover card) and a **ring** around the agent's icon
(pill, strip). It clears when the thread is opened.

A turn running in **fast mode** (D125) keeps the running spinner, drawn in the **fast** yellow
and turning once per 1.8 s instead of 2.5 s — in the sidebar, the strip, the agents panel, and
the thread header. Whether a turn is fast comes from the thread's summary (`fast`), never from
the window's own toggle. Needs you, paused, failed and running
take precedence: while a thread shows one of those marks the outline is not drawn (the mark
is kept and returns once the louder state has passed).

### Needing the user outranks everything

If a thread has a pending approval or question, its mark is **needs you** everywhere it
appears — sidebar row, thread header, agents panel — even though a turn is technically still
running. It is never shown as "running" while it is waiting for the user. This is the thread's **own**
state: its parents carry no mark for it. Blocked threads float to the top of their group in
the sidebar (D116), and the **Needs-you button** beside the history buttons (D117) opens them
one after the other.

### One shell

```
┌────────────┬───────────────────────────────────────────────────────────┐
│ Sidebar    │ Thread header                                             │
│ 280 px     ├───────────────────────────────────────────────────────┬───┤
│            │ Transcript                                            │ ◉ │  ← agents pill,
│ NEEDS YOU  │                                                       │ ◉↓│    floating; widens
│ flat list  │                                                       │ ◉ │    to a card on hover;
│            ├───────────────────────────────────────────────────────┴───┤    can be docked as
│            │ Composer                                                  │    a third column
│ ⚙  · Hub   │ Control row                                               │
└────────────┴───────────────────────────────────────────────────────────┘
```

- **The window has no system title bar** (D104). Its **top row is the title bar**, about
  42 px tall, like a browser's tab strip: over the sidebar it holds `YAAC`, ‹ ›, **New** and the
  collapse button (over the collapsed strip, the strip's mark); over the thread, the thread's
  state mark, title and tag (the breadcrumb stays on the line below); at the far right,
  minimize, maximize (restore while maximized; the icon changes) and close, which turns the
  failed colour on hover. Every empty part of the row, and the title itself, drags the window;
  a double click maximizes or restores it; a press on a control in the row never moves the
  window. The docked agents panel starts below the row. The window's edges and corners resize
  it. No "Pinned / Active / Recent" lists, no
  "Add agent" button, no "+ Agent / + Tool" controls. Threads are the only objects.
- The sidebar and the thread are separated by a single hairline, and by nothing else. A
  docked agents panel gets one more.
- **The selected sidebar row is always the thread that is open.**
- **Settings** is a full-window page: its own left navigation, a "← Back to threads" control at
  the top, no thread tree, no agents panel, no composer.

### The sidebar: a flat list under sticky headers

**There is no tree.** No indentation, no chevrons, no folder icons, no nested levels. The
sidebar is a single scrolling list of thread rows, divided by section headers. It should feel
closer to a mail client's message list than to a file explorer.

- **A section header** is one quiet line, in small secondary text, naming the group: the
  project on the left — `yaac` — and the machine on the right — `devbox`. (The branch is on
  each row, so by default sections are per project; ticking *Worktree* in the grouping menu
  splits them further, and the header then reads `yaac · feature/protocol`.) A connection mark appears beside the machine only when it is *not* connected.
  Headers are **sticky**: as the list scrolls, the header of the group being read stays pinned
  at the top until the next one pushes it away. A header has no fill and no rule; it is set
  apart by extra space above it.
- **A thread row has two lines.**
  - **Line 1:** a state mark, the title, and — right-aligned, in secondary text — a relative
    time.
  - **Line 2**, smaller, aligned under the title: what the thread is working on in git, **as
    compactly as it can be said**, with **real icons and the colours every git tool uses** —
    never Unicode stand-ins such as Δ or ↑:
    - the thread's **harness logo**, small, then a **branch icon** and the branch name, in
      secondary text; a thread outside git still shows the logo, alone on line 2;
    - then the files that differ from origin, each kind as a tiny icon and a number in its
      colour: **added** (a plus in a small square, green), **modified** (a dot in a small
      square, soft yellow), **deleted** (a minus in a small square, red);
    - then, in secondary text with no colour, an **up-arrow icon** and the number of commits
      not pushed, and a **down-arrow icon** and the number not yet pulled.
    - Every count is shown only when it is not zero, so a clean, pushed branch shows just its
      icon and name.
    - **The number of workers sits at the right edge of line 2**, directly under the time: a
      small people icon and the count of the thread's direct workers, in secondary text. Shown
      only when there are workers. The right-hand column of a row is therefore: time on line
      1, workers on line 2.
    - Example, reading left to right: branch icon `feature/protocol` · green plus-square `2` ·
      yellow dot-square `5` · red minus-square `1` · up-arrow `2`.
  - A thread whose folder is **not in git** has no second line; its row is one line tall.
  - Still nothing else on the row: no state word, no "spawned" arrow.
  - The git colours are softer than the state colours and sit on small icons on line 2; a
    thread's **state** is the larger shaped mark on line 1. Green, yellow, and red on line 2
    always mean files, never the agent. The state mark is a distinct shape, so it reads without the word; the word
    appears in the thread's header when it is open.
  - Rows in *Muted* and *Archived* stay one line: they are for finding a thread, not for
    reading its status.
  - **Narrow, a row keeps every mark** (D115): the title — and on line 2 the branch — is
    ellipsised, down to nothing; the state mark, the harness logo, the git counts, and ahead /
    behind keep their places.
  - A row whose last turn ended while nobody looked has a **cornflower outline** (D114).
- **The selected row** is the open thread, drawn with the selected-row tint.
- **Grouping** is one small icon button at the right end of the search field, opening a menu
  with three checkboxes — Machine, Project, Worktree. It is not three chips. Whatever is ticked
  makes up the header text; with nothing ticked there are no headers at all, just the list,
  newest first. Sections are ordered by most recent activity.
- **Ordering** (D116) is a second small icon button beside the grouping button, opening a menu
  in the shared style: three radio choices — **Last message** (the default), **A → Z**,
  **Z → A** (by title, ignoring case, numbers in natural order) — then two checkboxes, both
  on by default: **Keep unseen finished threads at the top** and **Keep threads that need you
  at the top**. The order applies inside each group, in *Pinned*, and in the plain list.
  Floated threads sit at the top of their group — those that need the user first, then the
  unseen ones, each in the chosen order — and drop back once answered or seen. While the
  pointer is over the list (or a row's menu is open) a reorder waits until it leaves, so a
  row never jumps away under the pointer. The choice is remembered like the grouping.
- **There is no *Needs you* section** (D116): what needs the user floats to the top of its
  group and carries the red exclamation, and the **Needs-you button** (D117) — the red
  exclamation with a count, beside the back and forward buttons, shown only while something
  needs the user — opens the next such thread on each press (Ctrl+J too), cycling through
  the ones still unresolved. Its tooltip: "Next thread that needs you (2)". A blocked muted
  thread is listed in its group, not under *Muted*, until it is answered.
- **Pinned** is the first section, with the header "Pinned · 1": threads the user pinned,
  whatever their project. A pinned thread is listed here instead of under its project. Hidden
  when empty.
- **A project header shows the name the user gave the project** — "Billing API", not a path.
- **A temporary project** — a folder that has discovered threads but is not a project yet —
  has its header in *italics*, with a small dashed-circle mark, showing the folder path
  (`~/projects/abc/123`). Right-clicking that header opens a one-item menu,
  **Create project…**.
- **Muted** follows the projects, collapsed to its header — "Muted · 4" — and expands on click.
- **Archived** is the last section, under *Muted*, also collapsed — "Archived · 12". Archiving
  a thread takes it out of *Pinned* and *Muted*: it is only ever listed here.
- **Right-clicking a row** opens the thread menu (D109), in the shared menu style: **Copy
  thread ID**, **Rename** — hairline — **Fork**, **Pin** (or Unpin), **Mute…** (or Unmute and
  Change mute…), **Archive…** — hairline — **Delete…** in the failed colour. An archived row has
  **Unarchive** where Archive was and no Pin or Mute. An entry the harness cannot do is shown
  unavailable with its reason. While the menu is open the row **keeps its hover colour**
  (D108). The docked agents panel's rows have the same menu.
- **Archive…** asks first (D111): "Archive *billing-service*?", one filled **Archive  ⏎** and
  **Cancel  Esc**.
- **Delete…** asks first (D110): "Delete *billing-service*?", with a checkbox **"Also delete
  the session from Claude"** (the harness's display name), checked by default, shown only when
  the harness can delete its own sessions (the `threadDelete` capability); a line in secondary
  text says what will happen; **Cancel  Esc** is the focused default and **Delete** is the one
  filled button, in the failed colour. Without the checkbox (or unchecked) YAAC forgets the
  thread and its history, and the harness keeps its session. If the target's answer is lost,
  the window says the outcome is unknown and never asks again by itself.

Unless a screen says otherwise, the sidebar shows exactly this:

- `YAAC`, with the back and forward buttons beside it, then the Needs-you button with "2"; a
  search field with the ordering and grouping buttons at its right end; a **New** button.
- **Pinned · 1**
  - running mark · Migrate auth module — orchestrator · "now" / line 2: branch `main` · 1 added · 3 modified, and a
    people icon with "3"
- **api** — `local`
  - done mark · users-service · "12m" / line 2: branch `auth/users` · 4 added · 7 modified · 1 deleted · 3 not pushed
- **yaac** — `devbox` (the thread that needs the user floats to the top)
  - needs-you mark · Fix frame decoder · "2m" / line 2: branch `feature/protocol` · 2 added · 5 modified · 2 not pushed
  - idle mark · Explain the handshake · "2h" / line 2: branch `feature/protocol` (clean)
- **api** — `devbox`
  - done mark · billing-service · "9m" / line 2: branch `auth/billing` · 3 added · 6 modified · 1 not pushed
  - waiting mark · CI integration · "1m" / line 2: branch `main` (clean)
- **search** — `gpu-box`
  - needs-you mark · index-rebuild · "41m" / line 2: branch `auth/search` · 3 modified
  - running mark · search-service · "now" / line 2: branch `auth/search` · 3 modified
- **docs** — `staging`, with an offline mark and the word "offline" beside the machine
  - idle mark · Release notes · "3h" / line 2: branch `main` (clean)
- *~/scratch/spike-7* — `devbox` — a **temporary project**: italic, dashed mark
  - idle mark · Try the new parser · "1d" (not in git: one line only)
- **Muted · 4** — collapsed
- Footer: a **Settings** control, and a quiet hub status (D106): the connection icon, "↑ 3",
  "↓ 1" (drawn as arrow icons).

### The transcript: the conversation first

**What the user and the agent say to each other is the content. Everything else is activity,
and activity is quiet.**

- **Full size, primary text:** the user's messages and the agent's prose. Nothing else.
- **Activity** — reasoning, tool calls, messages to and from other agents, workers being
  started, waiting — is shown **collapsed by default, as one muted line each**: small
  secondary text, a small mark, a short description, and nothing more. No card, no fill, no
  header row, no body. For example:
  - `Thought for 4s`
  - `Ran cargo test -p yaac-protocol — passed`
  - `Edited 6 files`
  - `Started 3 workers · 2 of 3 finished`
  - `← billing-service · Migration merged. 14 tests pass. One flaky test…`
  - `→ CI integration · delivered`
  - `→ Release notes · queued, staging is offline`
- **A run of consecutive activity lines sits tight together**, so a burst of ten tool calls is
  a small quiet block between two paragraphs of prose, not ten rows competing with them. A
  long run folds further into a single line — `12 steps · 9 commands, 3 messages` — that
  expands to the individual lines.
- **Clicking an activity line expands it in place** to its detail: the command and its output,
  the full message with its *To* or *From* and its receipt, the list of workers. Expanded
  detail is still drawn in the muted style. It collapses again on a second click.
- **Something that went wrong stays one line, but is not muted**: a failed command or a failed
  delivery keeps the failed mark and colour, so it is noticed without being opened.
- **What needs the user is never collapsed**: approvals, questions, the permission prompt, and
  the budget notice are full cards. They are the only cards in the transcript.
- **In a thread an agent started**, the message that set its task is that thread's prompt, and
  is shown in full where a user message would be — labelled *From* the agent that sent it —
  because it is what this agent is answering. Everything after it follows the rules above.

Who is working with whom, and how each worker is doing, is the job of the agents panel, not of
the transcript.

### Times are always in the UI host's time zone

Every time the interface shows or asks for — "2m", "until Sat 14:30", the mute date picker, a
message's timestamp, "last seen 3h ago" — uses the **local time zone of the computer the
window is running on**. Never the target's: a server in another time zone, or one set to UTC,
does not change what the user reads or types. On the wire and on disk a time is an absolute
instant; it is converted only when drawn or entered.

### Muting for a while

Choosing **Mute** on a thread opens a small dialog, because a mute is often temporary — "not
now, but tell me again tomorrow".

- Title: "Mute *Fix frame decoder*".
- Three choices, one selected:
  - **Until I unmute it** — no end. The default.
  - **For** a number and a unit — a number field and a dropdown of *minutes*, *hours*,
    *days*. Beneath it, in secondary text, the moment that works out to: "until Sat 21 Nov,
    14:30".
  - **Until** a date and a time — a **date field** ("2026-11-21", typed or picked) with a
    **calendar button** that opens the calendar as a **dropdown** (D112), not inside the
    dialog, and an editable time of day. The calendar is the month with two arrows, weekday
    initials, and **always six week rows**, so it never changes size between months; past
    days are unavailable; picking a day fills the field and closes the dropdown.
- One line in secondary text: "The agent keeps working. If it gets blocked it still comes to
  the top of its group."
- Actions: one filled button **Mute  ⏎**, and plain-text **Cancel  Esc**.

A duration is turned into a fixed moment when the dialog is confirmed; the thread stores that
moment, not the duration. When it passes, the thread unmutes by itself and returns to its
place in the list. In the *Muted* section a timed mute shows when it ends — "until 14:30",
"until Sat" — in secondary text, and its right-click menu offers **Unmute** and **Change…**.
A time in the past cannot be chosen.

### The composer's height

- **Two lines tall when empty.**
- **Shift+Enter** inserts a line break; **Enter** sends.
- It grows with its text, one line at a time, **up to five lines**. A long line that wraps
  counts as the lines it takes up.
- **Past five lines the height stops growing** and the text scrolls inside the composer, with
  a thin scrollbar; the caret is always kept in view.
- It shrinks again as text is removed and returns to two lines after sending.
- Attachment chips and the queued-message strip sit above the text and are not counted in the
  five lines.

### Resizing and collapsing the sidebar

- **Resizable.** Dragging the hairline between the sidebar and the thread changes the
  sidebar's width, between about 220 and 480 px. The width is remembered.
- **Collapsible into a vertical strip**, about 48 px wide: by a small button at the top of the
  sidebar, by **Ctrl+B**, or by dragging the edge below the minimum width. The same controls
  expand it again; the strip remembers the width it came from.
- **The strip shows, from top to bottom:**
  - the `YAAC` mark;
  - a **+** button (new thread);
  - the **‹ ›** history buttons, **side by side on one row** (D103);
  - the **Needs-you button** under them (D117), only while something needs the user;
  - then the list itself: each **project's name written vertically** (rotated a quarter turn,
    reading bottom to top) as a quiet header — *Pinned* as a **pin icon** — followed by **one
    harness logo per thread** in that project, in the same order as the full list, with the
    thread's state as a **small badge on the logo's bottom-right corner** (running, waiting,
    done, failed, idle); when the thread needs the user, the **red needs-you exclamation
    replaces the badge**, and a thread whose turn ended unseen has a **cornflower ring**
    around its logo (D114). A **hairline** follows each project's last thread. There is **no Needs-you
    section** in the strip (D103).
  - Settings at the bottom.
- **Hovering a thread icon** shows a floating card with everything its full row shows — title,
  time, branch and git counts, workers — and clicking the icon opens the thread. The open
  thread's icon has the selected-row tint behind it.
- **The mouse wheel scrolls the strip vertically.**
- When the window is too narrow for the full sidebar, it collapses to this same strip by
  itself (this replaces the separate narrow rail described for screen 21).

### Going back and forward

Following a link from one chat to another must be undoable. Beside the `YAAC` title at the top
of the sidebar sit two small icon buttons, **back** (‹) and **forward** (›), which work like a
browser's:

- Opening a thread — from the sidebar, a link in a transcript, the agents panel, the Needs-you
  button, or search — adds it to the history. **Back** returns to where the user was; **forward**
  becomes available once they have gone back.
- Going somewhere new after going back discards the forward entries, as in a browser.
- Each entry remembers **where the user was reading**, not just which thread: going back to a
  long transcript returns to the same scroll position, and a link that landed on a particular
  message lands there again going forward.
- **Jumping several steps.** Pressing and holding either button, or right-clicking it, opens a
  small list of the places in that direction — nearest first, up to ten, each a thread title
  with its machine in secondary text. Choosing one jumps straight there; the entries skipped
  over stay in the history on the other side, exactly as if the button had been pressed that
  many times. The forward button has its list whenever there is anywhere forward to go.
- A button with nowhere to go is drawn in the unavailable style and does nothing.
- The **mouse's back and forward buttons** do the same, as do **Alt+←** and **Alt+→**.
- Settings and a new-thread draft are places too: back from Settings returns to the thread
  that was open.
- A thread that has since been deleted is skipped.

### Text can be selected and copied

Everything readable in a transcript can be selected with the mouse and copied with Ctrl+C:
user messages, agent prose, code, activity lines and their expanded detail, cards. A selection
can **run across several items** — drag from the middle of one message to the middle of
another three items down — and copies as plain text in reading order, with a blank line
between items and code blocks kept as they are. The same holds for text in the agents panel,
settings, and dialogs. Right-clicking a message also offers "Copy message".

### Code blocks and diffs

- **Every code block has a copy button**, at its top right, shown when the pointer is over the
  block (and always reachable by keyboard). It copies the code alone, without line numbers.
  It briefly reads "Copied".
- **A file change is a diff.** Collapsed, it is one activity line: `Edited src/frame.rs`
  followed by the lines added in the git-added colour and removed in the git-deleted colour —
  `+12 −3`. Expanded in place, it is a unified diff: removed lines on a faint red ground,
  added lines on a faint green ground, unchanged context between them, old and new line
  numbers in secondary text, and its own copy button. A long diff shows its first hunks and a
  "show the rest" line.
- **A turn that changed files ends with one summary line**: `Changed 3 files  +40 −12 · show
  all`, which expands every diff of that turn.

### The agents panel: a pill, a card, or a docked panel

The agents a thread is working with are shown in one of three states. **The default is the
smallest.** It appears only for a thread that has a parent, workers, or contacts, and is
absent otherwise.

**1. The pill — the default.** A narrow vertical pill floating over the right edge of the
thread, vertically centred, about 44 px wide, with fully rounded ends and a soft shadow. It
holds one small **harness icon** per related agent in **three fixed clusters** (D118) — the
parent, then workers, then contacts — separated by short hairlines, each headed by a tiny
muted glyph drawn as an icon: an **up arrow** for *spawned by*, a **branch** for *workers*,
**⇄** for *in contact with*. An empty cluster and its hairline are not drawn. No names, no
text.

- Beside an icon, a small **arrow** shows data moving: **up** while this thread is sending to
  that agent, **down** while it is receiving from it. The arrow is there only while something
  is in flight and fades when it is done. An idle agent has no arrow.
- An agent that **needs the user** shows the red exclamation on its icon's corner. That is the
  agent's own state. An agent whose turn ended unseen has a **cornflower ring** around its
  icon (D114).
- A finished agent's icon is dimmed. The pill holds at most eight slots: the largest cluster
  gives up icons first (never below one) and ends with "+N".
- The harness icon is a simple glyph per provider kind; a provider with no glyph gets its
  initial in a circle.

**2. The card — on hover or keyboard focus.** The pill widens leftwards into a floating
rounded rectangle, about 300 px wide, over the transcript; it does not push the layout. **It
must not flicker:** once open, the card stays open for as long as the pointer is anywhere
over the card *or* the strip where the pill was, and closes only after the pointer has been
outside both for a short moment (about a quarter of a second). Opening and closing never
depend on the pill's own shrinking and growing outline. Each
icon becomes a row: the icon, the agent's **name**, and beneath it one line of secondary text
with **the last thing it did**, cut off with an ellipsis — "Rebuilding the search index
(68%)…", "Merged to main. 14 tests pass.", "Waiting for your approval". The arrows stay at the
right of each row. The rows sit under the same three quiet headings as the docked panel —
**Spawned by**, **Workers**, **In contact with** (D118) — and an unseen agent's row has the
cornflower outline. The card is titled **Related agents**. Clicking a row opens that thread.
Moving the pointer away shrinks it back to the pill. At the top right of the card is a small
**dock** button. The card's and the pill's hover regions follow their real heights.

**3. Docked.** The user pins it to the side. It becomes a fixed panel on the right, like a
sidebar, titled **Related agents** (D115), and the thread narrows to make room. Each row: the
state mark, the title, and the state word at the right; then, as in the sidebar, the harness
logo, branch, git counts and ahead / behind; then provider · model · target and the latest
activity. When the panel is narrow a row **ellipsises its title (and branch) and keeps every
mark**. A row keeps its hover colour while its menu (the thread menu) is open, and has the
cornflower outline when its turn ended unseen. It shows the rows under three quiet
headings — **Spawned by**, **Workers**, **In contact with** — with the orchestration's budget
in small secondary text at the top ("14 of 100 agent turns · 4 of 20 threads"). It is drawn in
the same muted style as everything else: no boxes around sections, no rules, rows as plain
lines, names as links. An **undock** button returns it to the pill. The choice is remembered.

In every state a row or icon is a link to that thread.

### The control row

Shows only what can actually be changed right now. In a draft: no Fork, no context meter. When
a thread is read-only: no selectors at all. It is a line of plain-text selectors, each a word
and a chevron, with no boxes around them.

On a thread a person started, the row includes **agents: ask ▾** (or *allow*): whether this
chat's agent may start and message other threads without asking. A new chat starts on *ask*.
A worker thread has no such control — it acts under its root's answer.

### Links between agents

A thread name in a **To** or **From** line, a workers card, the agents panel, an ancestry
line, or the Needs-you list is a link. **A link is not coloured.** It is the same colour as
the text around it, set slightly bolder, with no underline; the underline appears only while
the pointer is over it or it has keyboard focus. A link should be findable, not loud. (Rows of the thread list are selectable
rows in primary text, not links.)

- **Delivered:** following a *To* opens the other thread at the matching *From*, and back.
- **Queued:** the name still opens the other thread; there is no matching item yet, and the
  receipt stays visible saying why it is waiting.
- **Failed because the recipient no longer exists:** the name is plain text, not a link, and
  the permanent reason sits beside it. Nothing pretends to lead somewhere.

### Keyboard and focus

- Primary actions show their key: "Allow once  ⏎", "Deny  Esc".
- **Those keys act only while that card has focus.** Enter in the composer sends a message; it
  never approves something further up the page.
- **A request that arrives while the user is typing does not take focus.** It is announced —
  the Needs-you count changes and a one-line notice appears above the composer, "index-rebuild
  needs an approval · Ctrl+J to go there" — and the caret stays where it was.
- **Ctrl+J** opens the Needs-you list from anywhere; choosing an entry opens that thread,
  scrolls to the request, and focuses its card.
- One element per screen carries a visible focus ring (a 2 px accent outline).

### A request held by another program

Wherever a request is listed — the Needs-you section, *Since you left*, the agents panel — one
that belongs to a thread in use elsewhere is labelled "answer in the other program" and has no
buttons. It is never offered as something the user can act on here.

## The fleet used in every mockup

One story, so screens agree with each other.

| Thread | Agent | Where | State |
| --- | --- | --- | --- |
| **Migrate auth module — orchestrator** | Codex · gpt-6.2-sol | local · api · main | running; the root, started by the user |
| users-service | Claude · opus-5.5 | local · api · main | done — worker of the orchestrator |
| billing-service | Claude · opus-5.5 | devbox · api · main | done — worker of the orchestrator |
| search-service | Claude · opus-5.5 | gpu-box · search · main | running — worker of the orchestrator |
| index-rebuild | Claude · opus-5.5 | gpu-box · search · main | **needs you** — worker of search-service |
| CI integration | Codex · gpt-6.2-sol | devbox · api · main | waiting — a contact, not a worker |
| Release notes | Codex · gpt-6.2-sol | staging | unreachable — staging is offline |
| **Fix frame decoder** | Codex · work · gpt-6.2-sol | devbox · yaac · feature/protocol | **needs you** — unrelated to the migration |

## Screens

Each screen is drawn in dark and in light.

### 01 — A thread that needs an approval

*The everyday screen. Must answer at a glance: what is this agent doing, and does it need me?*

- Sidebar as standard; **Fix frame decoder** selected.
- Header: needs-you mark, "Fix frame decoder", a tag "Needs you — approval", breadcrumb
  `devbox · yaac · feature/protocol`. No "Running" anywhere.
- Transcript: user message "Why does decode() drop the last frame?" with an attachment chip
  `decoder-log.txt`; two muted activity lines, tight together — "Thought for 4s" and
  "Ran cargo test -p yaac-protocol — passed"; then the agent's prose at full size: "The length
  prefix counts the kind byte, so the reader stops one byte early."; a six-line Rust code
  block.
- **Approval card** (amber left edge, focus ring on the first button): title "Run command",
  `cargo fix --allow-dirty` in monospace, the directory `/home/dev/yaac`, and the buttons the
  harness offers: **Allow once  ⏎**, **Allow for this thread**, **Deny  Esc**.
- Composer: placeholder "Message Codex…", dimmed, hint "Answer the approval above to continue".
- Control row: `Codex · work ▸ gpt-6.2-sol ▾` · `effort: high ▾` · `mode: ask ▾` ·
  `agents: ask ▾` · context meter "38%" · **Fork**.
- Agents panel: hidden (this thread has no relationships).

### 02 — The orchestrator: agents talking to agents

*The reason YAAC exists. Must make it obvious who is talking to whom, and how far along
everyone is, without opening five windows.*

- Sidebar as standard; **Migrate auth module — orchestrator** selected.
- Header: running mark, title, the word "Running", breadcrumb `local · api · main`.
- Transcript — the conversation is what stands out; the machinery is five quiet lines:
  1. User, full size: "Split this migration across three Opus workers, one per service. Tell
     the Codex thread on devbox when billing is merged, and send release notes to staging."
  2. Agent prose, full size: "I'll run one worker per service and keep CI and the release
     notes thread informed."
  3. A tight run of muted activity lines:
     `Started 3 workers · 2 of 3 finished`
     `← billing-service · Migration merged. 14 tests pass. One flaky test…`
     `→ CI integration · delivered`
     `→ Release notes · queued, staging is offline`
     `Waiting for search-service…`
  4. Agent prose, full size: "users-service and billing-service are merged. One flaky test
     was skipped in billing: `test_refund_race`. I'm waiting on search-service."
  One of the activity lines — the one from billing-service — is shown **expanded** to
  demonstrate: beneath it, still in the muted style and indented slightly, "From
  **billing-service** · Claude · devbox" and the full text "Migration merged. 14 tests pass.
  One flaky test skipped: `test_refund_race`."
- Composer: "Message Codex…"; a **Stop** button.
- Control row: `Codex ▸ gpt-6.2-sol ▾` · `effort: high ▾` · `mode: ask ▾` · `agents: allow ▾`
  · "38%" · **Fork**.
- **Agents panel, docked** (this is the docked state; the default is the pill — screens 27
  and 28). Header "Agents", an undock button, and the budget line
  "14 of 100 agent turns · 4 of 20 threads".
  - **Spawned by** — "started by you".
  - **Workers (3)** — a tree: users-service done; billing-service done; search-service
    running, with a child row beneath it: **index-rebuild — needs you · approval**.
  - **In contact with (2)** — CI integration · Codex · devbox · waiting;
    Release notes · Codex · staging · offline.

### 03 — The same conversation from a worker's side

*A worker is a full thread. Must show where its task came from and that its answer went back.*

- Sidebar as standard; **billing-service** selected.
- Header: done mark, "billing-service", breadcrumb `devbox · api · main`, and a link line:
  "Spawned by **Migrate auth module — orchestrator** ↗ on local".
- Transcript:
  1. The task that started this thread, **in full**, where a user message would be, labelled
     "From **Migrate auth module — orchestrator** · Codex · local": "Migrate the billing
     service to the new auth module. Report back when it is merged."
  2. A tight run of muted activity lines: `Thought for 9s`, `Edited 6 files`,
     `Ran cargo test -p billing — 14 passed`.
  3. Agent prose, full size: "Merged to main. One flaky test was skipped."
  4. One muted activity line: `→ Migrate auth module — orchestrator · delivered`
- Composer enabled: "Message Claude…" — the user can continue a worker by hand.
- Agents panel open: **Spawned by** — Migrate auth module — orchestrator · Codex · local ·
  running; **Workers (0)**; **In contact with (0)**.

### 04 — A blocked worker, two levels down

*The hardest case: the thing that needs you is a grandchild on another machine. Must be
reachable in one click and say where it sits.*

- Sidebar as standard; **index-rebuild** selected (it is also at the top of its group, with the needs-you mark).
- Header: needs-you mark, "index-rebuild", tag "Needs you — approval", breadcrumb
  `gpu-box · search · main`, and an ancestry line of links:
  "Migrate auth module — orchestrator › search-service › **index-rebuild**".
- Transcript: the task that started this thread, in full, labelled "From **search-service** ·
  Claude · gpu-box": "Rebuild the search index against the new auth tables."; one muted
  activity line "Ran ./scripts/reindex.sh --dry-run — 3 shards would be dropped"; then an
  **approval card** — the only card on the screen: "Run command",
  `./scripts/reindex.sh --drop-old`, directory `/srv/search`, a line "Started by an agent, not
  by you", buttons **Allow once  ⏎**, **Deny  Esc**.
- Agents panel open: **Spawned by** — search-service · Claude · gpu-box · running; above it in
  small text "root: Migrate auth module — orchestrator".

### 05 — Every state a request can be in

*A request is not always answerable. Each outcome must be plain, and none may invite a blind
retry.*

Main shell, **Fix frame decoder** open, the transcript showing six request cards stacked, each
labelled with its state:

1. **Pending** — a question card: chip "Approach", "Which fix do you want?", radio options
   "Patch the decoder", "Rewrite the reader", "Add a compatibility shim", a "Something else…"
   text field; a second question chip "Scope" with checkboxes `yaac-protocol`,
   `yaac-transport`, `yaac-core`; buttons **Send answers  ⏎** and a quiet text link
   "Answer in my own words".
2. **Submitted** — the same card collapsed: "You chose: Patch the decoder · yaac-protocol",
   a spinner mark, "Sent — waiting for the agent to confirm". A second small example beside
   it for a secret question: "You answered: ••••••••" — masked here too.
3. **Confirmed** — a done mark: "**Answered by you** · Patch the decoder · yaac-protocol ·
   confirmed 12s ago". This is the normal end of a request. Beside it, the secret example:
   "Answered by you · •••••••• · confirmed".
4. **Resolved elsewhere** — "Allowed in another program on devbox · 2m ago". No buttons.
5. **Expired** — "No longer needed — the turn ended before this was answered." No buttons.
   A secret answer is masked **while it is typed and in every summary afterwards** — pending,
   submitted, confirmed. It is never shown in clear, and never cached on the host.

   When the agent acknowledges an answer, the marks are **recomputed from the requests still
   open**: if none remain on this thread its header returns to "Running", its row leaves
   *Needs you*, the composer is enabled again, and each ancestor and the agents rail drop the
   needs-you mark unless another descendant still needs the user.
6. **Outcome unknown** — an amber note: "YAAC lost contact while sending your answer. It may
   or may not have arrived." One button: **Check latest status**. Beneath, in secondary text:
   "Your answer will not be sent again." No retry button.

### 06 — (removed)

*"Since you left" was removed by the owner in revision 5. The **Needs you** list is where
waiting work is found. The number is left unused so the other screens keep theirs.*

### 07 — First run

*Nothing is configured. Must get to a first message in under a minute, with one obvious next
step.*

- Main shell with an **empty** sidebar (just `YAAC`, a disabled search field, and Settings /
  the connection icon in the footer). No composer. No agents panel.
- Centre: "YAAC", the sentence "Run your coding agents on any machine, and let them work
  together.", and three cards:
  - **This computer** — "Codex — ready" with a check; "Claude — not signed in" with a warning
    mark, the hint "Run `claude` in a terminal to sign in" (the command in monospace, no
    literal backticks), and a small **Refresh** button beside it that re-checks this computer.
    One primary button: **Start a thread  ⏎** (the only filled button on the screen).
  - **Add a server** — "Run agents on another machine over SSH." Button **Add server…**
  - **Turn on WSL** — "Run agents inside a Linux distro on this PC." Button **Turn on…**,
    and a small note "Windows only".
- Footer line: "Agents keep running when you close this window."

### 08 — A new thread, before the first message

*Must make three choices feel like one: which machine, which folder, which model.*

- **This is the first thread ever, straight from screen 07.** The sidebar is that of a fresh
  install: `YAAC`, search, **New**, the three toggle chips, **no Needs-you section**, and a
  tree with only `local — connected`; the draft is not listed (D107). Footer: Settings, the
  connection icon and "↑ 1".
- Header: "New thread", breadcrumb `local · api · main`.
- Centre: empty, one hint line: "Describe what you want done."
- A **draft bar** above the composer with one selector: `Project: api — local ▾`. The
  thread will start in the root of that project's folder; the user picks a project, never a
  path.
- The **model picker is open**, attached directly above its trigger in the control row,
  grouped by provider instance:
  - **Codex** — gpt-6.2-sol (ticked, "default"), gpt-6.2-astra, gpt-6.1-terra
  - **Claude** — "not signed in on local" (the whole group disabled, with the reason)
  Each enabled row: a small image icon when the model reads images, and "effort: low – high".
  Footer of the menu: "Read from local · updated just now".
- Composer with text typed, **markdown styled in place** with its syntax still visible:
  `# Investigate frame parsing` as a heading; "Reproduce the `**last-frame**` drop" with the
  word bold and the asterisks shown; `decode()` as inline code. A **Send  ⏎** button.
- Control row: `Codex ▸ gpt-6.2-sol ▾` (matching the tick) · `effort: high ▾` · `mode: ask ▾`.
  **No Fork. No context meter.**
- Agents panel hidden.

### 09 — The first message, sent

*The outcome of 07 and 08: the draft became a real thread.*

- Sidebar, still the fresh install: no Needs-you section; under `local › api › main` the
  first and only thread, selected: running mark · **Investigate frame parsing**. Footer: the
  connection icon and "↑ 1".
- Header: running mark, "Investigate frame parsing", tag "Running", breadcrumb
  `local · api · main`.
- Transcript: the user's message rendered as markdown (heading, bold, inline code); the agent:
  collapsed "Thinking…" with a spinner; a tool line "Reading `src/frame.rs`"; prose streaming,
  ending mid-sentence with a cursor.
- Composer: "Message Codex…", a **Stop** button.
- Control row: `Codex ▸ gpt-6.2-sol ▾` · `effort: high ▾` · `mode: ask ▾` · "4%" · **Fork**.

### 10 — In use elsewhere

*Must explain itself in one line and never look actionable.*

- Sidebar as standard; **Explain the handshake** selected, with a small lock mark.
- Header: a lock mark, "Explain the handshake", tag "Read-only", breadcrumb.
- A banner under the header: "This thread is open in another program on devbox. You can
  watch, but not send." and one button: **Fork to continue here**.
- Transcript live and normal, including an approval card with **no buttons**: "Waiting for an
  answer in the other program."
- **No composer input, no Send, no Stop.** In its place a single quiet line: "Held by a
  terminal session on devbox since 14:02."
- **No control row selectors.** No second Fork.

### 11 — Fork

*Must say what will be carried over, to whom, and why it is a handoff.*

- A dialog centred over the main shell, the background clearly dimmed by the scrim.
- Title "Fork this thread".
- "From": "Fix frame decoder · Codex · work · gpt-6.2-sol", "at the last completed turn
  (turn 12)".
- "Continue with": a model selector showing `Claude ▸ opus-5.5`; beneath it
  "Instance: Claude on devbox · account: work@example.com".
- A **result line, not a choice**: "**Handoff** — the destination is a different provider, so
  the conversation is passed as a transcript file." And beneath it in **secondary text** (it
  is a reason the user must be able to read): "Native fork is only available within the same
  provider and store."
- "Carried over: 12 turns · 9,400 words · 3 files still on the target."
  "Not carried over: hidden reasoning · 2 images no longer on the target."
- A notice with an info mark: "This sends the conversation to Anthropic (work@example.com)."
- A collapsible "Preview transcript" showing a few lines.
- Buttons: **Cancel  Esc**, **Fork  ⏎**.

### 12 — The forked thread

*The outcome of 11.*

- Sidebar: under `devbox › yaac › feature/protocol`, a new selected row:
  idle mark · **Fix frame decoder (fork)** · a small fork mark.
- Header: "Fix frame decoder (fork)", breadcrumb, and a link line:
  "Forked from **Fix frame decoder** ↗ at turn 12 · handoff".
- Transcript: the inherited history in a slightly dimmed, read-only style (the user question
  and the agent's answer), then a full-width divider labelled
  "Inherited from Fix frame decoder — read-only above this line", then empty space.
- Composer enabled: "Message Claude…", **Send  ⏎**.
- Control row: `Claude ▸ opus-5.5 ▾` · `effort: high ▾` · `mode: ask ▾` · "6%" · **Fork**.

### 13 — Settings: targets

*Must show, per machine, whether YAAC can reach it and what YAAC changed there.*

- The settings shell: "← Back to threads" top-left; left navigation **Targets** (selected),
  Providers, Appearance, Window, Advanced.
- Page title "Targets". Buttons top-right: **Add SSH server**, **Import from SSH config**.
- **Local** — "This computer" — enabled switch.
- **SSH** — rows, each with a state line and its own actions:
  - `devbox` — done mark — "Ready — set up by YAAC on 2026-11-20" — switch on — buttons
    **Revoke**, **Remove**.
  - `gpu-box` — done mark — "Ready — access was already in place" — switch on — **Revoke**
    disabled with the visible note "YAAC installed nothing on this server" — **Remove**.
  - `staging` — offline mark — "Offline — last seen 3h ago · set up by YAAC on 2026-11-02" —
    switch on — **Retry**, **Revoke**, **Remove**. Revoke stays available while offline,
    because YAAC installed a key there; it leads to "Could not revoke" on screen 16 if the
    server still cannot be reached.
  - `legacy` — dashed mark — "Not set up" — **Set up…**, **Remove**.
- A one-line legend: "Disable keeps the connection but stops using it. Remove forgets it.
  Revoke removes the key YAAC installed on the server."
- **WSL** — master switch on. Rows: `Ubuntu` · user "(default)" · on; `Ubuntu` · user `deploy`
  · on · note "second session to this distro"; `Debian` · off. An add row: distro dropdown,
  "User (optional)" field, **Add**.

### 14 — SSH setup: confirm the server

*The first contact with a new server.*

- Settings shell, Targets. A panel "Add SSH server".
- Fields filled: Name `legacy`, Host `legacy.example.com`, Port `22`, User `deploy`.
- A step list: "Reached legacy.example.com" — done mark. "Confirm this is the right server" —
  current step.
- A box: "This server's key fingerprint", `SHA256:nThbg6kXUpJWGl7E1IGOCspRomTxdCARLviKw6E5SY8`
  in monospace, "ED25519", and the line "Check it against what your server administrator gave
  you."
- Buttons: **Cancel  Esc**, **This is the right server  ⏎**.

### 15 — SSH setup: password, key install, verified

*The same panel, further on. Each step is visible; the result is unambiguous.*

- Step list with marks:
  "Reached legacy.example.com" done; "Server confirmed" done;
  "Password-less login — not available yet" (hollow mark);
  "Signed in with your password" done;
  "Installed YAAC's key for deploy" done;
  "Verified: signed in with the key alone" done.
- A result block with a done mark: "**Ready.** YAAC can now connect to legacy without a
  password. Your password was not saved."
- Beneath, in secondary text: "YAAC added one line to `~/.ssh/authorized_keys` on the server,
  marked `yaac:laptop:legacy`. Revoke removes exactly that line."
- Buttons: **Done  ⏎**, and a quiet **Revoke**.
- (Above the result, greyed as completed: the choice "Password / Key file" with Password
  selected.)

### 16 — SSH setup: when it fails, and after a revoke

*Failure is recoverable and claims nothing it has not checked. Revoke has two endings, and
both are honest.*

Three panels in the settings shell:

- "Could not finish" — steps "Signed in with your password" done; "Installed YAAC's key for
  deploy" done; "Verified: signed in with the key alone" **failed** — "This server does not
  accept key logins." Then, with a done mark: "Removed the key again — confirmed: nothing of
  YAAC's is left on the server." Buttons: **Try again** (returns to the sign-in step, screen
  22) and **Close**.
- "Revoked" — the row for `devbox` reads dashed mark — "Not set up — YAAC's key was removed on
  2026-11-20" — button **Set up…**. A note: "Only YAAC's own key was removed. Your other keys
  were not touched."
- "Could not revoke" — a warning mark: "**staging could not be reached**, so YAAC's key is
  still on that server." Then: "YAAC has kept its copy of the key so it can finish this later."
  A monospace box titled "To remove it by hand, delete this line from
  `~/.ssh/authorized_keys` on staging:" containing the **complete** installed line, wrapped,
  never abbreviated:
  `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFq4mW1nJt0Xc8uPz3yKb7Vd2Lh9sRgE5oTa6QwNiYk3 yaac:laptop:staging`,
  with a **Copy** button that copies the whole line. Buttons: **Try again**, and
  **I removed it myself** — which records that the user confirmed it, deletes YAAC's local
  copy of the key, and returns the row to "Not set up — removed by hand on 2026-11-20".

### 17 — Settings: providers

*Per machine: what is installed, whether it works, and where it came from.*

- Settings shell; **Providers** selected. A target selector: `devbox ▾`.
- Instances, each a row with a mark, version, backend, and a note:
  - **Codex** — done mark — "Ready" · "daemon 0.171" · "found running" · "12 models" ·
    **Refresh**.
  - **Codex · work** — done mark — "Ready" · "daemon 0.171" · socket
    `/run/user/1000/codex-work/control.sock` · "started by YAAC" · **Edit**, **Remove**.
  - **Claude** — done mark — "Ready" · "2.1.329" · "SDK" · "account: work@example.com".
  - Each instance row ends with a small switch: "Offer YAAC tools to sessions started outside
    YAAC" — off — with the hint "edits this harness's own configuration".
- An "Add instance" form, **empty**: Kind dropdown "Codex", Name field with placeholder
  "optional", Socket path field with placeholder "/path/to/control.sock", **Add** (disabled).
- A second example row, on a different target, shown beneath a divider "local":
  **Claude** — warning mark — "Not signed in — run `claude` in a terminal on this computer".

### 18 — (removed)

*The Agents settings tab was removed by the owner in revision 5. Budgets are set in the
permission prompt (screen 25); the tools switch for outside sessions moved to each provider
instance on screen 17.*

### 19 — Notices

*Each problem says what happened, what it means, and the one thing to do next.*

Main shell with **Fix frame decoder** open, plus:

- Sidebar: `devbox` carries a small warning mark; `staging` reads "offline · retrying in 20s"
  with a **Retry** link (in both themes); `wsl: Ubuntu` reads "not running".
- A banner under the header: "devbox is running an older YAAC daemon (0.7). It will be updated
  when idle." with **Update now**.
- In the transcript, an attachment chip in an older message, greyed, with a broken-file mark:
  "screenshot.png — no longer on the target".
- In the composer, two chips: an image chip with a warning mark, "diagram.png — this model
  cannot see images; it will be sent as a file"; a file chip "trace.log" with a progress bar
  "62%".
- A notification card bottom-right: "WSL distro Ubuntu is not running." with one action,
  **Start it**. A thread on that distro, when open, is readable but disabled: no composer, no
  control row, a single line "Ubuntu is not running · Start it" in its place.

### 20 — The budget ran out

*Runaway work stops by pausing, not by being killed, and one click resumes it.*

- Main shell, the orchestrator open.
- Header tag: "Paused — budget reached".
- The transcript is as in 02 (prose at full size, activity as muted lines). At its end, a
  notice card with a pause mark — a card, because it needs the user: "**This orchestration reached
  its budget**: 100 of 100 agent-started turns. Nothing was cancelled. 2 messages are waiting
  to be delivered." Buttons: **Continue (+100 turns)  ⏎**, **Review workers**, and a quiet
  link "Change budgets".
- Agents panel header: the budget line in the needs-you colour with a pause mark:
  "100 of 100 agent turns · 7 of 20 threads".
- Sidebar: the orchestrator row shows a pause mark at the top of *Pinned*, and the Needs-you
  button counts it ("3").

### 21 — A narrow window

*At the smallest supported width (960 px) nothing is lost; panels fold away, and what needs
you is still one keystroke off.*

- The same thread as 02, at 960 × 720.
- The sidebar is collapsed to a 56 px rail: at the top a Needs-you badge "2"; below it a
  single "threads" button that opens the full list as an overlay; Settings at the bottom.
- **The Needs-you list is open**, as a popover attached to the badge, with the first entry
  focused: "index-rebuild — approval · gpu-box" and "Fix frame decoder — approval · devbox".
  Footer of the popover: "↑ ↓ to move · ⏎ to open · Ctrl+J".
- The agents panel is collapsed to a rail showing "5".
- The transcript uses the full remaining width; message blocks and the workers card reflow
  without truncating names. The control row wraps to two lines.

### 22 — SSH setup: signing in

*The step between confirming the server (14) and the result (15).*

- Settings shell, Targets, the "Add SSH server" panel for `legacy`.
- Step list: "Reached legacy.example.com" done; "Server confirmed" done;
  "Tried signing in without a password — not available" with a hollow mark; then the current
  step, "Sign in once so YAAC can set up password-less access".
- A choice: **Password** (selected) / **Key file**. A password field, masked, with a
  show/hide control; beneath it in secondary text: "Used once. YAAC does not save it."
- Beneath, as the unselected alternative: a "Key file" chooser and "Passphrase, if the key has
  one", in the unavailable-control style.
- Buttons: **Cancel  Esc**, **Connect  ⏎**.
- A one-line preview of what happens next: "YAAC will sign in, install its own key for
  deploy, and check that the key works."

### 23 — The queued message arrives, and one that never will

*The outcome of the queued message in 02, seen from the sender.*

- Main shell, the orchestrator open, later in the same transcript. Sidebar as standard, except
  `staging` now reads "SSH · connected".
- Transcript, later in the same thread — muted activity lines between two pieces of prose:
  `staging reconnected · 3h 12m after it went offline`
  `→ Release notes · delivered, after 3h 12m queued`
  `← Release notes · Draft is in docs/release/auth-migration.md.`
  and one line that is **not** muted, with the failed mark and colour:
  `→ Old changelog bot · failed — that thread was deleted on staging`
  That failed line is shown **expanded**: "To Old changelog bot · Codex · staging" with the
  name in plain text, not a link; "Archive the 1.x notes."; and "**Failed** — that thread was
  deleted on staging. The message was not delivered."
  Then agent prose, full size: "The release notes draft is ready. I could not reach the old
  changelog bot — that thread no longer exists."
- Agents panel: **In contact with (2)** — CI integration · waiting; Release notes · Codex ·
  staging · done.

### 24 — The same message at its destination

*The other end of 23: following a "To" lands on the matching "From".*

- Sidebar as standard with staging connected; under `staging › docs › main` the selected row:
  done mark · **Release notes**.
- Header: done mark, "Release notes", breadcrumb `staging · docs · main`.
- Transcript:
  1. User, full size (this thread was started by a person): "Keep the release notes current."
  2. One activity line, **expanded and highlighted** because it is the item just navigated to:
     `← Migrate auth module — orchestrator · Draft the notes for the auth migration.` — and
     beneath it "From **Migrate auth module — orchestrator** · Codex · local", the full text,
     and "Sent 3h 14m ago · arrived when staging reconnected".
  3. Muted activity lines: `Thought for 6s`, `Wrote docs/release/auth-migration.md`.
  4. Agent prose, full size: "Drafted the release notes."
  5. One muted activity line: `→ Migrate auth module — orchestrator · delivered`
- Agents panel open: **Spawned by** — "started by you"; **Workers (0)**;
  **In contact with (1)** — Migrate auth module — orchestrator · Codex · local · running.
- Composer enabled: "Message Codex…".

### 25 — An agent asks to work with other agents

*The chat is set to "agents: ask". Must show what the agent wants and let the budget be set in
the same place, in one decision.*

- Main shell, the orchestrator open, at the start of its story. Sidebar as standard, except
  the orchestrator floats to the top of *Pinned* with the needs-you mark, and the Needs-you
  button shows "3".
- Header: needs-you mark, title, the words "Needs you — permission".
- Transcript: the user's message; the agent's line "I'll split this across three workers.";
  then a **permission card** (faint fill, amber left edge):
  - Title: "This agent wants to work with other agents"
  - "It will:" then two plain lines —
    "Start 3 threads — Claude · opus-5.5 on local, devbox, and gpu-box";
    "Message CI integration · Codex · devbox".
  - "Budget for this orchestration" — three small number fields in a row:
    "Agent turns 100", "Threads 20", "Hours 12"; beneath in secondary text:
    "When a budget runs out, work pauses and asks you. Nothing is cancelled."
  - Actions: one filled button **Allow  ⏎** (with the focus ring), then as plain text
    **Deny  Esc** and "Always allow in this chat".
- Composer dimmed with the hint "Answer above to continue".
- Control row: `Codex ▸ gpt-6.2-sol ▾` · `effort: high ▾` · `mode: ask ▾` · `agents: ask ▾`
  — the agents selector open as a small two-item menu: "Ask for permission" (ticked) and
  "Allow without asking".
- Agents panel hidden (no workers yet).

### 26 — Muting and archiving

*Too many agents to watch. Must let the list be cut down to the few that matter, without
stopping anything.*

- Main shell, **Fix frame decoder** open. In the sidebar, the row for **billing-service** has
  a small **context menu** open beside it: "Mute", "Archive", "Rename", "Fork".
- The **Muted** section at the foot of the list is **expanded**: header "Muted · 4", then four
  one-line rows in secondary text: running mark · search-service; done mark · users-service;
  idle mark · Old spike; done mark · Lint pass. One of them is running — a muted thread keeps
  working. (Because they are muted, search-service and users-service do not also appear in
  their project sections above.)
- index-rebuild, muted but blocked, stays in its group at the top until answered. A muted
  thread that is blocked is never hidden.
- The grouping menu is open from the search field, showing three checkboxes — Machine,
  Project, Worktree, all ticked — and, beneath a gap, a switch "Show archived", off.

### 27 — The agents pill

*The default. A thread working with five other agents, and the screen is still calm.*

- Main shell, the orchestrator open, exactly as in 02 **except the agents panel is not
  docked**: the thread uses the full width.
- Floating over the right edge of the transcript, vertically centred: the **pill**, in two
  clusters (D118): a branch glyph over the workers' Claude icons, a hairline, then a ⇄ glyph
  over the two contacts' Codex icons. The first two Claude icons are dimmed (finished). The third has a small **down
  arrow** beside it (receiving from search-service). The first Codex icon has a small **up
  arrow** (sending to CI integration). The last Codex icon has the offline dashed mark.
- Nothing else is different. No names, no labels on the pill.

### 28 — The agents card, on hover

*The pointer is over the pill.*

- The same screen as 27, with the pill widened leftwards into the floating **card**, over the
  transcript, with a soft shadow. Its title "Related agents" and a small dock button at its
  top right.
- The rows under the headings **Workers** and **In contact with** (D118), each: harness icon,
  name, and one line of secondary text beneath, ellipsised:
  - users-service — "Merged to main. 14 tests pass."
  - billing-service — "Merged to main. One flaky test skipped…"
  - search-service — "Rebuilding the search index (68%)…" — down arrow at the right
  - CI integration — "Resuming the integration tests…" — up arrow at the right
  - Release notes — "Offline — message queued"
- One row, search-service, is highlighted as the row under the pointer.

### 29 — Creating a project from a discovered folder

*A thread was found in a folder YAAC knows nothing about. Must turn that folder — or, more
often, one of its parents — into a named project in a few seconds.*

- Main shell. In the sidebar, the temporary project *~/scratch/spike-7* is highlighted, as the
  thing that was right-clicked.
- A small dialog, centred, the background dimmed: title "Create project".
  - "Machine": `devbox` — plain text, not editable here.
  - "Folder": a text field containing `/home/dev/scratch/spike-7/` with the caret at the end,
    and a quiet **Browse…** beside it. Beneath, in secondary text: "Prefilled from the thread's
    folder. Shorten it if the project is a parent folder."
  - A one-line suggestion as a link: "Use the repository root: /home/dev/scratch".
  - "Name": a text field containing `spike-7`, with beneath it "Shown in the sidebar instead
    of the folder name."
  - A line in secondary text: "3 threads in this folder will move into the project."
  - Actions: one filled button **Create project  ⏎**, and plain-text **Cancel  Esc**.

### 30 — Confirming a mode that bypasses approvals

*The one choice that removes the safety net. Must be deliberate, and impossible to make by
reflex.*

- Main shell, a thread open; a small dialog centred over a dimmed background.
- Title, with a warning icon: "Let this agent act without asking?"
- Body, primary text: "In **bypass permissions** mode the agent runs commands and changes files
  without asking you first." Then three plain lines in secondary text: "Thread: Fix frame
  decoder", "Machine: devbox", "Folder: /home/dev/yaac".
- A last line in secondary text: "This applies to this thread only. You can switch back at any
  time."
- Actions: **Cancel  Esc** has the focus ring and is the default; the confirming action is the
  screen's one filled button, in the failed colour: **Bypass permissions**. Enter does not
  confirm.
- Behind the dialog, the control row's mode selector is open on the mode being chosen.

### 37–40 — Attachments in the composer

*Every chip state, in the composer of billing-service on devbox.*

- **37** — two chips above the text: `trace.log` with a thin progress bar and "62%", and an
  image chip `plot.png` with its thumbnail. Under the text, "Waiting for 1 upload"; **Send** is
  plain until the upload finishes. A paper clip beside Send opens the file picker.
- **38** — the model cannot see images: `diagram.png` with a warning mark, "this model cannot
  see images; it will be sent as a file"; and `build-cache.tar`, which failed: the hub's
  message in the failed colour and **Retry**. Under the text: "build-cache.tar was not
  uploaded — remove it or retry".
- **39** — the model's image support is unknown: `sketch.png` asks "send as image · file";
  "Choose how to send sketch.png" under the text.
- **40** — files dragged over the window: the thread view dims and shows a quiet faint panel
  with a paper clip and "Drop to attach".

### 41 — A context menu

*One menu style everywhere: context menus, dropdowns, pickers, the history list.*

- A sidebar row's right-click menu (D109): **Copy thread ID, Rename** — hairline — **Fork,
  Pin, Mute…, Archive…** — hairline — **Delete…** in the failed colour. The row it belongs to,
  billing-service, keeps its hover tint while the menu is open (D108). The menu is the raised
  surface with a soft shadow, a small corner radius and a little inner padding.
  Items are full-width rows with no fill of their own and no gaps; the hovered row
  (**Mute…**) has the hovered-sidebar-row tint, a pressed one is a little stronger. Disabled
  items are in the unavailable colour with no hover; destructive items in the failed colour;
  separators are hairlines; a shortcut, when an item has one, is right-aligned in the
  secondary colour (the transcript's **Copy  Ctrl+C**).

### 42 — Deleting a thread

*Deleting must be deliberate, and must say whether the harness's own session goes too.*

- Main shell, Fix frame decoder open; the shell dimmed by the scrim.
- A dialog: "Delete billing-service?"; a checkbox, ticked, **"Also delete the session from
  Claude"** (shown because Claude can delete its sessions); in secondary text "The thread and
  its session in the harness are deleted. This cannot be undone."; **Cancel  Esc** and a
  filled **Delete** in the failed colour (D110).

### 43 — Archiving asks first

- The same shell; a dialog "Archive billing-service?", "It moves to Archived, and loses its
  pin and its mute. Unarchive brings it back.", a filled **Archive  ⏎** and **Cancel  Esc**
  (D111).

### 44 — Turns that finished unseen

*Work that ended while nobody looked must be findable at a glance, without being mistaken for
something that needs the user.*

- The orchestrator open with the agents panel docked. billing-service and users-service
  finished while no window showed them: their sidebar rows and their rows in the docked panel
  have a **cornflower rounded outline** (D114). index-rebuild needs the user and has no outline
  (the red exclamation wins).

### 45 — Ordering the sidebar

- The ordering menu (D116) open from its button on the search field: Order — Last message,
  **A → Z** (chosen), Z → A — then both checkboxes ticked. In each group what needs the user
  is first (index-rebuild above search-service), then what finished unseen (billing-service
  above CI integration), then the rest A → Z.

### 46 — The pill with all three relations

- The orchestrator, this time spawned by another agent: the pill has three clusters (D118) —
  an up-arrow glyph over the parent, a branch glyph over the four workers (two with the
  cornflower ring, one with the red exclamation), a ⇄ glyph over the contact — separated by
  short hairlines.

### 47 — The pill with workers only

- A thread started by the user with one worker: one cluster, the branch glyph over one icon,
  and no hairline.

### 48 — The docked panel at its narrowest

- The docked panel at 240 points: titles and branches are ellipsised; every state mark, the
  state word, the harness logo, the git counts and ahead / behind stay visible (D115).

### 60 — Pull request badges

*Which branch already has a pull request, and in what state, at a glance — without leaving
the list.*

- The orchestrator open, agents docked. Line 1 of each row ends with the PR badge before the
  time (D119): index-rebuild and search-service `[ PR# 33290 ]` with a merge mark (merged);
  billing-service `[ PR# 33343 +1 ]` with a dashed outline (draft) and a dotted ring (checks
  running); Fix frame decoder `[ PR#      ]` (create); CI integration's badge shows a lock — `az`
  is signed out on devbox, and the reason is the tooltip. The orchestrator, on `main` with no
  pull request, has none. The docked rows carry the same badges (users-service with a cross:
  checks failing). Titles ellipsize; badges and times never do.

### 61 — The `+1` list

- Hovering billing-service's `+1`: a small list under the badge, "Also from this branch",
  `#33301 · closed · Billing: first try at the token refresh`; clicking a row opens it.

### 62 — The badge's menu

- Right-click on billing-service's badge: **Create a new PR**, **Copy link**.

### 63 — Creating a pull request

- The Create PR modal for Fix frame decoder (D120): *feature/protocol · acme/platform*; Into
  **main**; the title and description prefilled from the branch's commits; **Draft**; **Ask the
  agent to write it** (greyed: that agent is busy, which the tooltip says); "Create will push
  feature/protocol first: it is not on the remote yet."; the warning "3 files have uncommitted
  changes · they are not part of the pull request."; **Create** and **Cancel**.

### 49 — Settings: window

*Closing to the tray, and what the window draws with (D127, D128).*

- Left navigation with **Window** selected. Page title "Window".
- **Closing the window** — a switch, on, "Keep running in the tray when closed"; under it, in
  the secondary colour: "Closing the window hides it in the system tray. Click the tray icon to
  open it again; its menu has Quit. The hub keeps running either way." (On Linux the switch is
  off and disabled, and the note says the tray icon is available on Windows.)
- **Display** — "Renderer" and a dropdown showing the choice (*Automatic*); its list has the
  renderers this build offers. Under it, "A change applies when YAAC starts again. If the
  renderer cannot start, YAAC falls back to software rendering and says so here." — after a
  change, "Applies when YAAC starts again." in the primary colour instead.
- The in-use line: "In use: OpenGL (glow)", or with a fallback, in the primary colour, "In use:
  Direct3D 12 (WARP, software) · fell back because Direct3D 12 failed to start: …". When a
  renderer did not finish starting last time: "Vulkan is skipped until you try it again." and
  **Try it again**.

### 50 — Fast mode

- The search-service worker (Claude · opus-5.5, a model that reports fast mode) is running in
  fast mode (D125). Its spinner is yellow and a little quicker in the sidebar row and the
  thread header; the orchestrator's spinner beside it keeps the neutral running colour.
- Under the composer, after the effort selector, the lightning toggle is **on**: filled in the
  fast yellow on the selected-row tint (an outline in the secondary colour when off), tooltip
  "Fast mode". On a thread whose model has no fast mode the icon is not drawn at all.

### 64 — The Commander

*Everything the window can do, one key away (D130).* Over the orchestrator thread.

- The window dims under the scrim. A raised panel, 600 points wide, sits centred near the top:
  a borderless search field ("Type a command…") over a hairline, then the list.
- With nothing typed: **Recent** first (*Show only running*, *Fork…*), then the catalogue's
  groups — *Go to*, *Threads*, … — each a small bold heading in the secondary colour.
- Each row: the name; under it the explanation in the secondary colour; the key on the right in
  an outlined key cap (Ctrl+Shift+R, Ctrl+Space…). The highlighted row has the selection tint.
  A command that cannot run now is drawn in the unavailable colour with its reason in place of
  the explanation ("Nowhere to go forward to"). View switches carry a tick when on.

### 65 — The Commander, filtered

- "agent" typed: *Agents: ask / allow…*, *Dock the agents panel* (Ctrl+Shift+B), *Go to a related
  agent…* (Alt+↓), *Copy message* (its explanation mentions the agent). No group headings while
  a query is typed; best matches first, recently used ones ahead.

### 66 — A step inside the Commander

- *Change model…* (Ctrl+M) continues in place: a small tinted chip "Change model" before the
  field ("Choose…"), and the list becomes the harness's models — the current one ticked, each
  with its instance, "default", and its effort range — and the other provider listed disabled
  with "fork to switch provider". Esc goes back to the whole list.

### 67 — Settings: keyboard

*Every key, rebindable (D131).*

- Left navigation with **Keyboard** selected (between Window and Advanced). Title "Keyboard",
  **Reset all** on the right, and one line on how recording works.
- Groups as in the catalogue. Each row: name and explanation on the left; on the right the key
  cap (click to record: "Press a key…  Esc cancels"), **Clear**, and **Reset** (enabled only
  when the row differs from its default). A command with no key shows "Add a key".
- Under *Commander*, Ctrl+K was just recorded: a warning mark and "Ctrl+K is used by Search
  threads." with **Replace** (filled) and **Cancel**. Under it, an information mark and the note
  about Ctrl+Space and input-method layouts.
- Local keys (Send, Stop, Allow once, Deny, Paste, Copy) show their key in a plain cap with
  where they work ("in the message box", "on the focused card"), and no buttons.
