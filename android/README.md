# YAAC for Android

The Android client ([docs/14-android.md](../docs/14-android.md), plan in
[docs/plan/android.md](../docs/plan/android.md), work package A3). It is a thin view over the
YAAC window controller, the same one the Windows WPF renderer drives
(`crates/yaac-app/src/wpf.rs`). The controller sends JSON view frames and receives JSON commands
(`{"command": "...", "payload": {...}}`). This app draws the frames with Jetpack Compose and
sends the commands WPF sends. The reference for every view is its counterpart in
`tools/wpf-preview`.

## Build

```sh
cd android
./gradlew assembleDebug          # app/build/outputs/apk/debug/app-debug.apk
```

Gradle 9.4.1 (wrapper), AGP 9.2.1 with its built-in Kotlin, the Compose compiler plugin 2.3.10,
Compose 1.10, Material 3 1.4 (used only for sheets and dialogs, restyled to YAAC tokens).
`minSdk` 29, `targetSdk` 36. The SDK path is in `local.properties` (not committed).

## Connecting

On first launch the app asks **Connect to your YAAC hub**: the hub's Tailscale name (MagicDNS)
or 100.x address, port 47380 unless `host:port` is given, or **Try the demo**. The choice is
saved (SharedPreferences) and reused; intent extras override it. When the hub does not answer
or refuses the hello (another build), the list shows which hub and why, with **Retry** and
**Change hub**; when the controller ends (`{"stopped": {"error"}}`), a screen says why, with the
same two actions. **Settings → Change hub…** (below the pages) returns to the connection screen.
The phone and the hub must be the same build (`hub/hello`): install the APK built from the same
tree as the PC's YAAC.

## Transports

The UI does not know which transport carries the frames (`core/Transport.kt`).

- **In-process (production).** `dev.yaac.android.core.NativeController` loads
  `libyaac_android.so` from `app/src/main/jniLibs/<abi>/` (built by A2 from
  `crates/yaac-android`) and calls `start(configJson)`, `command(handle, json)`,
  `waitFrame(handle, timeoutMs)` and `stop(handle)`. It is used whenever the library is packaged.
- **Dev TCP.** Without the library, the app connects to `127.0.0.1:47100`. Run a desktop-built
  controller behind a TCP bridge, one controller per connection, and forward the port:

  ```sh
  ~/.cargo/bin/cargo build -p yaac-app
  socat TCP-LISTEN:47100,reuseaddr,fork EXEC:"target/debug/yaac --wpf-controller fleet dark"
  adb reverse tcp:47100 tcp:47100
  ```

  Intent extras override the choice: `--es transport tcp|native`, `--es host`, `--ei port`,
  `--es scenario`, `--es theme`.

## Layout

