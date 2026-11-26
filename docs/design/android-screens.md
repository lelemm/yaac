# Android screens

**Proposed.** This is the brief for the Android client. It is written by Claude together with
Codex (`gpt-6.2-astra`), and the owner has not reviewed it yet. The mockups are generated from
this brief, under [`mockups/android/`](mockups/android/), and reviewed the same way as the
desktop set ([review.md](review.md)): two reviewers score Usability, Journeys and Dark/Light,
and rounds continue until both give every criterion 9 or more.

## What the Android client is

The Android app is **a second client of the hub**. It works the same way the Windows window
does: the same attach protocol, the same behaviour, and the same features. It reaches the hub
over **Tailscale**. It has no pairing step and no reduced mode.

The rule for this brief: **parity, nothing invented.** Every feature, state and control of the
Windows client is in this brief, and the [parity checklist](android-parity.md) checks that. A
feature never becomes "desktop only", and the phone gains no feature of its own. The only design
work is **how** each existing feature is delivered on a small touch screen. When a desktop
mechanic has no meaning on a phone (hover, right-click, a resizable window, the tray), it maps to
the Android mechanic that does the same job. That mapping is listed once, below, and every
screen uses it.

## Visual direction (unchanged)

Everything in [screens.md → Visual direction](screens.md#visual-direction) holds: monochrome,
calm before complete, no outlines, the conversation first, local time, real icons and harness
logos, the same dark and light tokens, the same contrast rules, and custom themes from the same
JSON. The type uses the same bundled fonts, at Android sizes: 14 sp for UI, 16 sp for agent
prose, and the monospace font for code, commands, paths and ids. Material components are used
only for their mechanics (sheets, the drawer, the predictive back gesture, the IME). They are
restyled to YAAC's tokens, with no Material accent colour, no elevation tints and no FAB colour.
The one filled button per screen remains the primary action.

## Form factors

| Width class | Layout |
| --- | --- |
| **Compact** (phone portrait, < 600 dp) | One pane at a time: the list, a thread, or Settings. This is the main design and most mockups show it (412 × 915 dp). |
| **Medium** (phone landscape, small tablet, unfolded foldable, 600–839 dp) | The list and the thread side by side, the list in its collapsed **strip** form unless widened. The agents panel is the pill or the card. |
| **Expanded** (tablet, ≥ 840 dp) | The desktop layout: sidebar, thread, and the docked agents panel, with the side pane (Files / Browser) able to split the thread as on desktop. |

The desktop already has rules for narrow windows: the strip below 1,000 points, the agents panel
folding to the pill, composer buttons on their own row below 500 points. Android applies the
same rules at these widths.

## How desktop mechanics map to touch

| Desktop | Android |
| --- | --- |
| Hover (tooltip, card, preview) | **Touch and hold** shows the same content in a small floating card, which stays until you lift your finger. Where the desktop shows something on hover that you then *click* (the `+N` PR list, the agents card), a **tap** opens it instead. |
| Right-click / context-menu key | **Touch and hold** opens the same menu as a bottom sheet, with the same items in the same order and the same separators. A **⋮** on headers gives a second way in, as the desktop's hover dots do. Where a context menu exists, it takes precedence over the tooltip. Elsewhere hold reveals details, or begins text selection in selectable prose. Tap a message for its timestamp. In Related agents hold previews a row (A08), and **⋮** opens its menu. |
| Ctrl+click, Shift+click, Ctrl+A | A tap on a row's **state mark** (its left 48 dp) adds the row to a **selection** or takes it out (Ctrl+click). Once a selection exists, a tap anywhere on a row toggles it. **Select range** in the selection bar selects every row from the last one selected to the next one tapped (Shift+click). **Select all** in the selection bar is Ctrl+A. With no selection, a tap on the rest of the row opens the thread. |
| Dragging rows onto a group | Select the row (or rows) and drag the **drag handle** that selected rows show at their right edge onto a group header or its rows. The same outline, the same "Move to api" card, and the same not-allowed reason. |
| Keyboard shortcuts, the Commander | Every command is in the **Commander**, which has its own icon (the command mark) in both top bars, separate from thread search. With a hardware keyboard, every binding works as on desktop, including Ctrl+Space, and the key caps are shown. Without one, the key caps are hidden. |
| Back / forward buttons, mouse buttons, Alt+← / → | The system **Back** gesture goes back through YAAC's history: threads, Settings pages, drafts. **Forward**, and the history lists in both directions, are in the thread's ⋮ menu. |
| Resizable sidebar, docked panel, composer width, divider | Compact width has no dividers: each region becomes full width. Medium and expanded widths keep the drag handles. |
| The tray: hide on close, the needs-you badge, Quit | **Keep running in the background** (the same setting, on by default) keeps the attach alive through an Android foreground service. Its ongoing notification is the tray icon: "YAAC · 2 need you". Tapping it opens the app, and it has a **Quit** action, which ends the client, never the hub. The launcher icon carries the same badge. |
| Sticky request toasts (P355) | Android notifications on a high-importance channel, with sound. They are not auto-cancelled and are withdrawn when the request is answered anywhere. They offer the same actions: **Open**, and **Allow once** / **Deny** for a plain allow/deny. The same single reminder becomes "N requests need you" with the three oldest previewed. |
| File pickers (import a theme, a font, a skill folder, a key file; Save PNG/SVG) | The Android system pickers: the photo picker for images, and the document picker for files and folders. |
| Ctrl+click a link, "Open in browser", the PR badge | The system browser (Custom Tab). |
| Paste and drop files | Paste from the clipboard, the **+** attach button (photo picker or document picker), and Android's share sheet into a thread. |

## Shell (compact)

```
┌ List (home) ───────────────┐     ┌ Thread ───────────────────────┐
│ ≡YAAC   (!)2  ⌕  ⇅  ＋       │     │ ←  ◆ Fix frame decoder  (!)2 ⋮ │
│ Pinned                      │ tap │ Chat · Files · Terminal · ⊕   │
│ api              local      │ ──► │ timeline│ transcript      ◉◉ │
│  ▶ Migrate auth…   2m       │     │         │                  ◉ │
│    ⑂ main  +2 ~1      3     │     │ [queued strip] [running cmds] │
│ …                           │     │ ┌ composer ──────────────────┐│
│ ⚙  ⇡4 ⇣1   [limits]         │     │ └ [+]        [Queue] [Send] ┘│
└─────────────────────────────┘     │ model ▾ effort ▾ ⚡ mode ▾ … │
                                    └───────────────────────────────┘
```

- The **list is home** (it is the sidebar). Opening a thread pushes the thread screen, and Back
  returns along the history.
- The **Needs-you button** (the red exclamation and its count) is in both top bars whenever
  something needs you. Each tap opens the next thread that needs you, exactly like the desktop
  button and Ctrl+J.
- The sidebar's **footer** is the list's bottom bar: Settings, the hub status (the connection
  icon, ⇡ connected, ⇣ not connected), and the rate-limit panel.

## Screens

Each screen is drawn in dark and in light, on the fleet from
[screens.md](screens.md#the-fleet-used-in-every-mockup). **Covers** names the desktop screens
and the [parity](android-parity.md) ids that this screen delivers. **Frames** lists the images to
generate. Each frame is one image at 412 × 915 dp (compact) unless it says otherwise. The
interactions where a mistake is costly get a frame of their own.

The top bars used throughout:

- **List:** the YAAC mark · the Needs-you button "(!) 2" · the search field (searches threads only)
  · ⇅ (grouping and order) · the command mark (Commander) · ＋ New.
- **Thread:** ← · the state mark and title, with the tag and breadcrumb on a second line · the
  Needs-you button · the command mark · ⋮ (the thread menu, plus Forward, the history lists,
  Expand all / Collapse all, Previous / Next message).

### A01 — The list

*Covers: Sidebar (07); 21, 35, 36, 44, 45, 60, 110; F-ids of the shell and fleet.*

- **Pinned**, then the project sections with sticky headers (the project on the left, the machine
  on the right, the connection mark only when the machine is not connected), then **Muted · N**
  and **Archived · N**, both collapsed. A temporary project is in italics, with a dashed mark and
  its folder path.
- Two-line rows, exactly as on desktop: the state mark, the title, the PR badge, the relative
  time; the branch icon, the branch, the git counts, ahead and behind, the worker count. Unseen
  rows have the cornflower outline, and floated rows sit at the top of their group. Rows are
  56 dp tall. The open thread keeps its selected tint when you come back to the list.
- The ⇅ sheet: Machine / Project / Worktree checkboxes; Last message / A → Z / Z → A; *Keep
  unseen finished threads at the top*; *Keep threads that need you at the top*; Show only running and Show dismissed. Show archived opens the Archived section.
- "Show older" ends a section, and search offers "Search older threads on *devbox*".
- The bottom bar: ⚙ Settings, the hub status (the connection icon, ⇡ 4, ⇣ 1), and the rate-limit
  groups (logo, a segmented bar per window, "55% · 2d").
- An offline machine says "offline". An older daemon has the warning mark.

**Frames:** A01a the list (dark and light). A01b the ⇅ sheet open. A01c the rate-limit
group held: plan, email, exact resets in local time, targets, staleness. A01d search with
results and "Search older threads on devbox".

### A02 — The list: menus and dialogs

*Covers: 26, 33, 41, 42, 43, 61, 62, 72.*

- **A row's menu** (touch and hold): Copy thread ID, Rename — Fork, Move…, Take over (when it
  applies), Pin / Unpin, Mute… / Unmute, Archive… / Unarchive, Create pull request… — Delete… in
  the failed colour; Dismiss on a thread outside any project. Unavailable items keep their
  reason.
- **A group header's menu** (touch and hold, or its ⋮): New Thread, Archive all (N)…, Mute
  all…; on a temporary project, Create project… and Dismiss all above them.
- **The PR badge**: a tap opens the PR, a tap on `+1` lists the others (each one opens), and
  touch and hold gives Create a new PR and Copy link.
- **Mute…** is a sheet: until unmuted, minutes, hours, days, or until a date and time, with
  the six-row calendar.
- **Archive…** and **Delete…** are dialogs as on desktop. Delete has "Also delete the session
  from *harness*" under `threadDelete`.

**Frames:** A02a the row menu. A02b the group header menu. A02c the `+1` list. A02f the badge context menu. A02d Mute with the calendar open. A02e Delete.

### A03 — Selecting and moving several threads

*Covers: 69, 70, 71, 73, 78–82; D138, D144.*

- A tap on a row's state mark selects it. Selected rows have the multi-selection tint, the
  3 px bar, and a drag handle at the right edge. The top bar becomes the selection bar: "3
  selected" · **Archive** · **Mute** · **Move** · ⋮ (Unmute selected, Select range, Select
  all, Clear selection) · ×.
