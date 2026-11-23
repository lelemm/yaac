# Commands and key bindings (D130, D131)

The **Commander** (default **Ctrl+Space**) is a searchable list of everything the window can
do. Each row shows the command's name, a one-line explanation of what selecting it does, and
its key binding when it has one. Every command below is in the Commander; the ones with a key
are also reachable directly. Every key is **rebindable** in Settings → Keyboard.

Rules:

- A command that does not apply right now (no thread open, the harness lacks the capability,
  the target is offline) is **listed but disabled, with the reason**, unless it would only be
  noise (thread commands with no thread open are hidden).
- Commands that act on "this thread" act on the open thread; in the sidebar, a row's context
  menu offers the same commands for that row.
- Commands that take an argument (model, effort, mode, mute duration, project, order…) open a
  **second step inside the Commander** (a sub-list or a small input), not a separate dialog,
  where that is enough; dialogs that already exist (fork, delete, archive, create PR, mute
  with a date) open as today.
- Destructive commands (Delete, Revoke, Remove target, Quit with the tray off) keep their
  confirmation.
- The Commander searches names, explanations and aliases (e.g. "dock" finds *Dock the agents
  panel*), fuzzy, and ranks recently used commands first.
- Keys never fire while a text field has focus unless they carry Ctrl or Alt; plain keys
  (F2, Delete) act only when focus is in the list or thread view. The keys text editing itself
  uses (Delete, Backspace, Home, End, Ctrl+←/→, Ctrl+A/C/V/X/Y/Z, so also Ctrl+Delete) never
  fire in a text field (P77).

## Catalogue

Keys marked **new** are proposals; existing ones are kept. "—" means no default key.

### Go to

| Command | Does | Key |
| --- | --- | --- |
| Commander | Open this list. | Ctrl+Space **new** |
| Search threads | Focus the sidebar's search field. | Ctrl+K |
| Find a thread… | Fuzzy-find any thread by title or id across every target (hub search), and open it. | Ctrl+P **new** |
| Next thread that needs you | Open the next thread waiting on you, at its request (D117). | Ctrl+J |
| Jump to the bottom | Scroll the open thread to its newest message; with the view scrolled up, the button above the composer says how many messages arrived since (D142). | Ctrl+End |
| Go to previous / next message | Scroll the open thread to the prompt before / after the one in view — what the timeline's dashes stand for (D153, P212). Disabled below two prompts. | Alt+PageUp / Alt+PageDown **new** |
| Next unseen finished thread | Open the next unseen finished thread in sidebar order, or the newest unseen worker if the sidebar has none (D114, D171). | Ctrl+U **new** |
| Next / previous thread in the list | Move the selection down / up the sidebar. | Ctrl+↓ / Ctrl+↑ |
| Back / forward | Through visited threads (D87). | Alt+← / Alt+→, mouse buttons |
| History… | The back/forward list (D88). | — |
| Go to the parent thread | Open the thread that spawned this one. | Alt+↑ **new** |
| Go to a related agent… | Pick one of this thread's related agents and open it. | Alt+↓ **new** |
| Settings | Open Settings. | Ctrl+, |
| Settings: a page… | Open one Settings page (Targets, Providers, Keyboard, Display, Window…). | — |

### Threads

| Command | Does | Key |
| --- | --- | --- |
| New thread | Start a draft (choose target, project, model). | Ctrl+N |
| New thread in this project | Draft in the open thread's project and target, same harness and model. | Ctrl+Shift+N **new** |
| Rename | Rename this thread. | F2 **new** |
| Copy thread ID | Copy the harness's conversation id. | Ctrl+Shift+C **new** |
| Pin / Unpin | Keep it in Pinned. | Ctrl+Shift+P **new** |
| Mute… / Unmute | Mute for a duration or until a date (D90). | Ctrl+Shift+M **new** |
| Archive… / Unarchive | With confirmation (D111). | Ctrl+Shift+E **new** |
| Delete… | With the "also in the harness" choice (D110). | Ctrl+Delete **new** |
| Dismiss / Show again | Hide a discovered thread in the window. | — |
| Dismiss all in this temporary project | As the header's action. | — |
| Fork… | Open the fork dialog (native or handoff). | Ctrl+Shift+F **new** |
| Take over | Continue here a thread another program holds (D140); that program is not stopped. Enabled only when its harness can hand it over. | —  |

