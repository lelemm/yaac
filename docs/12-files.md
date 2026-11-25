# 12 — Files, file view and diffs

The side pane that holds the browser ([11](11-preview.md)) also shows the thread's worktree: a
**Files** tab with the folder tree and git marks, **File** tabs with a file's current contents,
and **Diff** tabs with what changed. Text selected in a File or Diff tab can be added to the
composer. Everything is read-only.

The window and target implement P326–P329.

## The pane and its tabs

The browser area becomes the thread's **side pane**. Its tabs have a kind — **Browser**,
**Files**, **File**, **Diff** — and share the tab strip, the split with the chat, **Full
screen** and **Restore** ([11](11-preview.md#the-browser-area)). The title bar's **+** menu
lists **Browser** and **Files**. Each kind brings its own toolbar under the tabs. Tabs belong
to the thread and are not restored after a restart, like browser tabs. Files appears for connected targets advertising fileView and works in a build without the embedded browser. The tab-strip **+** also offers both kinds.

## Files tab

- The tree of the thread's worktree (its project folder on the target), loaded one folder at a
  time as folders are expanded. Folders first, then files, each sorted by name.
- **Git marks** after the name, coloured by kind: **M** modified, **A** added, **D** deleted
  (listed although the file is gone), **R** renamed, **U** untracked, **!** conflicted. A folder
  that contains any change shows a small dot.
- **Ignored** files and folders are hidden by default; **Show ignored** in the toolbar shows
  them grey. Outside a git repository there are no marks.
- The toolbar also has **Refresh** and **Collapse all**. The tree refreshes by itself when a
  turn of the thread ends, and about every 5 s while the tab is visible; never while hidden.
- **Click a name** → a **File** tab. **Click a mark** → a **Diff** tab. Opening a file or diff
  that already has a tab focuses that tab. Middle-click opens in a new tab even so.

## File tab

The current local version of the file on the target: line numbers, syntax colours, read-only,
selectable. Files above 4 MiB, and binary files, show a notice with their size instead. The tab
title is the file name; hovering shows the path relative to the worktree. **Reload** in the
toolbar reads it again; the tab also reloads when the Files tree sees its entry change, including its size, modification time or git mark. Reload cancels pending highlighting, and replies for closed tabs or earlier reads are discarded.

## Diff tab

The file at `HEAD` against the working copy (for an untracked file, everything is added; for a
deleted one, everything removed). **Unified** or **Side by side** (a toolbar toggle,
remembered), both with syntax colours, changed words highlighted inside changed lines, and
unchanged runs longer than 8 lines folded behind "⋯ N unchanged lines". **Next/previous
change** buttons visit the first row of each consecutive change. Unified groups removed lines
before added lines. Side by side pairs the lines of a replacement in order, with a blank cell
opposite each excess line. A file that is staged and changed again shows the working copy against
`HEAD`, which is what the agent's work amounts to.

## Add to composer

Selecting text in a File or Diff tab and right-clicking offers **Add to composer** (also
Ctrl+Shift+A). It inserts a chip at the composer's caret: `app.rs:120–148` for a file, `app.rs
diff 120–148` for a diff. When the message is sent, each chip becomes plain text the agent can
use with any harness — the path relative to the worktree, the line range, and the selected
lines in a fenced block (with the language for a file, `diff` for a diff, keeping the `+`/`-`
markers). The sent text starts with the relative path and one-based inclusive range, such as
`src/app.rs:120–148`. A chip can be removed like an attachment
chip. The draft keeps the excerpt alongside a compact inline reference. The window expands references when queuing or sending, preserving their positions among the typed text.

## Syntax colours

- **syntect** with **two-face**'s extended syntax set (the one `bat` ships, about 200
  languages) — pure Rust, nothing extra to compile for the Windows build.
- The language is chosen from the file name and extension, then the first line (shebang,
  modeline); an unknown language is shown plain.
- The shared Rust crate `yaac-code` loads the syntax set once on the first background job.
  Colouring runs once per loaded text. It yields **spans** with line-relative UTF-8 byte ranges
  and theme classes for both renderers, like the composer's Markdown styling (P317). The egui
  window draws them in its own viewer. WPF converts the ranges to UTF-16 for AvalonEdit.
  Lines longer than 20 000 bytes are shown plain. Closing or reloading a tab drops its job
  and cancels any remaining work at the next cancellation check.
- The background job builds both layouts with `similar`, first comparing lines and then
  finding changed words within paired replacements. It skips inline word comparison when
  either line exceeds 20 000 bytes. Comparisons have time budgets so difficult diffs can
  return a coarser result.
- The egui tree uses `egui_ltreeview`.

## Windows WPF

WPF uses the browser pane's existing tab strip and splitter. The Files tab uses a virtualized,
recycling `TreeView` with lazy folder children from the Rust controller. File and Diff tabs use
read-only AvalonEdit (MIT). A document transformer applies YAAC's light and dark palette to the
Rust syntax spans and changed-word ranges. Diff gutters show the original line numbers and
change signs. Both editors in side-by-side mode have aligned rows and share a vertical scroll
position. Clicking a folded row expands it, and its gutter control collapses it again.

The side-pane view over the existing WPF pipe carries tab kind, path, tree and toolbar state, and
the active document revision. Text, diff rows and UTF-16 span coordinates carry that revision
in separate `filesDocument` messages with UTF-8-safe chunks of up to 24 KiB before JSON framing.
The controller produces the projection on a worker and sends one chunk between controller
events. WPF assembles and prepares the editor document off the dispatcher. A superseded
revision can no longer update the view or supply a composer selection.

WPF sends `files.*` commands for tree actions, opening and reloading tabs, diff mode and change
navigation, and adding a selection. The controller returns the destination of a navigation
command in a `filesChange` message. For Add to composer, WPF sends the selected line range and
UTF-16 columns in the active revision. The controller extracts the original text, formats the
chip and inserts it at the composer's caret.

## Target methods

All additive and read-only. The daemon advertises `fileView` in `initialize` and the hub
reports `daemon.fileView` to the window. Requests take an absolute worktree `root` and a
relative `path`. Resolved paths must stay inside the root. The tree lists symlinks as such and
requests refuse to follow them outside the root.

| Method | Purpose |
| --- | --- |
| `fs/tree` | `{ root, path }` → `{ entries: [{ name, kind: file\|dir\|symlink, size?, mtime?, git?, ignored, changed }], repo, truncated }`. One folder's children, including deleted tracked files. An empty path lists the root. Git marks (`M`, `A`, `D`, `R`, `U`, `!`) and ignored flags come from one `git status --porcelain=v2 --ignored=matching -z` call at the repository root. `changed` marks a directory with changes below it. `mtime` is Unix milliseconds. The listing stops at 5 000 entries or the frame budget, whichever comes first. |
| `fs/read` | `{ root, path, maxBytes, offset? }` → `{ text?, binary?: true, size, truncated, nextOffset?, etag }`. `size` is the full byte count. If the file exceeds `maxBytes` or 4 MiB, `truncated` is true and text is absent. Bulk response. |
| `git/fileDiff` | `{ root, path, oldOffset?, newOffset? }` → `{ old?, new?, oldPath?, status, binary, nextOldOffset?, nextNewOffset?, etag }`. Compares the blob at `HEAD` (using the source path for a rename) with the working copy, each capped at 4 MiB. Untracked files have no old side, deleted files have no new side. For a rename into the requested root from outside it, the old path and text are omitted. Oversize results return `file_too_large` with the size in bytes. Bulk response. |

Text is UTF-8 with replacement for malformed bytes and the leading BOM removed. A NUL in
the first 8 KiB marks a version as binary. If either side of a diff is binary, the response
omits both texts.

Bulk responses page by bytes to stay under the frame limit and let control traffic go first
between pages. Omit offsets on the first call, then pass back each returned next offset on
the same method. A diff continuation requests only the sides whose offsets it includes. The
daemon cuts text on a UTF-8 boundary. Compare the opaque `etag` on every page with the first
and discard the accumulated text if it changes.

## Later

- Language servers on the target (hover, go to definition, references), through `yaacd`.
- Editing, staging and reverting from the Diff tab.
