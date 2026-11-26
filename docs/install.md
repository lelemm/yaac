# Installing on Windows

**Decided** (D134): YAAC ships on Windows as an **Inno Setup** `setup.exe`, installed **per
user, without administrator rights**. The installer is `installer/yaac.iss`; it is built from
WSL with `scripts/build-installer.sh`, which builds the release `yaac.exe`
(`scripts/build-windows.sh`), reads the version from the workspace `Cargo.toml`, and writes
`dist/windows/YAAC-Setup-<version>+<id>.exe` (the build id, [02](02-architecture.md#build-ids)).
Inno Setup 6 must be installed on the Windows side
(`winget install --id JRSoftware.InnoSetup -e --scope user`); the script finds `ISCC.exe`
there, or takes `$ISCC`. Published releases are built by CI instead ([Releases](#releases)).

The window and the hub are the same `yaac.exe` (`yaac` and `yaac hub`,
[02](02-architecture.md#processes)). The installer includes the runtime and `yaacd` bundle
for native Windows provider discovery under **Local**. The hub also embeds the bundle it
installs inside WSL after the distro is added in **Settings → Targets → WSL**.

## What goes where

| What | Where |
| --- | --- |
| `yaac.exe` | `%LOCALAPPDATA%\Programs\YAAC\` — no directory page; `/DIR=<folder>` picks another |
| `yaacd.mjs`, `runtime\node.exe` and Node.js license | Beside `yaac.exe` and in its `runtime` subdirectory |
| Native daemon state, runtime and daemon builds | `%USERPROFILE%\.yaac\` (or `YAAC_HOME`) |
| Start menu shortcut **YAAC** | the user's Start menu programs folder |
| Desktop shortcut (optional, unticked by default) | the user's desktop |
| Uninstaller | `unins000.exe` beside `yaac.exe`, listed in *Apps & features* for this user (`HKCU\…\Uninstall\{68D966F3-0510-46E4-9581-B0279B70C15D}_is1`) |
| The hub's login entry | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `YAAC hub` = `"<install folder>\yaac.exe" hub` — written by YAAC itself, see below |

The installer writes nothing else. YAAC's own data stays where YAAC keeps it
([02](02-architecture.md#where-state-lives)): settings and `display.json` in `%APPDATA%\yaac`,
the cache, drafts, relay queue, runtime cache, and YAAC's generated SSH keys and confirmed host
keys in `%LOCALAPPDATA%\yaac` (`YAAC_CONFIG_DIR` and `YAAC_DATA_DIR` move them).

The setup is not code-signed. Windows SmartScreen therefore says *"Windows protected your PC"*
with *Unknown publisher* on the first run of a downloaded copy; *More info → Run anyway*
continues. `yaac.exe` itself is unsigned too.

## The commands the installer uses

The installer never duplicates what YAAC knows. It runs these `yaac` commands (they work by
hand too, and read the same environment as the window and the hub, so a test profile is
honoured everywhere):

| Command | What it does |
| --- | --- |
| `yaac quit [--wait-for <exe>]` | Asks the window to close (the tray's *Quit*, through a named event beside the single-instance one), waits until it has, then sends the running hub `hub/shutdown` — it checkpoints its stores and exits; turns keep running on the targets, where `yaacd` journals everything (invariant #8) — and waits for its lock to be released. With `--wait-for`, it then waits until that file can be opened for writing (no process maps it any more). Nothing running: nothing to do. |
| `yaac autostart on\|off\|status` | Writes, removes, or reports the hub's login entry, pointing at the running executable. |
| `yaac autostart sync [--start]` | The entry follows the hub's *start at login* setting (`always` in `hub.json`): on → written for this executable (replacing one another copy, such as a development build, wrote); off → removed. When YAAC has never run (no `hub.json`), nothing is written, so the window's first run still shows its first-run page and installs the entry then, as it always has. `--start` then starts the hub detached, as login would, when the entry is on. |
| `yaac remove-data` | Deletes YAAC's host-side folders: the config, data, and (if separate) runtime directories. Refuses while the hub runs, and leaves alone a folder that does not look like YAAC's (not named `yaac` and holding none of its files). |

A newer hub refuses `hub/shutdown` from an older build (N14), so the installer always runs
`quit` with the **installed** executable, the build the hub was started from, copied to its
temporary folder first so the installed file itself is not held open.

## Install and upgrade

1. **Before any file is replaced** (`PrepareToInstall`), if YAAC is already installed: the
   installed build runs `yaac quit --wait-for <install folder>\yaac.exe`. The window closes
   and the hub stops gracefully. If something still holds the file, Inno Setup's Restart
   Manager check (`CloseApplications=yes`) is the fallback: interactively it lists the
   program and offers to close it; it never restarts it.
2. `yaac.exe` is copied in; the shortcuts are written.
3. `yaac autostart sync --start` runs from the installed copy: the login entry now points at
   the installed `yaac.exe`, and the hub is running again, so the mesh keeps working without
   waiting for the next login.
4. The finish page's **Launch YAAC** (ticked) opens the window. A silent install
   (`/SILENT` or `/VERYSILENT`) does not open it, unless it was given **`/RELAUNCH`** (what the in-app
   updater passes): then the installed `yaac.exe` is started once setup has finished. A setup
   that failed runs neither.

Installing the same or another version over an existing install follows the same steps, in
the same folder (the folder of the first install is kept).

## Uninstall

1. The user is asked **"Also remove YAAC's settings and data?"**, default **No**. A silent
   uninstall keeps the data unless given `/REMOVEDATA=yes`.
2. The installed build runs `yaac quit --wait-for …` (window closed, hub stopped), then
   `yaac autostart off` (the login entry is removed), then, if asked, `yaac remove-data`.
3. The program files, shortcuts, and the *Apps & features* entry are removed.

**Kept by default:** everything in `%APPDATA%\yaac` and `%LOCALAPPDATA%\yaac` — targets and
settings, the cache, drafts, the relay queue, and YAAC's generated SSH keys — so a later
install picks up where this one left off. **Never touched, either way:** anything on a target
(its `~/.yaac`, the line YAAC added to `authorized_keys` — use *Revoke* before uninstalling to
remove that), and the user's own `~/.ssh` (invariant #12).

## Releases

**Decided** (D136): releases are made by GitHub Actions, `.github/workflows/release.yml`, on a
push of a tag `v*` (it must equal `v<version>` of the workspace `Cargo.toml`) or by hand
(*Run workflow*, which tags the commit `v<version>-build.<id>`). The repository is public, so
releases are read without a token.

| Job | Runs on | Does | Produces |
| --- | --- | --- | --- |
| `checks` | `ubuntu-latest` | Two parallel jobs run the full Rust workspace suite and the daemon and manifest suites. Rust test builds restore cached dependencies and use a profile without debug symbols. | test results |
| `build` | `ubuntu-latest` | Runs alongside the checks. Builds the daemon bundle and runs `scripts/build-windows.sh` with MinGW-w64 and the `x86_64-pc-windows-gnu` Rust target. It caches Cargo's release dependencies separately from the Linux test cache. | `yaac.exe` (window and hub, with `yaacd` embedded), `yaacd.mjs`, `yaacd-mcp.mjs`, the exe's `yaac.ico` |
| `installer` | `windows-latest` | `ISCC.exe installer\yaac.iss` with the same `/D` defines as `scripts/build-installer.sh`. Inno Setup 6 is preinstalled on GitHub's Windows images (actions/runner-images lists it); the job installs it with Chocolatey only if it is missing | `YAAC-Setup-<version>+<id>.exe` |
| `release` | `ubuntu-latest` | After the checks and installer succeed, runs `scripts/make-manifest.mjs`, then `gh release create <tag> … --latest` with the job's `GITHUB_TOKEN` (`contents: write`, this job only) | the release |

A release holds `YAAC-Setup-<version>-build.<id>.exe` (asset names keep to letters, digits,
`.`, `-` and `_`, which GitHub never renames), `yaac.exe`, `yaacd.mjs`, `yaacd-mcp.mjs`, and
`manifest.json`. Actions are pinned to commit SHAs.

When the dependency hash changes, each Cargo cache can restore from a compatible runner and
toolchain. Cargo checks which entries are still valid. Release executables and manifests
are collected from the current build after it runs with its own build id.

**Build ids.** Every artefact of a release carries `<version>+<id>` with
**id = 1000 + the workflow's run number** (`RELEASE_BASE` in the workflow), passed to the
build as `YAAC_BUILD_ID`. CI ids are the release ids. `scripts/build-id.sh` hands an id given
in `YAAC_BUILD_ID` straight back without touching the local counter, and refuses to count a
local build past 999, so every release is newer than every local build (P87).

### The manifest

`manifest.json`, written by `scripts/make-manifest.mjs` (`node --test
scripts/make-manifest.test.mjs` tests it; `scripts/testdata/manifest.sample.json` is the
sample the Rust parser is tested against):

```json
{
  "schema": 1,
  "version": "0.1.0",
  "build": 1005,
  "tag": "v0.1.0",
  "releasedAt": "2026-11-21T12:00:00.000Z",
  "artifacts": [
    { "name": "YAAC-Setup-0.1.0-build.1005.exe", "component": "installer", "version": "0.1.0+1005",
      "size": 9123456, "sha256": "<64 hex>", "url": "https://github.com/lelemm/yaac/releases/download/v0.1.0/YAAC-Setup-0.1.0-build.1005.exe" },
    { "name": "yaac.exe", "component": "app", … },
    { "name": "yaacd.mjs", "component": "daemon", … },
    { "name": "yaacd-mcp.mjs", "component": "daemon-mcp", … }
  ]
}
```

`schema` is the format: a reader uses only the schema it knows (a manifest of another one
offers no update) and ignores fields it does not know, so fields are only ever added.
`build` is the number the updater compares; each artefact's `version` is its own version with
the same build id.

## Updating

**Decided** (D136, D145). The details are P87–P90 and P140–P141. A Windows build
with a numeric build id checks for updates. Development builds leave the updater off.

1. **At window startup and every 10 minutes while the window runs**, including in the
   tray, a worker sends a `GET` to `https://api.github.com/repos/lelemm/yaac/releases/latest`
   with the generic `User-Agent: yaac-updater`, then fetches that release's `manifest.json`.
   The worker uses the same 10 s timeouts for every check and sleeps between checks. Offline
   and HTTP failures are shown only in Settings → About → Components. A failed check keeps
   any existing update offer. After a rate limit the worker waits for the latest of the
   regular 10-minute interval, GitHub's reset time, and its `Retry-After` delay, when supplied.
   Click **Refresh** next to **Components** to check before the regular interval. A server
   retry delay or an installer already downloading or ready can defer that release check.
   The hub, command-line commands, and demo never make these checks.
2. **A newer build** (`build` above this one's) puts an **update icon** in the title row
   beside Minimize in both WPF and egui: *"YAAC <version>+<id> is available · Download"*.
3. **Download** (a click): on a worker, into `%LOCALAPPDATA%\yaac\updates\` (YAAC's own
   data folder; earlier downloads there are deleted first). The icon becomes a **mini progress
   bar**, repainted only when it moves a visible step and at most ten times a second. The
   size and SHA-256 are checked against the manifest as the bytes arrive; a mismatch deletes
   the file. Only a verified installer gets **`pending.json`** (`build`, `version`, `path`,
   `sha256`, `size`, `attempted`). A failed download leaves the icon and periodic checks
   resume. Checks pause while downloading, waiting for restart, or installing. A verified
   installer stays selected even if another release is published while it waits. The
   restarted window checks for newer releases again.
4. **"Restart to update"** (a click): `pending.json` is marked `attempted`, the installer is
   started with `/SILENT /SUPPRESSMSGBOXES /NORESTART /SP- /RELAUNCH
   /LOG=<updates>\install.log`, and the window quits. Setup displays its progress window
   throughout installation. The installer closes anything left and
   stops the hub (`yaac quit`), replaces `yaac.exe`, starts the hub again (`autostart sync
   --start`, when *start at login* is on), and starts YAAC (`/RELAUNCH`).
5. **At the next start**, before the window opens:

   | `pending.json` | Then |
   | --- | --- |
   | missing | start |
   | unreadable, or its installer is outside the updates folder | delete it, start |
   | for this build or an older one (the update went in) | delete it and the installer, start |
   | newer, not yet attempted, installer still matches its hash | mark it attempted, run the installer as in 4, exit |
   | newer, but the installer no longer matches | delete it, start |
   | newer and already attempted (the installer failed) | delete it, start, and say once *"The update to YAAC … could not be installed"* with the log's path |
   | anything, and this is a development build | start, leaving it alone |

   An update can therefore be tried once; it never loops.

Settings → **About** → Components has a row **YAAC (latest release)**: the latest
release's build and whether this YAAC is behind it, or why the latest check is unavailable.
It also shows when the last check finished in local time, including after a failed check.

### How targets get the new daemon

The updater downloads only the installer. Targets never download YAAC from GitHub, and the
hub opens nothing new to reach them (invariant #1): the `yaacd` bundle is embedded in
`yaac.exe` (`yaac-hub/build.rs`, `config::embedded_bundle`), and the hub installs it on a
target in-band, over that target's one connection, when the target lacks that build
(stage 0's `PUT daemon`, [02](02-architecture.md#stage-0)). So installing a new YAAC gives
the hub a new `yaacd`, and each target receives it on its next connection. A target whose
serving `yaacd` is older is shown as older (D135) and moves to the new build by the rules of
[one serving daemon per user](02-architecture.md#one-serving-daemon-per-user): when nothing
is serving, stage 0 starts the newest installed build.

The hub then moves the target over: when the serving `yaacd` has a lower build number than
the bundle it just installed, it asks it to hand over when idle (`daemon/handover`), sees the
stream end, reconnects over the same single path, and stage 0 starts the newest installed
build. **Update now** on the thread banner does the same at once, interrupting running turns
(asking first when there are any). A pair of builds is asked at most once; if the old build
comes back, the target's row in Components says so instead of asking again
([how the hub does it](02-architecture.md#one-serving-daemon-per-user), P91–P93).

`yaacd-mcp.mjs` is not installed on targets: the daemon falls back to `yaacd mcp` from its
own bundle when the small bridge is not beside it.

## Testing the installer without touching the real profile

Every path the installer's commands use comes from the environment, so a throwaway profile is

```text
YAAC_CONFIG_DIR=<tmp>\config  YAAC_DATA_DIR=<tmp>\data  YAAC_AUTOSTART_KEY=Software\YAAC-test-<n>
YAAC-Setup-<version>.exe /VERYSILENT /SUPPRESSMSGBOXES /CURRENTUSER /DIR=<tmp>\app
<tmp>\app\unins000.exe /VERYSILENT /SUPPRESSMSGBOXES [/REMOVEDATA=yes]
```

with its own hub pipe and window events (both follow the runtime directory). The Start menu
shortcut and the *Apps & features* entry cannot be redirected: they are the real user's, and
the uninstall removes them again.

Setup registers the per-user AppUserModelID `YAAC.Desktop` and puts that identity on the
Start menu shortcut. Windows uses it for the hub’s native request reminders and their
notification preferences. Uninstall removes the registration.

### Mermaid diagram runtime

Windows Setup stages the pinned Mermaid JavaScript bundle, resvg WebAssembly converter and
Ubuntu Sans faces in `runtime/mermaid`. Both windows use the private Node executable and the
host's Edge or Chrome in a temporary headless profile. Each diagram is laid out locally with
network loads disabled. The renderer uses native SVG labels and converts text to paths before
rasterization. Additional scripts such as Han use available host fonts during conversion.

For a Linux development checkout run `npm ci --prefix tools/mermaid`. Put Node 22 on PATH and
install Chromium, or set `YAAC_MERMAID_BROWSER` to its executable. Run
`node --test tools/mermaid/render.test.cjs` to check themes, exports and parser errors.


## Bundled skills

The YAAC binary in the installer includes `yaac.create-yaac-theme`. The hub offers it in
Settings → Skills on the first run. Skills added by the user and overrides of the bundled
entry persist in the hub’s durable data across updates.