| Move to project… / Move selected… | List the thread (or the selected rows) under another project or worktree **of the same machine** (D144). *Move to project…* continues with the machine's projects and their known worktrees, then opens the Move dialog with that choice; it says whether the agent's folder follows (`threadMoveWorkingDirectory`) or only where YAAC lists it changes. Disabled with the reason when the thread is archived, in use elsewhere, or its machine is not connected. | — / — |
| Check for the latest | Re-read a thread used elsewhere. | — |
| Continue (budget) | Resume an orchestration paused on its budget. | — |
| Archive selected… / Mute selected… / Clear selection | The rows selected in the sidebar (D138): Archive asks once for all and says which are skipped and why; Mute opens the Mute dialog once and applies it to all. Enabled only with a selection. | — / — / Esc in the list |

### Composer and turns

| Command | Does | Key |
| --- | --- | --- |
| Focus the composer | Put the caret in the message box. | Ctrl+L **new** |
| Send / Queue message | Send; while a turn runs, queue it for the turn's end (D166). Reads "Queue message" while a turn runs. | Enter (Ctrl+Enter when the send key is Ctrl+Enter) |
| Send now | While a turn runs: steer it, or interrupt it and start again with the message — "Send now — interrupts" where the thread cannot steer (D166). Enabled only while a turn runs and no approval or question waits. | Ctrl+Enter (Ctrl+Shift+Enter when the send key is Ctrl+Enter) |
| New line | — | Shift+Enter |
| Stop the turn | Stop the running turn. | Esc |
| Attach files… | Open the file picker (D95). | Ctrl+Shift+A **new** |
| Paste | Text, files, images. | Ctrl+V |
| Change model… | Pick a model of this harness (sub-list). | Ctrl+M **new** |
| Change effort… | Pick an effort level (sub-list). | Ctrl+E **new** |
| Change mode… | Pick the approval mode (ask, auto, …), with the bypass confirmation where it applies. | — |
| Agents: ask / allow… | Set whether agents may spawn and message, with the budget. | — |
| Fast mode on / off | Toggle fast mode where the model offers it (D125). | Ctrl+Shift+L **new** |

### Requests (act only on the focused card, as today)

| Command | Does | Key |
| --- | --- | --- |
| Allow once / Allow for this thread / Deny | On the focused approval card. | Enter / — / Esc on the card |
| Answer the question | Focus the open question's input. | — |

### Pull requests (D119–D121)

| Command | Does | Key |
| --- | --- | --- |
| Open this branch's PR | Open it in the browser. | Ctrl+Shift+O **new** |
| Create a pull request… | Open the create modal. | — |
| Copy the PR link | — | — |

### View

| Command | Does | Key |
| --- | --- | --- |
| Toggle the sidebar | Collapse to the strip and back (D94). | Ctrl+B |
| Dock / undock the agents panel | D105. | Ctrl+Shift+B **new** |
| Grouping: machine / project / worktree | Toggle each (sub-list). | — |
| Order: last message / A → Z / Z → A | D116. | — |
| Keep unseen at the top on / off | D116. | — |
| Keep needs-you at the top on / off | D116. | — |
| Show only running on / off | D129. | Ctrl+Shift+R **new** |
| Show dismissed / Show archived | — | — |
| Theme: system / light / dark | — | — |
| Zoom in / out / reset | Interface scale (the Appearance font size, 11–18 pt, persisted). | Ctrl+= (or Ctrl++) / Ctrl+- / Ctrl+0 **new** |
| Expand / collapse all tool lines | In the open thread, including the work of finished turns folded into "Worked for …" (D156). | — |
| Copy the selection / Copy message | D84. | Ctrl+C / — |

### Targets and providers

