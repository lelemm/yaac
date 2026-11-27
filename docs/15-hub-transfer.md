# 15 — Moving the hub to another machine, and several hubs on desktop

**Proposed** (P408–P413). The owner's request, branch `hub-transfer`. This is the working brief the
implementation follows; the contract worker turns its names into `yaac-protocol` types and keeps
this page true.

## What the user sees

- **Settings → Hub → Transfer hub to…** (desktop egui, WPF, Android). A drop-down lists every
  target with whether it can take the hub and why not (`hub/transfer/candidates`). Choosing one asks
  for a confirmation, then shows progress (`hub/transfer/progress`): checking, installing,
  copying (bytes), handing over, done / failed.
- When it is done every attached client (the window on the PC, the phone) is told the hub's new
  address (`hub/moved`), replaces the old hub in its saved hubs by the new one (same `hubId`), and
  attaches there. Nothing is lost: turns keep running in `yaacd` and are replayed (invariant 8).
- **Several hubs on desktop (P412).** The Android model (P405/P406) comes to egui and WPF: a list
  of saved hubs — "This PC" (the local hub, local socket) plus any number of remote hubs over
  Tailscale (`AttachConfig::remote`) — a badge with the selected hub's initial left of the YAAC
  mark only when more than one is saved, a drop-down to switch, and **Settings → Hubs…** to add,
  remove and retry. Desktop keeps **one attached hub at a time** (switching detaches and attaches);
  the phone keeps running all of them for notifications. Saved hubs live in the host's own
  settings file next to the window prefs, not in the hub cache (they say which hub to attach to).

## Which targets can take the hub (P408)

`hub/transfer/candidates` answers every target with `eligible` or a `reason` (a sentence):
- **SSH target on Linux** (x86_64 first; aarch64 when the release ships it): eligible when
  `yaacd` there reports Tailscale up on the **same Tailscale login** as this hub, port 47380 free on
  its Tailscale addresses, and no hub already running there.
- **WSL target**: not eligible ("A hub inside WSL would share Windows' Tailscale address"), and
  the local Windows target: not eligible (it is this machine).
- A disconnected target: not eligible ("Connect it first").
- A development build hub: not eligible (there is no release binary to install).

## How it works (P409)

One connection per target (invariant 1) holds throughout: the old hub uses its existing `yaacd`
stream to the chosen target X for everything, and the new hub connects to targets only after the
old hub has let go of all of them.

1. **Probe** — `hubHost/probe` to `yaacd` on X: os, arch, Tailscale status (`tailscale status
   --json`: running, login name, Tailscale IPs), whether 47380 is free, whether a hub runs there.
2. **Install** — `hubHost/install {opId, url, sha256, size}`: `yaacd` on X downloads the Linux hub
   binary of **the hub's exact build** from the GitHub release (the URL and hash come from the
   release `manifest.json`, asset `yaac-hub-linux-<arch>`), checks size and SHA-256, and puts it in
   `~/.local/share/yaac/bin/yaac` (staged, not yet started). If X cannot reach GitHub the hub
   streams the bytes it downloaded itself (same `hubHost/put` path as the files below).
3. **Copy** — `hubHost/put {opId, name, size, sha256}` + stream chunks (stream credit, like
   attachments) into a staging dir `~/.local/share/yaac/transfer-<opId>/`. First a live copy of
   `cache.sqlite` (SQLite online backup into a temp file, then streamed), then:
4. **Quiesce** — the hub tells clients "moving to X" (progress `handingOver`), stops accepting
   state-changing client requests (`hub_moving` error), closes every target connection except X,
   checkpoints `relay.sqlite`, `drafts.sqlite`, `cache.sqlite`, and copies the final `relay`,
   `drafts`, the cache again (only if changed since the live copy), and a **rewritten
   `hub.json`** (P410). Agent messages in flight are in `relay.sqlite`, so they move with it
   (invariant 13).
