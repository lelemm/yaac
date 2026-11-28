# 15 — Moving the hub to another machine, and several hubs on desktop

**Proposed** (P408–P413, P415, P416; P414 is superseded by P416).

> **The owner's rule.** Hub transfer → the hub must be on one of the targets (Windows / WSL /
> SSH). Android usage → Tailscale with the hub.

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
- **`hub/moved`**: the saved entry with that `hubId` (else This PC) takes the new way in, any
  other entry for the same hub or way in is dropped, and the window attaches there. The first
  attempts fail until the new hub listens, and the normal reattach backoff (1 s, doubling, at most
  30 s) carries on. The window follows `via`, never Tailscale ([P415](#the-pcs-window-p415)). With
  `via: local` (back on this PC's Windows) the entry is This PC again, and the window waits for
  the tombstone to go before it attaches over the local socket. An older hub sends no `via`:
  the window then uses `address`, and the moved This PC entry *becomes* the remote one.
  The phone's controller only exposes it as `shell.hub.movedTo`, and only with an address (it is
  null when Tailscale does not run where the hub went). The Android app restarts that hub's
  controller on the new address.
- **At start**, a tombstone in the local hub's data dir moves This PC's entry the same way before
  anything attaches, so the window never starts a hub that has moved. A tombstone from build
  1059 (`wslDistro`, no `via`) is read as `via: wsl` for that distro.
- **Start the hub here again** is offered (Settings → Hubs, Settings → Advanced → Hub, and in place
  of the empty workspace) when the selected hub is the one that moved from this PC and has not
  answered for 20 seconds. After a confirmation it removes the tombstone
  (`remove_moved_tombstone`), turns the entry back into This PC, and attaches to the local hub,
  which starts as usual.

## Which targets can take the hub (P408)

`hub/transfer/candidates` answers every target with `eligible` or a `reason` (a sentence), and
for an eligible one a `warning` about phones. **Tailscale never decides.** It only decides
whether phones can reach the hub there:
- **A Linux target** (an SSH server, or a WSL distro of this PC from a Windows hub): eligible
  when it is connected, its `yaacd` has `hubHost`, there is a published hub for its arch
  (`x86_64` for now), this hub is a release build, and no hub already runs there.
- **The PC's Windows** (`windowsInterop`, the target a hub in WSL has for it, or any Windows
  target): eligible when `yaacd` there finds this exact build of YAAC installed (`hubBuild`,
  from `yaac.exe hub-info`). Nothing is downloaded. Otherwise "Install YAAC on X first." or
  "YAAC on X is build B and this hub is A: install the same build there first."
- **Port 47380** is checked only when X has a Tailscale address of its own: it must be free there.
- **Warnings** (shown in the confirmation, never a refusal):
  - No Tailscale on X, or X only sees this computer's own Tailscale (WSL with mirrored
    networking): "The phone can't reach the hub there until Tailscale runs on X." Then
    `hub/moved` has an empty address. The moved hub listens as soon as Tailscale comes up there
    (it asks again every minute).
  - Another Tailscale login on X than this hub's (or this computer has no Tailscale): "The hub
    will accept only devices signed in to Tailscale as L; your phone must use that account."
    The moved hub admits devices of its own login, as always.
