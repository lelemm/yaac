# Installing on Windows

**Decided** (D134): YAAC ships on Windows as an **Inno Setup** `setup.exe`, installed **per
user, without administrator rights**. The installer is `installer/yaac.iss`; it is built from
WSL with `scripts/build-installer.sh`, which builds the release `yaac.exe`
(`scripts/build-windows.sh`), reads the version from the workspace `Cargo.toml`, and writes
`dist/windows/YAAC-Setup-<version>.exe`. Inno Setup 6 must be installed on the Windows side
(`winget install --id JRSoftware.InnoSetup -e --scope user`); the script finds `ISCC.exe`
there, or takes `$ISCC`.

There is one program file. The window and the hub are the same `yaac.exe` (`yaac` and
`yaac hub`, [02](02-architecture.md#processes)); the `yaacd` bundle the hub installs on WSL
targets is embedded in it.

## What goes where

| What | Where |
| --- | --- |
| `yaac.exe` | `%LOCALAPPDATA%\Programs\YAAC\` — no directory page; `/DIR=<folder>` picks another |
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
   (`/VERYSILENT`) does not open it.

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