5. **Arm** — `hubHost/start {opId, armOnClose: true}`: `yaacd` validates the staging dir and
   replies; it will move staging into `~/.config/yaac` and `~/.local/share/yaac`, enable start at
   login (`yaac autostart sync --start`, XDG autostart; plus a `systemd --user` unit when systemd
   is there), and start `yaac hub` detached **only when this connection closes**. So the two hubs
   never hold targets at the same time.
6. **Hand over** — the old hub writes a **tombstone** `moved.json` in its data dir (`{hubId,
   address, host, at}`), turns its own start-at-login off, sends every client `hub/moved {hubId,
   address, hostName}`, closes X, and exits. On the PC, `yaac hub` (and `autostart sync --start`)
   with a tombstone does not start; the window reads it and attaches to the new address.
7. **Come back** — the new hub starts, binds its Tailscale addresses (Settings → Android "allow
   over Tailscale" is forced on for a moved hub, since every client is now remote), connects its
   targets, and clients attach with their normal reattach backoff. `yaacd` writes
   `~/.local/share/yaac/transfer-<opId>.result` (`started` or the error), which `hubHost/probe`
   reports later.

**Failure.** Before step 6 any failure is reported (`failed` with the reason), staging is
deleted (`hubHost/abort`), and the old hub reconnects its targets and carries on — nothing has
moved. After step 6, if the new hub does not answer, the window on the PC says so with **Start the
hub here again** (deletes the tombstone and starts the local hub; the data on the PC is the state
of the hand-over). No step is retried by itself (invariant 10); each request carries the opId.

## The rewritten hub.json (P410)

- X becomes the new hub's **local** target (`transport: local`), same target id, so its threads
  keep their keys.
- Other **SSH** targets stay, with YAAC's **own generated key** (the only key YAAC keeps,
  invariant 12) copied with the hub and deleted from the PC after the hand-over. A target that
  used the user's own key or agent stays but is marked "Sign in again on the new hub" (YAAC never
  copies the user's keys).
- **WSL** targets and the PC's **local** Windows target cannot be reached from Linux: they stay in
  the list, disabled, with "On <pc>: reachable only by a hub on that PC". Their threads stay in
  their `yaacd` journals; moving the hub back brings them back.
- `hubId` is kept (clients recognise the same hub); `hostName` becomes X's.

## The Linux hub binary (P411)

The release builds `yaac` for `x86_64-unknown-linux-gnu` with `--no-default-features --features
hub` (no window, no eframe), as `yaac-hub-linux-x86_64`, and lists it in `manifest.json` with
its size and SHA-256. It runs `yaac hub` headless; Tailscale admission (`tailscale whois`) is the
same code as on Windows. It is not a desktop app on Linux: no window, no tray.

## Contracts (names; the types are in `yaac-protocol`)

Attach (window/phone ⇄ hub):
- `hub/transfer/candidates {}` → `{ candidates: [{ targetId, label, eligible, reason? }] }`
- `hub/transfer/start { opId, targetId }` → `{ }` once accepted; then progress notifications.
- notification `hub/transfer/progress { opId, targetId, phase, done?, total?, message? }`,
  phase ∈ `checking | installing | copying | handingOver | moved | failed`.
- notification `hub/moved { hubId, address, hostName }` — sent just before the old hub exits.
- error code `hub_moving` for requests refused during the hand-over.

Daemon (hub ⇄ `yaacd` on X):
- `hubHost/probe {}` → `{ os, arch, tailscale: { running, login?, ips[] }, portFree, hubRunning, lastTransfer? }`
- `hubHost/install { opId, url, sha256, size }` → `{ path }` (progress via `hubHost/progress`)
- `hubHost/put { opId, name, size, sha256 }` → a stream to write into (stream credit)
- `hubHost/start { opId, armOnClose }` → `{ armed: true }`
- `hubHost/abort { opId }` → `{ }`
- capability `hubHost` in `initialize` (a `yaacd` without it is not a candidate: "Update YAAC on
  that target first").
