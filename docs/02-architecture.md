# 02 — Architecture

## Processes

```
HOST                                              TARGET
┌───────────────────────────────┐                ┌─────────────────────────────────────────────┐
│ yaac  (the window — optional) │                │ yaacd  (one per target, per user)           │
│  UI thread (egui) paints      │                │                                             │
│  reads the cache, attaches    │                │  session layer   frames, journal, uploads   │
│        │ local socket         │                │  thread registry bindings, lineage, index   │
│        ▼                      │                │  mesh            mailbox, outbox, MCP server│
│ yaac hub  (headless, stays up)│                │  provider instances                         │
│  I/O thread (async)           │  one stream    │   ├ codex "default"  ──► codex daemon sock A│
│   ├ connection: local  ───────┼────────────────┤   ├ codex "work"     ──► codex daemon sock B│
│   ├ connection: ssh A  ───────┼──► (other      │   └ claude "default" ──► Claude Agent SDK   │
│   └ connection: wsl X  ───────┼──►  targets)   │                                             │
│  mesh router + relay queue    │                │  journal, mailbox, uploads (on disk)        │
│  workers (small, bounded)     │                └─────────────────────────────────────────────┘
│  cache writer (SQLite)        │
└───────────────────────────────┘
```

The host runs up to two processes from one binary:

