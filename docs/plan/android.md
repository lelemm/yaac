# Android client: implementation plan

**Proposed** (P376). The design is in [14 Android](../14-android.md); the screens are in
[design/android-screens.md](../design/android-screens.md), with their parity checklist in
[design/android-parity.md](../design/android-parity.md).

## Shape

The Android app runs **the same Rust window controller as the Windows WPF renderer**, compiled
into the app as a native library. Today the WPF renderer is a thin view. The controller
(`crates/yaac-app/src/wpf.rs`, `serve`) sends JSON view frames, receives JSON commands, and holds
the state, the hub attach, and every feature. The Android app does the same in-process, through
JNI instead of anonymous pipes. Jetpack Compose draws the frames, with the mobile layout from
the screen brief.

```
 Android app (one process)
 ┌───────────────────────────────────────────────┐
 │ Kotlin / Compose UI  ◄── view frames (JSON) ──│
 │                      ── commands (JSON) ────► │  libyaac_android.so
 │                                               │  (yaac-app controller + yaac-core)
 └───────────────────────────────────────────────┴──► attach over Tailscale (TCP) ──► yaac hub
```

- **Parity comes by construction.** Every feature lives in the controller that WPF already
  drives. The Android UI only draws, and sends what WPF sends.
- **One connection per target stays the hub's** (invariants 1 and 14). The phone opens one
  connection only, the attach to the hub.
- **The same build.** The app ships the controller from the same tree as the hub, so `hub/hello`
  matches. A mismatch shows the existing "update" screen. Dialects across builds come later
  (14, Versions).

## Work packages

