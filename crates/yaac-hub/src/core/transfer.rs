//! Moving the hub to another machine (docs/15-hub-transfer.md, P408–P410):
//! `hub/transfer/candidates` and `hub/transfer/start`.
//!
//! Everything goes over the chosen target X's one existing `yaacd` connection (invariant 1);
//! the new hub connects to targets only after this one has let go of all of them. One transfer
//! at a time, one attempt per request (invariant 10): nothing is retried by itself. Blocking
//! work (the release lookup, SQLite snapshots, hashing, the tombstone) runs off the I/O thread
//! and reports back as [`Event`]s; every step is announced to every client as
//! `hub/transfer/progress`.
//!
//! ```text
//! checking ─ probe X + this node's Tailscale login ─► installing ─ release asset, hubHost/install
//!   ─► copying ─ live cache snapshot, hubHost/put
//!   ─► handingOver ─ refuse state changes (hub_moving), close every target but X, snapshot
//!        relay/drafts/cache on the store thread, put them + YAAC's own SSH files + hub.json,
//!        hubHost/start {armOnClose}
//!   ─► tombstone + start-at-login off ─► moved: hub/moved to every client, close X, exit
//! ```
//!
//! **Into this PC's WSL** (a WSL target X, P416): the same steps, but `yaacd` in the distro only
//! places the files (`hubHost/start {startHub: false}`). The new hub runs there as a systemd
//! system service, which `yaacd` (no root) cannot install. With the tombstone this hub starts
//! the one-shot `yaac wsl-hub install`, which waits for this hub to exit (no connection to the
//! distro left, invariant 1) and for the files to be placed, then writes and starts the unit
//! through `wsl.exe -u root`. This computer's start-at-login of the hub is turned off.
//!
//! A failure before the tombstone sends `hubHost/abort`, reconnects the targets, and reports
//! `failed` with the reason: nothing has moved. Once `hubHost/start` was sent the new hub may
//! be armed, so from then on a dropped connection counts as moved (the new hub starts when
//! this connection closes; the window offers **Start the hub here again** if it never answers).

use std::collections::{HashMap, VecDeque};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use yaac_protocol::attach::hub_transfer::{self as phases, codes};
use yaac_protocol::attach::{HubMoved, HubTransferCandidate, HubTransferCandidatesResult, HubTransferProgress, HubTransferStartParams, TargetTransport};
use yaac_protocol::methods::{self, names};
use yaac_protocol::rpc::{ErrorCode, ErrorObject};
use yaac_protocol::{
    Capability, HubHostAbortParams, HubHostInstallParams, HubHostProbeResult, HubHostProgressParams, HubHostPutCompleteParams,
    HubHostPutParams, HubHostPutResult, HubHostStartParams, HubHostStartResult, OpId, Sha256Hex, StreamId, TargetId,
};

use super::{ClientKey, Core, CoreEvent, Hello, Message, Pending, RpcId, attach, err, hub_err, parse, state, to_value};
use crate::drafts::hex;
use crate::tailnet::ThisNode;
use crate::transfer::{HUB_ARCHES, HubAsset, WslService, op_id_ok, rewrite_settings};

/// Progress is announced at most this often while bytes move (and at every phase change).
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// Bytes a pool worker reads per job.
const READ_CHUNK: u64 = 256 * 1024;
/// A new read is started once fewer than this many bytes wait in the scheduler.
const READ_LOW: u64 = 256 * 1024;

/// A file staged on this computer, to put on X.
#[derive(Clone, Debug)]
pub(crate) struct Staged {
    name: String,
    path: PathBuf,
    size: u64,
    sha256: String,
}

/// The file being put: its stream, how much of it is queued.
#[derive(Debug)]
struct Put {
    file: Staged,
    stream: Option<StreamId>,
    /// Bytes read and handed to the session.
    offset: u64,
    reading: bool,
    closed: bool,
}

/// A request to X (or to a candidate) and what its answer means.
#[derive(Debug)]
pub(super) enum Ask {
    CandidateProbe { serial: u64 },
    Probe,
    Install,
    Put,
    Start,
    Abort,
}

/// What a thread, the pool, or the store worker reports.
#[derive(Debug)]
pub(crate) enum Event {
    /// This hub's Tailscale node, for a candidates request.
    CandidateNode { serial: u64, login: Result<ThisNode, String> },
    /// This hub's Tailscale node, for the transfer.
    Node { op: OpId, login: Result<ThisNode, String> },
    /// The release asset of this build.
    Asset { op: OpId, result: Result<HubAsset, String> },
    /// The live copy of the cache.
    LiveCopy { op: OpId, result: Result<Staged, String> },
    /// The final copies, `hub.json`, and YAAC's own SSH files, after the quiesce.
    Final { op: OpId, result: Result<Vec<Staged>, String> },
    /// The next chunk of the file being put.
    Chunk { op: OpId, offset: u64, result: Result<Vec<u8>, String> },
    /// The tombstone is written (and start-at-login turned off).
    Tombstoned { op: OpId, result: Result<(), String> },
}

/// The transfer under way.
#[derive(Debug)]
pub(super) struct Transfer {
    op: OpId,
    target: TargetId,
    label: String,
    /// X's connection generation; another one means the connection dropped.
    generation: u64,
    phase: &'static str,
    done: Option<u64>,
    total: Option<u64>,
    /// Refusing state changes; every target but X closed.
    pub(super) quiesced: bool,
    /// `hubHost/start` was sent: the new hub may be armed.
    arming: bool,
    probe: Option<HubHostProbeResult>,
    login: Option<Result<ThisNode, String>>,
    /// X is one of this PC's WSL distros: its name, and this PC's `yaac.exe` as the distro
    /// sees it.
    wsl: Option<(String, String)>,
    /// Into WSL, once checked: the hub's data directory there and its system unit.
    service: Option<(String, String)>,
    /// X runs Windows, with this build of YAAC installed: nothing is downloaded.
    windows: bool,
    /// How the PC's window reaches the new hub (`hub/moved`'s `via`).
    via: Option<TargetTransport>,
    /// This computer's staging directory, `<data>/transfer-<op>`.
    dir: PathBuf,
    queue: VecDeque<Staged>,
    put: Option<Put>,
    /// Bytes of the current group already put.
    sent: u64,
    live_cache: Option<String>,
    /// Targets this hub closed for the hand-over (reconnected on failure).
    closed: Vec<TargetId>,
    last_progress: Option<Instant>,
    /// The new hub's address and host, once known.
    address: Option<String>,
    host_name: String,
}