- Not eligible: the hub's own computer ("This is the hub's own computer."), a disconnected
  target ("Connect it first."), a `yaacd` without `hubHost` ("Update YAAC on that target
  first."), another OS ("The hub runs on Linux or Windows; X is …"). A development build cannot
  move to Linux (there is no release binary). A Linux hub, which has no release lookup, cannot
  move to Linux either ("This build of YAAC cannot move its hub.").

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

- **Eligibility** (`hub/transfer/candidates`): the transport first. SSH is allowed. WSL is
  allowed only when the hub's `TransferHost` names this PC's `yaac.exe` for WSL, so only from
  YAAC for Windows: "Only YAAC for Windows can move the hub into WSL." `windowsInterop` is
  allowed. Then, for SSH and WSL only, a development build ("…a development build…; there is
  no release binary") and a hub whose binary supplies no release lookup ("This build of YAAC
  cannot move its hub."). Then for every target: not connected ("Connect it first."), no
  `hubHost` capability ("Update YAAC on that target first."). Only targets left after that are
  probed, on their existing connection. `judge` takes the probe as [above](#which-targets-can-take-the-hub-p408).
  `warning` adds the phone sentence; it compares logins without case and treats an address of
  this computer's own as none. While a transfer runs, every row says the hub is already moving.
- **To Windows** the install step is skipped (phase `installing` never shows). After the
  quiesce the rewrite is [`ToWindows`](#the-rewritten-hubjson-p410), and `hubHost/start`
  has `startHub: true`. `yaacd` on Windows places the files where `yaac.exe hub-info` says,
  removes the PC's `moved.json`, and runs `yaac.exe autostart sync --start`
  ([03](03-protocol.md#hub-transfer)).
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
- **To this PC's Windows** (from a hub in its WSL): X becomes `local`, the old hub's own `local`
  target becomes `wsl {distro}` again (`WSL_DISTRO_NAME`; from an SSH server it is unreachable
  as above), and the other WSL and SSH targets stay.
- **Into this PC's WSL** they stay usable. The other distros keep `transport: wsl`: a hub inside
  WSL finds `wsl.exe` through interop (`/mnt/c/Windows/System32/wsl.exe`, else `wsl.exe` on
  `PATH`, else `YAAC_WSL_EXE`). The PC's local target becomes `transport: windowsInterop, exe:
  "/mnt/c/…/yaac.exe"` (this PC's `yaac.exe` under WSL's default automount root), same id.
- `hubId` and `clientId` are kept (clients recognise the same hub; each `yaacd` sees the same
  client); `hostName` becomes X's; `tailscaleAttach` is on.

## Into this PC's WSL (P416)

**The owner's decision.** A hub that moves into WSL runs there as a **systemd system service**
(`/etc/systemd/system/yaac-hub.service`), so nothing on Windows has to start it ("otherwise
the hub always depends on something starting it, like a Windows app"). **WSL is started on
demand**, and the service then starts the hub by itself. Nothing on Windows needs elevation or
raises a UAC or Firewall prompt (the owner's Windows account is not an administrator). This
replaces the Windows-side keeper of P414.

### What keeps a distro running (verified 2026-11-28, WSL 2.6.3.0, `systemd=true`)

- **A systemd service does not keep the distro running.** Microsoft's systemd page says it
  plainly: "systemd services will NOT keep your WSL instance alive. Your WSL instance will stay
  alive in the same way it did previous to this update"
  (<https://learn.microsoft.com/en-us/windows/wsl/systemd>). A distro stops a few seconds after
  its last Windows-side client (`wsl.exe`, a terminal, an interop process) goes:
  `instanceIdleTimeout`, default 15000 ms, "The number of milliseconds that a distro is idle,
  before it is shut down. Set to -1 to disable auto shutdown"; the VM follows after
  `vmIdleTimeout` (default 60000 ms, Windows 11)
  (<https://learn.microsoft.com/en-us/windows/wsl/wsl-config>). `general.instanceIdleTimeout`
  came with WSL 2.5.4 ("Introduce general.instanceIdleTimeout to control distribution
  termination timeouts", <https://github.com/microsoft/WSL/releases>). Users report the same
  on 2.6.x: "WSL2 shuts down despite active systemd service"
  (<https://github.com/microsoft/WSL/issues/13416>).
- **Nothing starts a distro by itself at Windows login.** No Microsoft page says WSL ever boots
  a distro on its own. `[boot] command` in `wsl.conf` runs "when the WSL instance starts", not at
  login. A distro starts when something on Windows runs in it: `wsl.exe`, a terminal, `\\wsl$`.
  A per-user login entry (the `HKCU\…\Run` key, no admin) running `wsl.exe -d <d> --exec
  /bin/true` would boot it, but the distro would stop again 15 s later. With
  `instanceIdleTimeout=-1` in `%UserProfile%\.wslconfig` (a per-user file, no admin) it would
  stay up, for every distro of that user. YAAC sets neither (see the owner's choice below).
- **Not tested live.** A live test would have to leave the distro with no Windows-side client,
  and the session that checked this runs inside that distro. The docs above decide it. Checked
  live, harmlessly: `wsl.exe -d Ubuntu -u root --exec id` gives `uid=0(root)` with no password
  and no elevation, and a Windows `.exe` started from a systemd-managed process with an empty
  environment (`systemd-run --user … env -i … cmd.exe /c ver`) runs: interop falls back to
  `/run/WSL/1_interop`. So a system-service hub can reach the PC's Windows `yaacd` as
  `<yaac.exe> local-daemon`.
- **The owner's choice**: the hub runs while the distro runs. The window attaching starts the
  distro (its bridge is `wsl.exe`), and systemd starts the hub. Phones reach the hub over the
  distro's own Tailscale **only while the distro runs**. The transfer confirmation says so.

### How it is done

- **Eligible** only when `yaacd` in the distro reports `wsl.service` in `hubHost/probe`
  (`{ systemd, user, uid, dataDir, environment }`) with `systemd: true`. Without it the row
  says "Update YAAC in X first." With systemd off it says "The hub runs in X as a system
  service, and systemd is off there: add systemd=true under [boot] in /etc/wsl.conf, then
  restart X (wsl --terminate X)." The unit is built from the probe at that point
  (`yaac_hub::transfer::system_unit`). A user name or data directory that cannot go into a
  unit fails the check, before anything moves.
- **Arm without starting.** The old hub sends `hubHost/start {opId, armOnClose: true, startHub:
  false}`. When the connection closes, `yaacd` in the distro only promotes the files, the binary
  included. It enables no start-at-login entry and starts nothing. Then it writes
  `<data>/transfer-<opId>.result` with status `placed` (atomically). `<data>` is `YAAC_DATA_DIR`,
  else `$XDG_DATA_HOME/yaac`, else `~/.local/share/yaac`.
- **The service's install first, then the tombstone.** In the tombstone step the old hub writes
  the unit text to `<data>/wsl-hub-<opId>.service` on Windows. Then it starts, detached, `yaac
  wsl-hub install --distro <d> --data-dir <dir> --unit-file <f> --after-transfer <opId>`
  (`crates/yaac-app/src/wsl_service.rs`). If that cannot start, the transfer fails and nothing
  has moved. Then come `moved.json` (`wslDistro: <d>`, `via: wsl`), start-at-login of the hub
  **off** on this PC (as for any move), `hub/moved` to every client, X closed, exit.
- **The install** runs once and has no window:
  - It waits up to 60 s for this PC's old hub to exit (its lock). So no connection to the
    distro is left when it opens its own short one (invariant 1, the same rule as SSH Setup).
  - Then it runs `wsl.exe -d <d> -u root --exec sh -c <install script> sh <unit> <dir> <opId>
    /etc/systemd/system` (`yaac_hub::transfer::wsl_service_install_argv`). WSL lets the
    Windows user be root in their own distros without a password, so **no password is asked,
    sent or kept** (invariant 12).
  - The script waits up to 120 s for the result file and stops unless it says `placed`, then
    checks `<dir>/bin/yaac`. It writes the unit and runs `systemctl daemon-reload` and
    `systemctl enable --now yaac-hub.service`.
  - Output goes to `<data>/wsl-hub.log` on Windows. A failure is not retried (invariant 10): the
    moved hub does not answer, and the window offers **Start the hub here again**.
- **The unit** runs `<dir>/bin/yaac hub` as `yaacd`'s own user (`User=`). It sets `yaacd`'s
  `HOME`, `YAAC_CONFIG_DIR`, `YAAC_DATA_DIR`, `YAAC_RUNTIME_DIR`, `YAAC_HOME` (when set) and
  `PATH`. So the service, `yaac hub-bridge` in a WSL session and `yaacd` agree on the hub's
  socket and lock. It has `Wants=`/`After=user-runtime-dir@<uid>.service`, since the socket is
  under `/run/user/<uid>` as in a WSL session. It has `Restart=on-failure`: a crash is restarted;
  a clean exit, `hub/shutdown`, or a hub that finds its own tombstone stays stopped. It is
  `WantedBy=multi-user.target`: every boot of the distro starts the hub.
- **Nothing on Windows keeps it.** There is no keeper and no login entry for it. `yaac quit`
  and an update of YAAC on Windows leave the hub in WSL alone. Only a keeper of an earlier
  build (P414), if one still runs, is asked to stop its hub. `yaac autostart sync` turns the
  hub's login entry off while any tombstone exists. A login entry `yaac wsl-hub --distro …`
  left from P414 does nothing.
- **The new hub** in the distro is the Linux hub (P411). It binds the distro's own Tailscale
  address, never Windows', so Windows' port 47380 stays free for a hub here again. It reaches
  the PC's Windows `yaacd` as `<yaac.exe> local-daemon`
  (`yaac_hub::interop::InteropConnector`): one child process per target, as always
  (invariant 1). That command starts the Windows local `yaacd connect` bridge exactly as the
  Windows hub does (`yaac_hub::config::local_config`). It copies bytes between the bridge and
  its standard streams. Its Windows `yaacd` link lasts as long as the distro runs.
- **Start the hub here again** (offered as for any move). When the tombstone names a distro it
  first runs `wsl.exe -d <d> -u root --exec sh -c <remove script>`
  (`yaac_hub::transfer::wsl_service_remove_argv`), on a thread of its own. That boots the
  distro if needed, then runs `systemctl disable --now yaac-hub.service` (SIGTERM: the hub
  stops as on `hub/shutdown`), removes the unit and reloads systemd. It fails if the service
  still runs afterwards. Only then does the tombstone go and the local hub start. If removing
  fails, nothing else happens and the window says why. The confirmation says that the hub's
  system service in WSL is stopped and removed first and that whatever changed there since
  the move stays there.
- **Not yet**: a hub in WSL moving on to the PC's Windows leaves its unit enabled. At the next
  boot that hub finds its own tombstone and exits at once (it is not restarted), but the unit
  stays until **Start the hub here again** or a move back into WSL rewrites it. Without linger,
  logind may remove `/run/user/<uid>` once the user's last WSL session ends while the distro
  is kept up some other way (`instanceIdleTimeout=-1`). The hub's socket would go with it.

## The PC's window (P415)

The PC's window reaches a moved hub **without Tailscale**, the way it reaches the target the hub
moved to. Tailscale is the phones' way in. `hub/moved` and the tombstone carry `via`: the
target's transport as the old hub reached it (`wsl`, `ssh`), or `local` for the PC's own
Windows. The saved hub keeps it (`hubs.json`), so a later start attaches the same way.

- **A bridge per attach.** The window runs one command whose standard streams reach the hub. It
  is killed when the attach closes (`HubClient::connect_command`). The command runs `yaac
  hub-bridge` there (`yaac_hub::interop::serve_hub_bridge`). That copies bytes between its
  standard streams and the hub's own local socket, waiting up to 30 s for the hub to listen. So
  the hub sees an ordinary local attach on the user's own socket. An attach is no target
  connection, so invariant 1 is unaffected.
  - **WSL:** `wsl.exe -d <distro> [-u <user>] --exec sh -c '<script>'`
    (`yaac_hub::transfer::wsl_bridge_script`). Starting `wsl.exe` boots a stopped distro, and
    its systemd starts the hub's service (P416). While the bridge runs, it also keeps the
    distro up. If the service is enabled but not active, the script starts it only when this
    user may do so without a password (`systemctl start --no-ask-password`). Then it runs
    `<data>/bin/yaac hub-bridge`. If the bridge fails, the script writes the service's state
    (`yaac-hub.service is failed…`) on standard error. Standard output is the attach stream.
  - **SSH:** the system `ssh` with the hub's own options: `BatchMode=yes` (never a prompt),
    YAAC's known hosts, and YAAC's own key for that server when it is on the PC. Else the
    user's `ssh` defaults. The remote command is `exec sh -c '<data>/bin/yaac hub-bridge'`. **YAAC's own
    key stays on the PC** after the move for this bridge. It is still only the key YAAC generated
    for that server (invariant 12); nothing else is copied or kept.
- The entry is **This PC (in WSL: Ubuntu)** for a hub in this PC's WSL, else the server's name.
  Under it, how it is reached and the phones' address ("phones: no Tailscale there yet" when
  there is none). Such a hub is never started, stopped or replaced from the window (like a
  remote hub): a build mismatch says "Move it back to this PC, or install the same build there."
- **Back on Windows** (`via: local`) the entry is This PC again. The local attach waits (up to
  about 25 s) while the tombstone is still there, for `yaacd` to place the files and remove it,
  and never starts a hub itself while it exists.

## The Linux hub binary (P411)

The release builds `yaac` for `x86_64-unknown-linux-gnu` with `--no-default-features --features
hub` as the stripped release asset `yaac-hub-linux-x86_64` and lists it in `manifest.json`
with its size and SHA-256. `yaac hub` runs headless and uses the local XDG socket and the
same Tailscale admission as on Windows (`tailscale` on `PATH`). The binary also supports
`yaac quit`, `yaac autostart`, `yaac hub-bridge`, and `yaac --version`. On Windows,
`yaac hub-info` prints `{build, configDir, dataDir, runtimeDir, running, moved}` for `yaacd`.

The release embeds `yaacd.mjs`. On the new local target one stage 0 process reuses the
existing daemon and Node.js installation, with the same rules as SSH. It installs the embedded
daemon if that build is missing and reuses the pinned YAAC Node.js or an existing Node.js 22+
before fetching the pinned runtime. The daemon keeps its existing journals under `~/.yaac`
(or `YAAC_HOME`).

TODO: Publish a Linux aarch64 hub build.

## Contracts (names; the types are in `yaac-protocol`)

Target transport: `windowsInterop { exe }`, the PC's Windows `yaacd` reached from its WSL hub.

Attach (window/phone ⇄ hub):
- `hub/transfer/candidates {}` → `{ candidates: [{ targetId, label, eligible, reason?, warning? }] }`
- `hub/transfer/start { opId, targetId }` → `{ }` once accepted; then progress notifications.
- notification `hub/transfer/progress { opId, targetId, phase, done?, total?, message? }`,
  phase ∈ `checking | installing | copying | handingOver | moved | failed`.
- notification `hub/moved { hubId, address, hostName, via? }` — sent just before the old hub
  exits. `address` is empty without Tailscale there; `via` is the target's transport (`wsl`,
  `ssh`) or `local` (the PC's Windows).
- error code `hub_moving` for requests refused during the hand-over.
- error codes `transfer_not_eligible`, `transfer_in_progress`, and `transfer_unavailable` for
  a transfer start that the hub cannot accept.

Daemon (hub ⇄ `yaacd` on X):
- `hubHost/probe {}` → `{ os, arch, tailscale: { running, login?, ips[] }, portFree, hubRunning, wsl?: { distro, service? }, hubBuild?, lastTransfer? }`
- `wsl.service` is `{ systemd, user, uid, dataDir, environment[] }`: what the PC needs to install the
  hub's system unit in that distro (P416). `systemd` is whether systemd is the system manager.
  `environment` holds `KEY=value` entries for the unit (`HOME`, the `YAAC_*_DIR` dirs,
  `YAAC_HOME`, `PATH`).
- `lastTransfer` is `{ opId, status, message? }` with `status` of `started`, `placed` or `failed`.
- `hubHost/install { opId, url, sha256, size }` → `{ path }`
- notification `hubHost/progress { opId, done, total }` reports download progress in bytes.
- `hubHost/put { opId, name, size, sha256 }` → `{ stream, credit }`. Send data frames on
  `stream` as credit permits.
- notification `hubHost/putComplete { stream, message? }` confirms staging on success and
  carries an error message on failure.
- `hubHost/start { opId, armOnClose, startHub }` → `{ armed: true }` (`startHub` defaults to
  `true`; `false` places the files only, for a hub in this PC's WSL, whose system service the PC
  installs as root afterwards)
- `hubHost/abort { opId }` → `{ }`
- capability `hubHost` in `initialize.features` (a `yaacd` without it is not a candidate: "Update YAAC on
  that target first").