| File | What |
| --- | --- |
| `core/ViewStore.kt` | Frame merging as WPF does it: each view replaces the previous one; the transcript's `rowChanges` patch rows by id and `order` reorders; a transcript without `prompts` keeps the old index. Only changed views are republished, so Compose recomposes only on change. |
| `core/Transport.kt`, `core/NativeController.kt` | The two transports. |
| `ui/Theme.kt` | The palette tokens of `Palette.cs` (dark, light, custom from `shell.customPalette`), the bundled fonts, `shell.fontScale`. |
| `ui/ListScreen.kt` | A01: the list (sidebar frame). |
| `ui/ThreadScreen.kt`, `ui/Transcript.kt`, `ui/Markdown.kt` | The thread: header, banners, transcript rows, request cards. |
| `ui/Composer.kt` | The composer, queue, running commands and control row. |
| `ui/Agents.kt` | The agents pill and the Related agents sheet. |
| `ui/agents/AgentsModel.kt` | The agents frame as plain values (pill entries, sheet order and folds, budget, time-limit menu, row under a held finger), unit-tested in `AgentsModelTest`. |
| `ui/Common.kt` | Marks, buttons, action sheets (context menus), `shell.dialog`. |
| `ui/Commander.kt` | The Commander (`shell.commander`) and the header sheet (A06). |
| `core/Runtime.kt` | `Connection` (hub, demo or dev TCP; saved, overridable by intent extras) and `YaacRuntime`: the process's one controller, which outlives the activity. |
| `ui/Connect.kt` | The connection screen and the failed-connection screen. |
| `ui/Strip.kt` | The strip at medium width (A04). |
| `notify/Notifier.kt`, `notify/YaacService.kt` | A21: the foreground service ("Keep running in the background", `shell.trayOnClose`) with the ongoing "YAAC · N need you" and Quit; one sticky high-importance notification per thread that needs you, withdrawn when answered anywhere, summarised from two up. |
| `core/PlatformBridge.kt` | A2's platform hooks (pickers, clipboard, URLs); `init` runs before the controller starts. |
| `ui/SidePane.kt` | The side pane (A23, A24): its state (file documents in chunks, terminal screens, `filesChange`, paste confirmation, `terminalReadPaste`, notices), the tab strip (pinned Chat, icons, marks, overflow, full screen, Rename), the Files tree. |
| `ui/pane/FilePane.kt`, `ui/pane/TerminalPane.kt` | A file or diff tab; a terminal tab. |
| `ui/pane/*Model.kt`, `ui/pane/ComposerCaret.kt` | Their pure logic, unit-tested on the JVM: the `terminalFrame` wire and screen, hardware-key routing, cells and swipes, paste; the diff layout with folds, selection → `files.add`, change navigation; tab labels and marks; the composer's saved caret. |
| `ui/transcript/` | The transcript's logic in plain Kotlin, unit-tested: the scroll anchor (`Anchor.kt`: restore by `anchorToken`, `transcript.anchor` reports, keeping the top row through folding), Copy message and the code gutter (`CopyText.kt`), the image viewer's zoom (`Zoom.kt`), mermaid saving (`DiagramFiles.kt`), the header's budget reason (`HeaderBudget.kt`); `Viewers.kt` draws the image viewer and the mermaid preview. |

## Behaviour notes

- **Back.** The system Back walks YAAC's history (`shell.back`) while `shell.canBack` holds,
  then returns to the list. The ← in the thread's top bar goes straight to the list. Forward and
  the Back/Forward history lists are in the thread's ⋮ menu (`shell.forward`, `shell.history`).
