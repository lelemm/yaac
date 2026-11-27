//! `hub/update/request` (frozen; [`crate::host_update`], docs/install.md#updating-from-the-phone):
//! a phone newer than this hub asks for its build to be installed on this computer.
//!
//! Accepted whatever the client's build, after any `hub/hello`, only from a client admitted
//! over Tailscale and only for a released build newer than this hub's. One request is one
//! attempt (invariant 10): the hub never tries again by itself, and a second request while one
//! runs is refused. The work runs on a thread of its own, never on the I/O thread; every
//! step is reported to every client as `hub/update/progress`, so the window on this computer
//! says what is about to happen before the installer closes it.

use yaac_protocol::OpId;
use yaac_protocol::attach::host_update::{CHECKING, DOWNLOADING, FAILED, INSTALLING, codes};
use yaac_protocol::attach::{HostUpdateParams, HostUpdateProgress, HostUpdateResult};

use super::{ClientKey, Core, CoreEvent, ErrorObject, Hello, Message, RpcId, Value, attach, build_number, hub_err, parse, to_value};
use crate::host_update::HostRelease;

/// Progress is reported at most this often while downloading (and at the end).
const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// The update under way. Cleared when it fails; kept once the installer is started, so no
/// other request is taken before the installer stops this hub.
#[derive(Debug)]
pub(super) struct Running {
    op: OpId,
    build: String,
    phase: &'static str,
    done: u64,
    total: u64,
    /// The request, answered once the release is found (or not).
    request: Option<(ClientKey, RpcId)>,
}

/// What the update thread reports.
#[derive(Debug)]
pub(crate) enum Event {
    Found { op: OpId, result: Result<(String, u64), String> },
    Progress { op: OpId, done: u64, total: u64 },
    Failed { op: OpId, message: String },
    Installing { op: OpId },
}

/// `<version>+<id>` with a numeric id and a version of digits, letters, `.` and `-` only: it
/// becomes part of a release tag in a URL.
pub(crate) fn is_release_build(build: &str) -> bool {
    let Some((version, id)) = build.split_once('+') else { return false };
    version.starts_with(|c: char| c.is_ascii_digit())
        && version.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
}

impl Core {
    pub(super) fn host_update_request(&mut self, client: ClientKey, id: &RpcId, params: Value) -> Option<Result<Value, ErrorObject>> {
        let c = self.clients.get(&client)?;
        if !c.remote {
            return Some(Err(hub_err(codes::NOT_ALLOWED, "Only the Android client, attached over Tailscale, asks the hub to update this computer. A window here updates itself.")));
        }
        if c.hello == Hello::None {
            return Some(Err(hub_err(super::ext::codes::HELLO_REQUIRED, "send hub/hello first")));
        }
        let p: HostUpdateParams = match parse(params) {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };
        let mine = &self.cfg.build;
        let Some(hub_number) = build_number(mine) else {
            return Some(Err(hub_err(codes::NOT_NEWER, format!("This computer runs a development build of YAAC ({mine}); it is not updated from releases."))));
        };
        let wanted = p.build.trim();
        if !is_release_build(wanted) {
            return Some(Err(hub_err(codes::NOT_NEWER, format!("{wanted} is not a YAAC release build."))));
        }
        if build_number(wanted).is_none_or(|n| n <= hub_number) {
            return Some(Err(hub_err(codes::NOT_NEWER, format!("This computer already runs YAAC {mine}; it is never moved to an older or the same build ({wanted})."))));
        }
        let Some(installer) = self.cfg.host_installer.clone() else {
            return Some(Err(hub_err(codes::UNSUPPORTED, "Updating YAAC from the phone is not supported on this host: the installer is for Windows.")));
        };
        if let Some(r) = &self.host_update {
            return Some(Err(hub_err(codes::IN_PROGRESS, format!("YAAC {} is already being installed on this computer ({}).", r.build, r.phase))));
        }
        let op = p.op_id;
        let build = wanted.to_owned();
        self.host_update = Some(Running { op: op.clone(), build: build.clone(), phase: CHECKING, done: 0, total: 0, request: Some((client, id.clone())) });
        self.host_update_announce();
        let tx = self.tx.clone();
        let send = move |e: Event| {
            let _ = tx.send(CoreEvent::HostUpdate(e));
        };
        let started = std::thread::Builder::new().name("yaac-hub-update".into()).spawn(move || run(installer.as_ref(), &op, &build, &send));
        if let Err(e) = started {
            self.host_update = None;
            return Some(Err(hub_err(codes::UNAVAILABLE, format!("Could not start the update: {e}"))));
        }
        None
    }

