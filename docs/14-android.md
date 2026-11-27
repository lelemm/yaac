# 14 — Android

**Proposed** (P372–P375). Post-MVP ([01](01-overview.md#mvp-scope)). This design was agreed
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
([09](09-compatibility.md)). A phone never replaces or restarts the hub.

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

## Open points

- The browser host. On desktop, the tabs that agents drive live in the window. Which client hosts
  them when both are attached?
- Key files, themes, fonts and skill folders imported from the phone's storage must be sent to the
  hub. Today the desktop reads them on the host.
- Delivering notifications in the background: Android's limits on foreground services must be
  checked on real phones.