/// A `hub/transfer/candidates` request waiting for its probes.
#[derive(Debug)]
pub(super) struct Candidates {
    client: ClientKey,
    id: RpcId,
    waiting: usize,
    rows: Vec<(TargetId, String, Option<String>)>,
    probes: HashMap<TargetId, Result<HubHostProbeResult, String>>,
    login: Option<Result<ThisNode, String>>,
}

/// Whether a request only reads: served during the hand-over, when everything else is refused
/// with `hub_moving`.
fn read_only(method: &str) -> bool {
    use crate::protocol::names as ext;
    matches!(
        method,
        attach::names::TARGETS_LIST
            | attach::names::TRANSFER_CANDIDATES
            | attach::names::RATE_LIMITS_GET
            | attach::names::NOTIFICATIONS_SUBSCRIBE
            | attach::names::NOTIFICATIONS_VISIBLE
            | attach::names::PREVIEW_LIST
            | ext::STATUS
            | ext::SETTINGS_GET
            | ext::TARGETS_LOG
            | ext::VIEW_GET
            | ext::VIEW_SHOWING
            | ext::DRAFTS_LIST
            | ext::OPERATIONS_LIST
            | ext::UI_STATE_GET
            | ext::PROJECTS_LIST
            | ext::THREADS_FIND
            | ext::UPLOAD_LIST
            | ext::BLOB_READ
            | ext::BLOB_LIST
    )
}

/// Whether X, as `yaacd` there describes it, can take the hub (P408): a Linux machine with a
/// published hub for its architecture, or a Windows machine with this exact build of YAAC
/// installed, and no hub already running there. `mine`: this hub's own Tailscale node, when
/// Tailscale runs here.
///
/// Tailscale never decides: it only decides whether phones can reach the hub there
/// ([`warning`]). When X has an address of its own, port 47380 must be free on it.
fn judge(name: &str, probe: &HubHostProbeResult, mine: Option<&ThisNode>, build: &str) -> Result<(), String> {
    match probe.os.as_str() {
        "linux" => {
            if !HUB_ARCHES.contains(&probe.arch.as_str()) {
                return Err(format!("There is no YAAC hub for {} yet.", probe.arch));
            }
        }
        WINDOWS => match probe.hub_build.as_deref() {
            None => return Err(format!("Install YAAC on {name} first.")),
            Some(theirs) if theirs != build => {
                return Err(format!("YAAC on {name} is build {theirs} and this hub is {build}: install the same build there first."));
            }
            Some(_) => {}
        },
        other => return Err(format!("The hub runs on Linux or Windows; {name} is {other}.")),
    }
    if address(probe, mine).is_some() && !probe.port_free {
        return Err(format!("Port {} is in use on {name}.", crate::tailnet::TAILSCALE_ATTACH_PORT));
    }
    if probe.hub_running {
        return Err(format!("A YAAC hub already runs on {name}."));
    }
    Ok(())
}

/// Whether a WSL distro, as `yaacd` there describes it, can run the moved hub as a system
/// service (P416): systemd must be its system manager. The unit is checked here too, so a
/// distro that cannot take it fails before anything moves.
fn judge_wsl(name: &str, probe: &HubHostProbeResult) -> Result<(String, String), String> {
    let Some(service) = probe.wsl.as_ref().and_then(|w| w.service.as_ref()) else {
        return Err(format!("Update YAAC in {name} first."));
    };
    if !service.systemd {
        return Err(format!(
            "The hub runs in {name} as a system service, and systemd is off there: add systemd=true under [boot] in /etc/wsl.conf, then restart {name} (wsl --terminate {name})."
        ));
    }
    Ok((service.data_dir.clone(), crate::transfer::system_unit(service)?))
}

/// `process.platform` of a Windows `yaacd`.
const WINDOWS: &str = "win32";

/// What the confirmation tells the user about phones before the hub moves to X: they reach a
/// moved hub only over X's Tailscale, as X's own login.
fn warning(name: &str, probe: &HubHostProbeResult, mine: Option<&ThisNode>) -> Option<String> {
    if address(probe, mine).is_none() {
        return Some(format!("The phone can't reach the hub there until Tailscale runs on {name}."));
    }
    let theirs = probe.tailscale.login.as_deref().filter(|l| !l.is_empty());
    if theirs.zip(mine).is_some_and(|(t, m)| crate::tailnet::same_login(t, &m.login)) {
        return None;
    }
    Some(match theirs {
        Some(login) => format!("The hub will accept only devices signed in to Tailscale as {login}; your phone must use that account."),
        None => format!("The hub will accept only devices signed in to the Tailscale account of {name}; your phone must use that account."),
    })
}

/// The new hub's attach address for phones: X's first Tailscale IPv4 (else IPv6) and the
/// Tailscale port. `None` when Tailscale does not run on X, or X only sees this computer's own
/// Tailscale (WSL with mirrored networking, or Windows' `tailscale.exe`): it is no node of X's.
fn address(probe: &HubHostProbeResult, mine: Option<&ThisNode>) -> Option<String> {
    if !probe.tailscale.running {
        return None;
    }
    let ips: Vec<std::net::IpAddr> = probe.tailscale.ips.iter().filter_map(|ip| ip.parse().ok()).collect();
    if mine.is_some_and(|m| ips.iter().any(|ip| m.addresses.contains(ip))) {
        return None;
    }
    let ip = ips.iter().find(|ip| ip.is_ipv4()).or_else(|| ips.first())?;
    Some(std::net::SocketAddr::new(*ip, crate::tailnet::TAILSCALE_ATTACH_PORT).to_string())
}

/// How the PC's window reaches the hub on a target of this transport, without Tailscale
/// (`hub/moved`'s `via`): the same way this hub reached it; `local` for the PC's own Windows.
fn via(transport: &TargetTransport) -> Option<TargetTransport> {
    match transport {
        TargetTransport::Ssh(_) | TargetTransport::Wsl(_) => Some(transport.clone()),
        TargetTransport::WindowsInterop(_) => Some(TargetTransport::Local(yaac_protocol::attach::LocalTransport {})),
        TargetTransport::Local(_) | TargetTransport::Unknown(_) => None,
    }
}

