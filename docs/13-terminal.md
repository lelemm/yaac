# 13 — Terminal

A **Terminal** tab in the side pane ([12](12-files.md#the-pane-and-its-tabs)) runs a shell on
the thread's target, in its worktree. The owner's bar: "a performant terminal, not a half-baked
one that looks bad or lags more than the network". Everything here serves that.

Status: **Proposed** (P331–P337), asked for on 2026-11-25.

## The shell

| Target | Shell |
| --- | --- |
| Windows (the local target on a Windows host, or any Windows target) | **PowerShell**: `pwsh.exe` (PowerShell 7) when it is on `PATH`, else `powershell.exe` (Windows PowerShell 5.1), through **ConPTY** (Windows 10 1809 or later). Started with `-NoLogo`; the console uses UTF-8 (code page 65001) so output is never mangled. |
| Linux (WSL, SSH), macOS | The target user's **login shell** from the password database (`$SHELL` as a fallback), started as a login, interactive shell so the user's rc files, prompt and aliases load; if it is missing, `bash`, then `sh`. |

- The shell starts in the thread's worktree folder. `TERM=xterm-256color`, `COLORTERM=truecolor`,
  `TERM_PROGRAM=YAAC`, and the user's own environment otherwise.
- The **+** menu's **Terminal** opens the default shell. Its submenu lists the other shells
  found on that target (Windows: PowerShell 7, Windows PowerShell, Command Prompt, and Git Bash
  when installed; Linux/macOS: the shells in `/etc/shells` that exist), each **once**: paths that name the same
  file — `/bin/bash` and `/usr/bin/bash` on a merged `/usr`, `rbash` linked to `bash` — are one
  entry, and `terminal/open` accepts any of them.
- The same emulator handles all of them: PowerShell's PSReadLine, `cmd`, `bash`, `zsh`, `fish`,
  and full-screen programs (`vim`, `htop`, `less`, `tmux`) behave as in Windows Terminal or a
  Linux terminal.

## Architecture

```
keys ─▶ window (yaac-term: alacritty_terminal grid) ─▶ hub ─▶ target connection ─▶ yaacd ─▶ node-pty ─▶ shell
screen ◀─ grid renderer (egui mesh / WPF GlyphRun) ◀─ hub ◀─ (interactive lane, raw data frames) ◀─ yaacd ◀─┘
```

### On the target — `yaacd`

- **`node-pty`** (MIT; what VS Code uses for remote terminals): Unix PTYs and Windows ConPTY.
  Prebuilt native binaries for every target platform YAAC supports (linux-x64, linux-arm64,
  darwin-x64, darwin-arm64, win32-x64) are shipped beside the Node runtime YAAC already
  installs; no compiler is needed on the target. A target without a working build reports no
  `terminal` feature, and the + menu shows Terminal disabled with the reason.
- Terminals belong to `yaacd`, not to the window: they **keep running when the window closes**
  or the connection drops, like a turn. Each keeps a scrollback ring of the last 2 MiB of
  output on the target; reattaching replays it, then streams live.
- A terminal ends when its shell exits (the tab shows "Process exited with code N" and a
  **Restart** button) or when the user closes the tab (the shell is sent SIGHUP / closed).

### Methods (YAAC protocol, additive, feature `terminal`)

| Method | Purpose |
| --- | --- |
| `terminal/shells` | → the shells available on the target, the default first. |
| `terminal/open` | `{ cwd, shell?, threadId?, cols, rows }` → `{ terminal, stream, initialCredit }`. One data stream carries input (host → target) and output (target → host). |
| `terminal/attach` | `{ terminal, cols, rows }` → `{ stream, initialCredit }`: a host that comes back (or a second window) reattaches. |
| → `terminal/detach` (host to daemon) | `{ stream }` releases one attachment. Other windows and the shell keep running. |
| `terminal/resize` | `{ terminal, cols, rows }`. |
| `terminal/close` | `{ terminal }`: ends the shell. |
| `terminal/list` | The target's running terminals: id, cwd, shell, title, the thread it was opened from, and the user's `name` if it has one. |
| `terminal/rename` | `{ terminal, name? }` (P343): the user's name for the terminal; absent or empty goes back to the automatic title. An older `yaacd` answers *method not found*: the hub reports `capability_missing` without turning the terminal feature off, and the window keeps the name itself. |
| → `terminal/exited` | `{ terminal, code }`. |
| → `terminal/title` | `{ terminal, title }` from the shell's OSC 0/2 title. |
| → `terminal/renamed` | `{ terminal, name? }` to every connection attached to it, so every window shows the new name. |

Each attachment has its own stream. The open/attach result grants input credit. The receiver
then sends `stream/credit` granting 1 MiB for output, including replay. The target pauses PTY
reads when an attached receiver falls behind by 1 MiB and resumes as the backlog drains.
An unattached PTY continues reading into its ring. Exit follows the final credited bytes.

The daemon build stages the five platform archives of MIT-licensed `@lydell/node-pty`
1.2.0-beta.18, a split distribution of Microsoft’s node-pty. Each archive is checked against
its SHA-512 integrity in `daemon/package-lock.json` and carries the upstream licence. The
Windows installer stages the ConPTY package beside node.exe and the local connector copies
both to the versioned private runtime before connecting. The hub embeds the four small
Unix archives and sends the matching one over stage 0 when missing, including when the target
already has a suitable Node. Stage 0 checks SHA-256 and installs atomically under
`$YAAC_HOME/runtime/pty-1.2.0-beta.18`. The JavaScript bundle keeps the native module
external. A load error is carried by `initialize.terminalUnavailableReason` and the hub
target summary. [Package source and licensing](https://github.com/lydell/node-pty)

The hub relays a terminal's data stream between the window and the target over the attach
protocol (`hub/terminal/*`), as raw data frames — never JSON. The window still owns no target
connection (invariant 14).

The six attach requests are `hub/terminal/shells`, `open`, `attach`, `resize`, `close` and
`list` under the same prefix. They add `target` beside the daemon method’s fields. The open
and attach results name a window-local stream. On this stream the window sends input up to
`initialCredit` and grants 1 MiB for output with an ordinary `stream/credit` notification.
Each receiver returns consumed bytes to the sender with that same notification. The hub maps
stream numbers and forwards credit in each direction. The attach writer schedules terminal
frames ahead of queued bulk traffic and gives other messages a turn during sustained output.

A second window receives its own stream, credit and replay from the daemon. Only attached
windows receive `hub/terminal/title` and `hub/terminal/exited`, carrying the target and the
daemon event fields. Window disconnect sends `terminal/detach { stream }` to the daemon for
each attachment. A target disconnect invalidates its streams. The window follows the target’s
connection state, then issues a fresh attach for its existing terminal id to receive replay.

## Latency and throughput

The real risk is the transport, not the emulator: today a 64 KiB bulk chunk can sit in front of
a keystroke for hundreds of milliseconds on a slow link.

- **An interactive lane.** Terminal data is a third scheduling class, **interactive**, sent
  ahead of bulk on every hop (window → hub → target and back) and interleaved with control.
  Bulk chunks are capped while interactive data is waiting. This is a change to the scheduler
  in both the Rust and TypeScript implementations ([03](03-protocol.md#scheduling-and-flow-control)).
- **Keystrokes are never batched**: each is written as soon as it is typed. **Output** is
  flushed when a batch reaches 16 KiB or its 1 ms timer fires, within the 4 ms batching budget.
  The first output after input flushes immediately for echo latency.
- **Credit** for terminal output is 1 MiB, so fast output never waits a round trip for credit;
  the window always reads the stream (it never stops granting credit while the tab is open,
  hidden or not), so a flood is limited by the link, not by YAAC.
- **Budget, checked by a benchmark** (`cargo test -p yaac-hub --test terminal -- --ignored`, run
  on demand on a real machine, not in CI, whose runners are too variable for timing gates). Keystroke-to-echo over a loopback target adds **at most 5 ms**
  to the network round trip (p99), measured end to end through window, hub and `yaacd`.
  Throughput: `cat` of a 100 MB file through YAAC finishes within 1.5× of the **raw PTY floor**
  (the same `cat` through `node-pty` read by a local process, no protocol) and at no less than
  50 MB/s on loopback; the UI's frame time stays under 16 ms meanwhile. (Comparing with a raw
  socket copy measured the kernel's PTY, not YAAC.)
- Later: predictive local echo for high-latency links (as VS Code and mosh do).

## In the window

### Emulator — `yaac-term`

A renderer-neutral crate around **`alacritty_terminal`** 0.26 (Apache-2.0; Alacritty's own
parser and grid, also used by Zed): it parses output off the UI thread, keeps the grid and
scrollback (10 000 lines in the window, beyond what the target replays), and exposes per frame a
snapshot of the **rows that changed** (damage), the cursor, and the selection. It also turns keys
into the bytes each mode expects (application cursor keys, bracketed paste, Alt as Esc, Ctrl
combinations, function keys, the Windows keys PSReadLine uses) and handles mouse reporting,
selection (word on double click, line on triple click), copy, paste (bracketed, with a
confirmation for multi-line paste into a shell prompt), search, links (OSC 8 and URLs,
Ctrl+click), and resize with reflow.

- **Threading.** Each terminal has its own thread. Output up to 4 KiB that arrives while nothing
  is queued (an echo) is parsed at once on the caller's thread, so the next frame shows it;
  larger output, resizes and searches go to the terminal's thread in order, which holds the grid
  for at most 16 KiB of parsing at a time (a fair lock), so a snapshot never waits more than a
  fraction of a millisecond. The window stops returning credit while more than 4 MiB of output
  waits to be parsed, so a flood is held back by the link, not by memory.
- **Snapshot** (`Terminal::snapshot(&mut Frame)`): rows of resolved cells — character, width
  (wide and spacer), foreground and background as the theme's default, an index 0–255 or RGB,
  and bold, italic, underline styles, strikethrough, dim and link flags; inverse and hidden are
  already resolved — plus the list of rows whose cells changed, the cursor (shape, visibility,
  the program's blink wish), the selection and search matches as cell spans, the modes, the
  scroll position and the title. Rows the emulator reports as damaged are compared with the
  previous frame's, so the list is exact. `Row::runs()` gives style runs.
- **Theme**: `Theme::DARK` and `Theme::LIGHT` hold the 16 colours, default colours, cursor,
  selection and search colours; both renderers resolve cells through them. Colour queries
  (OSC 10/11/4 — vim asks for the background) are answered from the current theme.
- **Windows targets**: ConPTY's *win32-input-mode* (`CSI ? 9001 h`) is recognised; keys that
  VT cannot carry exactly (Ctrl+Space, Ctrl+Backspace, Ctrl and Alt combinations, the editing
  and function keys) are then sent as Windows key events, so PSReadLine sees what Windows
  Terminal would give it. Plain text stays UTF-8.
- **Fixtures**: `crates/yaac-term/fixtures` holds output recorded from bash, vim, less, htop and
  PowerShell/PSReadLine at 69 × 28 (see its README); the tests replay them, and both front ends
  draw them for their screenshots.

### Drawing

Both front ends draw the same grid, cell for cell, with the same colours and font:

- **egui**: one mesh per frame for the whole grid, on egui's font texture: the default
  background, background runs as rectangles, the selection and search highlights, the glyphs,
  then the cursor — one draw call, never a text layout per cell. Each distinct character is laid
  out once as a one-character galley (so no ligatures) and its quad is cached and copied into
  cells; row meshes are cached and rebuilt only for damaged rows, and an unchanged frame reuses
  the assembled mesh. egui clears its glyph atlas when it is 80 % full (same size, new places),
  so the cache follows the identity of a probe galley and is rebuilt when it changes. Bold is the
  glyph drawn twice one pixel apart and italic a shear (the app ships only the regular face).
- **WPF**: one retained `DrawingVisual` per row, with `GlyphRun`s grouped by style
  and typeface. Only damaged rows are rebuilt.
- **Frame pacing**: output is parsed as it arrives and drawn at most once per display refresh;
  the frame after a keystroke always shows its echo. A hidden tab parses but does not draw.
- Font: the app's monospace font (CaskaydiaCove Nerd Font Mono, P315), with its ligatures off,
  box-drawing and block characters drawn as shapes so they join perfectly, bold as bold,
  wide characters (CJK, emoji) taking two cells. The cursor blinks only while the tab has
  focus, and only for 30 s after the last keystroke (no endless repaints, invariant 7).
- Colours: a 16-colour scheme per YAAC theme (light and dark), plus 256-colour and truecolor.
- If the WPF renderer cannot hold the frame budget, the fallback is Windows Terminal's WPF
  control for WPF only — kept in reserve, not built now.

### egui: measured

Release build, this repository's profile (`opt-level = "s"`, fat LTO), a 16-core laptop under
WSL2; numbers from the tests that hold them:

- **Parser** (`yaac-term`, `tests::throughput`): 100 MB of coloured output parsed at
  **120 MB/s**; a one-character echo plus its snapshot on a 200 × 60 grid: **4.6 µs**.
- **Flood** (`terminal_ui::tests::flood`): 100 MB streamed through the emulator and the egui
  renderer (grid, mesh, tessellation; 1100 × 700 points) as fast as it parses, 2 MiB per frame:
  **87 MB/s** end to end; frame time **p50 0.69 ms, p99 1.92 ms, max 3.73 ms** — every frame far
  under 16 ms; CPU 172 % of one core (the terminal's thread parsing, the UI thread drawing).
- **Typing**: echo bytes in → that frame's mesh painted and tessellated, worst of 200:
  **0.60 ms**; the echo is always in the frame that follows its bytes
  (`an_echo_is_on_screen_in_the_frame_after_its_bytes`). An unchanged terminal costs nothing:
  no row is rebuilt and the previous mesh is reused.

### Windows WPF

The Windows controller keeps the terminal grid in Rust. On the existing anonymous
presentation pipe, a `terminalFrame` identifies the tab and frame and carries the
height, cursor, input modes, and the replacement content of each damaged row. Each
row contains style runs with a starting column, cell width, foreground, background,
attributes, and UTF-8 text. Colours come from the shared theme palette. An empty row
clears its previous drawing. Selection, search and hovered-link changes invalidate
the affected rows, even when the underlying terminal text stays the same.

WPF decodes the rows on the pipe reader thread. At the next
`CompositionTarget.Rendering` callback it replaces the affected row visuals and
sends `terminal.ack` with the frame number and surface (side pane or chat panel).
Each surface holds at most one unacknowledged frame. Both can show a terminal at
once, even when their tab numbers coincide. Output received before the acknowledgement
accumulates in the emulator's damage map. Selecting a different terminal starts with
a full projection. The cursor has its own visual so blinking does not rebuild text.
The renderer unsubscribes from the composition callback after painting.

WPF reports the viewport in cells measured from the packaged monospace face.
`terminal.key` carries the logical non-text key and Shift, Alt, Ctrl and Windows
modifier bits. WPF shows the IME composition at the cursor and sends its committed
text once through `terminal.text`.
Mouse input carries the viewport cell, half-cell, button, click count and modifiers.
Paste, focus, clear, search, copy, restart and adding a selection use the same
controller pipe.

While a WPF tab title is being edited, the tab strip retains its text field across
shell title changes. Enter sends `terminal.rename` with the thread, strip, tab and
entered name to the shared controller. Escape and clicking away discard the edit.

A Windows Release fixture fed 100,000,000 ANSI bytes through `yaac-term` at
120 × 40. The projection sent 30 frames totalling 153,313 bytes. Rebuilding
the damaged row visuals took 0.25 ms at p50 and 4.32 ms at p99 and maximum.
For 120 single-row typing echoes, the time from receiving a complete pipe line
to finishing the visual was 0.11 ms at p50, 0.34 ms at p99 and 1.22 ms at
maximum. Opening the first tab also incurred a cold dispatcher wait of 33.86 ms.
These timings end before physical display scan-out.

A separate presentation-pipe stress run replaced all 80 rows of a 240-column
grid 4,610 times, sending 100,012,843 bytes. Each redraw in that workload used
21,692–21,695 bytes. Drawing took 0.20 / 0.87 / 8.85 ms at p50 / p99 / maximum.

### The tab

- Icon: a terminal prompt; title: the shell's title (OSC 0/2) or the shell's name, with the
  folder. A bell flashes the tab; a tab with new output while not shown gets a dot.
- **Rename inline** (P343): double-click the tab's title, press F2 while the tab strip has
  focus, or choose **Rename** in the tab's right-click menu. The title becomes a text field in
  place (all text selected); Enter keeps it, Esc or clicking away without changes cancels, and
  an empty name goes back to the automatic title. A name the user gave wins over the shell's
  OSC title until it is cleared. The name belongs to the terminal in `yaacd`
  (`terminal/rename`), so it survives the window closing, a reattach, and shows the same in
  every window and in `terminal/list`. The same for terminals in the side pane and in the
  panel under the composer, egui and WPF.
- Toolbar: the shell name and folder in secondary text; **Clear**, **Search** (Ctrl+Shift+F),
  **Restart** (after exit). Copy and paste: see [Clipboard and selection](#clipboard-and-selection).
- **Add to composer** on a selection (as in File tabs, [12](12-files.md#add-to-composer)):
  sends the selected output to the agent as a fenced `console` block.
- A click focuses the terminal. Settings → Keyboard has a **Works in the terminal**
  switch for each command. It defaults on for terminal commands and the full-screen tab
  switcher. While the terminal has focus, other bindings, including Ctrl+Space, reach the shell.

### Clipboard and selection

The owner: "nothing is more traumatic than the clipboard in terminals". Agreed rules (P339),
identical in egui and WPF:

**Copy**
- **Ctrl+C** copies when there is a selection (and clears it); with no selection it goes to the
  shell (interrupt). **Ctrl+Shift+C** always copies.
- **Right-click** with a selection opens a menu: Copy, Add to composer, Search for this; with no
  selection: Paste (and the tab's other actions).
- **Copy on select**: off by default; a setting (Settings → Terminal) turns it on.
- What is copied is clean: rows the terminal soft-wrapped are joined without a false newline,
  trailing spaces at line ends are trimmed, wide characters (CJK, emoji) are not doubled.
- **Selecting**: drag; double click selects a word, triple click a line; Shift+click extends;
  **Alt+drag** selects a block (columns).
- **Programs that capture the mouse** (vim, tmux, htop): **Shift+drag** always selects in the
  terminal instead of sending the mouse to the program.

**Paste**
- **Ctrl+V** and **Ctrl+Shift+V** paste, as does right-click with no selection. A setting lets
  Ctrl+V go to the shell instead (for bash's literal-next `Ctrl+V`).
- **Bracketed paste** whenever the shell asks for it (bash, zsh, PSReadLine), so pasted text
  never runs line by line.
- Line endings are normalised: `\r\n` and `\n` become `\r`, what a PTY expects, so nothing is
  doubled in PowerShell or bash.
- Several lines into a shell **without** bracketed paste: a confirmation "Paste 12 lines? They
  will run." with "Don't ask again".
- A large paste is written in pieces under the stream's credit, never freezing the window.

**The program setting the clipboard (OSC 52)**
- The shell runs on the target, so OSC 52 is how a remote vim or tmux yank reaches the host's
  clipboard. **Writes are allowed**, with a quiet notice "Copied by the terminal"; **reads are
  always refused**, so a remote program can never read what the user copied.

## The terminal panel under the composer

Besides terminal tabs in the side pane, each thread has a **terminal panel** at the bottom of
the chat column, under the composer — like an editor's integrated terminal (P338).

- A **terminal icon button** in the row under the composer, right after **Fork**, toggles the
  panel exactly as Ctrl+` does — so it is discoverable without knowing the key. Its hover text
  is "Terminal (Ctrl+`)" with the key as currently bound; it shows the selected fill while the
  panel is open. Hidden when the target has no `terminal` feature.
- **Ctrl+`** (the command *Toggle terminal panel*; rebindable in Settings → Keyboard, and in the
  Commander) opens the panel; pressing it again closes it. Opening it with no terminal yet
  starts one with the default shell. Opening focuses the active terminal; closing returns focus
  to the composer. Closing the panel only hides it: its terminals keep running (they live in
  `yaacd`) and are there again when it reopens.
- **Tabs**: the same tab strip as the side pane — same height, tab shape, icons, close **×**,
  shrinking and scrolling ([12](12-files.md#tab-strip)) — but every tab is a terminal, and
  **+** adds a new terminal with the default shell at once (no menu). Other shells: the
  Commander's *New terminal with shell…*, or the tab's right-click menu *New terminal with
  shell…*. Closing the last tab closes the panel. At the strip's right end: **Maximise panel**
  (the panel takes the chat column's whole height, the transcript and composer hidden; the
  button becomes **Restore**) and **Close panel** (same as Ctrl+`).
- **Size**: a draggable divider between the composer and the panel; default a third of the
  column's height, at least 120 px for the panel and 160 px for the transcript above the
  composer. The height is remembered per window.
- **Where it sits**: inside the chat column, so it spans only the chat's width; in split view
  the side pane stays to the right, untouched. In full screen it is part of the **Chat** tab.
  Related agents (pill or docked panel) keep their place beside the transcript, above the
  panel.
- **Per thread**: each thread has its own panel terminals (started in its worktree), and
  whether its panel is open. Switching threads shows that thread's panel state.
- A terminal in the panel is the same terminal as in a side-pane tab — same emulator,
  renderer, toolbar actions (in its right-click menu: Copy, Paste, Clear, Search, Add to
  composer, Restart) and performance budget. Both egui and WPF.

| Command | Does | Default key |
| --- | --- | --- |
| Toggle terminal panel | Open or close the panel under the composer. | Ctrl+` |
| New terminal | Add a terminal to the panel (opening it) with the default shell. | Ctrl+Shift+` |
| New terminal with shell… | Step: the target's shells; adds a terminal in the panel. | — |
| Next / Previous terminal | Switch tabs in the panel. | Ctrl+PageDown / Ctrl+PageUp while a terminal has focus |
| Maximise terminal panel | Toggle the panel taking the chat column's height. | — |

## Shell commands and the agent

Two ways a command the user runs reaches the agent (P344, P345). Research, verified against the
installed harnesses: [research/user-shell-commands-codex.md](research/user-shell-commands-codex.md),
[research/user-shell-commands-claude.md](research/user-shell-commands-claude.md).

### `!command` in the composer — the agent answers

Typing `!npm test` in a thread's composer and sending it runs the command on the thread's target
in the thread's folder, records the command and its output in the conversation exactly as the
harness's own `!` does, and **then the agent answers** (a model turn starts with it).

The window offers it on a thread whose effective capabilities include **`userShellCommand`**
and sends it as `turn/shellCommand { opId, threadId, command, delivery }`
([03](03-protocol.md#turns)); how the command runs is the provider's business inside `yaacd`,
never the UI's (invariant 4):

| Provider | How |
| --- | --- |
| Codex (native; codex-cli ≥ 0.170.0, verified on 0.170.0 and 0.172.0) | `thread/shellCommand` on the existing daemon connection: Codex runs it (its shell, the thread's cwd, no sandbox), emits its `commandExecution` item (`source: userShell`) in a shell-only turn, and records the `<user_shell_command>` record itself. When that turn completes, `yaacd` sends **`turn/start` with `input: []`**: the model's input then ends with the record and nothing invented (verified, [research](research/user-shell-commands-codex.md#making-the-model-answer-a--command-p344)). Both Codex turns are reported as **one** YAAC turn (the shell turn's id); stopped or failed, no answer is asked for. |
| Claude Code (emulated) | `yaacd` runs it in the session's folder with the target user's shell (`$SHELL -c`, `cmd /d /s /c` on Windows; timeout 120 s, then exit 124 and a note; Stop kills its process group, exit 130), streaming its output live. It then pushes **one** SDK user message in Claude Code's own markup — `<bash-input>cmd</bash-input>` (as typed) and `<bash-stdout>…</bash-stdout><bash-stderr>…</bash-stderr><bash-exit-code>N</bash-exit-code>` (ANSI stripped, carriage returns resolved, XML-escaped, each stream capped at 30 000 characters with `[output truncated: N more characters not shown]`) as **separate text blocks** — with `shouldQuery: true`, so the model answers in the same turn, as the CLI's `!` does by default. Its uuid is derived from the operation id (the CLI skips a uuid it has seen). The turn ends with the answer's `result`; a `result` with `num_turns: 0` (a context-only append) does not end it. Stopped while running: the record is still appended, context only (`shouldQuery: false`, `priority: "later"`), and no answer is asked for. |

- **Only from that thread's own composer**, typed by the user: `turn/shellCommand` is a
  host→daemon request, and nothing else in `yaacd` calls a provider's `shellCommand`. A message
  from another agent that starts with `!` is plain text, and so is `turn/start` with a leading
  `!`; no `yaac` tool runs a command.
- Sent while a turn runs it waits in the queue like any message (D166; `delivery: queue`, the
  entry's `shellCommand` set) and runs when the turn ends. There is no *now* for a command:
  Send now on a `!` draft queues too, and *Send now* on a queued command waits for the turn's
  end. Editing a queued command keeps it a command while its text starts with `!`; without
  the `!` it becomes an ordinary message.
- **The transcript, live**: `turnStarted` (origin `user`), a `userMessage` whose only part is
  the text `!command` (role `command`, carrying the `opId`, so it replaces the "Sending…" echo),
  a `toolCall` (`tool: command`, `source: userShell`, the typed command as `title`, `cwd`,
  streamed `output`, `exitCode`, `durationMs`), then the agent's answer, then `turnCompleted`.
  From history, Codex gives the `userShell` command item (titled by the typed command, not the
  `/bin/bash -lc` wrapper); Claude Code gives the `!command` user message and its "Shell output"
  / "Shell error output" / "Shell exit code" parts. The window draws all three as the same
  command block (below). **Stop** on the block is the turn's Stop: it stops the command while it
  runs (Codex: `turn/interrupt` of the shell turn aborts it), the answer after.
- Runs outside the sandbox, as the harness's own `!` does; nothing more is asked.
- Attachments are not sent with a command; they stay in the composer.

### How it looks (P346)

**The composer while the draft starts with `!`** (egui and WPF alike):

- The border and a 3 px bar on the left edge take a dedicated **shell colour** (a muted terminal
  green, `Palette.shell`, contrast-checked in light and dark; distinct from every state and
  speed colour), with a faint tint of it behind the field.
- A **prompt row** above the text: the terminal icon `>_`, then where it runs — target · folder —
  in secondary text.
- The `!` is absorbed into that row: the field shows `npm test`, in the **monospace font**.
- **Removing the bang returns to the normal composer at once**, keeping the text: Backspace
  with the caret at the very start of the field (with or without text after it), the small ×
  at the end of the prompt row, or select-all + delete. The frame, prompt row, font and
  buttons all go back to normal; typing `!` at the start again re-enters the mode.
- The primary button reads **Run ↵** with the terminal icon (**Queue run ↵** while a turn runs);
  Send now is hidden; the attach button is hidden; model, effort and mode stay.
- The hint line: "Runs here, then <harness> answers".
- **↑** on the first line walks back through this thread's earlier `!` commands.

**In the transcript** a command is not a chat bubble:

- One row: the prompt icon and the command in mono; at the right a status badge — a spinner and
  **Stop** while running, then ✓ `exit 0` or ✗ `exit N` and the duration.
- The output below on a code surface, drawn by the terminal renderer (same colours and glyphs),
  folded to about 12 lines with "Show all N lines".
- Then the agent's answer as a normal message.
- Commands appended from a terminal (P345) use the same block, smaller and dimmed, labelled
  "from terminal · context only".

### "Send commands to the agent" in a terminal — context only

A toggle in a terminal tab's toolbar (an icon; off by default; the tab shows a small mark while
it is on). While on, every command the user finishes in that terminal is **appended to the
thread's context without starting a turn** — the agent sees it with the user's next message.

- **Shell integration** marks each command's start, end and exit code: YAAC injects the
  standard prompt markers (OSC 633/133, as VS Code and Windows Terminal do) into bash, zsh and
  PowerShell when it starts the shell; a shell without them shows the toggle disabled with the
  reason.
- The record: the command line, its output (ANSI stripped, capped at 30 000 characters with a
  truncation note) and the exit code. Full-screen programs (alternate screen: `vim`, `htop`)
  send only the command line and "interactive program; output not captured".
- How it is appended, never re-running anything:

| Harness | Append |
| --- | --- |
| Codex | `thread/inject_items` with a user message holding the `<user_shell_command>` record — no execution, no turn. |
| Claude Code | an SDK user message with the bash markup, `shouldQuery: false`, `priority: "later"` — no turn; it joins the next prompt. |

- While a turn is running, records are held and appended when it ends.
- Every appended record also appears in the transcript (a quiet "sent from terminal" line,
  expandable), so the user always sees what the agent was given.

## Later

- Predictive local echo; split terminals; agents reading or driving a terminal.
