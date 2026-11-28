//! The window's side of updating YAAC for Windows while this PC's hub runs in WSL (P423,
//! [`crate::wsl_update`]): "Restart to update" stops the hub there first, and the new window
//! brings the hub there to its own build. Both renderers show the same lines (the empty
//! workspace while the window has no hub) and the same **Retry** ([`Action::RetryHub`]).

use std::sync::Arc;

use yaac_core::client::Client;
use yaac_protocol::ClientId;

use super::{DeskHubs, HubSshPending, YaacApp};
use crate::wsl_update::{self, Next, Progress, Seen, State, SystemWsl, WslHub, WslOps};

impl DeskHubs {
    /// This PC's hub, when it moved into one of its WSL distros (its tombstone, or the
    /// `hub/moved` the window followed).
    pub(crate) fn wsl_hub(&self) -> Option<WslHub> {
        self.moved_here.as_ref().and_then(|t| t.via.as_ref()).and_then(WslHub::of)
    }

    /// The stop for an update recorded for this PC's hub in WSL.
    pub(crate) fn stopped_for_update(&self) -> Option<wsl_update::Marker> {
        wsl_update::read_marker(&self.local.paths.data_dir, &self.wsl_hub()?)
    }

    fn wsl_ops(&self, releases: Option<crate::updater::Config>) -> Arc<dyn WslOps> {
        self.wsl_ops.clone().unwrap_or_else(|| Arc::new(SystemWsl::new(&self.local.paths, releases)))
    }
}

impl YaacApp {
    /// "Restart to update" with this PC's hub in WSL: the window lets go of the hub, the hub's
    /// service is stopped there on a worker, and only then does the updater run the installer
    /// (and the window quit). If it cannot be stopped the update stays ready, the notice says
    /// why, and the window attaches again ([`Self::wsl_hub_upkeep`]).
    pub(super) fn install_update_after_wsl_hub(&mut self) -> Result<(), String> {
        let Some(d) = &self.desk_hubs else { return Err("no hub".into()) };
        let Some(hub) = d.wsl_hub() else { return Err("no hub in WSL".into()) };
        if !matches!(self.updater.phase, crate::updater::Phase::Ready(_)) {
            return Err("no update is ready".into());
        }
        let (ops, data_dir, build) = (d.wsl_ops(None), d.local.paths.data_dir.clone(), d.wsl_build);
        let label = hub.label();
        self.detach_hub(format!("Stopping the hub in {label} to install the update…"));
        let stop = move || wsl_update::stop_for_update(&hub, build, &data_dir, ops.as_ref(), wsl_update::DAEMONS_WAIT).map_err(|e| format!("The update was not installed: {e}"));
        let started = self.updater.install_after(&self.ctx, label, stop);
        if started.is_err() {
            self.attach_selected_hub();
        }
        started
    }

    /// The window lets go of its hub (no bridge runs, so nothing starts the hub in WSL again
    /// while it is stopped or replaced); `line` says why until it attaches again.
    fn detach_hub(&mut self, line: String) {
        self.client.flush_drafts();
        self.keep_device_drafts(true);
        let mut c = Client::new(Box::new(yaac_core::link::DetachedLink), crate::BUILD, ClientId::new("window"));
        c.state.hub.last_error = Some(line);
        self.client = c;
        self.hub = None;
        self.ssh = Box::new(HubSshPending::default());
        self.browsers = Default::default();
        self.preview_ui = Default::default();
        self.browser_agent = Default::default();
    }

