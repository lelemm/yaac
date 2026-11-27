# 14 — Android

**Proposed** (P372–P375, P403, P405, P406). Post-MVP ([01](01-overview.md#mvp-scope)). This design was agreed
between a Claude thread and a Codex Astra thread, from the owner's direction. It is waiting for
the owner to review it.

## What it is (P372)

The Android app is **a second client of the hub**, and it works the same way as the Windows
window: the same attach protocol, the same behaviour, the same features. The hub still owns every
target connection and stays the only writer of the cache (invariants 1 and 14). The phone never
connects to a target.

```
 yaac (Windows window) ──local attach──►  yaac hub  ──one stream each──►  yaacd on every target
 Android client ────attach over Tailscale──►  (the same hub)
```

**Reaching the hub (P373).** The phone reaches the hub over **Tailscale**. It has no pairing step
of its own and no QR code: the phone and the hub host are on the same tailnet, and the attach is
the one the window uses, carried over that network instead of the local socket. The hub binds
only its Tailscale addresses, and admits only devices of its own Tailscale login, checked with
`tailscale whois` before anything is read. So the local socket's guarantees (user-only, the same
owner) hold for the phone too. It is off until the user turns on **Settings → Android → Allow
the Android client over Tailscale** ([02](02-architecture.md#attaching-over-tailscale), P377).

**Versions.** The window and the hub are always the same build, but a phone updates on its
own. When the builds differ, the attach adapts through the dialect layer, as `yaacd` does
([09](09-compatibility.md)). A phone never stops, replaces or restarts the hub. The hub enforces
this: `hub/shutdown` and the start-at-login and idle-exit settings are refused from a client
over Tailscale (`remote_not_allowed`). The phone shows that setting read-only. It can do one
thing when it is newer: ask the PC to install the phone's build. **Settings → About → Update
YAAC on <pc> to build <n>** appears only when both are release builds and the phone's number is
higher (P403). After a confirmation, the hub downloads that release's installer, verifies it,
and runs it. The phone follows the progress, shows *"<pc> is updating…"* once the connection
drops, and attaches again on the matching build. This uses the frozen `hub/update/request`
([09](09-compatibility.md#the-frozen-core), [install](install.md#updating-from-the-phone)).

**Updates.** Until dialects across builds exist, the phone must run the hub's exact build. Each
release carries the Android app, signed with one release key, and the phone offers the APK of
**the hub's build** when they differ, and Check for updates in Settings (the latest release only
with the warning that it needs the PC on the same build). Nothing is downloaded or installed
without a tap; the system installer does the install. Details in
[android/README.md](../android/README.md#releases-and-updates).

**Two clients, one owner.** The window and the phone are the same user. Everything that already
holds for two attached windows holds here too: an action carries an operation id, a request is
resolved once, and a late tap on an old card cannot answer a newer request.

## How it is built (P376)

The app runs **the same Rust window controller as the Windows WPF renderer**, compiled into the
app as a native library (`crates/yaac-android`). Jetpack Compose draws the view frames that the
controller produces, and sends back the same commands WPF sends. All the features therefore live
in one place, and parity comes by construction. The controller attaches to the hub over
Tailscale. The plan is in [plan/android.md](plan/android.md).

## Parity (P374)

The phone has **the features of the Windows client that make sense on a phone**, and none of its
own. The design work is how each feature is delivered on a small touch screen. Features with no
use on a phone (diagnostics and troubleshooting, desktop renderer and window mechanics,
desktop-only developer tooling) are marked *not needed on Android* in the list, each with its
reason. This was the owner's call. The full list, with sources, is in
[design/android-parity.md](design/android-parity.md). The screens, and the mapping from desktop
mechanics to touch, are in [design/android-screens.md](design/android-screens.md). The generated
mockups are in [design/mockups/android/](design/mockups/android/).

The mechanics that do not exist on a phone map to Android mechanics that do the same job (P375):

| Desktop | Android |
| --- | --- |
| Hover | Touch and hold |
| Right-click | Touch and hold, which opens the menu as a bottom sheet |
| Ctrl / Shift selection | A tap on the state mark; Select range and Select all |
| Keyboard, Commander | The Commander has its own icon; with a hardware keyboard, every key binding works as on desktop |
| Back and forward | The system Back gesture; Forward and the history lists in ⋮ |
| The tray | "Keep running in the background": a foreground service with the ongoing notification "YAAC · N need you" and **Quit** |
| Sticky toasts | High-importance notifications with the same actions |
| Side pane and divider | Tabs at compact width; the split at expanded width |
| File pickers, drag and drop | The Android system pickers, and the share sheet |

Medium and expanded widths (foldables, tablets) use the desktop's own narrow-window rules, and
at expanded width the desktop layout as it is.

## The link (P393)

The phone's link is often mobile data, so the attach carries only what is on screen:

- **Drafts stay on the phone.** Typing sends nothing over the link: the composer's drafts are
  the phone's own, kept on the device ([06](06-attachments.md#drafts-across-a-restart)), never
  in the hub's draft store and never shown in another client.
- **Reconnection keeps the user's place.** After the phone sleeps or changes network the
  controller attaches again by itself; the open thread stays open (its transcript is read
  again once attached) and its composer keeps what was typed, including during the outage.
- **Only open threads stream.** The hub sends a thread's events only to clients that
  subscribed to it; a transcript evicted from the client's small recently-used set lets go of
  its subscription.
- **The sidebar fills a page at a time.** A phone starting without an index requests a 24 KiB
  first page from each connected target. It can show those rows while the rest of the index
  arrives (P400).

Measured with `crates/yaac-app/tests/remote_link.rs` (`--ignored`): the Android controller
through a 75 ms-each-way, 1 Mbit/s loopback proxy to a real hub and `yaacd` (fake provider).
Echo of a key in the controller: about 3 ms at the 90th percentile, independent of the link.
Typing 59 keys: 0 bytes on the link (about 1.4 KB before P393). Idle: 0 bytes. Streaming a
400-word answer in the open thread: about 19 KB down.

## Several hubs (P405)

The phone can be attached to **several hubs at once**: the home PC and the work PC, say. Each
hub is a separate attach with its own controller, as if each had its own phone. Nothing about one
hub reaches another, and each hub still sees one ordinary client.

- **One controller per hub.** The app starts one `NativeController` per saved hub, each with
  its hub's address and its **own `dataDir`** (the device drafts of P393 live there). A second live
  controller on a `dataDir` already in use is refused. The platform hooks (pickers, clipboard,
  opening links) are shared, and each call blocks the controller that made it, so its answer
  returns to that controller.
- **Each hub's own choices.** Each controller asks its own hub for the stored choices
  (`hub/uiState/get`), such as the theme, and writes them back there. Two hubs can have two
  themes.
- **Which hub is which.** `hub/hello` names the hub: `hubId` (stable per hub install) and
  `hostName` (the PC's name) ([02](02-architecture.md#attaching-over-tailscale)). The shell view
  shows them as `hub.id`, `hub.name` and `hub.address` (the address the controller dials). Two saved
  addresses that answer with the same `hubId` are one hub. An older hub sends neither field, and
  the phone then knows that hub by its address and the label the user gave it.
- **On screen (P406).** One hub is shown at a time, with its own list, threads, Settings and
  theme; the others run hidden (`shell.visible` hidden: their hubs stream no open thread). A badge
  with the selected hub's initial is attached left of the YAAC mark, only when more than one hub
  is saved; a drop-down switches between hubs and shows each hub's name, address, connection and
  which one is selected. **Settings → Hubs…** adds, removes and retries hubs. Every hub's requests
  raise notifications, tagged with the hub's name; opening one switches to its hub and opens the
  thread, and its Allow once / Deny go to that hub. A call stays with its hub when another is
  selected. Details in [07](07-ui.md#several-hubs-on-android).
- **Cost.** Each saved hub is an attach and a controller kept running (some MB of native memory
  and the radio waking for its frames) whether or not it is shown — the price of its
  notifications. Remove a hub you no longer use.

## Open points

- The browser host. On desktop, the tabs that agents drive live in the window. Which client hosts
  them when both are attached?
- Key files, themes, fonts and skill folders imported from the phone's storage must be sent to the
  hub. Today the desktop reads them on the host.
- Delivering notifications in the background: Android's limits on foreground services must be
  checked on real phones.
