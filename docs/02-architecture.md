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
  cache for an instant first frame, and paints. Closing it stops nothing.

Inside the window the UI thread never blocks on I/O: it reads an in-memory state snapshot and
paints. Inside the hub one I/O thread owns every connection and decodes frames; anything that
can block — SQLite writes, hashing a pasted file, decoding an image — runs on a small bounded
worker pool, so one slow disk operation cannot stall every target at once.

The window and the hub are always the same build: a newer window restarts an older hub, which
is safe because the hub holds no work of its own — turns live in `yaacd` on the targets and
are journaled there.

## The single-connection rule

**Decided.** For each target the host holds exactly one open connection, and it is a plain
byte stream: the standard input and output of one child process. The hub holds it; the window
never opens a connection of its own.

| Target | The one child process | Notes |
| --- | --- | --- |
| Local | `<private runtime> yaacd.mjs connect` | No shell in between. |
| SSH | `ssh -T <host> <bootstrap>` | One SSH session, one exec channel, no PTY. |
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

The app unpacks its private runtime and the `yaacd` bundle once into the user data directory
and spawns the runtime directly. On Windows this is a native Windows process tree; no WSL and no
shell are involved.

### SSH

**Decided:** YAAC drives the system OpenSSH client (`ssh`, `ssh.exe`) as the one child process.

- It honours `~/.ssh/config`, agents, `ProxyJump`, hardware keys, `known_hosts`, and every
  corporate quirk already solved on the user's machine.
- Windows has shipped `ssh.exe` since Windows 10 1809.
- Prompts (a password, a key passphrase, a new host key to confirm) are answered through
  `SSH_ASKPASS`, which we point at the YAAC binary, so they come from YAAC's own window.

Invocation shape: `ssh -T -o EscapeChar=none -o ServerAliveInterval=15 <args> <host> <bootstrap>`.
`-T` and `EscapeChar=none` keep the stream binary-clean.

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

**Verify (spike S9):** the askpass flow for a password, a key passphrase, and a host-key prompt
through the system `ssh` on both Windows and Linux, with no terminal attached.

### WSL

Only offered on Windows hosts, and **off by default**. When the user turns it on, YAAC lists
distros with `wsl.exe -l -q` (note: that command prints UTF-16LE) and the user adds the ones
they want.

**Decided:** a WSL connection is a **distro** plus an **optional user**. A distro can have
several Linux users, each with its own home directory, harness sign-ins, and conversations.
Left empty, the connection runs as the distro's default user; filled in, it runs as that user
(`wsl.exe -d <distro> -u <user> --exec …`). The user field is validated against the distro
when the connection is tested.

Each enabled WSL connection gets one long-lived `wsl.exe` child. The same distro may be added
more than once with different users; see [what a target is](#what-a-target-is).

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
(present on Windows 10+ and on practically every Linux desktop), keeping the host binary free
of an HTTP and TLS stack ([footprint](#footprint-budget)).

### Stage 0

The command the transport runs on a remote target is a tiny POSIX shell program, *stage 0*. It
does as little as possible: report what is there, get a runtime in place, and hand over to it.

```
target → host   <any shell start-up noise — ignored up to the marker>
target → host   YAAC0 os=<uname -s> arch=<uname -m> libc=<glibc|musl> runtime=<version|none> daemon=<builds|none> serving=<build|none>
host → target   GET runtime <version> <url> <sha256>          # target downloads it itself …
          or    PUT runtime <version> <byte-count> <sha256>   # … or receives exactly that many bytes
target → host   OK runtime | ERR <reason>
host → target   PUT daemon <build> <byte-count> <sha256>      # only if missing
host → target   RUN <runtime-version> <build>
target          exec the bridge of the build that is serving — or, if none is, of the newest installed:
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
- Three things are versioned separately and all visible in diagnostics: the **protocol
  version** (what is spoken on the wire, negotiated in the handshake), the **build** (which
  code is running), and the **state format** (what is on disk).

The full design — negotiation, the frozen core, how each side adapts without version checks —
is [09 Compatibility](09-compatibility.md).

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
| YAAC-generated SSH keys, one per connection that YAAC set up | **Host**, in a user-only key directory | Needed to connect; removed on Revoke. |
| **Drafts** — unsent text and attachments | **Host**, in their own durable store | User-authored data. Never part of the disposable cache. |
| Agent-to-agent messages in transit | **Target** outbox and mailbox; **host** relay queue | Each hop stores a message on disk before acknowledging it ([10](10-agent-mesh.md#durability-hop-by-hop)). |
| Thread index, model lists, opened transcripts | **Host** cache (SQLite), written by the hub, read by the window | Speed only. Always safe to delete. |

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

- **No network stack in the host.** The UI talks only to child processes over pipes, so it links
  no HTTP client and no TLS library.
- **One small async runtime** on a single I/O thread, plus a small bounded worker pool — in
  the hub. The window links no async runtime at all beyond what its one local socket needs.
- **One binary, two modes.** `yaac hub` loads no window, GPU, or font code, so the part that
  stays running is the small part.
- **Reactive repaint.** egui repaints on input or on an explicit request from the I/O thread.
- **Bounded memory by construction.** Only open threads hold transcripts in memory; everything
  else is rows in SQLite. See [07 UI](07-ui.md#rendering-and-memory).
- **Renderer chosen by measurement.** egui offers an OpenGL backend (`glow`) and a `wgpu`
  backend; both are GPU-accelerated. `glow` usually uses noticeably less memory, `wgpu` is the
  more future-proof path. Spike S1 measures both and picks one, with the other kept behind a
  cargo feature.