    /// Every frame: the updater's outcome after stopping the hub in WSL, the refresh's progress,
    /// and, once, a stop for an update that this window finds at its start.
    pub(super) fn wsl_hub_upkeep(&mut self) {
        if self.updater.take_quit() {
            self.shell.quit(&self.ctx);
            return;
        }
        if self.updater.take_reattach() {
            // The installer did not start. A hub that was stopped is started again first.
            let stopped = self.desk_hubs.as_ref().and_then(DeskHubs::stopped_for_update);
            match stopped {
                Some(m) => self.begin_wsl_update(Seen::Stopped { from_build: &m.from_build }),
                None => self.attach_selected_hub(),
            }
            return;
        }
        let Some(d) = &mut self.desk_hubs else { return };
        if let Some(p) = &mut d.wsl_update {
            match p.poll() {
                None => {}
                Some(Ok(())) => {
                    d.wsl_update = None;
                    self.attach_selected_hub();
                    return;
                }
                Some(Err(e)) => p.state = State::Failed(e),
            }
            let line = p.line();
            if self.client.state.hub.last_error.as_deref() != Some(line.as_str()) {
                self.client.state.hub.last_error = Some(line);
            }
            return;
        }
        if !d.wsl_checked {
            d.wsl_checked = true;
            if let Some(m) = d.stopped_for_update() {
                d.wsl_tried = true;
                self.begin_wsl_update(Seen::Stopped { from_build: &m.from_build });
            }
        }
    }

    /// The bridged hub in this PC's WSL answered as `hub_build` (another build than this
    /// window's): an older one is brought to this build, once by itself (then **Retry**).
    pub(super) fn wsl_hub_outdated(&mut self, hub_build: &str) {
        let Some(d) = &mut self.desk_hubs else { return };
        if d.wsl_tried || d.wsl_update.is_some() || !d.on_moved_hub() || d.wsl_hub().is_none() {
            return;
        }
        d.wsl_tried = true;
        self.begin_wsl_update(Seen::Answered { hub_build });
    }

    fn begin_wsl_update(&mut self, seen: Seen<'_>) {
        let Some(d) = &mut self.desk_hubs else { return };
        let Some(hub) = d.wsl_hub() else { return };
        let ops = d.wsl_ops(self.updater.config().cloned());
        let build = match wsl_update::after_update(d.wsl_build, ops.can_fetch(), &hub, seen) {
            Next::Refresh { build } => Some(build),
            Next::Start => None,
            Next::Leave(note) => {
                d.wsl_note = note;
                if self.hub.is_none() {
                    self.attach_selected_hub();
                }
                return;
            }
        };
        let ctx = self.ctx.clone();
        let progress = Progress::start(hub, build, d.local.paths.data_dir.clone(), ops, move || ctx.request_repaint());
        let line = progress.line();
        d.wsl_update = Some(progress);
        self.detach_hub(line);
    }

    /// **Retry** after a failed refresh: the same steps once more, as the user asked.
    /// `false`: none failed.
    pub(super) fn retry_wsl_update(&mut self) -> bool {
        let Some(d) = &mut self.desk_hubs else { return false };
        let Some(p) = d.wsl_update.take_if(|p| p.failed()) else { return false };
        let ops = d.wsl_ops(self.updater.config().cloned());
        let ctx = self.ctx.clone();
        let progress = Progress::start(p.hub, p.build, d.local.paths.data_dir.clone(), ops, move || ctx.request_repaint());
        self.client.state.hub.last_error = Some(progress.line());
        d.wsl_update = Some(progress);
        true
    }

    /// The refresh's line while it runs or after it failed.
    pub(crate) fn wsl_update_line(&self) -> Option<String> {
        self.desk_hubs.as_ref()?.wsl_update.as_ref().map(Progress::line)
    }

    /// The refresh failed: **Retry** is offered.
    pub(crate) fn wsl_update_failed(&self) -> bool {
        self.desk_hubs.as_ref().and_then(|d| d.wsl_update.as_ref()).is_some_and(Progress::failed)
    }