| WP | What | Where | Done when |
| --- | --- | --- | --- |
| **A0** | Toolchain: NDK 27, the Rust targets `aarch64-linux-android` and `x86_64-linux-android`, `cargo-ndk`, the Gradle wrapper, an emulator AVD. | the machine; `android/` | `cargo ndk -t x86_64 build -p yaac-android` and `./gradlew assembleDebug` both work. |
| **A1** | **Attach over Tailscale.** The hub listens for attaches on its Tailscale address, as well as on the local socket, when **Settings → Android → Allow the Android client over Tailscale** is on (off by default). It accepts a connection only when `tailscale whois` on the peer names the same Tailscale login as the hub host. Otherwise it closes the connection, and nothing is logged beyond the refusal. The framing and the methods are the same as the local socket's. `HubClient::connect_tcp`. | `crates/yaac-hub` (`server.rs`, `client.rs`, `settings`), `yaac-protocol` if a setting is added | Integration test: a hub with a TCP listener on loopback and a fake whois accepts the same login, refuses another, and runs `hub/hello`. Docs: 02 (the attach), 07 (the setting). **Done (P377):** port 47380; `yaac_hub::tailnet`; `crates/yaac-hub/tests/tailscale.rs`. |
| **A2** | **The controller on Android.** Make the `serve` loop independent of its transport (a channel pair instead of stdin and stdout), and make `yaac-app` build for Android with a feature that leaves out the desktop-only pieces (eframe and winit windowing, tray, rfd, arboard, fontdb scans, updater, CEF preview host). Each piece is replaced by a hook that the Android side fills (file picking, clipboard, opening URLs). New crate `crates/yaac-android` (`cdylib`): JNI exports `start(config)`, `command(json)`, `nextFrame() / waker`, `stop()`. A `demo` config runs the fleet scenario without a hub. | `crates/yaac-app` (features, `wpf.rs` → a transport-agnostic `view_protocol`), `crates/yaac-android` | `cargo ndk -t arm64-v8a -t x86_64 build -p yaac-android` builds. Host test: the demo fleet emits the same first frame through the channel transport as through the pipe. WPF behaviour is unchanged (its tests pass). **Done (P378):** `yaac-app` feature `desktop` (default; `yaac-android` builds with `default-features = false, features = ["hub"]`); `yaac_app::platform` hooks; `yaac_app::view_protocol::ViewController` (the WPF `serve` loop over channels; test `view_protocol::tests`); `crates/yaac-android` exports `dev.yaac.android.core.NativeController` (handles are table keys, not pointers); hub mode uses `AttachConfig::remote` → `HubClient::connect_tcp`. A build mismatch with that hub reads "The hub on <pc> is a different YAAC build. Update YAAC on the PC or on this phone so they match." (`AttachConfig::mismatch_text`): the phone never shuts down, restarts or replaces the hub. |
| **A2b** | **The platform hooks on Android.** `crates/yaac-android/src/bridge.rs` implements `PlatformHooks` with JNI calls into `dev.yaac.android.core.PlatformBridge` (Kotlin, `@JvmStatic`, called on controller workers, blocking on a future while the system UI runs on the main thread). Pickers: photo picker or document picker for attachments, `OpenDocument`, `OpenDocumentTree` (copied recursively), `CreateDocument` for saving a staged copy; clipboard text and copied files; links via `ACTION_VIEW`; documents through a FileProvider (`${applicationId}.yaac.files`, `res/xml/yaac_bridge_paths.xml`). `PlatformBridge.init(activity)` sets `TMPDIR` and the `YAAC_*_DIR` variables. Files from the phone go to the hub as bytes (06). | `crates/yaac-android`, `crates/yaac-app` (`platform`, `files`), `android/…/PlatformBridge.kt` | A photo attached to the demo composer on the emulator shows its chip and thumbnail. |
| **A2c** | **Texts for touch.** `yaac_core::input::InputMode` (`pointer`, `touch`). The Android config's `input` (default `touch`) and the `shell.inputMode` command (`{"mode": …}`, e.g. when a mouse is attached) set it. Only texts that name an input mechanic change, per the mapping table in design/android-screens.md: the Commander's reasons "Nothing is selected… (Ctrl+click rows)" → "tap a row's state mark" and "Focus the request card first (Ctrl+J)" → "Open the request card first (Needs you)", and Settings → Keyboard's intro. Key caps stay in the frames: the app hides them without a hardware keyboard. Never per provider. `yaac_core::input::Platform` (`shell.platform`, set to `android` by `yaac-android`) does the same for Settings → Window: **Keep running in the background** (the tray preference, with a line on the notification's Open and Quit), the renderer as one read-only line, and no Tailscale section (that is on Settings → Android, whose line on the phone says the setting belongs to the hub on its PC). | `yaac-core` (`input`, `commands`), `yaac-app` (`wpf`, `view_protocol`), `yaac-android` | Tests: `reasons_that_name_an_input_mechanic_follow_the_input_mode`, `touch_input_changes_only_the_texts_that_name_a_mechanic`; pointer output unchanged. |
| **A2d** | **Request notifications.** The hub publishes its reminder's unseen list to subscribed clients and accepts answers by revision (P379); the controller shows it as `shell.requests` once `shell.requestFeed` is sent (the Android config's `requests`, default on) and takes `shell.requestAction` and `shell.requestOpen`. The demo hub does the same. | `yaac-protocol` (attach), `yaac-hub` (notifications), `yaac-core` (client, demo), `yaac-app` (wpf, view_protocol), `yaac-android` | Tests: `the_feed_is_the_reminders_unseen_list_even_without_a_desktop_notifier`, `a_feed_subscriber_sees_the_unseen_requests_and_answers_with_the_current_revision` (wp11), `the_request_list_is_sent_only_when_asked_and_answers_with_its_revision`. WPF frames are unchanged. |
| **A2e** | **Side-pane data in the demo.** Scene `SidePanes` ("Side panes: files, diffs, a terminal, images (users-service)", the last in `Scene::ALL`) turns on the file view and terminals for users-service's machine. The demo hub then answers `fs/tree`, `fs/read`, `git/fileDiff` and `upload/read` for the `api` worktree: every git mark, changed and ignored folders and files, a binary and an oversize file, a rename, a deletion, a merge conflict, and multi-hunk diffs. The demo terminal scripts `git status`, `cargo test` and `bell`; `exit N` ends it. users-service gains a turn with a screenshot from a tool, a diagram in its final message and two published files. The browser stays "This build has no browser host" on Android: there is no CEF host there. | `yaac-core` (demo: `sidepane.rs`, `terminal.rs`), tests in `yaac-app` | `the_side_pane_scene_serves_a_worktree_files_and_diffs`, `the_side_pane_scene_puts_a_screenshot_a_diagram_and_published_files_in_the_transcript`. |
| **A2f** | **The browser on Android (P380).** Attach method `hub/preview/tunnel` / `tunnelClose` (the hub relays a target `tunnel/open` over the attach connection's data frames, credit both ways); the controller's loopback SOCKS endpoint (`tunnel_proxy`), one target at a time; the external host (`external_host`: `browserHost` state frames, `browserHostCall` agent actions with the desktop's page scripts, `browser.hostEvent` / `browser.hostResult` / `browser.history`); the WebView host `android/…/core/BrowserHost.kt`. The demo machines serve a dev server on port 5173 through the same tunnel. | `yaac-protocol`, `yaac-hub` (`core/attach_tunnel.rs`), `yaac-core` (client, demo), `yaac-app`, `yaac-android`, `BrowserHost.kt` | `a_tunnel_over_the_attach_connection_reaches_a_target_server_and_closes` (real daemon), `a_socks_client_fetches_the_target_page_through_the_controllers_proxy`, `agent_actions_go_to_the_host_with_the_desktops_page_scripts_and_list_is_answered_here`. On a device: A3's pass. **Deferred by the owner (2026-11-26):** `BrowserHost.kt` stays unwired; the Android config's `browserHost` is off by default, so no browser action (tab, "Open in integrated browser") is offered until the app passes `"browserHost": true`. The system WebView aborts on the software emulator; testing needs a real phone or a KVM emulator. |
| **A3** | **The Compose app.** `android/`: Gradle with Kotlin, Compose and the NDK libs; the YAAC theme from the palette tokens (dark, light, custom); the view-frame store (whole frames and the transcript diff, as WPF applies them); and the screens in the order below, sending the commands WPF sends. The reference for each view is its WPF counterpart in `tools/wpf-preview`. | `android/` | Runs on the emulator with the demo fleet; screenshots of each finished screen are put beside the mockup frame they implement. |

**Order of screens in A3**, following the user's daily path:

1. The list (A01–A03).
2. The thread: header, transcript, request cards, composer, control row (A05, A07, A09–A14).
3. Related agents (A08) and Needs you.
4. Notifications and running in the background (A21).
5. The rest, in the brief's order, until every parity id is ticked.

A1 and A2 run in parallel. A3 starts on the demo fleet, first through a desktop-built controller
over `adb reverse` (the TCP dev transport), then in-process once A2 lands. The UI code does not
depend on which transport carries the frames.

## Rules carried over

- Nothing about models is hardcoded. The Kotlin code never reads a provider kind; it draws what
  the frames say (invariants 3 and 4).
- The UI thread never blocks. JNI calls queue; frames arrive through a waker, and Compose
  recomposes only on change (invariant 7).
- Unsafe code is confined to `crates/yaac-android`'s JNI boundary, which opts out of the
  workspace's `unsafe_code = "forbid"` with a comment saying why. No other crate changes its
  lints.
- No CPU- or GPU-bound test in CI. Android builds are not part of CI until the owner says so.