| Command | Does | Key |
| --- | --- | --- |
| Retry connecting… | Retry a target now (sub-list of offline targets). | — |
| Start a WSL distribution… | D96. | — |
| Enable / disable a target… | — | — |
| Add an SSH target… / Add WSL… | Open the add form. | — |
| Refresh providers and models… | Re-read a target's harnesses. | — |

### Window

| Command | Does | Key |
| --- | --- | --- |
| Hide to the tray | D127 (when on). | Ctrl+W **new** |
| Quit YAAC | Closes the window (the hub keeps running). | Ctrl+Q **new** |
| Renderer… | Settings → Display (D128). | — |

## Default key conflicts

- **Ctrl+Space** is also the input-method switch on some Windows keyboard layouts (Chinese,
  Japanese, Korean) and the completion key in many editors. It is the owner's choice and is
  rebindable; if the OS swallows it, Settings → Keyboard shows the binding as not received.
- **Ctrl+W / Ctrl+Q / Ctrl+P / Ctrl+L** are common browser/editor keys; YAAC is its own window,
  so they do not collide while it has focus.

## As built

The table lives in `yaac-core` (`commands::COMMANDS`): stable id, name, explanation, aliases,
group, default keys, scope, and the choice it continues with. The Commander, Settings →
Keyboard and the window's one keyboard dispatcher all read it. A test checks that every row of
the catalogue above is in the table and every command is in the catalogue.

- Rows with "/" are one command each, or one toggle whose name follows the state (P75).
- **Local keys** — Send, Send now, New line, Stop (in the message box), Allow once, Deny (on
  the focused card), Paste and Copy the selection — are shown but not rebindable (P76); from
  the Commander, Send and Send now send the composer, New line adds a line break, Stop stops,
  Paste pastes into the composer, Copy copies the selection.
- **Allow once / Allow for this thread / Deny** from the Commander act on the card that had
  focus when it opened; otherwise they are disabled with "Focus the request card first
  (Ctrl+J)" (P78).
- Steps inside the Commander: *Find a thread…* (threads the window knows, every cached target),
  *History…*, *Go to a related agent…* (parent, workers, contacts), *Settings: a page…*, *Change
  model / effort / mode…*, *Agents: ask / allow…*, *Mute…* (1 hour, 4 hours, 1 day, 1 week,
  until unmuted, "Until a date…" opens the Mute dialog), *Order*, *Grouping* (stays open to
  toggle several), *Theme*, *Retry connecting…*, *Start a WSL distribution…*, *Enable / disable a
  target…*, *Refresh providers and models…* (P82).
- *Add an SSH target…* opens Settings → Targets with the SSH form; *Add WSL…* and *Start a WSL
  distribution…* exist only on Windows (hidden elsewhere); *Renderer…* opens Settings → Window.
- *Quit YAAC* asks first when the tray is not in use; *Hide to the tray* is disabled where there
  is no tray icon (Linux) (P79).
- **Keys in the sidebar list** (D138, P96): with focus in the list (a row was clicked last),
  **Esc** clears the selection, **Ctrl+A** selects every listed row and **Shift+↑ / Shift+↓**
  extend the selection a row at a time. They never fire in a text field. Esc is listed as
  *Clear selection*'s local key; Ctrl+A and Shift+↑/↓ are list keys like the clicks
  (Ctrl+click, Shift+click). *Archive selected…* and *Mute selected…* have no default key.
- *Check for the latest* is enabled for a thread in use elsewhere or with an answer whose
  outcome is unknown; *Continue (budget)* when the orchestration is paused on its budget.
- Every catalogue command exists; none is missing. Commands disabled by design in some states
  carry their reason on the row.

## Settings → Keyboard

A list of every command (same names and explanations as the Commander), grouped as above,
each with its binding: click to record a new chord, Backspace/Delete to clear, a reset per row
and "Reset all". A chord already in use is flagged with the command that has it and must be
resolved (replace or cancel). Bindings are a window preference stored like the others
(`keyBindings`); only differences from the defaults are stored. Local keys are listed with
where they work, without a record button. Ctrl+Space carries a fixed note about input-method
layouts: a chord the OS swallows never reaches the window, so it cannot be detected (P81).