- **Transcript text (T05).** One `SelectionContainer` covers the whole transcript, so a touch and hold starts a selection whose handles run across rows; the toolbar's Copy (and Ctrl+C) joins rows by line. Buttons, disclosure labels, chips, times and code line numbers are in `DisableSelection` and are never copied. A tap on a message shows its time and **Copy message**.
- **Selection (A03).** A tap on a row's state mark adds or removes it; once a selection exists, a
  tap anywhere on a row does. The selection bar sends `bulk.*` with the ids; Move lists the
  sections that offer a drop (`section.drop`, with the controller's reason when refused).
  Every change is reported with `sidebar.selection`; `selectionReset` clears it. Selected rows
  have a drag handle: dragging it over a group outlines the group and shows "Move to …" or the
  controller's refusal; a drop sends `section.drop` with the ids.
- **The composer's text.** The field is the source of truth while typing. As in WPF's
  `UpdateText`, the controller's `text` is ignored when it is an echo of one of our own edits
  and adopted otherwise (sent, cleared, a queued message opened for editing).
- **The controller belongs to the process** (`YaacRuntime`), not to the activity. Closing the
  window keeps it, and its attach, alive while Keep running in the background is on; Quit (in
  the ongoing notification) ends the client, never the hub. The native controller starts on the
  frame thread, and the library is loaded there too (invariant 7).
- **Request notifications follow the hub's reminder list** (`shell.requests`, P355): one per
  request, nested workers included, Allow once / Deny where the hub offers them
  (`shell.requestAction` with the list's revision; a stale one is refused, never re-sent), Open
  (`shell.requestOpen`), sound only when the count grows, a summary from two up. The request
  cards on screen are reported with `shell.visibleRequests`, and none when the app is paused.
  With an older controller without `shell.requests`, the list's needs-you rows stand in.
- **The foreground service starts when the UI thread is idle**, so a slow first frame never
  turns its start into an ANR.
- **Settings** (`ui/Settings.kt`): the page list, then a page with Back and its name. Rows,
  toggles (switches), radio and choice fields, text fields, and actions are drawn from the
  `settings` view; `settings.page`, `settings.action{id, values}`, `settings.close`.
  Toggles among a row's actions get their own labelled line; a disabled action's reason sits
  under the actions; key bindings are shown, not edited (they are recorded by pressing keys).
- **First run** (A15) is the `settings` view in `firstRun` mode: the sentence, the sections as
  stacked cards with their actions, and the footer. Start a thread, a Commander pick and Needs
  you open what they open: the app follows the open thread or draft (`shell.threadId`, else the
  composer `key`) for 20 s after such a command.
- **Medium and expanded width** (A04, ≥ 600 dp): the list and the thread side by side, with
  one hairline between them; horizontal system insets (the landscape navigation bar, a cutout)
  are kept clear. Medium width (600–839 dp) uses the strip (`ui/Strip.kt`, D103): the mark or
  Needs you, New, Back and Forward, vertical project names, a harness logo per thread with its
  state badge (tap opens; touch and hold shows the row card), the rate-limit bars and Settings.
  Expanded width (≥ 840 dp) shows the full list.
- **Dialogs** use the full width on a phone (Fork carries long previews); Create pull request has
  its own form (Into, Title, Description, Draft, Ask the agent), as WPF's `PullRequestForm`.
- **Window notices:** `shell.status` (Dismiss → `shell.dismissStatus`) and a card per stopped
  WSL distro (`shell.stoppedWsl`, Start it → `shell.startWsl`) on the list; the thread shows
  the status as a banner.
- **Key bindings** are recorded with a hardware keyboard: tap the key cap, press the chord
  (`settings.action{id, values:{key, ctrl, alt, shift, command}}`, WPF's key names); Esc
  cancels, Backspace or Delete clears.
- **Testing:** see "Testing on the emulator" for the emulator settings this needs.
- **Theme.** Dark, light, system (`shell.systemTheme` on every UI-mode change) and custom
  (`shell.customPalette`); the system bars follow it.
- **The composer.** `/` and `$` show the controller's skill picker; `@` queries files
  (`composer.fileQuery`) for the token at the caret; a draft starting with `!` on a thread with
  `shellCapable` switches to `!` mode; an empty composer offers the message history
  (`composerMessageHistory`); touch and hold **+** offers Paste through the controller.
- **Files and terminals** (`ui/SidePane.kt`, `ui/pane/`). A file or diff: tap a line, tap
  another to extend, then Copy or Add to composer (`files.add`, at the composer's caret as last
  reported by `composer.edit`/`composer.caret`); touch and hold selects text; a fold opens on a
  tap. A terminal: swipes scroll back (wheel notches), touch and hold then drag selects (word
  first), a tap focuses the keyboard. With a hardware keyboard every key reaches the shell except
  bindings marked Works in the terminal (`shortcuts` → `shell.shortcut`). The phone shows only the
  side pane's terminals, not the panel under the composer.
- **Demo scenes.** In the demo, touch and hold the YAAC mark for `shell.scenes`.

## Testing on the emulator

Without KVM the emulator runs in software (`ANDROID_I_WANT_MY_TCG=yes emulator -avd yaac-a3
-no-window -gpu swiftshader_indirect -accel off`); it boots in about ten minutes. `adb shell input
text` drops keys on it when typing fast, so type in short bursts. `adb shell setprop
log.tag.yaac-cmd DEBUG` logs the name of every command the app sends (`yaac-frames` logs
which views each frame changed).

Changes made to the shared AVD `yaac-a3` (`emulator-5554`) for testing, to undo with the values
in brackets if the emulator is used for anything else:

- `hide_error_dialogs` was set to `1` for a while and is back to `0`: with it on, Android kills
  the app on an ANR instead of asking. The ANRs are real on this software-emulated device (a
  touch can take over 5 s to handle) and are answered with **Wait**.
- `adb root` [`adb unroot`]: to read `/data/anr` traces.
- `window_animation_scale`, `transition_animation_scale`, `animator_duration_scale` set to `0`
  [`1`]: faster, deterministic screenshots.
- `POST_NOTIFICATIONS` granted to `dev.yaac.android` with `pm grant`.

The fonts in `res/font` are copies of `crates/yaac-app/assets/fonts` (Ubuntu Sans Nerd Font
Propo, CaskaydiaCove Nerd Font Mono; licences beside the originals).