    pub(super) fn host_update_event(&mut self, event: Event) {
        let Some(r) = self.host_update.as_mut() else { return };
        match event {
            Event::Found { op, result } if op == r.op => {
                let request = r.request.take();
                match result {
                    Ok((build, size)) => {
                        (r.phase, r.done, r.total) = (DOWNLOADING, 0, size);
                        if let Some((client, id)) = request {
                            self.reply(client, id, Ok(to_value(&HostUpdateResult { build, size })));
                        }
                        self.host_update_announce();
                    }
                    Err(message) => {
                        self.host_update = None;
                        if let Some((client, id)) = request {
                            self.reply(client, id, Err(hub_err(codes::UNAVAILABLE, message)));
                        }
                    }
                }
            }
            Event::Progress { op, done, total } if op == r.op => {
                (r.done, r.total) = (done, total);
                self.host_update_announce();
            }
            Event::Failed { op, message } if op == r.op => {
                let build = r.build.clone();
                self.host_update = None;
                crate::core::log(format!("the update to {build} failed: {message}"));
                self.host_update_notify(&HostUpdateProgress { op_id: op, build, phase: FAILED.into(), done: 0, total: 0, message: Some(message) });
            }
            Event::Installing { op } if op == r.op => {
                r.phase = INSTALLING;
                self.host_update_announce();
            }
            _ => {}
        }
    }

    /// Tells every client how the update stands.
    fn host_update_announce(&self) {
        let Some(r) = &self.host_update else { return };
        let progress = HostUpdateProgress { op_id: r.op.clone(), build: r.build.clone(), phase: r.phase.into(), done: r.done, total: r.total, message: None };
        self.host_update_notify(&progress);
    }

    /// To every client that said hello, whatever its build: the phone that asked is of another
    /// build, and the window here must hear it before the installer closes it.
    fn host_update_notify(&self, progress: &HostUpdateProgress) {
        let message = Message::notification(attach::names::UPDATE_PROGRESS, to_value(progress));
        for c in self.clients.values().filter(|c| c.hello != Hello::None) {
            let _ = c.tx.send(message.clone());
        }
    }
}

/// The update thread: find, download and verify, start the installer. Each step once.
fn run(installer: &dyn crate::host_update::HostInstaller, op: &OpId, build: &str, send: &dyn Fn(Event)) {
    let release: HostRelease = match installer.find(build) {
        Ok(r) if r.build == build => r,
        Ok(r) => {
            let message = format!("GitHub answered with YAAC {} instead of {build}.", r.build);
            send(Event::Found { op: op.clone(), result: Err(message) });
            return;
        }
        Err(message) => {
            send(Event::Found { op: op.clone(), result: Err(message) });
            return;
        }
    };
    send(Event::Found { op: op.clone(), result: Ok((release.build.clone(), release.size)) });
    let mut last: Option<std::time::Instant> = None;
    let downloaded = installer.download(&release, &mut |done, total| {
        let now = std::time::Instant::now();
        if done >= total || last.is_none_or(|at| now.duration_since(at) >= PROGRESS_EVERY) {
            last = Some(now);
            send(Event::Progress { op: op.clone(), done, total });
        }
    });
    if let Err(message) = downloaded {
        send(Event::Failed { op: op.clone(), message });
        return;
    }
    // Said before the installer starts: it closes the window here within seconds.
    send(Event::Installing { op: op.clone() });
    if let Err(message) = installer.install(&release) {
        send(Event::Failed { op: op.clone(), message });
    }
}

#[cfg(test)]
mod tests {
    use super::is_release_build;

    #[test]
    fn only_release_builds_become_tags() {
        assert!(is_release_build("0.1.0+1060"));
        assert!(is_release_build("0.2.0-rc.1+1061"));
        for bad in ["0.1.0+dev.ab", "0.1.0", "+1060", "0.1.0+", "0.1.0/../x+1060", "v0.1.0+1060", "0.1.0+10 60", "0.1.0+1060+1"] {
            assert!(!is_release_build(bad), "{bad}");
        }
    }
}