- The "Archive 3 threads?" dialog says what it skips and why ("3 of 5 can be archived; 2 are
  running"). The bar under the list then reports "2 could not be archived: … · uncertain: …".
- Dragging the handle onto a group outlines that group in the focus colour, with the card "Move
  to api". Over another machine, the card says "A thread cannot move to another machine" and the
  drop is refused. After a drop: "Moved 2 threads to api · **Undo**".
- The Move sheet: the machine, the **Project** and **Worktree** dropdowns, and the one sentence
  ("The agent will work in <path> from its next turn" or "Only where YAAC lists it changes…").

**Frames:** A03a three selected, with the bar. A03b Archive asks once. A03c a drag over a
project. A03d a refused drop. A03e Moved · Undo. A03f the Move sheet, folder follows. A03g the
Move sheet, folder stays.

### A04 — Medium width

*Covers: 21, 35, 112; the strip (D103).* 800 × 412 dp landscape: the strip (the mark, ＋, the
history buttons, vertical project names, a harness logo per thread with its state badge, the
rate-limit bars, Settings) beside the thread with the agents pill. Touch and hold an icon shows
its row card.

**Frames:** A04a landscape with the strip. A04b an icon held, with its card.

### A05 — A thread that needs an approval

*Covers: 01; Thread view; R01, R02 (P357).*

- The top bar shows "Fix frame decoder", with "Needs you — approval" and the breadcrumb `devbox ·
  yaac · feature/protocol` under it.
- The tab row is shown only when the thread has side-pane tabs. Otherwise a ⊕ at the right of
  the tag line opens Browser / Files / Terminal.
- The transcript: the user's bubble at the right with the chip `decoder-log.txt`, "Thought for
  4s" and "Ran cargo test -p yaac-protocol — passed" as tight muted lines, the agent's prose, and
  a six-line Rust block with **Copy**.
- **The approval card**: an amber left edge; "Run command"; the explanation labelled **AI
  explanation — check the command**; the deterministic danger mark when it applies; "Show command
  (N lines)"; `/home/dev/yaac`; the harness's reason; the offered decisions as stacked full-width
  buttons: **Allow once** (filled), Allow for this thread, Deny.
- **The composer stays usable**: you can type, and **Queue** is the only send button, with the
  hint "Queued until the turn ends".
- The control row is one line of text selectors that scrolls sideways: `Codex · work ▸
  gpt-6.2-sol ▾ · high ▾ · ask ▾ · agents: ask ▾ · 38% · Fork`.

**Frames:** A05a the screen. A05b the card with the command expanded and the danger mark.

### A06 — The header sheet and history

*Covers: Header (07); Back / Forward (D-history); 31.* A tap on the title opens a sheet: the
editable title (under `ThreadRename`), origin, project, worktree, the links (*spawned by*,
*forked from*, the ancestry), Copy thread ID. The ⋮ menu's **Back history** and **Forward
history** list the places in each direction (screen 31).

**Frames:** A06a the header sheet of index-rebuild, with its ancestry. A06b the back history
list.

### A07 — The orchestrator, with its agents

*Covers: 02, 27, 46, 47, 90, 91, 130, 133; A01, A08, A09.*

- The orchestrator's transcript: agent-call rows with pills (`⑂ Started [users-service] · opus-5.5
  · done`, `↗ [billing-service · devbox] "…" · delivered`, `⧗ Waiting for [pill] [pill]`,
  `↙ [pill] reported back`), and mesh lines `→ CI integration · delivered`, `← billing-service ·
  Migration merged…`.
- **The pill** floats on the right edge, the same 44 dp pill: the clusters, the arrows while a
  message is in flight, the red exclamation on the worker that needs you, the unseen rings,
  finished agents dimmed, and "more (23) · 4 alive". Dragging it moves it along the edge.
- **The timeline**: the dash column at the left edge. Touch and hold a dash shows its prompt's
  first line and time. Tap to jump directly, or open the prompt list when several share a dash.

**Frames:** A07a the orchestrator. A07b an agent call opened: the pills under **To**, the task in
full, the receipt, **Result**, **Raw call**. A07c thirty agents, "more" in the pill.

### A08 — Related agents

*Covers: 28, 48, 131, 132; A02–A07, A10; P341, P347, P352.*

- A tap on the pill opens **Related agents** as a sheet at half height (the card). Rows: logo,
  name, last activity, age, under *Siblings* (the open thread marked **self**), *Workers*, *In
  contact with*, *Spawned by*. Each row has a ⋮ for the thread menu.
- Dragging the sheet to full height is **docked**: the full docked rows (state word; logo,
  branch, git counts, PR badge; provider · model · target; activity and age), the speed bolts,
  "› 17 finished", and the budget line at the top, with *Spawned by* fixed at the bottom when a parent exists. The
  sheet comes back at the height you left it at (the dock is remembered). **Open root** is
  there when the root's thread is known.
- **Preview**: touch and hold a row. The sheet drops to half height (from full height too), and
  the area above it shows that row's transcript while the current thread stays selected. Sliding
  your finger onto another row previews that one, and lifting your finger restores the view.
- **Mark all as read** in the sheet's header.
- **The budget line**: "41 of 100 agent turns", "14 h since you acted (limit 12 h)". Touch and
  hold the time line opens Set / Extend time limit (+1 h, +4 h, +12 h, +24 h, your own hours
  with **Add**, Remove time limit, or the too-old reason). When the budget has run out: "Paused:
  12 h without you · **Continue**".
- At expanded width the docked form is the panel beside the thread, as on desktop.

**Frames:** A08a the half sheet. A08b full height (docked) with the budget line. A08c a row held,
with the preview above. A08d Extend time limit.

### A09 — A worker's request, mirrored in its ancestors

*Covers: 03, 04, 24; R06 (P354).*

- The orchestrator is open. index-rebuild (two levels down) asks a question, and its card is
  mirrored in the orchestrator's transcript as a copy, labelled with the worker's pill and
  **Go to the original**.
- A tap on the **Needs-you** button with that copy on screen scrolls to it and focuses it,
  without opening the worker.
- The copy can be answered here. Its resolution then shows everywhere: in the copy, in the
  original, and the sticky reminder is withdrawn or updated.
- The orchestrator's composer stays editable while the worker waits.
- From the copy, **Go to the original** opens index-rebuild at its card. Its header shows
  the ancestry links, and the **From** task sits on the faint fill at the left.

**Frames:** A09a the orchestrator with the mirrored question focused. A09b index-rebuild at its
original card. A09c the copy after it was answered, with "confirmed — answered by you".

### A10 — Every state a request can be in

*Covers: 05, 20, 25, 30; R03–R05, R07, A07.*

- The question card: options, multi-select, free text, a masked secret, **Submit answer**. A form
  or another request kind that the window cannot render tells you to answer it in the harness,
  as the desktop does.
- The outcomes: submitted, confirmed — answered by you, resolved elsewhere, expired, and
  outcome unknown with **Check latest status** (never resend).
- **The permission card** (screen 25): "This agent wants to work with other agents", "It
  will:" with its lines, "Budget for this orchestration" with **Agent turns 100** and **Hours
  12**, and **Allow** (filled), **Deny**, **Always allow in this chat**.
- **The budget ran out** (screen 20): the notice with how many messages are waiting, nothing
  cancelled, **Continue**.
- **Approved automatically**: the quiet line, opened.
- **The bypass confirmation**: it names the thread, the machine and the folder, with Cancel as
  the default. For a thread with workers, the scope confirmation names the workers that will
  be interrupted, with **Only this thread** as the default.

**Frames:** A10a the question card. A10b the five outcomes stacked. A10c the permission card.
A10d the budget notice. A10e the bypass confirmation. A10f the scope confirmation.

### A11 — The transcript in detail

*Covers: 76, 77, 83, 84, 85, 100–104; D148, D153, D156, D167, D179.*

- `Worked for 26s ›` folds, one of them opened, a failed fold with its error, "Changed 3 files ·
  show all", "Thought for 4s", an open reasoning row, and a failed command breaking the run with
  its red cross. Also **Interrupted**.
- A user message with **Pasted text · 42 lines** (collapsed, then opened, with its copy
  button), a command chip `/model opus`, and the **Context** disclosure.
- Notice lines from Claude Code ("Subagent schema-check reported back").
- A table in an agent's message scrolls sideways inside its block.
- **Selecting text**: long-press starts the system selection, and the handles can be dragged
  across several items. The selection toolbar has **Copy**, which copies across items as the
  desktop does.
- **A message's time**: a tap on a message shows its time in the line kept under it (the hover
  time).

**Frames:** A11a folded turns with one opened. A11b the failed turn. A11c pasted text, command
and Context, opened. A11d an open tool call, and hidden reasoning. A11e a selection across two
messages, with the toolbar. A11f the timeline beside a long thread, with a dash held.

### A12 — The composer while a turn runs

*Covers: 120, 23; C07, C08, C16; D165, D166.*

- The strip **Queued · 2 · sent when the turn ends**, each row with **Send now**, **Edit**, and ✕.
  Edit opens the message in the composer with **Save** and **Cancel edit**.
- The collapsible **Running commands** strip: who ran it, the target, the age, **Logs** (a live
  tail), and **Stop**.
- "3 new messages ↓" above the composer when you have scrolled up.
- The buttons sit on their own row under the field (the desktop's rule below 500 points): **+**
  attach, **Queue** (filled), **Send now** (touch and hold says whether it joins the turn or
  interrupts it), **Stop**. The field grows to five lines, then scrolls.
- On a touch keyboard, Enter adds a line. The send key setting applies with a hardware keyboard.
- Bubbles: "Sending…"; **not sent** with Edit and Retry; and **outcome unknown** with Edit and
  Dismiss.

**Frames:** A12a search-service running on gpu-box, with the queue and running commands. A12b Logs open. A12c the three
bubble states.

### A13 — The composer: pickers, modes, attachments

*Covers: 37, 38, 39, 40; C01–C06, C09–C11, E08; D172, P346, P362.*

- `/`, `$` and `@` open their picker as a list directly above the composer, which sits above
  the keyboard. "Loading skills…", or the failure with **Retry**.
- **Message history**: in an empty composer, a history icon in the button row lists the last
  ten messages sent from this client. With a hardware keyboard, ↑ and ↓ work as on desktop.
- **`!` mode**: the monospace field, the prompt row `>_ devbox · ~/yaac ×`, "Runs here, then
  Codex answers", **Run** (or **Queue run**).
- Attachments, the states from desktop 37–40: an upload in progress with a thumbnail, with Send
  held until the upload finishes; an image the model cannot read, sent as a file with the warning;
  unknown image support, with the choice between image and file; a failed upload with
  **Retry**; and a large paste, which offers **Attach as file**, **Insert text** or **Cancel**.
  The share sheet into YAAC stands in for the drag-and-drop overlay (40).
- "Compacting context…", and the **Context compacted** line.

**Frames:** A13a the `/` skill picker. A13b the `@` file picker. A13c `!` mode. A13d the
attachment states. A13e the large-paste choice. A13f sharing a screenshot into a thread (the
share sheet's destination picker).

### A14 — The control row

*Covers: 08, 50; C12–C15.* One sheet per control:

- **Model**: grouped by provider instance. A started thread can switch within its instance when allowed. A draft can choose any available instance on its target. Each row shows the image icon and available effort range,
  "Read from local · updated just now", other instances in a started thread disabled with "fork to switch
  provider", and **off in Settings**.
- **Effort**.
- **Speed**: Standard, Fast, Ultrafast, with the Ultrafast confirmation and its usage warning
  above the composer. The bolt is outlined, yellow or blue.
- **Mode**, which opens A10e or A10f for a mode that bypasses approvals; the active bypass mode is
  drawn in the failed colour with the warning icon.
- **Agents**: Ask for permission / Allow without asking.
- **The context meter**.

**Frames:** A14a the model sheet. A14b the speed sheet with the Ultrafast warning. A14c the
control row in bypass mode.

### A15 — First run

*Covers: 07.* The empty list (the mark, the disabled search, Settings, the connection icon). The
main area: "YAAC", the sentence, and the three cards stacked: **This computer** (Codex — ready,
Claude — not signed in with "Run claude in a terminal to sign in", **Refresh**, **Start a
thread**, filled), **Add a server** (**Add server…**), **Turn on WSL** (**Turn on…**, "Windows
only"). The footer line: "Agents keep running when you close this window."

**Frames:** A15a first run.

### A16 — A new thread

*Covers: 08, 09, 29; D107.* ＋ New opens a draft: "New thread", "Describe what you want done.",
the draft bar `Project: api — local ▾`, the model sheet as in screen 08, markdown styled as you
type, **Send**. The initial draft has no project or checkout selected. A16a shows the draft after the owner chooses api on local. A group’s New thread can prefill these values. The control row starts with **Project** and **Worktree**, and the project picker
ends with **New project…**, which opens the create-project sheet with the folder prefilled. Then
the first message, sent (screen 09).

**Frames:** A16a the draft with the model sheet. A16b the first message sent. A16c creating a
project from a discovered folder.

### A17 — In use elsewhere, Take over

*Covers: 10, 74, 75; D140.* "Explain the handshake" is held by **a terminal session on devbox**
(a program that allows it). Its banner reads "This thread is open in another program on devbox.
You can watch, but not send.", with **Take over** (filled) and **Fork to continue here**. A
touch and hold on Take over shows its explanation. The cards have no buttons ("Waiting for an
answer in the other program"), and the composer is replaced by "Held by a terminal session on
devbox since 14:02". Then: Take over is not possible now (a turn is running there), with its reason.
Then: taken over. A harness-owned subagent instead shows its role, the harness’s reason and **Open parent**. The composer stays read-only. Its request is answerable when its parent permits it.

**Frames:** A17a read-only with Take over. A17b not possible now. A17c taken over. A17d a read-only harness subagent with Open parent.

### A18 — Fork

*Covers: 11, 11n, 12.* A full-screen dialog: the destination picker (*native* / *handoff*), the
instance and account, what is and is not carried over. For a handoff: the transcript size, the
cut-off turn, the preview, the first message, **Send and fork**. Then the forked thread, with its
inherited history above a divider.

**Frames:** A18a a native fork. A18b a handoff fork. A18c the forked thread.

### A19 — Pull requests

*Covers: 60, 63; PR01, PR02.* Create PR as a full-screen dialog: *Into*, *Title*,
*Description*, **Draft**, **Ask the agent to write it**, the push notice, the uncommitted
warning, the uncertain outcome.

**Frames:** A19a Create PR.

### A20 — Notices and banners

*Covers: 19, 23, 34; R08; the older daemon; another store.* An offline target, a queued message
that arrives and one that never will, "Updating yaacd on devbox…", "Ubuntu is not running" with
**Start it** in place of the composer, the another-store banner, and a dismissible error at the
bottom.

**Frames:** A20a notices above the composer. A20b the distro is down. A20c the older-daemon
banner with **Update now**.

### A21 — Notifications and running in the background

*Covers: R09 (P355); S18 (the tray, D127).* The shade and the lock screen: the reminder for
one approval (**Open** · **Allow once** · **Deny**), the "3 requests need you" summary with the
three oldest (Open only), and the ongoing "YAAC · 2 need you" notification with **Quit**. Also
the launcher badge.

**Frames:** A21a the shade with one approval. A21b the summary on the lock screen. A21c the
ongoing notification and the launcher badge.

### A22 — The Commander

*Covers: 64, 65, 66; K01.* Full screen, from the command mark: the search field, *Recent*, then
the groups. Each row has its name and explanation, or the reason it cannot run now. Key caps
appear only with a hardware keyboard. Steps continue inside it, with Back on an empty field.

**Frames:** A22a the Commander. A22b filtered. A22c a step (models).

### A23 — Files and diffs

*Covers: 12 Files; D01–D05.*

- The **Files** tab: the worktree tree, folders first, with git marks M/A/D/R/U/! and the
  changed-folder mark. The toolbar has **Show ignored**, **Refresh**, **Collapse all**.
- A **file** tab: read-only, syntax colours, line numbers, the relative path, **Reload**, and the
  binary or oversize notice.
- A **diff** tab: against HEAD, **unified** or **side by side** (side by side scrolls sideways on
  a phone), intraline marks, folded unchanged regions that open, the change count with
  **previous** / **next**, and **Reload**.
- Long-press a range, then **Add to composer** in the selection toolbar, inserts the chip at the
  saved caret and goes back to Chat.

**Frames:** A23a the tree. A23b a file. A23c a unified diff with its navigation. A23d the selection
with Add to composer. A23e the resulting chip at the original caret in Chat.

### A24 — Terminal

*Covers: 13 Terminal; E01–E07, E09–E11.*

- The **Terminal** tab: the shell's grid, the tab title and icon, the folder, the bell and
  unseen-output marks; tabs with **New terminal** and a choice of the available shells; Rename
  (with clear to restore); exit status with **Restart**.
- The toolbar: **Clear**, **Search** with match navigation, **Add to composer** (a fenced
  console excerpt), the **Send commands to the agent** switch (context only), with the reason
  when it is not available.
- On a touch keyboard, a row of the keys a shell needs (Esc, Tab, Ctrl, Alt, the arrows, `|` `~`
  `/` `-`) sits above the IME. It is the Android form of the keyboard the desktop has. The existing interface-scale command applies to the whole app.
- The panel under the composer is the half-height form; **Maximise** / **Restore**.
- Clipboard: long-press selection and Copy; Paste, with the warning before pasting several lines
  (**Don't ask again**); the notice when the target writes to the clipboard (OSC 52), and denied
  reads.
- `!` commands in the transcript: streaming output, **Stop**, the exit code and duration, **Show
  all lines**, then the agent's reply.

**Frames:** A24a the terminal tab with the key row. A24b search. A24c the multi-line paste
warning. A24d the panel under the composer. A24e a `!` command in the transcript.

### A25 — Browser

*Covers: 11 Web preview; B01–B10.*

- The **Browser** tab: back, forward, reload / stop, the address field (`5173`, `:5173`), **Open in
  browser**, **Inspect**, **Full screen**.
- A new tab lists the target's discovered ports (process and project), or says there are none or
  the target is offline.
- Tabs show the agent mark while an agent drives them. **Full screen** hides the chat but keeps
  the tab row, with **Chat** pinned first and marked when the chat has new activity.
- **Inspect** opens DevTools for the tab, as a full-height sheet, with element picking.
- JavaScript alert / confirm / prompt, and a page's file chooser (the document picker).

**Frames:** A25a a page in the browser tab. A25b the new-tab ports list. A25c full screen with the
pinned Chat marked. A25d DevTools. A25e a JavaScript prompt.

### A26 — Images, diagrams and delivered files

*Covers: 06 Viewing images; P358, P359.*

- The image viewer: full screen, pinch to zoom and pan, **Reset** to fit again, and **Close**.
- A delivered file's artifact card: **Download**, **Open**, **Open in the integrated browser**,
  **Preview diagram** for `.mmd`, **Dismiss**, and the worker it came from.
- The mermaid preview, with **Save PNG**, **Save SVG**, **Save source** (the document picker), and
  an invalid diagram's reason beside its source.

**Frames:** A26a the image viewer. A26b the artifact cards. A26c the mermaid preview.

### A27 — Settings

*Covers: 13, 13d, 17, 49, 67, 68; S01–S17, K02.* A list of pages: Targets, Providers,
Appearance, Window, Terminal, Keyboard, Approvals, Notifications, Skills, Advanced, About. Each
page opens full screen (side by side at expanded width), and Back returns to the threads.

- **Targets**: Local; SSH connections (add, edit, enable, disable, remove, retry, and the line
  about Disable / Remove / Revoke); WSL (the master switch, the distros from the hub with
  default, running and already added, the optional user, the second-session note).
- **Providers**: the target picker with **Refresh**; automatic session discovery; per instance,
  its status, the backend, the version, found running or started; enable; add / edit / remove an
  instance; harness home with **Change**, and the another-store question; the outside-tools
  switch; **Trust folders**; **Model guidance**, with the editor for **Best used as** and the
  on/off switches.
- **Appearance**: Follow the system / Dark / Light / Custom, the preview cards, **Import JSON…**,
  the normal and code fonts with their samples, **Import font…**, and the interface scale
  (larger / smaller / reset).
- **Window**: Keep running in the background (the tray setting), and the renderer row, which
  shows what is in use.
- **Terminal**: Copy on select, Ctrl+V goes to the shell, Ask before pasting several lines.
- **Keyboard**: the same table. A tap on a key cap records a chord from a hardware keyboard. The
  table also has conflicts with **Replace** / **Cancel**, **Clear**, **Reset**, **Reset all**, and
  the **In terminal** switch.
- **Approvals**: Explain commands with: main and fallback, from the live lists, or None.
- **Notifications**: on/off, include questions, sound, and which sound.
- **Skills**: the catalog, create / edit, import a folder (the document picker), Manual / Auto
  when relevant, remove, and the bundled skill's override with restore.
- **Advanced**: the send key, the hub's start at login, clear cache (drafts kept), and the
  diagnostics per target with the last error.
- **About**: the version and build, the release and source links, Components with **Refresh**,
  the update states (available, downloading, failed with Try again, restart to update), and
  **Troubleshoot** (the symptom, the chosen target, **Diagnose**, the editable report, **Copy**).

**Frames:** A27a the page list. A27b Targets with WSL distros. A27c Providers. A27d Appearance with
theme cards. A27e Keyboard with a conflict. A27f Skills. A27g About with an update and
Troubleshoot.

### A28 — SSH setup

*Covers: 14, 15, 16, 22, 32; S04, S05.* The SSH form in one column: **Import from SSH config**,
Name, Host, Port, User, **Setup**; the fingerprint confirmation; the password or key file (the
document picker) with its passphrase, **Connect**; the step log in plain words; Ready, "set up by
YAAC on …", with **Revoke**; the failure; the manual-removal line with "I removed it myself". The
hub runs every step, as on desktop.

**Frames:** A28a the form. A28b the fingerprint. A28c the step log, verified. A28d a failed revoke with the manual-removal line. A28e the sign-in step with the password/key-file alternatives.

### A29 — Tablet

*Covers: the desktop layout at expanded width.* Screen 02 at 1280 × 800 dp: the sidebar, the
thread with its timeline, the docked agents panel, the composer and its control row, operated by
touch. A second frame adds the side pane with a diff beside the chat.

**Frames:** A29a the tablet orchestrator. A29b the tablet with the side pane.

## Open points (for the owner)

- **The browser host.** On desktop, the browser tabs that agents drive are hosted by the window
  (CEF). On Android they would be hosted by a WebView in the app. With both clients attached,
  which window hosts an agent's tab?
- **Key files and imports** come from the phone's own storage and are sent to the hub. Today the
  desktop reads them on the host.
- **The renderer row** under Window: Android has one renderer, so the row shows only what is in
  use.