- **`yaac hub`** — headless. It owns the connection to every target, routes messages between
  agents on different targets, and writes the cache. It keeps running when the window is
  closed, so agents on different machines can go on working together
  ([10 Agent mesh](10-agent-mesh.md#the-hub)).
- **`yaac`** — the window. It attaches to the hub over a user-only local socket, reads the
  cache for an instant first frame, and paints. Closing it stops nothing. By default closing
  only hides it in the system tray (D127, [07](07-ui.md#the-tray)); Quit in the tray menu ends
  the window process, never the hub.

Inside the window the UI thread never blocks on I/O: it reads an in-memory state snapshot and
paints. Inside the hub one I/O thread owns every connection and decodes frames; anything that
can block — SQLite writes, hashing a pasted file, decoding an image — runs on a small bounded
worker pool, so one slow disk operation cannot stall every target at once.

The window and the hub are always the same build: a newer window restarts an older hub, which
is safe because the hub holds no work of its own — turns live in `yaacd` on the targets and
are journaled there.

On Windows both are one `yaac.exe`, installed per user by an Inno Setup installer that stops
the window and the hub before an upgrade and starts the hub again after it; the hub's login
entry is the per-user `Run` value `YAAC hub` ([Installing on Windows](install.md)).

## The single-connection rule

**Decided.** For each target the host holds exactly one open connection, and it is a plain
byte stream: the standard input and output of one child process. The hub holds it; the window
never opens a connection of its own.

| Target | The one child process | Notes |
| --- | --- | --- |
| Local | `<private runtime> yaacd.mjs connect` | No shell in between. |
| SSH | `ssh -T … -- <host> <one-line stage 0 loader>` | One SSH session, one exec channel, no PTY, no connection sharing. |
| WSL | `wsl.exe -d <distro> [-u <user>] --exec <bootstrap>` | One `wsl.exe` per enabled WSL connection. |

### What a target is

A target is **a user on a machine**: `local`, *user@host:port* over SSH, or *user in a distro*
on WSL. `yaacd` runs as that user, sees that user's harness sign-ins and conversations, and
keeps its state in that user's home directory.

So two connections to the same machine as different users are two targets, each with its own
one connection and its own `yaacd`. That is the only way one machine ever gets more than one
connection, and it is deliberate — the two users share nothing. The settings UI warns when a
second user is added for a WSL distro, because every extra `wsl.exe` session is exactly what
the single-connection rule exists to keep to a minimum.

Everything is multiplexed inside that stream by the [YAAC protocol](03-protocol.md): requests for
every provider instance, event streams for every thread, and binary file uploads. Nothing in
the design may open a second channel "just for" uploads, health checks, or directory listings.
Even the "test connection" button in settings reuses the live connection when there is one.

Consequences worth stating:

- The fan-out to harnesses happens **on the target**, between `yaacd` and local sockets or child
  processes. Ten Codex daemons on one host are still one connection from the hub.
- **Agents on different targets talk through that same connection.** A message from a thread
  on one target to a thread on another goes target → hub → target, on the two existing
  streams. Targets never connect to each other.
- Installing or upgrading `yaacd` on a target must also happen in-band on that same stream
  (see [Bootstrap](#bootstrap)).
- **One stream means one queue, so the queue needs priorities.** Over a slow link, a single
  large frame can hold the wire for seconds. Stop, approve, and other control messages must not
  wait behind an upload or a long history page. The protocol therefore schedules control traffic
  ahead of bulk traffic in *both* directions and keeps bulk chunks small
  ([protocol: scheduling](03-protocol.md#scheduling-and-flow-control)).

## Transports

The transport layer has one job: produce a reliable, binary-clean, bidirectional byte stream to
a `yaacd` on the target, and report when it drops.

```rust
// yaac-transport (sketch)
pub trait Connector: Send {
    /// Spawns the one child process for this target and completes the bootstrap handshake.
    fn connect(&self, ctx: &ConnectCtx) -> BoxFuture<'_, Result<Stream, ConnectError>>;
}

pub struct Stream {
    pub reader: Box<dyn AsyncRead + Send + Unpin>,
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
    pub child: ChildGuard, // kills the child when dropped
}
```

### Local

The hub registers a Local target for the current Windows user, including on upgrade from
a build that did not register one. A disabled Local target stays disabled. The Windows
installer includes the private runtime and daemon bundle. On first connection the hub copies
them into versioned directories under the native daemon's home before spawning the runtime
directly. The copies let a serving daemon and its tool bridges outlive installer upgrades.

The Windows daemon's bridge uses a local named pipe with TLS pre-shared-key authentication
in both directions. The key is created under the start-up lock and stored in the daemon's
Windows user profile, including when the daemon's home is configured on another drive.
The same bridge reconnects to an already serving daemon. POSIX
daemons use the user-only Unix socket.

### SSH

**Decided:** YAAC drives the system OpenSSH client (`ssh`, `ssh.exe`) as the one child process.

- It honours `~/.ssh/config`, agents, `ProxyJump`, hardware keys, `known_hosts`, and every
  corporate quirk already solved on the user's machine.
- Windows has shipped `ssh.exe` since Windows 10 1809.
- Prompts happen only during [Setup](#setting-up-an-ssh-connection): the password or key
  passphrase is answered through `SSH_ASKPASS`, which points at the YAAC binary, so it comes
  from YAAC's own window. A new host key is confirmed in the window before any login.

Invocation shape (`yaac-transport`, `ssh.rs`):

```
ssh -T -o ControlMaster=no -o ControlPath=none -o EscapeChar=none -o ServerAliveInterval=15
    -o StrictHostKeyChecking=yes -o UserKnownHostsFile="<YAAC's known_hosts>" ~/.ssh/known_hosts …
    -o BatchMode=yes [-o IdentitiesOnly=yes -o IdentityFile=<YAAC's key>] [-p <port>] [-l <user>]
    -- <host> "exec sh -c 'eval \"\$(dd bs=1 count=<n> 2>/dev/null)\"' yaac-stage0 'YAAC_…=…'"
```

- `-T` and `EscapeChar=none` keep the stream binary-clean; `ControlMaster=no` and
  `ControlPath=none` keep it from sharing (or offering) a multiplexed master connection.
- **A normal connection never prompts** (`BatchMode=yes`). It uses YAAC's own key when YAAC
  set the server up, otherwise whatever the user's `ssh` is set up with (agent, config, keys).
  A refused login, an unknown host key, or a changed one fails with a plain-words message.
- **The remote command is a one-line loader** and stage 0 itself is the first thing written on
  standard input (P38): the command passes through the user's login shell, whatever it is.
- Reconnection is the hub's supervisor's, with capped backoff, as for every transport.

**Verified on Linux** against OpenSSH 9.6 (a throwaway `sshd` in the tests). On Windows the
same arguments go to `ssh.exe` with no console window; that path is built but not yet run
against a server.

**Decided:** SSH targets are POSIX machines — Linux, including musl distributions; macOS works
as a side effect. Windows machines reached over SSH are not supported in the MVP. Windows is
supported as the local machine and through WSL.

The `Connector` trait keeps room for an embedded SSH client later, for machines without `ssh`.

#### Setting up an SSH connection

**Decided:** adding an SSH connection is a guided setup that ends in password-less access, and
YAAC can undo what it set up.

A connection is **host**, **port**, and **user** (plus a display name). The **Setup** button is
enabled once host and port are filled in. Then:

```
 Setup
   │
   ▼
 1. Try to log in with no password          ssh -o BatchMode=yes …
   │
   ├─ works ──────────────────────────────► READY · access was already in place
   │                                          (Revoke is disabled: YAAC installed nothing)
   ▼ fails
 2. Ask for a password — or a key file (PEM)
   │
   ▼
 Connect
   │
 3. Log in with what was given ── fails ──► show the SSH error; let the user try again
   │
   ▼ works — and in that same session:
 4. Install a YAAC key for this user         append one line to ~/.ssh/authorized_keys
   │
   ▼
 5. Log in again with the YAAC key only  ── fails ──► say why; remove the line just added
   │
   ▼ works
 READY · set up by YAAC                      (Revoke is enabled)
```

How each step works:

- **Host key first.** On the very first contact the server's host-key fingerprint is shown and
  must be accepted. A host key that later *changes* stops the connection with a clear warning;
  YAAC never accepts a changed key on its own.
- **The key is YAAC's own.** Step 4 does not reuse the user's personal keys. YAAC generates a
  dedicated key pair for this one connection (Ed25519), keeps the private half in its data
  directory readable only by the user, and installs the public half on the server with a
  comment identifying it: `yaac:<this machine>:<connection id>`. From then on the connection
  uses that key and nothing else (`-i <key> -o IdentitiesOnly=yes`).
- **The password is used once and never stored.** It lives in memory for the length of step 3,
  reaches `ssh` through the askpass callback, and is wiped. It is never written to settings,
  the cache, or a log.
- **A key file (PEM) is treated the same way.** It is used for the step-3 login only, and YAAC
  then installs its own key. The connection never depends on a file the user might move, and
  Revoke has exactly one thing to remove. If the file has a passphrase, it is asked for once.
- **Installing is careful.** The remote command creates `~/.ssh` (mode `0700`) and
  `authorized_keys` (mode `0600`) if missing, and appends the line only if it is not already
  there. It never rewrites or reorders existing entries.
- **Step 5 is the proof.** Some servers accept passwords but not keys, or keep authorised keys
  somewhere else. If the key login fails, YAAC says so, removes the line it added, and leaves
  the connection not set up rather than half-working.

**Revoke** undoes step 4 and is available only when YAAC performed it:

1. Close the live connection to that target.
2. Log in with the YAAC key and remove exactly the line holding that public key — matched by
   the key itself, not by its comment — writing the file atomically.
3. Delete the local key pair, and mark the connection as not set up.

If password-less access already existed before Setup (step 1 succeeded), YAAC installed nothing
and Revoke is disabled: there is nothing of YAAC's to remove, and removing the user's own
access would be wrong. Each connection records which case it is.

If the server cannot be reached when Revoke is pressed, YAAC says the server still holds the
key, shows the exact line to remove by hand, and keeps the local key until the user confirms.
Removing a connection from settings offers Revoke first when it applies.

**The single-connection rule still holds.** Setup and Revoke use a few short SSH sessions, one
after another, and only while the target has no live connection. They are never open alongside
it. The last step of Setup can simply become the target's one live connection.

Hosts that are already configured — an alias in `~/.ssh/config`, an agent, a jump host — go
through the same Setup button: step 1 succeeds and the connection is ready. Settings can import
hosts from `~/.ssh/config` to pre-fill the form.

##### How the hub does it

Setup, Verify, and Revoke are the hub's (`hub/ssh/*`, in `yaac-hub`'s `protocol` and `ssh`
modules); the window only sends the steps and shows their answers (P45).

| Step | Sessions |
| --- | --- |
| Reach | `ssh -G` resolves what `ssh` would connect to (aliases, `HostName`, `HostKeyAlias`); `ssh-keyscan` reads the host key; the strongest (Ed25519 first) is shown with its `SHA256:` fingerprint. |
| Confirm + probe | The confirmed key goes into **YAAC's own** `known_hosts` (`<data>/ssh/known_hosts`), never the user's (P40). Then `ssh BatchMode=yes … echo` with the user's own setup: works → *already in place*, recorded, Revoke stays disabled. |
| Sign in + install | `ssh-keygen` makes YAAC's Ed25519 key (`<data>/ssh/keys/<server>/id_ed25519`, user-only). One session signs in with the password (`PubkeyAuthentication=no`) or the key file (`IdentitiesOnly=yes`), and runs the install script: `~/.ssh` `0700` and `authorized_keys` `0600` if missing, the line appended only if its key is absent. The session then **waits** (P42). |
| Proof | A second login with YAAC's key alone. Works → the waiting session is told `KEEP` and records the line in `~/.yaac/ssh/<id>` on the server; fails → `REMOVE`, and the line is removed and its absence checked. If the hub goes away, the session ends and the server removes the line itself. |
| Revoke | The target's connection is closed first. A login with YAAC's key runs the revoke script: every line in which some field equals YAAC's key is dropped, the file is rewritten atomically (temporary file, `mv`), and the key's absence is checked; then YAAC's key and record are deleted. Unreachable, or YAAC's key no longer accepted → `ssh_unreachable`, the key is kept, and the window shows the line to remove by hand. Revoking twice is a no-op; Revoke where access was already in place is refused (`nothing_to_revoke`). |

**The password never touches a disk.** The window moves it from its zeroised buffer into the
one `hub/ssh/signIn` request. The hub moves it into a zeroised buffer and serves it once, over
a one-shot user-only local socket, to the `SSH_ASKPASS` helper (the `yaac` binary itself,
which `ssh` runs with the socket's path and a random token in its environment — never the
secret). The secret is wiped as soon as the sign-in is over (P41). The attach protocol's
frame buffers and the JSON value of that one request are transient and not zeroised.

**The single-connection rule is enforced by the hub** (P44): a setup step is refused with
`target_connected` while the server has a connection open or being opened, and a connection
attempt waits out a running setup step. The record of what YAAC did is kept per server in
`hub.json` (P43), so it applies to every target of that server.

**Verified on Linux** (OpenSSH 9.6): a throwaway `sshd` runs the whole matrix in
`crates/yaac-hub/tests/ssh_setup.rs` — key-file setup, verify, revoke leaving exactly the
original lines, revoke again, access already in place, a failed proof, setup refused while
connected, and a turn end to end. A non-root `sshd` cannot check passwords, so the password
case puts a stand-in in front of `ssh` that asks `SSH_ASKPASS` exactly as `ssh` does.

**Verify (spike S9):** the askpass flow for a password and a key passphrase through
`ssh.exe` on Windows, with no terminal attached (Linux is verified).

### WSL

Only offered on Windows hosts, and **off by default**. When the user turns it on, the hub
lists the installed distros (`hub/wsl/distros`, from `wsl.exe -l -v` and `-l --running -q`,
which start nothing; note: they print UTF-16LE) and the user adds the ones they want from a
dropdown that starts on Windows' default distro.

**Decided:** a WSL connection is a **distro** plus an **optional user**. A distro can have
several Linux users, each with its own home directory, harness sign-ins, and conversations.
Left empty, the connection runs as the distro's default user; filled in, it runs as that user
(`wsl.exe -d <distro> -u <user> --exec …`). The user field is validated against the distro
when the connection is tested.

Each enabled WSL connection gets one long-lived `wsl.exe` child. The same distro may be added
more than once with different users; see [what a target is](#what-a-target-is).

Before stage 0, that child can fill in `CODEX_HOME` and `CLAUDE_CONFIG_DIR` from exports in
the selected user's `~/.bashrc`. It runs the file in an isolated interactive Bash with no
terminal, discards its output, and gives it two seconds plus one second to terminate.
Inherited values take precedence. If Bash or `timeout` is unavailable or the profile times
out, startup continues with the inherited environment. A daemon
started by the bridge inherits these directories. An already serving daemon keeps the
environment it started with until it exits or hands over through the normal update path.

Two things to prove before building on WSL (spikes S2 and S8):

- **Binary-clean pipes.** Standard input and output through `wsl.exe` must pass arbitrary bytes
  in both directions, with no newline or encoding translation. If they do not, a text-safe
  encoding is selected for that transport — and it must be selected *before* any binary install
  bytes are sent, not negotiated afterwards.
- **Lifetime.** WSL can shut a distro down, which stops `yaacd` and any turn running in it.
  How and when that happens is what the spike measures.

**Decided:** YAAC does not keep a distro alive artificially, and it does not start one behind
the user's back. When a WSL distro that has an enabled connection is down, the host shows a
notification — *"WSL distro Ubuntu is not running"* — with **one action: Start it** (YAAC
connects, which boots the distro). It is not a choice between starting and disabling (D96):
while the distro is down, **its threads stay visible but are disabled** — readable from the
cache, with the composer and every action unavailable and a single line in its place, "Ubuntu
is not running · Start it". Pressing Start it — on the notification or in any of its threads
— boots the distro, YAAC connects, and the threads become responsive again by themselves.
Turning a WSL connection off remains a setting on the Targets page, not something offered
here. Turns that were running when the distro stopped are shown as interrupted, with
everything the harness had saved.

Because merely running `wsl.exe -d <distro>` boots a distro, the automatic reconnect logic does
not apply to WSL as it does to SSH: YAAC first checks the distro's state with
`wsl.exe -l -v`, which does not start anything, and reconnects by itself only when the distro
is already running.

### Paths

A path on a target is not a path on the host. A Windows path, a WSL path, and a remote Linux
path follow different rules even when they name the same disk. Target paths therefore travel as
an opaque `TargetPath` value that only `yaacd` on that target interprets. The host never parses,
joins, or normalises them, and never stores them in a host `PathBuf`.

## Bootstrap

Getting `yaacd` onto a target without a second connection, and without requiring anything to be
installed there first.

### What gets installed

**Decided:** `yaacd` is self-contained. A target needs the harness CLIs it wants to use and a
POSIX shell — not Node.js, not npm.

`yaacd` on a target is two artefacts under `~/.yaac/`, installed and upgraded independently:

| Artefact | Path on the target | Size | Changes |
| --- | --- | --- | --- |
| **Runtime** — a private, pinned JavaScript runtime binary | `~/.yaac/runtime/<runtime-version>/` | tens of MB | Rarely: only when we move to a new runtime version. |
| **Daemon** — `yaacd` bundled into a single JavaScript file | `~/.yaac/daemon/<build>/yaacd.mjs` | target: under 3 MB | Every YAAC release. |

Splitting them keeps the common case cheap: a YAAC update re-sends a few megabytes, not the
runtime. The runtime is private to YAAC — it is never put on `PATH` and never touches a Node
installation the user already has. A system Node is deliberately **not** used even when present,
so every target runs the exact runtime we tested.

The daemon bundle contains the Claude Agent SDK's JavaScript and a client for the Codex
app-server protocol, but no harness binaries. It drives the `codex` and `claude` already
installed and signed in on the target.

Which runtime (Node.js or Bun) is decided by spike S3 on size, memory, musl support, and SDK
compatibility.

### How the runtime gets there

**Decided:** fetched on demand. The installer carries only the runtime for the host's own
platform, so the local target always works offline.

When a remote target has no runtime (or the wrong version), in this order:

1. **The target downloads it itself** from the runtime's official distribution, using `curl` or
   `wget` there, and verifies it against a checksum pinned inside the YAAC build. This is the
   fast path: the bytes never cross the SSH link.
2. **If the target cannot** (no internet, no download tool, checksum mismatch), the **host**
   obtains the same artefact, verifies the same pinned checksum, caches it, and uploads it
   in-band over the existing connection, with a progress bar.

The checksum is compiled into YAAC per runtime version and platform, so neither path can be
tricked into running something else.

*Proposed detail:* for step 2 the host downloads by running the operating system's own `curl`
(present on Windows 10+ and on practically every Linux desktop). (The window now links one
HTTPS client for [the updater](#the-updater); the runtime fetch has not moved to it.)

**Built (P39):** the runtime is **Node.js 22.22.3**, `.tar.gz` builds from nodejs.org (musl
builds from the Node.js project's unofficial-builds site), checksums in
`yaac-transport/src/runtime.rs`. The target downloads with `curl` or `wget` and checks the
archive with `sha256sum` or `shasum`; the host caches its verified copy in
`<data>/runtime-cache/` and sends it with `PUT runtime`. Both paths were run once against the
real nodejs.org; the tests use a fake archive served from a temporary folder. Until spike S3
decides otherwise, a **Node.js 22+ already installed for the user is used** when YAAC's own is
not there (P39 departs from P4 here); `YAAC_SYSTEM_NODE=0` makes stage 0 ignore it.

### Stage 0

The command the transport runs on a remote target is a tiny POSIX shell program, *stage 0*. It
does as little as possible: report what is there, get a runtime in place, and hand over to it.

```
target → host   <any shell start-up noise — ignored up to the marker>
target → host   YAAC0 os=<uname -s> arch=<uname -m> libc=<glibc|musl> runtime=<node-version|none> daemon=<builds|none> serving=<build|none> found=<older node|none> private=<name|none> fetch=<curl|wget|none>
host → target   GET runtime <name> <url> <sha256>             # target downloads it itself …
          or    PUT runtime <name> <byte-count> <sha256>      # … or receives exactly that many bytes (if GET failed)
target → host   OK runtime | ERR <reason>
host → target   PUT daemon <build> <byte-count> <sha256>      # only if missing
host → target   RUN node <build>                              # the installed build with the highest build number
target          exec that build's bridge, which attaches to the daemon that is serving or starts itself:
                ~/.yaac/runtime/<version>/<bin> ~/.yaac/daemon/<build>/yaacd.mjs connect
                ── from this byte on, the stream is the YAAC protocol ──
```

Rules that make this robust:

- **The host looks for the marker.** Login scripts sometimes print to standard output even
  without a terminal. The host discards bytes until it sees the `YAAC0` line, within a bounded
  size and time, and shows what it discarded if the marker never comes.
- **The shell hands over early.** As soon as a runtime exists, stage 0 execs it, and that one
  process owns standard input for the rest of the install and for the session. Shell tools read
  bytes only for the very first runtime upload.
- **Installs are atomic and exclusive.** Files are written to a private temporary name, fully
  verified, then renamed into place, under an install lock. Two hosts connecting at once, or a
  connection that dies mid-upload, cannot leave a half-installed runtime or daemon.
- **Install is not replace.** The UI ships the daemon build it was compiled with and makes
  sure it is installed. If an older daemon is already serving, the UI attaches to that one and
  the new build waits its turn ([one serving daemon](#one-serving-daemon-per-user)). The
  protocol version is negotiated in the
  [`initialize` handshake](03-protocol.md#versioning-and-compatibility).
- **The line format is frozen and additive.** It is a list of `key=value` tokens; new keys may
  appear and unknown ones are ignored, so any YAAC can read any stage 0.
- **Failure is explicit.** Unsupported OS or architecture, full disk, hash mismatch, download
  refused: each is a specific error on that target, not a dead connection.

On the local target there is no shell stage.

**Verify (spike S3):** that the Claude Agent SDK survives being bundled into one file with its
harness binary left out; that the chosen runtime runs that bundle on glibc and musl Linux and on
Windows; that the handshake works under `dash`, `bash`, and `busybox`, with a noisy login
script, a non-login `PATH`, an interrupted install, and two simultaneous installs.

## Daemon lifecycle

**Decided:** a running turn survives the UI closing and the connection dropping, and `yaacd`
keeps collecting everything the host will need when it reconnects.

So `yaacd` is a persistent, per-user daemon on the target, and the connection merely attaches
to it:

```
ssh/wsl/local child ──► yaacd connect   (a small stdio ⇄ socket bridge)
                              │
                              ▼
                        yaacd serve     (the daemon; listens on a user-only Unix socket /
                                         Windows named pipe; started on demand, detached)
```

`yaacd connect` attaches to the running daemon, starting it first if needed. This is the same
shape Codex itself uses.

### While no host is attached

`yaacd` behaves exactly as if a host were watching:

- It keeps consuming every provider's event stream — **including keeping its upstream
  subscription to each running thread**. A harness is never blocked, slowed, cancelled, or
  unloaded because nobody is connected.
- Every event is appended to that thread's **journal** — an on-disk, per-thread log with
  sequence numbers ([protocol: journal and replay](03-protocol.md#journal-and-replay)). The
  journal is what makes "nothing is lost" true even if `yaacd` itself restarts.
- Approvals and questions stay **pending**. The agent waits; `yaacd` never answers for the user
  and never times a request out on its own. When the host returns, the thread shows as needing
  attention.
- Thread-index changes (new sessions, renames, state changes) accumulate behind the sync token,
  so the sidebar catches up in one exchange.

What survives what: closing the UI, losing the network, and `yaacd` restarting are all survived.
The target machine (or WSL distro) shutting down, or the harness process itself dying, is not — the thread then
shows the turn as interrupted, with whatever the harness saved.

### One serving daemon per user

On a target there is at most **one** `yaacd serve` per user, enforced by a start-up lock —
regardless of how many hosts or YAAC versions connect. Two daemons would otherwise both own the
same registry, journal, and live harness sessions.

**Decided:** a newer UI never forces out an older daemon. It attaches to whatever is serving,
speaks that daemon's protocol version, shows a warning that the target is on an older version
and will be updated once idle, and offers a button to update it immediately.

- **Daemons only move forward.** When nothing is serving, stage 0 starts the newest build
  installed on the target, whatever the connecting UI asked for. An older UI is then served by
  a newer daemon, which adapts to it.
- **Upgrade happens at idle.** The UI installs its newer build alongside and asks the running
  daemon to hand over when idle. With no running turn and no pending request, the old daemon
  exits; the next connection starts the new one, which migrates the on-disk state forward.
- **"Update now"** interrupts running turns — marked as interrupted by an update — and hands
  over at once.

**How the hub does it** (*Proposed*, P91–P93). The hub is the UI side here: it owns the
connections (invariant #14).

- **Which build is newer.** Each target's connector names the build it installs and starts
  (`Connector::daemon_build`): for WSL and SSH, the hub's embedded bundle, whose install
  folder is `<version>+<id>-<sha16>` (the build id is in the folder name since P92); for the
  local target, only a build the configuration declares. The hub hands over only to a
  **strictly higher build number** than the serving daemon's; a serving development build
  counts as older than any numbered one, so a release replaces it (P94). No bundle, a
  development bundle, or the same or an older build: no hand-over, and no **Update now**
  (`DaemonSummary.canUpdate` is false).
- **"Newest" in stage 0.** The host chooses what `RUN` starts: of the installed builds, the
  one with the highest build number (its own on a tie); its `yaacd connect` attaches to
  whatever serves, or starts itself when nothing does. A development bundle runs itself, as
  before build numbers. So an older YAAC never starts an older daemon over a newer one.
- **At idle.** When a target connects and its daemon can be updated, the hub sends
  `daemon/handover { when: "idle" }` once. The daemon exits as soon as it has no running turn
  and no pending request (a pending approval or question holds it, and stays pending); it
  flushes its journals and marks its state clean first. The hub sees the stream end and
  reconnects through its normal path — the same one connection, never a side channel
  (invariant #1) — and stage 0 starts the newest build, which migrates the state. A starting
  `yaacd` waits (at most 8 s) for the process named in `run/yaacd.pid` to exit, so the new
  daemon never opens state the old one is still writing.
- **The loop guard** (invariant #10). The hub remembers, per target, the pair of builds it
  asked for. It asks a pair at most once per hub run. If after the reconnect the same older
  build serves again (the new build did not start, or the connector started the old one), or
  the daemon refused, it stops: the update shows as **stuck** with the reason, in the banner,
  in Components, and in the target's connection log; it is not asked again. A different
  build serving clears it. The memory is the hub's, in memory only: a restarted hub may ask
  once more. **Update now** stays available, since it is the user's own action.
- **Update now** is `hub/targets/updateDaemon`, sent as `daemon/handover { when: "now" }`.
  It asks first only when turns are in progress on that target, with how many: "*N* turns
  are in progress on devbox. Updating now interrupts them…". Each interrupted turn is marked
  *interrupted because yaacd was updated*; requests waiting in those turns end as expired
  (journaled, visible); a mesh permission prompt is yaacd's own and survives. The window
  shows the answer only through the target's state; a lost answer is reported, never re-sent.
- **What the window shows.** The hub keeps the update in the target's state
  (`ConnectionState.update`: `waiting`, `updating`, `stuck`) across the reconnect: the
  banner says *"devbox is running an older YAAC daemon (…). It will be updated when idle."*
  with **Update now**, then *"Updating yaacd on devbox…"*; Settings → Advanced → Components
  shows *updating to …* or the stuck reason, and the new build once it serves.
- Three things are versioned separately and all visible in diagnostics: the **protocol
  version** (what is spoken on the wire, negotiated in the handshake), the **build** (which
  code is running), and the **state format** (what is on disk).

The full design — negotiation, the frozen core, how each side adapts without version checks —
is [09 Compatibility](09-compatibility.md).

### Build ids

**Decided (D135):** every build carries one **build id**, an increasing number, appended to
each component's own version: `<version>+<id>`. All artefacts of one build share it — the
window and the hub (one `yaac.exe`), the `yaacd` bundle the hub installs on targets, and the
installer (`YAAC-Setup-<version>+<id>.exe`, file version `<numeric version>.<id>`).

- `scripts/build-id.sh` allocates the next number once per build (counter in `.build/counter`,
  not committed); `scripts/build-windows.sh` passes it as `YAAC_BUILD_ID` to `npm run build`
  (the daemon) and to `cargo build` (window and hub), and writes it beside the exe for
  `scripts/build-installer.sh`.
- A build made without the scripts — a plain `cargo build`, tests — is a **development build**:
  `+dev.<hash of its sources>` for the window and hub, `+dev` for the daemon. Development
  builds are told apart but have no order among themselves; a numbered build counts as newer
  than a development daemon (P94).
- **Checked when YAAC starts, not when it is built.** The window compares itself with the hub
  at `hub/hello`: a hub of an older build (or a different development build) is replaced by
  this build's hub; a window older than the hub is told to update. The hub compares each
  target's serving `yaacd` with its own build as it connects, and reports a daemon of a lower
  build number as **older** (`DaemonSummary.older`, also set when the daemon only speaks an
  older protocol). The window gathers these answers — window, hub, and `yaacd` on each enabled
  target — into **Settings → About → Components**, and the footer shows "N to update" while
  any is older.
- Updating an older daemon follows the rules above (at idle, or **Update now**; the hub's
  side is *How the hub does it*). Build ids are build identity, not the protocol version: no protocol
  behaviour depends on them (CLAUDE.md, invariant 5).
- Release builds are numbered by CI: id = 1000 + the release workflow's run number; local
  ids stay at or below 999 (P87, [install](install.md#releases)).

### The updater

**Decided (D136, D145):** on Windows the window updates YAAC from the repository's GitHub releases
([install](install.md#updating)). The flow lives in `yaac-app/src/updater.rs`, with decisions
in `yaac-core/src/update.rs` (manifest, build comparison, the start-up decision table, check
scheduling, progress throttling). The title-row item lives in `yaac-app/src/titlebar.rs`.
The start-up step runs in `yaac_app::run` before the window is created.

- **Network use while running:** the window checks `api.github.com` for the latest release at
  startup and 10 minutes after each check finishes, including while in the tray. The same
  worker fetches the release's `manifest.json` from GitHub's download host with the existing
  10 s timeouts. Requests carry a generic `User-Agent: yaac-updater` and no personal data.
  Failures are silent, and rate limits postpone the next check until GitHub's reset time or
  retry delay if either is later than the normal interval. The hub never makes these
  requests. Targets get the new `yaacd` from the updated hub over their existing connection
  (invariant #1, [install](install.md#how-targets-get-the-new-daemon)).
- **Downloading** the installer happens only on a click, on a worker; progress repaints are
  throttled (a visible step, at most ten a second). The installer is verified (size,
  SHA-256 from the manifest) before `pending.json` is written.
- **Installing** never happens behind the user's back: on *Restart to update*, or at the next
  start when a verified update is waiting. Each update is tried once (`attempted`); a failed
  one starts the old build and says so once.
- **The worker sleeps between checks** and requests one repaint when a check finishes. The
  window takes the result without blocking (invariant #7), including when hidden. The worker
  serializes checks and downloads and suspends checks while a verified installer is waiting
  or installing. At startup, verification of a waiting installer runs before the window
  exists.

### Staying small

- An idle runtime process on the target (target: under 60 MB resident). `yaacd` exits by itself
  after a configurable idle period, but **only** when no client is attached, no turn is running,
  and no request is pending. The journal is on disk, so exiting loses nothing.
  - *Attached* means a connection that has finished `initialize`. A connection that has not (a
    liveness probe, a client still in its handshake) neither holds the daemon nor restarts the
    idle period; it gets a short grace to finish its handshake (10 s from when it was
    accepted) and is closed by the exit after that.
  - A *running turn* holds the daemon until the harness reports it ended, with or without a
    host: the idle period starts when the last turn ends. A *pending request* holds it: an
    approval or a question until it is answered, resolved elsewhere, or expires with its turn;
    a mesh permission prompt, which outlives its turn, until the user answers it. YAAC never
    times a request out to be able to exit.
  - Nothing else counts as activity: journal writes, uploads (a dropped connection discards its
    partial uploads), mail waiting in the outbox for the hub (on disk, sent when a hub returns),
    and open provider connections (a Codex daemon is left running) do not hold the daemon.
  - Stopping is bounded: journals are flushed and closed first; then it waits at most 2 s for
    git status work in flight and 5 s for the providers to close, so an idle exit or a signal
    always ends the process.
- Idle SDK-backed threads are evicted: their harness process is closed after an idle timeout and
  transparently resumed on the next message.
- Journals are compacted and trimmed, so disk use tracks what the host has not yet seen, not the
  lifetime of the thread.

Detaching correctly is platform-specific — surviving the SSH session's cleanup on Linux, a job
object on Windows, a distro shutdown on WSL. Each is specified and tested on its own (spike S8).

## Reconnection

1. The transport notices the stream closed (EOF, child exit, or missed heartbeats).
2. The target is marked *offline* in the UI. Its threads stay visible from cache, read-only.
3. The connector retries with capped exponential backoff, and immediately on user request or
   when the OS reports the network is back. (WSL is the exception: a stopped distro is not
   restarted automatically — see [WSL](#wsl).)
4. After the handshake, the UI resubscribes to its threads with the last sequence number it
   saw, and `yaacd` replays everything after it from the journal. Work that happened while the
   host was away — finished turns, new approvals — appears as if the UI had been watching.

## Where state lives

| State | Lives on | Why |
| --- | --- | --- |
| Conversation transcripts | The **harness** (Codex rollouts, Claude session files) | Single source of truth. `yaacd` does not keep a second transcript. |
| Thread registry: instance bindings, fork lineage, title overrides, provider-instance config | **Target**, in `~/.yaac/` | It describes things on that machine, and a second host should see the same picture. |
| Journals and the thread index | **Target**, in `~/.yaac/` | What the host has not seen yet. |
| Uploaded attachments | **Target**, in the system temp directory | The harness reads them there; the operating system cleans them up. |
| Fork handoff transcripts | **Target**, in `~/.yaac/` | A forked thread's inherited history must outlive a reboot. |
| Targets (SSH and WSL connections, how each SSH connection was set up), appearance, UI preferences | **Host** settings file | They describe this host's view of the world. |
| YAAC-generated SSH keys, one per server that YAAC set up | **Host**, `<data>/ssh/keys/<server>/` (user-only) | Needed to connect; removed on Revoke. |
| Host keys the user confirmed during SSH setup | **Host**, `<data>/ssh/known_hosts` | Checked before the user's own `known_hosts`, which YAAC never writes. |
| What YAAC did on each SSH server | **Host** settings file (`ssh`); **target**, `~/.yaac/ssh/<id>` | Revoke must know whether there is anything of YAAC's to remove. |
| The private runtime archives the host fetched | **Host**, `<data>/runtime-cache/` | Verified against the pinned checksum; sent to targets that cannot download. Safe to delete. |
| **Drafts** — unsent text and attachments | **Host**, in their own durable store | User-authored data. Never part of the disposable cache. |
| Agent-to-agent messages in transit | **Target** outbox and mailbox; **host** relay queue | Each hop stores a message on disk before acknowledging it ([10](10-agent-mesh.md#durability-hop-by-hop)). |
| Thread index, model lists, opened transcripts | **Host** cache (SQLite), written by the hub, read by the window | Speed only. Always safe to delete. |
| The renderer choice and its start marker | **Host**, `<config>/display.json`, written by the window | Needed before the window exists, and must survive "clear cache" so a crashing renderer stays skipped. |

## Security and privacy

- Neither `yaacd` nor the hub ever listens on TCP. Its socket is a Unix socket (mode `0600`, in a `0700`
  directory) or a Windows named pipe restricted to the current user. All remote access rides on
  SSH or WSL, which already authenticate the user.
- The host stores no harness credentials and **no passwords**. The only SSH secret it keeps is
  the private half of a key that YAAC itself generated for one connection during
  [setup](#setting-up-an-ssh-connection) — user-only file permissions, never the user's own
  keys, and deleted on Revoke. Protecting it with the operating system's credential store is a
  later hardening step.
- Agents run tools. Approval prompts from a harness are surfaced to the user and are never
  auto-answered by YAAC. "Skip approvals" modes are available only where the provider offers
  them, and the active mode is always visible under the composer.
- Uploaded file names are sanitised on the target. Uploads can only land inside the upload
  store.
- Anything rendered from an agent is untrusted: markdown rendering does no network fetches, and
  links open only on explicit click.
- The window's only internet traffic is [the updater](#the-updater)'s release check (and an
  installer download the user asked for): no personal data, a generic User-Agent, and every
  installer checked against the release manifest's SHA-256 before it is kept or run.
- **The host cache contains conversation content** — previews, transcripts, tool output,
  thumbnails. It lives in the user's private data directory with user-only permissions.
  Removing a target from settings deletes that target's cached data, and "clear cache" removes
  all of it. Diagnostic logs never contain message content.

## Footprint budget

Targets to hold ourselves to, measured from the first milestone onward. These are goals, not
measurements.

| Metric | Budget |
| --- | --- |
| Host binary, stripped | under 20 MB |
| Window resident memory, idle, 5 000 cached threads | under 80 MB |
| Hub resident memory, window closed | under 20 MB |
| Host CPU, idle | 0 % (no repaint without input or events) |
| Cold start to interactive sidebar | under 300 ms |
| Streaming repaint rate | capped at display refresh; deltas coalesced |
| Stop / approve latency while an upload is running | under 250 ms at the 95th percentile on a slow link |
| `yaacd` daemon bundle (sent on each YAAC update) | under 3 MB |
| `yaacd` runtime (once per target per runtime version) | as small as the chosen runtime allows; shown with progress |
| `yaacd` resident memory, idle | under 60 MB |

Memory is measured for the whole local process tree — the window, the hub, its `ssh` or
`wsl.exe` children, and the local `yaacd` — not one binary alone.

Design choices that serve the budget:

- **One small network stack, in the window only, for the updater.** The UI talks to targets only
  through the hub over pipes. Its one HTTP client is [the updater](#the-updater)'s: `ureq` with
  rustls on *ring* and the webpki root store (no OpenSSL, no system TLS), used on worker
  threads and only by a Windows build (P89). `yaac hub` never calls it. Measured cost: the
  release `yaac.exe` grew from 18.42 MB to 19.60 MB (+1.18 MB), inside the 20 MB budget with
  little room left.
- **One small async runtime** on a single I/O thread, plus a small bounded worker pool — in
  the hub. The window links no async runtime at all beyond what its one local socket needs.
- **One binary, two modes.** `yaac hub` loads no window, GPU, or font code, so the part that
  stays running is the small part.
- **Reactive repaint.** egui repaints on input or on an explicit request from the I/O thread.
- **Bounded memory by construction.** Only open threads hold transcripts in memory; everything
  else is rows in SQLite. See [07 UI](07-ui.md#rendering-and-memory).
- **Renderer chosen by measurement, and by the user.** egui offers an OpenGL backend (`glow`)
  and a `wgpu` backend; both are GPU-accelerated. Both are built in, and the user picks one
  ([the renderer](#the-renderer)); *Automatic* is OpenGL through wgpu (D132): the lowest memory and near-lowest CPU while things move.

### The renderer

D128. The window can draw with OpenGL through `glow`, or with `wgpu` on Direct3D 12, Vulkan or
OpenGL; and in software — on Windows Direct3D 12 on **WARP**, Microsoft's software rasteriser
that every Windows has, so YAAC runs in VMs without a GPU such as Windows 365; elsewhere
wgpu's CPU adapter (lavapipe for Vulkan, llvmpipe for OpenGL).

At start, before any window exists, the window reads its **display file**
(`<config>/display.json`, written by the window itself — it is not the hub's cache, and it is
needed before the attach): the user's choice, and a start marker. Then:

| Last start | First try | If it fails to initialise |
| --- | --- | --- |
| reached its first frame, or first start | the choice | software |
| the choice never reached a first frame (crashed or killed) | software | the choice |
| the choice is software | software | *Automatic* |
| the choice is Windows WPF | WPF window | *Automatic* |

"Fails to initialise" is eframe returning an error — no adapter, no device, no OpenGL context
— and the retry happens in the same process (eframe keeps its event loop for a second
window). The marker "starting with X" is written before X is initialised and cleared after the
first frame, so a driver that takes the whole process down is caught at the next start; that
renderer is then skipped until the user picks a renderer again (P72), so a crash cannot become
a loop. On wgpu the adapter is picked by YAAC: hardware first (discrete, integrated, virtual),
the CPU adapter only for software — or when it is all there is, and then Settings says
"software". What runs is recorded and shown in Settings → Window ([07](07-ui.md#display)).

`YAAC_RENDERER=<id>` or `--renderer <id>` (`auto`, `dx12`, `vulkan`, `wgpu-gl`, `glow`,
`software`, `windows-wpf`) overrides the choice for one start; `YAAC_RENDERER_FAIL=1` makes the first attempt
fail, to exercise the fallback.

Building `wgpu` in costs **3.3 MB** of executable: the Windows release `yaac.exe` is 14.6 MB with
`glow` only and 17.9 MB with both (still under the 20 MB budget). The tray adds four small
crates on Windows (`tray-icon`, `muda`, `keyboard-types`, `crossbeam-channel`).

Footprint per renderer, measured on a Windows 11 laptop with an integrated and a dedicated GPU, the
window maximised, 12 s to settle, then CPU time over 15 s (`Get-Process` TotalProcessorTime)
and memory at the end. *Idle* is the demo's first-run page (`--demo fresh`), where nothing
moves; *busy* is the demo fleet (`--demo`), whose streaming turns and spinners repaint
continuously.

| Renderer | Adapter picked | CPU idle | CPU busy | Working set (idle) | Private bytes (idle) |
| --- | --- | --- | --- | --- | --- |
| OpenGL (glow) — *Automatic* | the default OpenGL driver | 0.1 % | 18.8 % | 163 MB | 88 MB |
| Direct3D 12 | dedicated GPU | 0.1 % | 2.5 % | 247 MB | 427 MB |
| Vulkan | dedicated GPU | 0.3 % | 2.3 % | 199 MB | 418 MB |
| OpenGL (wgpu) | integrated GPU | 0.1 % | 3.5 % | 196 MB | 59 MB |
| Software (WARP) | Microsoft Basic Render Driver | 0.1 % | **~2 300 %** (23 cores) | 145 MB | 360 MB |

(CPU percentages are of one core.) Idle is zero for every renderer, as the reactive repaint
promises. While something animates, software rendering is expensive — WARP spreads each frame
over every core — which is acceptable for a fallback that exists to make YAAC usable on a
machine with no GPU, and is a reason to keep animations few and slow. `glow` costs more CPU per
repaint than wgpu on this machine but has the smallest memory, which is why *Automatic* stays
`glow`. A window hidden in the tray used 0 ms of CPU over 15 s while the demo fleet streamed
(it had used 25 % visible). On WSLg the only Vulkan adapter is lavapipe ("llvmpipe"), so
*Vulkan* and *Software* both run on the CPU there, and Settings says so.

### Windows compatibility

The Windows compatibility renderer uses WPF and runs through the same Rust controller and hub
connection. The controller launches the Windows presentation and exchanges view updates and
input over anonymous pipes. WPF owns the visible window and notification-area icon. The
controller retains subscriptions, drafts, preferences, and the single-instance and quit
signals. The installer ships the self-contained WPF runtime.

Choose **Windows WPF** under **Settings → Window → Display**. Existing `windows-compat`
preferences use WPF. The demo command is `yaac.exe --demo fleet --renderer windows-wpf`.
Renderer selection applies on restart.
