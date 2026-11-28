# 15 — Moving the hub to another machine, and several hubs on desktop

**Proposed** (P408–P414).

## What the user sees

- **Settings → Advanced → Hub → Transfer hub to…** (desktop egui, WPF, Android). A drop-down lists every
  target with whether it can take the hub and why not (`hub/transfer/candidates`). Choosing one asks
  for a confirmation, then shows progress (`hub/transfer/progress`): checking, installing,
  copying (bytes), handing over, done / failed.
- When it is done every attached client (the window on the PC, the phone) is told the hub's new
  address (`hub/moved`), replaces the old hub in its saved hubs by the new one (same `hubId`), and
  attaches there. Nothing is lost: turns keep running in `yaacd` and are replayed (invariant 8).
  The phone keeps the entry's key, slot, label and data directory (device drafts, left-off marks,
  notification routing) and only changes its address ([14](14-android.md#several-hubs-p405)).
- **Several hubs on desktop (P412).** The Android model (P405/P406) comes to egui and WPF: a list
  of saved hubs — "This PC" (the local hub, local socket) plus any number of remote hubs over
  Tailscale (`AttachConfig::remote`) — a badge with the selected hub's initial left of the YAAC
  mark only when more than one is saved, a drop-down to switch, and **Settings → Hubs…** to add,
  remove and retry. Desktop keeps **one attached hub at a time** (switching detaches and attaches);
  the phone keeps running all of them for notifications. Saved hubs live in the host's own
  settings file next to the window prefs, not in the hub cache (they say which hub to attach to).
  On the desktop that is `hubs.json` in the config dir, beside `display.json`. The window's own
  drafts for a remote hub stay on the PC (P393), under `remote-hubs/<address>/`.

### In the desktop window

The window controller (`crates/yaac-app`, shared by egui and the WPF view frames) does all of it;
the renderers only draw ([07](07-ui.md#several-hubs-on-desktop)):
- **Transfer hub to…** asks `hub/transfer/candidates` each time Settings → Advanced is shown
  (and on **Refresh**). Ineligible targets are greyed with their reason. **Transfer hub…** is
  enabled once an eligible one is picked and no transfer runs; it asks for confirmation and sends
  one `hub/transfer/start` with a new opId. A refusal or an unknown outcome is shown and never
  re-sent (invariant 10). Every client shows the progress, whoever started it.
- **`hub/moved`**: the saved entry with that `hubId` (else This PC) takes the new address, any
  other entry for the same hub or address is dropped, and the window attaches there. The first
  attempts fail until the new hub listens, and the normal reattach backoff (1 s, doubling, at most
  30 s) carries on. When the moved hub was This PC, This PC's entry *becomes* the remote one.
  The phone's controller only exposes it as `shell.hub.movedTo`, and the Android app restarts that
  hub's controller on the new address.
- **At start**, a tombstone in the local hub's data dir moves This PC's entry the same way before
  anything attaches, so the window never starts a hub that has moved.
- **Start the hub here again** is offered (Settings → Hubs, Settings → Advanced → Hub, and in place
  of the empty workspace) when the selected hub is the one that moved from this PC and has not
  answered for 20 seconds. After a confirmation it removes the tombstone
  (`remove_moved_tombstone`), turns the entry back into This PC, and attaches to the local hub,
  which starts as usual.

## Which targets can take the hub (P408)

`hub/transfer/candidates` answers every target with `eligible` or a `reason` (a sentence):
- **SSH target on Linux** (x86_64 first; aarch64 when the release ships it): eligible when
  `yaacd` there reports Tailscale up on the **same Tailscale login** as this hub, port 47380 free on
  its Tailscale addresses, and no hub already running there.
- **WSL target** (a distro of this PC), from a Windows hub: eligible on the same terms as an SSH
  target. The distro must run **Tailscale itself**: its own `tailscaled`, its own machine and
  100.x address on the tailnet, on the same login. Windows' Tailscale seen from inside WSL is not
  enough: its address through mirrored networking, or Windows' `tailscale.exe` through interop.
  A probe whose Tailscale addresses include one of this PC's is refused for that reason. Port
  47380 must be free on the distro's own address, and no hub may run there. Otherwise: "Tailscale
  is not running inside this WSL distro. Install it there (it needs its own machine on your
  tailnet)." The hub then lives in WSL, kept running from Windows ([below](#into-this-pcs-wsl-p414)).
- The local Windows target: not eligible (it is this machine).
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
   attachments) into a staging dir `~/.local/share/yaac/transfer-<opId>/`. The destination sends
   `hubHost/putComplete` after it verifies and stages the file. The hub waits for this outcome
   before continuing. First it streams a live copy of `cache.sqlite` from a SQLite online backup.
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
   address, hostName, atMs}`), turns its own start-at-login off, sends every client `hub/moved {hubId,
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
of the hand-over). Steps are never retried automatically (invariant 10). Each state-changing
request carries the transfer's `opId`.

## The old hub's side, as built (`crates/yaac-hub/src/core/transfer.rs`)

- **Eligibility** (`hub/transfer/candidates`): the transport first (SSH; WSL only when the
  hub's `TransferHost` names this PC's `yaac.exe` for WSL, so only YAAC for Windows: "Only YAAC
  for Windows can move the hub into WSL."), then a
  development build (every target: "…a development build…; there is no release binary"), a hub
  whose binary supplies no release lookup ("This build of YAAC cannot move its hub."), not
  connected ("Connect it first."), no `hubHost` capability ("Update YAAC on that target first.").
  Only targets left after that are probed, on their existing connection. Their probe must say:
  Linux; an arch with a published hub (`x86_64` for now); Tailscale running; the same login as
  this hub's own node (`tailscale` on this computer, compared without case); a Tailscale
  address that is not one of this computer's; 47380 free; no hub already running. A WSL distro
  that fails the Tailscale checks gets the WSL sentence above. Every row has a sentence when it is not eligible. While a transfer runs,
  every row says the hub is already moving.
- **Release asset**: a `TransferHost` (`yaac_hub::transfer`) supplied by the `yaac` binary
  (`updater::HubTransfer`). It finds the release of exactly this build and the manifest asset
  `yaac-hub-linux-<arch>` (URL, SHA-256, size), and it turns start-at-login off. Builds without
  the `desktop` feature (the Linux hub itself) have no HTTP client and cannot move again yet.
- **Files put, in order**, one stream at a time, each waiting for its `hubHost/putComplete`:
  `data/cache.sqlite` (live `VACUUM INTO` through a separate read-only connection), then after
  the quiesce `data/relay.sqlite`, `data/drafts.sqlite`, `data/cache.sqlite` (only if its hash
  changed), YAAC's own keys `data/ssh/keys/<slug>/id_ed25519(.pub)`, `data/ssh/known_hosts`
  (the host keys the user confirmed), and `config/hub.json` last. The final copies are
  `VACUUM INTO` on the store thread itself, after every write queued before them.
- **During the hand-over** only reads are served (target list, status, settings, view,
  drafts and operations lists, UI state, finds, blob reads). Every other client request, including
  `hub/shutdown` and routed requests, gets `hub_moving`. `mesh/relay` and
  `mesh/deliveryReceipt` from X get `hub_moving` too: they would write `relay.sqlite` after its
  final copy, so the daemon keeps them for the new hub. The other targets are stopped with "The
  hub is moving to X." and restarted if the transfer fails.
- **Once `hubHost/start` is sent**, a connection to X that drops counts as moved, not as a
  failure. `yaacd` may have armed the new hub, and that hub starts when this connection closes.
  So the old hub writes its tombstone and stops (no two hubs). If nothing started, the window
  offers **Start the hub here again**.
- **The tombstone** is `<data>/moved.json` (`yaac_hub::paths::MovedTombstone`). `run_hub`
  checks it under the hub lock and returns `HubError::Moved`, and `yaac hub` exits successfully
  with a message. **Start the hub here again** is `yaac hub --reclaim`, which deletes the
  tombstone and runs the hub. The window can also call
  `yaac_hub::paths::remove_moved_tombstone` and then start the hub as usual.
- The hub exits with `ExitReason::Moved` after `hub/moved` reaches every client that said hello,
  whatever its build, and X's connection is closed.
- **Not yet**: streaming the binary when X cannot reach GitHub; deleting YAAC's key from the PC
  after the hand-over; a time limit on an unanswered `hubHost/*` request (the transfer waits,
  and the user sees the phase it is in).

## The rewritten hub.json (P410)

- X becomes the new hub's **local** target (`transport: local`), same target id, so its threads
  keep their keys.
- Other **SSH** targets stay, with YAAC's **own generated key** (the only key YAAC keeps,
  invariant 12) copied with the hub and deleted from the PC after the hand-over. A target that
  used the user's own key or agent stays but is marked "Sign in again on the new hub" (YAAC never
  copies the user's keys). In `hub.json` the mark is the target's `signInAgain` field (a
  sentence).
- **WSL** targets and the PC's **local** Windows target cannot be reached from Linux: they stay in
  the list, disabled, with "On <pc>: reachable only by a hub on that PC" (the target's
  `movedUnreachable` field). Their threads stay in
  their `yaacd` journals; moving the hub back brings them back.
- **Into this PC's WSL** they stay usable. The other distros keep `transport: wsl`: a hub inside
  WSL finds `wsl.exe` through interop (`/mnt/c/Windows/System32/wsl.exe`, else `wsl.exe` on
  `PATH`, else `YAAC_WSL_EXE`). The PC's local target becomes `transport: windowsInterop, exe:
  "/mnt/c/…/yaac.exe"` (this PC's `yaac.exe` under WSL's default automount root), same id.
- `hubId` and `clientId` are kept (clients recognise the same hub; each `yaacd` sees the same
  client); `hostName` becomes X's; `tailscaleAttach` is on.

## Into this PC's WSL (P414)

WSL stops a distro that nothing runs in, so a hub there cannot be left to `yaacd` or systemd. A
small Windows process, the **keeper**, runs it instead:

- **Arm without starting.** The old hub sends `hubHost/start {opId, armOnClose: true, startHub:
  false}`. When the connection closes, `yaacd` in the distro only promotes the files, the binary
  included. It enables no start-at-login entry and starts nothing. Then it writes
  `<data>/transfer-<opId>.result` with status `placed` (atomically). `<data>` is `YAAC_DATA_DIR`,
  else `$XDG_DATA_HOME/yaac`, else `~/.local/share/yaac`.
- **The keeper first, then the tombstone.** In the tombstone step the old hub starts `yaac
  wsl-hub --distro <d> [--user <u>] --after-transfer <opId>` detached. If the keeper cannot
  start, the transfer fails and nothing has moved. Then the old hub writes `moved.json` with
  `wslDistro: <d>` and replaces its start-at-login entry by `yaac wsl-hub --distro <d> [--user
  <u>]`. On Windows that is the `Run` value `YAAC hub`. Then `hub/moved` goes to every client,
  X is closed, and the hub exits as for any move.
- **The keeper** (`crates/yaac-app/src/wsl_keeper.rs`) runs once per hub profile (named event
  `Local\<pipe>-wsl-hub`) and has no window:
  - It starts no hub unless this PC's tombstone names its distro. A keeper left over after
    **Start the hub here again** never runs a second hub.
  - After a transfer it waits up to 60 s for the old hub's lock to be free.
  - It runs `wsl.exe -d <d> [-u <u>] --exec sh -c <script> sh [<opId>]`
    (`yaac_hub::transfer::keeper_wsl_argv`). With an opId, the script waits up to 120 s for the
    result file and stops unless it says `placed`. Then it `exec`s `<data>/bin/yaac hub`, so the
    keeper's `wsl.exe` lives exactly as long as the hub.
  - Output goes to `<data>/wsl-hub.log` on Windows.
  - When the hub exits, the keeper exits. It restarts nothing (invariant 10). The next login
    starts it again.
- **`yaac quit`** also signals the keeper (`<name>-quit`). The keeper runs `yaac quit` in the
  distro (`hub/shutdown`, stores checkpointed) and waits up to 20 s for the hub to exit. Then it
  ends its `wsl.exe` and exits. **`yaac autostart sync`** keeps the keeper's entry while the
  tombstone names a distro. With `--start` it starts the keeper, not `yaac hub`, so an update
  restarts the hub in WSL.
- **The new hub** in the distro is the Linux hub (P411). It binds the distro's own Tailscale
  address, never Windows', so Windows' port 47380 stays free for a hub here again. It reaches
  the PC's Windows `yaacd` as `<yaac.exe> local-daemon`
  (`yaac_hub::interop::InteropConnector`): one child process per target, as always
  (invariant 1). That command starts the Windows local `yaacd connect` bridge exactly as the
  Windows hub does (`yaac_hub::config::local_config`). It copies bytes between the bridge and
  its standard streams.
- **Start the hub here again** (offered as for any move). When the tombstone names a distro, it
  first stops the keeper, which stops the hub there (on a thread of its own: it waits). Then the
  login entry goes back to `yaac hub`, the tombstone goes, and the local hub starts. If the
  keeper could not stop it, nothing else happens and the window says why. Its confirmation
  says that the hub in WSL is stopped first and that whatever changed there since the move stays
  there.

## The Linux hub binary (P411)

The release builds `yaac` for `x86_64-unknown-linux-gnu` with `--no-default-features --features
hub` as the stripped release asset `yaac-hub-linux-x86_64` and lists it in `manifest.json`
with its size and SHA-256. `yaac hub` runs headless and uses the local XDG socket and the
same Tailscale admission as on Windows (`tailscale` on `PATH`). The binary also supports
`yaac quit`, `yaac autostart`, and `yaac --version`.

The release embeds `yaacd.mjs`. On the new local target one stage 0 process reuses the
existing daemon and Node.js installation, with the same rules as SSH. It installs the embedded
daemon if that build is missing and reuses the pinned YAAC Node.js or an existing Node.js 22+
before fetching the pinned runtime. The daemon keeps its existing journals under `~/.yaac`
(or `YAAC_HOME`).

TODO: Publish a Linux aarch64 hub build.

## Contracts (names; the types are in `yaac-protocol`)

Target transport: `windowsInterop { exe }`, the PC's Windows `yaacd` reached from its WSL hub.

Attach (window/phone ⇄ hub):
- `hub/transfer/candidates {}` → `{ candidates: [{ targetId, label, eligible, reason? }] }`
- `hub/transfer/start { opId, targetId }` → `{ }` once accepted; then progress notifications.
- notification `hub/transfer/progress { opId, targetId, phase, done?, total?, message? }`,
  phase ∈ `checking | installing | copying | handingOver | moved | failed`.
- notification `hub/moved { hubId, address, hostName }` — sent just before the old hub exits.
- error code `hub_moving` for requests refused during the hand-over.
- error codes `transfer_not_eligible`, `transfer_in_progress`, and `transfer_unavailable` for
  a transfer start that the hub cannot accept.

Daemon (hub ⇄ `yaacd` on X):
- `hubHost/probe {}` → `{ os, arch, tailscale: { running, login?, ips[] }, portFree, hubRunning, wsl?: { distro }, lastTransfer? }`
- `lastTransfer` is `{ opId, status, message? }` with `status` of `started`, `placed` or `failed`.
- `hubHost/install { opId, url, sha256, size }` → `{ path }`
- notification `hubHost/progress { opId, done, total }` reports download progress in bytes.
- `hubHost/put { opId, name, size, sha256 }` → `{ stream, credit }`. Send data frames on
  `stream` as credit permits.
- notification `hubHost/putComplete { stream, message? }` confirms staging on success and
  carries an error message on failure.
- `hubHost/start { opId, armOnClose, startHub }` → `{ armed: true }` (`startHub` defaults to
  `true`; `false` places the files only, for a hub in this PC's WSL)
- `hubHost/abort { opId }` → `{ }`
- capability `hubHost` in `initialize.features` (a `yaacd` without it is not a candidate: "Update YAAC on
  that target first").