    /// Why the hub in WSL is not brought to this build (a development build), for the
    /// mismatch line.
    #[cfg(test)]
    fn wsl_note(&self) -> Option<&str> {
        self.desk_hubs.as_ref()?.wsl_note.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;
    use yaac_hub::transfer::{wsl_place_hub_argv, wsl_service_restart_argv, wsl_service_stop_argv};

    use super::*;
    use crate::wsl_update::tests::FakeWsl;

    /// This window's build in these tests (the test binary's own is a development build).
    const OURS: &str = "0.1.0+1064";

    fn moved_into_wsl(dir: &std::path::Path) -> crate::attach::AttachConfig {
        let config = super::super::hub_views::tests::local(dir);
        std::fs::create_dir_all(&config.paths.data_dir).unwrap();
        let via = serde_json::from_value(json!({ "kind": "wsl", "distro": "Ubuntu" })).unwrap();
        let tombstone = yaac_hub::paths::MovedTombstone { hub_id: "h-pc".into(), address: String::new(), host_name: "ubuntu".into(), at_ms: 1, wsl_distro: Some("Ubuntu".into()), via: Some(via) };
        yaac_hub::paths::write_moved_tombstone(&config.paths, &tombstone).unwrap();
        config
    }

    /// A window on that hub, with `ops` instead of the system's.
    fn window(config: &crate::attach::AttachConfig, ops: &Arc<FakeWsl>) -> YaacApp {
        let mut app = super::super::hub_views::tests::window(config.clone());
        let d = app.desk_hubs.as_mut().unwrap();
        d.wsl_ops = Some(ops.clone());
        d.wsl_build = OURS;
        app
    }

    fn until(app: &mut YaacApp, what: &str, ready: impl Fn(&YaacApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready(app) {
            assert!(Instant::now() < deadline, "{what}");
            app.updater.poll();
            app.hub_upkeep();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn ubuntu() -> WslHub {
        WslHub { distro: "Ubuntu".into(), user: None }
    }

    #[test]
    fn a_hub_stopped_for_an_update_is_brought_to_this_build_before_attaching() {
        let dir = tempfile::tempdir().unwrap();
        let config = moved_into_wsl(dir.path());
        // The window of the earlier build stopped it.
        let earlier = FakeWsl::default();
        wsl_update::stop_for_update(&ubuntu(), "0.1.0+1063", &config.paths.data_dir, &earlier, Duration::ZERO).unwrap();
        assert_eq!(earlier.argvs(), [wsl_service_stop_argv("Ubuntu")]);

        let ops = Arc::new(FakeWsl::default());
        let mut app = window(&config, &ops);
        assert!(app.hub.is_none(), "no bridge while the hub there is replaced");
        app.hub_upkeep();
        let line = app.wsl_update_line().unwrap();
        assert_eq!(line, "Updating the hub in Ubuntu (WSL) to build 1064…");
        assert_eq!(app.client.state.hub.last_error.as_deref(), Some(line.as_str()), "shown where the window says why it has no hub");
        until(&mut app, "the refresh did not finish", |a| a.wsl_update_line().is_none());
        assert!(app.hub.is_some(), "attached again");
        assert_eq!(*ops.fetched.lock().unwrap(), [OURS], "this window's own build");
        assert_eq!(ops.argvs(), [wsl_place_hub_argv("Ubuntu", None, "abc", 3), wsl_service_restart_argv("Ubuntu")]);
        assert!(app.desk_hubs.as_ref().unwrap().stopped_for_update().is_none(), "done once");
    }

    #[test]
    fn a_failed_refresh_is_said_and_retried_only_on_request() {
        let dir = tempfile::tempdir().unwrap();
        let config = moved_into_wsl(dir.path());
        let ops = Arc::new(FakeWsl { answers: std::sync::Mutex::new(vec![Err("systemd is not running".into())]), ..FakeWsl::default() });
        let mut app = window(&config, &ops);
        // The bridged hub there answered as an older build.
        app.wsl_hub_outdated("0.1.0+1063");
        assert!(app.hub.is_none(), "let go of while it is replaced");
        until(&mut app, "the refresh did not fail", YaacApp::wsl_update_failed);
        let line = app.wsl_update_line().unwrap();
        assert_eq!(line, "The hub in Ubuntu (WSL) could not be updated to build 1064: the new hub could not be put in place: systemd is not running");
        let runs = ops.argvs().len();
        // Never again by itself.
        app.wsl_hub_outdated("0.1.0+1063");
        for _ in 0..20 {
            app.hub_upkeep();
        }
        assert_eq!(ops.argvs().len(), runs);
        // Retry: the same steps once more, as asked.
        app.apply(vec![super::super::Action::RetryHub]);
        until(&mut app, "the retry did not finish", |a| a.wsl_update_line().is_none());
        assert_eq!(ops.argvs().len(), runs + 2);
        assert!(app.hub.is_some());
    }

    #[test]
    fn a_newer_or_development_build_leaves_the_hub_in_wsl_alone() {
        let dir = tempfile::tempdir().unwrap();
        let config = moved_into_wsl(dir.path());
        let ops = Arc::new(FakeWsl::default());
        let mut app = window(&config, &ops);
        app.wsl_hub_outdated("0.1.0+1065");
        assert!(app.wsl_update_line().is_none() && app.hub.is_some());
        let mut app = window(&config, &ops);
        app.desk_hubs.as_mut().unwrap().wsl_build = "0.1.0+dev.ab";
        app.wsl_hub_outdated("0.1.0+1063");
        assert!(app.wsl_update_line().is_none());
        assert!(app.wsl_note().unwrap().contains("development build"));
        assert!(ops.argvs().is_empty() && ops.fetched.lock().unwrap().is_empty(), "nothing is run or downloaded");
    }

    /// A verified update in the updates folder, ready for "Restart to update".
    fn ready(app: &mut YaacApp, dir: &std::path::Path) -> std::path::PathBuf {
        let updates = dir.join("updates");
        std::fs::create_dir_all(&updates).unwrap();
        let setup = updates.join("YAAC-Setup.exe");
        std::fs::write(&setup, b"setup").unwrap();
        let cfg = crate::updater::Config { latest_url: "http://127.0.0.1:9/x".into(), dir: updates.clone(), https_only: false, timeout: Duration::from_secs(1) };
        app.updater = crate::updater::Updater::new(cfg, "0.1.0+1063", None);
        use sha2::Digest as _;
        let sha256 = sha2::Sha256::digest(b"setup").iter().map(|b| format!("{b:02x}")).collect();
        let pending = yaac_core::update::Pending { build: 1064, version: OURS.into(), path: setup, sha256, size: 5, attempted: false };
        std::fs::write(updates.join("pending.json"), serde_json::to_vec(&pending).unwrap()).unwrap();
        app.updater.phase = crate::updater::Phase::Ready(pending);
        updates
    }

    #[test]
    fn restart_to_update_stops_the_hub_in_wsl_first_and_never_installs_half() {
        let dir = tempfile::tempdir().unwrap();
        let config = moved_into_wsl(dir.path());
        let ops = Arc::new(FakeWsl { answers: std::sync::Mutex::new(vec![Err("yaac-hub.service still runs".into())]), ..FakeWsl::default() });
        let mut app = window(&config, &ops);
        assert!(app.install_update().unwrap_err().contains("no update is ready"));
        assert!(ops.argvs().is_empty() && app.hub.is_some(), "nothing done");

        // It cannot be stopped: started again, the update stays ready, the window attaches.
        let updates = ready(&mut app, dir.path());
        app.install_update().unwrap();
        assert!(app.hub.is_none(), "the window let go of the hub first");
        assert!(matches!(app.updater.phase, crate::updater::Phase::StoppingHub { .. }));
        until(&mut app, "the stop did not finish", |a| a.hub.is_some());
        assert!(app.updater.notice.take().unwrap().contains("The update was not installed: the hub in Ubuntu (WSL) could not be stopped"));
        assert!(matches!(app.updater.phase, crate::updater::Phase::Ready(_)));
        assert_eq!(ops.argvs(), [wsl_service_stop_argv("Ubuntu"), wsl_service_restart_argv("Ubuntu")]);
        let pending: yaac_core::update::Pending = serde_json::from_slice(&std::fs::read(updates.join("pending.json")).unwrap()).unwrap();
        assert!(!pending.attempted, "the installer never ran");

        // Stopped: the installer is started (here it is no executable, so it fails to start,
        // and the hub that was stopped is started again before the window attaches).
        ops.ran.lock().unwrap().clear();
        app.install_update().unwrap();
        until(&mut app, "the window did not attach again", |a| a.hub.is_some());
        let pending: yaac_core::update::Pending = serde_json::from_slice(&std::fs::read(updates.join("pending.json")).unwrap()).unwrap();
        assert!(pending.attempted, "the installer was started");
        if cfg!(unix) {
            assert_eq!(ops.argvs(), [wsl_service_stop_argv("Ubuntu"), wsl_service_restart_argv("Ubuntu")]);
            assert!(app.desk_hubs.as_ref().unwrap().stopped_for_update().is_none());
        }
    }
}