/// A consistent copy of a SQLite file, through its own read-only connection (the hub keeps
/// writing to the original meanwhile).
fn snapshot_file(src: &Path, dest: &Path) -> Result<(), String> {
    use rusqlite::OpenFlags;
    let conn = rusqlite::Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| format!("cannot open {}: {e}", src.display()))?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
    crate::transfer::vacuum_into(&conn, dest)
}

/// Size and SHA-256 of a file.
fn hash(path: &Path) -> Result<(u64, String), String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        h.update(&buf[..n]);
        size += n as u64;
    }
    Ok((size, hex(&h.finalize())))
}

fn staged(name: &str, path: PathBuf) -> Result<Staged, String> {
    let (size, sha256) = hash(&path)?;
    Ok(Staged { name: name.to_owned(), path, size, sha256 })
}

fn read_chunk(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    file.take(len).read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

impl Core {
    pub(super) fn transfer_quiesced(&self) -> bool {
        self.transfer.as_ref().is_some_and(|t| t.quiesced)
    }

    /// During the hand-over, every request that may change state is refused.
    pub(super) fn transfer_refuses(&self, method: &str) -> Option<ErrorObject> {
        let t = self.transfer.as_ref().filter(|t| t.quiesced)?;
        (!read_only(method)).then(|| hub_err(codes::MOVING, format!("The hub is moving to {}; try again once it is there.", t.label)))
    }

    /// This PC's `yaac.exe` as its WSL distros see it, when this hub can move into one.
    fn interop_exe(&self) -> Option<String> {
        self.cfg.transfer_host.as_ref().and_then(|h| h.interop_exe())
    }

    /// Why this target cannot take the hub, before asking it anything.
    fn transfer_static_reason(&self, id: &TargetId) -> Option<String> {
        let t = self.targets.get(id)?;
        match &t.def.transport {
            TargetTransport::Ssh(_) => {}
            TargetTransport::Wsl(_) if self.cfg.transfer_host.is_none() || self.interop_exe().is_some() => {}
            TargetTransport::Wsl(_) => return Some("Only YAAC for Windows can move the hub into WSL.".into()),
            TargetTransport::Local(_) => return Some("This is the hub's own computer.".into()),
            // This PC's Windows, from its WSL: YAAC is installed there, nothing is downloaded.
            TargetTransport::WindowsInterop(_) => return self.transfer_connected_reason(t),
            TargetTransport::Unknown(_) => return Some("YAAC cannot move the hub over this kind of connection.".into()),
        }
        if super::build_number(&self.cfg.build).is_none() {
            return Some(format!("This hub is a development build of YAAC ({}); there is no release binary to install.", self.cfg.build));
        }
        if self.cfg.transfer_host.is_none() {
            return Some("This build of YAAC cannot move its hub.".into());
        }
        self.transfer_connected_reason(t)
    }

    /// Why a target cannot be asked: not connected, or its `yaacd` cannot host a hub.
    fn transfer_connected_reason(&self, t: &super::Target) -> Option<String> {
        let conn = t.conn.as_ref().filter(|c| c.ready);
        let Some(conn) = conn else { return Some("Connect it first.".into()) };
        if !conn.session.features().is_some_and(|f| f.allows(&Capability::HubHost)) {
            return Some("Update YAAC on that target first.".into());
        }
        None
    }

    fn this_node(&self, done: impl FnOnce(Result<ThisNode, String>) -> Event + Send + 'static) {
        let node = self.cfg.tailnet.this_node();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let login = node.await;
            let _ = tx.send(CoreEvent::Transfer(done(login)));
        });
    }

    // ── hub/transfer/candidates ─────────────────────────────────────────────────────────

    pub(super) fn transfer_candidates(&mut self, client: ClientKey, id: &RpcId) {
        self.transfer_serial += 1;
        let serial = self.transfer_serial;
        let busy = self.transfer.as_ref().map(|t| format!("The hub is already moving to {}.", t.label));
        let mut c = Candidates { client, id: id.clone(), waiting: 1, rows: Vec::new(), probes: HashMap::new(), login: None };
        let ids: Vec<TargetId> = self.settings.targets.iter().map(|d| d.id.clone()).collect();
        for target in ids {
            let label = self.targets.get(&target).map_or_else(String::new, |t| t.def.name.as_str().to_owned());
            let reason = busy.clone().or_else(|| self.transfer_static_reason(&target));
            if reason.is_none() && self.send::<methods::HubHostProbe>(&target, &yaac_protocol::Empty {}, Pending::Transfer(Ask::CandidateProbe { serial })) {
                c.waiting += 1;
                self.flush(&target);
            }
            c.rows.push((target, label, reason));
        }
        self.candidates.insert(serial, c);
        self.this_node(move |login| Event::CandidateNode { serial, login });
    }

    fn candidates_answered(&mut self, serial: u64, target: Option<(&TargetId, Result<HubHostProbeResult, String>)>, login: Option<Result<ThisNode, String>>) {
        let Some(c) = self.candidates.get_mut(&serial) else { return };
        if let Some((t, probe)) = target {
            c.probes.insert(t.clone(), probe);
        }
        if login.is_some() {
            c.login = login;
        }
        c.waiting = c.waiting.saturating_sub(1);
        if c.waiting > 0 {
            return;
        }
        let c = self.candidates.remove(&serial).expect("present");
        let mine = c.login.and_then(Result::ok);
        let build = self.cfg.build.clone();
        let wsl: std::collections::HashSet<TargetId> =
            self.targets.iter().filter(|(_, t)| matches!(t.def.transport, TargetTransport::Wsl(_))).map(|(id, _)| id.clone()).collect();
        let candidates = c.rows.into_iter().map(|(target_id, label, reason)| {
            let probe = c.probes.get(&target_id).and_then(|p| p.as_ref().ok());
            let reason = reason.or_else(|| match c.probes.get(&target_id) {
                Some(Ok(probe)) => judge(&label, probe, mine.as_ref(), &build)
                    .and_then(|()| if wsl.contains(&target_id) { judge_wsl(&label, probe).map(drop) } else { Ok(()) })
                    .err(),
                Some(Err(e)) => Some(e.clone()),
                None => Some("Connect it first.".into()),
            });
            let warning = probe.filter(|_| reason.is_none()).and_then(|p| warning(&label, p, mine.as_ref()));
            HubTransferCandidate { target_id, label, eligible: reason.is_none(), reason, warning }
        }).collect();
        self.reply(c.client, c.id, Ok(to_value(&HubTransferCandidatesResult { candidates })));
    }

    // ── hub/transfer/start ──────────────────────────────────────────────────────────────

    pub(super) fn transfer_start(&mut self, client: ClientKey, id: &RpcId, params: Value) -> Option<Result<Value, ErrorObject>> {
        let p: HubTransferStartParams = match parse(params) {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };
        if !op_id_ok(p.op_id.as_str()) {
            return Some(Err(err(ErrorCode::InvalidParams, "opId: letters, digits, '-' and '_' only, at most 128")));
        }
        if let Some(t) = &self.transfer {
            return Some(Err(hub_err(codes::IN_PROGRESS, format!("The hub is already moving to {}.", t.label))));
        }
        let Some(t) = self.targets.get(&p.target_id) else { return Some(Err(err(ErrorCode::TargetUnknown, "no such target"))) };
        if let Some(reason) = self.transfer_static_reason(&p.target_id) {
            return Some(Err(hub_err(codes::NOT_ELIGIBLE, reason)));
        }
        let label = t.def.name.as_str().to_owned();
        let host_name = t.host.clone().unwrap_or_else(|| label.clone());
        let wsl = match &t.def.transport {
            TargetTransport::Wsl(w) => {
                let Some(exe) = self.interop_exe() else { return Some(Err(hub_err(codes::NOT_ELIGIBLE, "Only YAAC for Windows can move the hub into WSL."))) };
                Some((w.distro.clone(), exe))
            }
            _ => None,
        };
        let generation = t.conn.as_ref().map_or(0, |c| c.generation);
        let dir = self.cfg.paths.data_dir.join(format!("transfer-{}", p.op_id.as_str()));
        self.transfer = Some(Transfer {
            op: p.op_id.clone(),
            target: p.target_id.clone(),
            label,
            generation,
            phase: phases::CHECKING,
            done: None,
            total: None,
            quiesced: false,
            arming: false,
            probe: None,
            login: None,
            wsl,
            service: None,
            windows: false,
            via: via(&t.def.transport),
            dir,
            queue: VecDeque::new(),
            put: None,
            sent: 0,
            live_cache: None,
            closed: Vec::new(),
            last_progress: None,
            address: None,
            host_name,
        });
        crate::core::log(format!("transfer {} to {}: started", p.op_id.as_str(), p.target_id));
        self.reply(client, id.clone(), Ok(json!({})));
        self.transfer_announce(true);
        let target = p.target_id;
        if !self.send::<methods::HubHostProbe>(&target, &yaac_protocol::Empty {}, Pending::Transfer(Ask::Probe)) {
            self.transfer_fail("The target is not connected.".into());
            return None;
        }
        self.flush(&target);
        let op = p.op_id;
        self.this_node(move |login| Event::Node { op, login });
        None
    }

    /// An answer from X (or a candidate).
    pub(super) fn transfer_response(&mut self, id: &TargetId, ask: &Ask, outcome: Result<Value, ErrorObject>) {
        if let Ask::CandidateProbe { serial } = *ask {
            let probe = outcome.map_err(|e| e.message).and_then(|v| serde_json::from_value::<HubHostProbeResult>(v).map_err(|e| e.to_string()));
            self.candidates_answered(serial, Some((id, probe)), None);
            return;
        }
        if matches!(ask, Ask::Abort) {
            if let Err(e) = outcome {
                crate::core::log(format!("{id}: hubHost/abort: {}", e.message));
            }
            return;
        }
        if self.transfer.as_ref().is_none_or(|t| &t.target != id) {
            return;
        }
        let outcome = outcome.map_err(|e| e.message);
        match ask {
            Ask::Probe => match outcome.and_then(|v| serde_json::from_value::<HubHostProbeResult>(v).map_err(|e| e.to_string())) {
                Ok(probe) => {
                    self.transfer.as_mut().unwrap().probe = Some(probe);
                    self.transfer_checked();
                }
                Err(e) => self.transfer_fail(format!("The target could not be checked: {e}")),
            },
            Ask::Install => match outcome {
                Ok(_) => self.transfer_live_copy(),
                Err(e) => self.transfer_fail(format!("The hub could not be installed there: {e}")),
            },
            Ask::Put => match outcome.and_then(|v| serde_json::from_value::<HubHostPutResult>(v).map_err(|e| e.to_string())) {
                Ok(r) => {
                    let opened = self.targets.get_mut(id).and_then(super::Target::ready_conn).map(|c| c.session.open_stream(r.stream, r.credit));
                    if !matches!(opened, Some(Ok(()))) {
                        return self.transfer_fail("The target gave a stream that cannot be used.".into());
                    }
                    if let Some(put) = self.transfer.as_mut().and_then(|t| t.put.as_mut()) {
                        put.stream = Some(r.stream);
                    }
                    self.flush(id);
                }
                Err(e) => {
                    let name = self.transfer.as_ref().and_then(|t| t.put.as_ref()).map(|p| p.file.name.clone()).unwrap_or_default();
                    self.transfer_fail(format!("{name} could not be copied: {e}"));
                }
            },
            Ask::Start => match outcome.and_then(|v| serde_json::from_value::<HubHostStartResult>(v).map_err(|e| e.to_string())) {
                Ok(r) if r.armed => self.transfer_tombstone(),
                Ok(_) => self.transfer_fail("The new hub was not armed.".into()),
                Err(e) => self.transfer_fail(format!("The new hub could not be armed: {e}")),
            },
            Ask::CandidateProbe { .. } | Ask::Abort => {}
        }
    }

    /// `hubHost/progress` and `hubHost/putComplete` from X.
    pub(super) fn transfer_notification(&mut self, id: &TargetId, method: &str, params: Value) {
        if self.transfer.as_ref().is_none_or(|t| &t.target != id) {
            return;
        }
        if method == names::HUB_HOST_PROGRESS {
            let Ok(p) = serde_json::from_value::<HubHostProgressParams>(params) else { return };
            let t = self.transfer.as_mut().unwrap();
            if p.op_id == t.op && t.phase == phases::INSTALLING {
                (t.done, t.total) = (Some(p.done), Some(p.total));
                self.transfer_announce(p.done >= p.total);
            }
            return;
        }
        let Ok(p) = serde_json::from_value::<HubHostPutCompleteParams>(params) else { return };
        let t = self.transfer.as_mut().unwrap();
        let Some(put) = t.put.as_ref().filter(|put| put.stream == Some(p.stream)) else { return };
        if let Some(message) = p.message {
            let name = put.file.name.clone();
            return self.transfer_fail(format!("{name} could not be copied: {message}"));
        }
        t.sent += put.file.size;
        t.done = Some(t.sent);
        t.put = None;
        self.transfer_announce(false);
        self.transfer_next_put();
    }

    pub(crate) fn transfer_event(&mut self, event: Event) {
        let op_matches = |t: &Option<Transfer>, op: &OpId| t.as_ref().is_some_and(|t| &t.op == op);
        match event {
            Event::CandidateNode { serial, login } => self.candidates_answered(serial, None, Some(login)),
            Event::Node { op, login } if op_matches(&self.transfer, &op) => {
                self.transfer.as_mut().unwrap().login = Some(login);
                self.transfer_checked();
            }
            Event::Asset { op, result } if op_matches(&self.transfer, &op) => match result {
                Ok(asset) => {
                    let t = self.transfer.as_mut().unwrap();
                    (t.phase, t.done, t.total) = (phases::INSTALLING, Some(0), Some(asset.size));
                    let params = HubHostInstallParams { op_id: op, url: asset.url, sha256: Sha256Hex::new(asset.sha256), size: asset.size };
                    let target = t.target.clone();
                    self.transfer_announce(true);
                    if !self.send::<methods::HubHostInstall>(&target, &params, Pending::Transfer(Ask::Install)) {
                        return self.transfer_fail("The target is not connected.".into());
                    }
                    self.flush(&target);
                }
                Err(e) => self.transfer_fail(e),
            },
            Event::LiveCopy { op, result } if op_matches(&self.transfer, &op) => match result {
                Ok(file) => {
                    let t = self.transfer.as_mut().unwrap();
                    t.live_cache = Some(file.sha256.clone());
                    (t.sent, t.done, t.total) = (0, Some(0), Some(file.size));
                    t.queue.push_back(file);
                    self.transfer_announce(true);
                    self.transfer_next_put();
                }
                Err(e) => self.transfer_fail(format!("The cache could not be copied: {e}")),
            },
            Event::Final { op, result } if op_matches(&self.transfer, &op) => match result {
                Ok(files) => {
                    let t = self.transfer.as_mut().unwrap();
                    let live = t.live_cache.clone();
                    // The cache again only when it changed since the live copy.
                    let files: Vec<Staged> = files.into_iter().filter(|f| !(f.name == "data/cache.sqlite" && Some(&f.sha256) == live.as_ref())).collect();
                    (t.sent, t.done, t.total) = (0, Some(0), Some(files.iter().map(|f| f.size).sum()));
                    t.queue.extend(files);
                    self.transfer_announce(true);
                    self.transfer_next_put();
                }
                Err(e) => self.transfer_fail(format!("The hub's files could not be prepared: {e}")),
            },
            Event::Chunk { op, offset, result } if op_matches(&self.transfer, &op) => {
                let bytes = match result {
                    Ok(bytes) => bytes,
                    Err(e) => return self.transfer_fail(e),
                };
                let t = self.transfer.as_mut().unwrap();
                let target = t.target.clone();
                let Some(put) = t.put.as_mut().filter(|p| p.offset == offset) else { return };
                put.reading = false;
                if bytes.is_empty() && put.offset < put.file.size {
                    let name = put.file.name.clone();
                    return self.transfer_fail(format!("{name} changed while it was being copied."));
                }
                put.offset += bytes.len() as u64;
                let stream = put.stream;
                let queued = stream.and_then(|s| self.targets.get_mut(&target).and_then(super::Target::ready_conn).map(|c| c.session.send_data(s, bytes)));
                if !matches!(queued, Some(Ok(()))) {
                    return self.transfer_fail("The connection to the target dropped.".into());
                }
                self.transfer_announce(false);
                self.flush(&target);
            }
            Event::Tombstoned { op, result } if op_matches(&self.transfer, &op) => match result {
                Ok(()) => self.transfer_moved(),
                Err(e) => self.transfer_fail(e),
            },
            _ => {}
        }
    }

    /// The probe and this node's login are in: is X eligible? Then find the release asset
    /// (nothing to download for Windows: this build of YAAC is installed there).
    fn transfer_checked(&mut self) {
        let t = self.transfer.as_mut().unwrap();
        let (Some(probe), Some(login)) = (&t.probe, &t.login) else { return };
        let mine = login.as_ref().ok();
        if let Err(reason) = judge(&t.label, probe, mine, &self.cfg.build) {
            return self.transfer_fail(reason);
        }
        if t.wsl.is_some() {
            match judge_wsl(&t.label, probe) {
                Ok(service) => t.service = Some(service),
                Err(reason) => return self.transfer_fail(reason),
            }
        }
        let arch = probe.arch.clone();
        t.address = address(probe, mine);
        t.windows = probe.os == WINDOWS;
        if t.windows {
            return self.transfer_live_copy();
        }
        let Some(host) = self.cfg.transfer_host.clone() else { return self.transfer_fail("This build of YAAC cannot move its hub.".into()) };
        let op = t.op.clone();
        let build = self.cfg.build.clone();
        let tx = self.tx.clone();
        let started = std::thread::Builder::new().name("yaac-hub-transfer".into()).spawn(move || {
            let result = host.hub_asset(&build, &arch);
            let _ = tx.send(CoreEvent::Transfer(Event::Asset { op, result }));
        });
        if let Err(e) = started {
            self.transfer_fail(format!("Could not look up the release: {e}"));
        }
    }

    /// The binary is installed: a live copy of the cache, on a thread of its own.
    fn transfer_live_copy(&mut self) {
        let t = self.transfer.as_mut().unwrap();
        (t.phase, t.done, t.total) = (phases::COPYING, None, None);
        let (op, dir, src) = (t.op.clone(), t.dir.clone(), self.cfg.paths.cache_file());
        self.transfer_announce(true);
        let tx = self.tx.clone();
        let started = std::thread::Builder::new().name("yaac-hub-transfer".into()).spawn(move || {
            let result = (|| {
                crate::paths::ensure_private_dir(&dir.join("live")).map_err(|e| e.to_string())?;
                let dest = dir.join("live").join("cache.sqlite");
                snapshot_file(&src, &dest)?;
                staged("data/cache.sqlite", dest)
            })();
            let _ = tx.send(CoreEvent::Transfer(Event::LiveCopy { op, result }));
        });
        if let Err(e) = started {
            self.transfer_fail(format!("Could not copy the cache: {e}"));
        }
    }

    /// Puts the next staged file, or moves on when none is left.
    fn transfer_next_put(&mut self) {
        let t = self.transfer.as_mut().unwrap();
        if t.put.is_some() {
            return;
        }
        let Some(file) = t.queue.pop_front() else {
            return if t.quiesced { self.transfer_arm() } else { self.transfer_quiesce() };
        };
        let params = HubHostPutParams { op_id: t.op.clone(), name: file.name.clone(), size: file.size, sha256: Sha256Hex::new(file.sha256.clone()) };
        t.put = Some(Put { file, stream: None, offset: 0, reading: false, closed: false });
        let target = t.target.clone();
        if !self.send::<methods::HubHostPut>(&target, &params, Pending::Transfer(Ask::Put)) {
            return self.transfer_fail("The connection to the target dropped.".into());
        }
        self.flush(&target);
    }

    /// Feeds the file being put: reads more when the scheduler runs low, closes the stream
    /// once every byte is queued. Called from [`Core::flush`].
    pub(super) fn transfer_pump(&mut self, id: &TargetId) {
        let Some(t) = self.transfer.as_mut().filter(|t| &t.target == id) else { return };
        let op = t.op.clone();
        let Some(put) = t.put.as_mut() else { return };
        let Some(stream) = put.stream else { return };
        if put.closed || put.reading {
            return;
        }
        let Some(conn) = self.targets.get_mut(id).and_then(super::Target::ready_conn) else { return };
        if put.offset >= put.file.size {
            conn.session.close_stream(stream);
            put.closed = true;
            return;
        }
        if conn.session.scheduler().pending_data(stream).unwrap_or(0) >= READ_LOW {
            return;
        }
        put.reading = true;
        let (path, offset) = (put.file.path.clone(), put.offset);
        let len = READ_CHUNK.min(put.file.size - offset);
        self.pool.submit(move || Some(CoreEvent::Transfer(Event::Chunk { op, offset, result: read_chunk(&path, offset, len) })));
    }

    /// Step 4: refuse state changes, let go of every target but X, and snapshot the stores on
    /// the store thread (after every write queued before it).
    fn transfer_quiesce(&mut self) {
        let t = self.transfer.as_mut().unwrap();
        t.quiesced = true;
        (t.phase, t.done, t.total) = (phases::HANDING_OVER, None, None);
        let (x, label, op, dir) = (t.target.clone(), t.label.clone(), t.op.clone(), t.dir.clone());
        let interop_exe = t.wsl.as_ref().map(|(_, exe)| exe.clone());
        let from_wsl = crate::transfer::running_in_wsl().then(|| std::env::var("WSL_DISTRO_NAME").ok()).flatten().filter(|d| !d.is_empty());
        let to = match (&interop_exe, t.windows) {
            (Some(exe), _) => crate::transfer::Destination::IntoWsl { interop_exe: exe },
            (None, true) => crate::transfer::Destination::ToWindows { from_wsl: from_wsl.as_deref() },
            (None, false) => crate::transfer::Destination::Remote,
        };
        self.transfer_announce(true);
        let others: Vec<TargetId> = self.targets.iter().filter(|(id, t)| **id != x && t.supervisor.is_some()).map(|(id, _)| id.clone()).collect();
        for id in &others {
            self.stop_target(id, state(yaac_protocol::attach::ConnectionStatus::Offline, Some(format!("The hub is moving to {label}."))));
        }
        self.transfer.as_mut().unwrap().closed = others;
        let ssh_dir = self.cfg.ssh.as_ref().map(|s| s.dir.clone());
        let (settings, files) = rewrite_settings(&self.settings, &x, &self.cfg.host_name, ssh_dir.as_deref(), &to);
        let json = serde_json::to_vec_pretty(&settings).map(|mut v| {
            v.push(b'\n');
            v
        });
        self.worker.submit(move |stores| {
            let result = (|| {
                let json = json.map_err(|e| e.to_string())?;
                let fin = dir.join("final");
                crate::paths::ensure_private_dir(&fin).map_err(|e| e.to_string())?;
                let _ = stores.relay.checkpoint();
                let _ = stores.drafts.checkpoint();
                let _ = stores.cache.checkpoint();
                stores.relay.snapshot_into(&fin.join("relay.sqlite"))?;
                stores.drafts.snapshot_into(&fin.join("drafts.sqlite"))?;
                stores.cache.snapshot_into(&fin.join("cache.sqlite"))?;
                let hub_json = fin.join("hub.json");
                std::fs::write(&hub_json, json).map_err(|e| e.to_string())?;
                crate::paths::make_private_file(&hub_json).map_err(|e| e.to_string())?;
                let mut out = vec![
                    staged("data/relay.sqlite", fin.join("relay.sqlite"))?,
                    staged("data/drafts.sqlite", fin.join("drafts.sqlite"))?,
                    staged("data/cache.sqlite", fin.join("cache.sqlite"))?,
                ];
                for f in files {
                    out.push(staged(&f.name, f.path)?);
                }
                // Last: `yaacd` takes the staging as complete once the settings are there.
                out.push(staged("config/hub.json", hub_json)?);
                Ok(out)
            })();
            Some(CoreEvent::Transfer(Event::Final { op, result }))
        });
    }

    /// Step 5: every file is staged; arm the new hub to start when this connection closes.
    fn transfer_arm(&mut self) {
        let t = self.transfer.as_mut().unwrap();
        t.arming = true;
        // In WSL the hub's system service starts the new hub; `yaacd` there only places the files.
        let (target, params) = (t.target.clone(), HubHostStartParams { op_id: t.op.clone(), arm_on_close: true, start_hub: t.wsl.is_none() });
        if !self.send::<methods::HubHostStart>(&target, &params, Pending::Transfer(Ask::Start)) {
            return self.transfer_fail("The connection to the target dropped.".into());
        }
        self.flush(&target);
    }

    /// Step 6: the tombstone and start-at-login off, off the I/O thread.
    fn transfer_tombstone(&mut self) {
        let t = self.transfer.as_ref().unwrap();
        let moved = crate::paths::MovedTombstone {
            hub_id: self.settings.hub_id.clone().unwrap_or_default(),
            // Only for phones; empty when Tailscale does not run where the hub went.
            address: t.address.clone().unwrap_or_default(),
            host_name: t.host_name.clone(),
            at_ms: super::now_ms(),
            wsl_distro: t.wsl.as_ref().map(|(d, _)| d.clone()),
            via: t.via.clone(),
        };
        let service = t.wsl.as_ref().zip(t.service.as_ref()).map(|((distro, _), (data_dir, unit))| WslService {
            distro: distro.clone(),
            data_dir: data_dir.clone(),
            unit: unit.clone(),
            op: t.op.clone(),
        });
        let (op, paths, host) = (t.op.clone(), self.cfg.paths.clone(), self.cfg.transfer_host.clone());
        self.pool.submit(move || {
            let result = (|| {
                // Into WSL: the service's install first, so nothing is recorded as moved if it
                // cannot start. It does nothing until this hub has exited and the files are placed.
                if let (Some(service), Some(host)) = (&service, &host) {
                    host.install_wsl_service(service).map_err(|e| format!("The hub's service in WSL could not be set up: {e}"))?;
                }
                crate::paths::write_moved_tombstone(&paths, &moved).map_err(|e| format!("This computer could not record the move: {e}"))
            })();
            if result.is_ok()
                && let Some(host) = host
                && let Err(e) = host.stop_autostart()
            {
                // The tombstone keeps the hub from starting here anyway.
                crate::core::log(format!("transfer: start-at-login unchanged: {e}"));
            }
            Some(CoreEvent::Transfer(Event::Tombstoned { op, result }))
        });
    }

    /// Done: every client hears the new address, then X is closed (last) and the hub exits.
    fn transfer_moved(&mut self) {
        let Some(t) = self.transfer.as_mut() else { return };
        (t.phase, t.done, t.total) = (phases::MOVED, None, None);
        let moved = HubMoved {
            hub_id: self.settings.hub_id.clone().unwrap_or_default(),
            address: t.address.clone().unwrap_or_default(),
            host_name: t.host_name.clone(),
            via: t.via.clone(),
        };
        let (target, dir) = (t.target.clone(), t.dir.clone());
        crate::core::log(format!("transfer {}: moved to {}", t.op.as_str(), moved.address));
        self.transfer_announce(true);
        self.transfer_notify_all(&Message::notification(attach::names::MOVED, to_value(&moved)));
        // Bytes still queued for X (the last acks) go out before it is closed.
        self.flush(&target);
        let _ = std::fs::remove_dir_all(&dir);
        if let Some(kill) = self.targets.get_mut(&target).and_then(|t| t.conn.as_mut()).and_then(|c| c.kill.take()) {
            let _ = kill.send(());
        }
        self.exit = Some(super::ExitReason::Moved);
    }

    /// Before the tombstone: abort on X, reconnect the targets, report why. Nothing moved.
    fn transfer_fail(&mut self, reason: String) {
        let Some(t) = self.transfer.take() else { return };
        crate::core::log(format!("transfer {} to {} failed: {reason}", t.op.as_str(), t.target));
        let x = t.target.clone();
        if let Some(conn) = self.targets.get_mut(&x).and_then(super::Target::ready_conn).filter(|c| c.generation == t.generation) {
            if let Some(stream) = t.put.as_ref().and_then(|p| p.stream) {
                conn.session.cancel_stream(stream);
            }
            let params = HubHostAbortParams { op_id: t.op.clone() };
            self.send::<methods::HubHostAbort>(&x, &params, Pending::Transfer(Ask::Abort));
            self.flush(&x);
        }
        for id in &t.closed {
            if self.targets.get(id).is_some_and(|t| t.def.enabled && t.supervisor.is_none()) {
                self.set_state(id, state(yaac_protocol::attach::ConnectionStatus::Connecting, None));
                self.start_target(id);
            }
        }
        let dir = t.dir.clone();
        self.pool.submit(move || {
            let _ = std::fs::remove_dir_all(dir);
            None
        });
        let progress = HubTransferProgress { op_id: t.op, target_id: x, phase: phases::FAILED.into(), done: None, total: None, message: Some(reason) };
        self.transfer_notify_all(&Message::notification(attach::names::TRANSFER_PROGRESS, to_value(&progress)));
    }

    /// A connection closed with transfer requests unanswered.
    pub(super) fn transfer_closed(&mut self, id: &TargetId, asks: &[Ask]) {
        for ask in asks {
            if let Ask::CandidateProbe { serial } = ask {
                self.candidates_answered(*serial, Some((id, Err("The connection to it dropped.".into()))), None);
            }
        }
        let Some(t) = self.transfer.as_ref().filter(|t| &t.target == id) else { return };
        if t.arming {
            // `hubHost/start` may have armed it: the new hub starts now that the connection
            // closed. Never two hubs: this one records the move and stops.
            crate::core::log(format!("transfer {}: the connection closed while arming; the new hub may be starting", t.op.as_str()));
            return self.transfer_tombstone();
        }
        let label = t.label.clone();
        self.transfer_fail(format!("The connection to {label} dropped; nothing was moved."));
    }

    fn transfer_announce(&mut self, force: bool) {
        let Some(t) = self.transfer.as_mut() else { return };
        let now = Instant::now();
        if !force && t.last_progress.is_some_and(|at| now.duration_since(at) < PROGRESS_EVERY) {
            return;
        }
        t.last_progress = Some(now);
        let mut done = t.done;
        if let Some(put) = &t.put {
            done = Some(t.sent + put.offset);
        }
        let progress = HubTransferProgress { op_id: t.op.clone(), target_id: t.target.clone(), phase: t.phase.into(), done, total: t.total, message: None };
        self.transfer_notify_all(&Message::notification(attach::names::TRANSFER_PROGRESS, to_value(&progress)));
    }

    /// To every client that said hello, whatever its build: each must learn the new address.
    fn transfer_notify_all(&self, message: &Message) {
        for c in self.clients.values().filter(|c| c.hello != Hello::None) {
            let _ = c.tx.send(message.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_protocol::HubHostTailscale;

    fn probe() -> HubHostProbeResult {
        HubHostProbeResult {
            os: "linux".into(),
            arch: "x86_64".into(),
            tailscale: HubHostTailscale { running: true, login: Some("me@x".into()), ips: vec!["fd7a::1".into(), "100.64.0.7".into()] },
            port_free: true,
            hub_running: false,
            wsl: None,
            hub_build: None,
            last_transfer: None,
        }
    }

    fn me() -> ThisNode {
        ThisNode { addresses: vec!["100.91.112.95".parse().unwrap()], login: "me@x".into() }
    }

    const BUILD: &str = "0.1.0+1059";

    #[test]
    fn eligibility_says_why_not() {
        let me = me();
        let me = Some(&me);
        assert_eq!(judge("box", &probe(), me, BUILD), Ok(()));
        assert_eq!(address(&probe(), me).as_deref(), Some("100.64.0.7:47380"), "IPv4 first");
        type Change = Box<dyn Fn(&mut HubHostProbeResult)>;
        let cases: Vec<(Change, &str)> = vec![
            (Box::new(|p| p.os = "darwin".into()), "The hub runs on Linux or Windows; box is darwin."),
            (Box::new(|p| p.arch = "riscv64".into()), "There is no YAAC hub for riscv64 yet."),
            (Box::new(|p| p.port_free = false), "Port 47380 is in use on box."),
            (Box::new(|p| p.hub_running = true), "A YAAC hub already runs on box."),
        ];
        for (change, why) in cases {
            let mut p = probe();
            change(&mut p);
            assert_eq!(judge("box", &p, me, BUILD).unwrap_err(), why);
        }
    }

    #[test]
    fn tailscale_only_decides_what_phones_can_reach() {
        let mine = me();
        let me = Some(&mine);
        assert_eq!(warning("box", &probe(), me), None, "same login: nothing to say");
        let mut p = probe();
        p.tailscale.login = Some("Me@X".into());
        assert_eq!(warning("box", &p, me), None, "logins compare without case");
        // Another login (the owner's WSL on lelemm@github, Windows on another tailnet): eligible.
        p.tailscale.login = Some("lelemm@github".into());
        assert_eq!(judge("box", &p, me, BUILD), Ok(()));
        assert_eq!(warning("box", &p, me).unwrap(), "The hub will accept only devices signed in to Tailscale as lelemm@github; your phone must use that account.");
        // This computer without Tailscale: the login is unknown, so say whose it will be.
        assert!(warning("box", &probe(), None).unwrap().contains("as me@x"));
        // No Tailscale there: eligible; the port is not checked; phones wait.
        for off in [HubHostTailscale { running: false, login: None, ips: vec![] }, HubHostTailscale { running: true, login: Some("me@x".into()), ips: vec![] }] {
            let mut p = probe();
            p.tailscale = off;
            p.port_free = false;
            assert_eq!(judge("box", &p, me, BUILD), Ok(()));
            assert_eq!(address(&p, me), None);
            assert_eq!(warning("box", &p, me).unwrap(), "The phone can't reach the hub there until Tailscale runs on box.");
        }
        // This computer's own Tailscale seen from WSL (mirrored networking): no node of its own.
        let mut shared = probe();
        shared.tailscale.ips = vec!["100.91.112.95".into()];
        shared.port_free = false;
        assert_eq!(judge("Ubuntu", &shared, me, BUILD), Ok(()), "Windows' port is not the distro's");
        assert_eq!(address(&shared, me), None);
        assert!(warning("Ubuntu", &shared, me).unwrap().starts_with("The phone can't reach"));
    }

    #[test]
    fn windows_takes_the_hub_when_this_build_is_installed() {
        let mut p = probe();
        p.os = WINDOWS.into();
        p.arch = "aarch64".into();
        assert_eq!(judge("PC", &p, None, BUILD).unwrap_err(), "Install YAAC on PC first.");
        p.hub_build = Some("0.1.0+1058".into());
        assert_eq!(judge("PC", &p, None, BUILD).unwrap_err(), "YAAC on PC is build 0.1.0+1058 and this hub is 0.1.0+1059: install the same build there first.");
        p.hub_build = Some(BUILD.into());
        assert_eq!(judge("PC", &p, None, BUILD), Ok(()), "no Linux hub binary needed, any arch");
        p.hub_running = true;
        assert!(judge("PC", &p, None, BUILD).unwrap_err().contains("already runs"));
    }

    #[test]
    fn into_wsl_the_hub_needs_systemd_there() {
        let mut p = probe();
        assert_eq!(judge_wsl("Ubuntu", &p).unwrap_err(), "Update YAAC in Ubuntu first.", "not a WSL yaacd");
        p.wsl = Some(yaac_protocol::HubHostWsl { distro: "Ubuntu".into(), service: None });
        assert_eq!(judge_wsl("Ubuntu", &p).unwrap_err(), "Update YAAC in Ubuntu first.", "a yaacd from before P416");
        let service = yaac_protocol::HubHostWslService {
            systemd: false,
            user: "me".into(),
            uid: 1000,
            data_dir: "/home/me/.local/share/yaac".into(),
            environment: vec!["HOME=/home/me".into()],
        };
        p.wsl.as_mut().unwrap().service = Some(service);
        assert!(judge_wsl("Ubuntu", &p).unwrap_err().contains("systemd=true under [boot] in /etc/wsl.conf"));
        p.wsl.as_mut().unwrap().service.as_mut().unwrap().systemd = true;
        let (data, unit) = judge_wsl("Ubuntu", &p).unwrap();
        assert_eq!(data, "/home/me/.local/share/yaac");
        assert!(unit.contains("User=me\n"), "{unit}");
        p.wsl.as_mut().unwrap().service.as_mut().unwrap().user = "bad name".into();
        assert!(judge_wsl("Ubuntu", &p).is_err(), "a unit that cannot be written fails before anything moves");
    }

    #[test]
    fn the_pcs_window_follows_the_target_it_reached() {
        let ssh: TargetTransport = serde_json::from_value(json!({ "kind": "ssh", "host": "box" })).unwrap();
        let wsl: TargetTransport = serde_json::from_value(json!({ "kind": "wsl", "distro": "Ubuntu" })).unwrap();
        assert_eq!(via(&ssh), Some(ssh.clone()));
        assert_eq!(via(&wsl), Some(wsl.clone()));
        let windows = TargetTransport::WindowsInterop(yaac_protocol::attach::WindowsInteropTransport { exe: "/mnt/c/y.exe".into() });
        assert!(matches!(via(&windows), Some(TargetTransport::Local(_))), "back on the PC: the local socket");
    }

    #[test]
    fn only_reads_pass_during_the_hand_over() {
        assert!(read_only(attach::names::TARGETS_LIST));
        for m in [attach::names::REQUEST, attach::names::SHUTDOWN, crate::protocol::names::DRAFTS_PUT, attach::names::TRANSFER_START, attach::names::UPDATE_REQUEST] {
            assert!(!read_only(m), "{m}");
        }
    }
}
