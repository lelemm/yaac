//! Updating YAAC for Windows while this PC's hub runs in one of its WSL distros (P423,
//! docs/install.md#updating-with-the-hub-in-wsl).
//!
//! The hub there reaches the PC's Windows target as `yaac.exe local-daemon` through interop
//! (P416), a Windows process of the very file the installer replaces. So:
//!
//! - **Before the installer** ("Restart to update", or the pending update at start): the hub's
//!   system service is stopped as root through `wsl.exe` ([`stop_for_update`]), and the window
//!   waits until no `yaac.exe local-daemon` of this install runs. If either fails the installer
//!   is not run and why is shown: never half an update. `yaacd` keeps running everywhere, and
//!   so do its turns (invariant 8). A marker, [`MARKER`], records the stop.
//! - **After it** (the new window): a hub in WSL of an older build, or one stopped for an update,
//!   is brought to this window's build ([`after_update`], [`refresh`]): the Linux hub of exactly
//!   this build is downloaded from its release (size and SHA-256 checked), copied into the
//!   distro as `bin/yaac.new`, checked and tried there, renamed over `bin/yaac`, and the service
//!   is restarted as root. One attempt; a failure is shown with **Retry** (invariant 10).
//!
//! Every `wsl.exe` here is a short session of its own while the window has no attachment to
//! the hub (the exception of invariant 1 the owner confirmed for P416). Tests inject
//! [`WslOps`]; nothing here runs the real `wsl.exe` or `systemctl` in a test.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use yaac_hub::HubPaths;
use yaac_hub::transfer::{HubAsset, wsl_place_hub_argv, wsl_service_restart_argv, wsl_service_stop_argv};
use yaac_protocol::attach::TargetTransport;

/// How long the window waits, after the service stopped, for the last `yaac.exe local-daemon`
/// of this install to exit (it ends [`yaac_hub::interop::LOCAL_DAEMON_GRACE`] after its input
/// closed).
pub(crate) const DAEMONS_WAIT: Duration = Duration::from_secs(15);

/// How long a `systemctl stop` or `restart` through `wsl.exe` may take (a stopped distro boots
/// first).
const SERVICE_TIMEOUT: Duration = Duration::from_secs(90);

/// How long copying the new hub into the distro may take.
const PLACE_TIMEOUT: Duration = Duration::from_secs(180);

/// In this PC's data directory while the hub in WSL is stopped for an update: which hub, and
/// the build that stopped it.
pub(crate) const MARKER: &str = "wsl-hub-update.json";

/// Where the Linux hub is downloaded to, in this PC's data directory (not the updates folder,
/// which the updater empties).
const DOWNLOADS: &str = "wsl-hub-update";

/// This PC's hub in one of its WSL distros, as the window's bridge reaches it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WslHub {
    pub distro: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

impl WslHub {
    /// The hub a move's `via` names, when it is a WSL distro.
    pub(crate) fn of(via: &TargetTransport) -> Option<Self> {
        match via {
            TargetTransport::Wsl(w) => Some(Self { distro: w.distro.clone(), user: w.user.clone() }),
            _ => None,
        }
    }

    /// This PC's hub, when it moved into one of its WSL distros (its tombstone, including one
    /// from before `via`).
    pub(crate) fn of_this_pc(paths: &HubPaths) -> Option<Self> {
        let t = yaac_hub::paths::moved_tombstone(paths).ok().flatten()?;
        let via = t.via.or_else(|| crate::wsl_service::via_from_old_tombstone(paths))?;
        Self::of(&via)
    }

    /// `Ubuntu (WSL)`.
    pub(crate) fn label(&self) -> String {
        format!("{} (WSL)", self.distro)
    }

    fn same(&self, other: &Self) -> bool {
        self.distro.eq_ignore_ascii_case(&other.distro) && self.user == other.user
    }
}

/// [`MARKER`]'s contents.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Marker {
    #[serde(flatten)]
    pub hub: WslHub,
    /// The build of the window that stopped it.
    pub from_build: String,
}

/// The stop recorded for `hub`, if there is one.
pub(crate) fn read_marker(data_dir: &Path, hub: &WslHub) -> Option<Marker> {
    let bytes = std::fs::read(data_dir.join(MARKER)).ok()?;
    serde_json::from_slice::<Marker>(&bytes).ok().filter(|m| m.hub.same(hub))
}

fn write_marker(data_dir: &Path, marker: &Marker) -> Result<(), String> {
    let path = data_dir.join(MARKER);
    let tmp = data_dir.join(format!("{MARKER}.tmp"));
    std::fs::create_dir_all(data_dir).map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
    let bytes = serde_json::to_vec_pretty(marker).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn remove_marker(data_dir: &Path) {
    let _ = std::fs::remove_file(data_dir.join(MARKER));
}

/// What the steps need from the system: `wsl.exe`, the running `local-daemon`s, the release.
pub(crate) trait WslOps: Send + Sync {
    /// Runs `wsl.exe <argv>`, with standard input from `input`, for at most `timeout`. An
    /// error is what it said.
    fn run(&self, argv: &[String], input: Option<&Path>, timeout: Duration) -> Result<(), String>;
    /// Whether a `yaac.exe local-daemon` of this install still runs.
    fn daemons_running(&self) -> bool;
    /// Whether this window can take a hub from the releases (a release build on Windows).
    fn can_fetch(&self) -> bool;
    /// Downloads the verified Linux hub of exactly `build` for this PC into `dir`.
    fn fetch_hub(&self, build: &str, dir: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<(PathBuf, HubAsset), String>;
}

/// The real ones.
pub(crate) struct SystemWsl {
    log: PathBuf,
    /// [`yaac_hub::interop::local_daemon_event`] of this install.
    event: Option<String>,
    releases: Option<crate::updater::Config>,
}

impl SystemWsl {
    pub(crate) fn new(paths: &HubPaths, releases: Option<crate::updater::Config>) -> Self {
        let event = std::env::current_exe().ok().map(|exe| yaac_hub::interop::local_daemon_event(&exe));
        Self { log: crate::wsl_service::log_path(paths), event, releases }
    }
}

impl WslOps for SystemWsl {
    fn run(&self, argv: &[String], input: Option<&Path>, timeout: Duration) -> Result<(), String> {
        crate::wsl_service::run_wsl_with(argv, &self.log, input, Some(timeout))
    }

    fn daemons_running(&self) -> bool {
        #[cfg(windows)]
        return self.event.as_deref().is_some_and(yaac_winsys::named_event_exists);
        #[cfg(not(windows))]
        {
            let _ = &self.event;
            false
        }
    }

    fn can_fetch(&self) -> bool {
        self.releases.is_some()
    }

    fn fetch_hub(&self, build: &str, dir: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<(PathBuf, HubAsset), String> {
        let cfg = self.releases.as_ref().ok_or("this window does not update from releases")?;
        crate::updater::download_hub(cfg, build, std::env::consts::ARCH, dir, progress)
    }
}

/// Stops the hub's service in `hub`'s distro before YAAC for Windows (`from_build`) is
/// replaced, and waits up to `wait` for the last `yaac.exe local-daemon` of this install to
/// exit. On success [`MARKER`] records it. On failure the service is running as before (it is
/// started again when it was stopped) and the error says why the update was not installed.
pub(crate) fn stop_for_update(hub: &WslHub, from_build: &str, data_dir: &Path, ops: &dyn WslOps, wait: Duration) -> Result<(), String> {
    let label = hub.label();
    write_marker(data_dir, &Marker { hub: hub.clone(), from_build: from_build.to_owned() })?;
    if let Err(e) = ops.run(&wsl_service_stop_argv(&hub.distro), None, SERVICE_TIMEOUT) {
        // Stopped or not, the service is started again: the window attaches to it as before.
        let _ = ops.run(&wsl_service_restart_argv(&hub.distro), None, SERVICE_TIMEOUT);
        remove_marker(data_dir);
        return Err(format!("the hub in {label} could not be stopped: {e}"));
    }
    let deadline = Instant::now() + wait;
    while ops.daemons_running() {
        if Instant::now() >= deadline {
            let again = ops.run(&wsl_service_restart_argv(&hub.distro), None, SERVICE_TIMEOUT);
            remove_marker(data_dir);
            let restarted = if again.is_ok() { " The hub there was started again." } else { "" };
            return Err(format!("YAAC for Windows is still in use by the hub in {label} (yaac.exe local-daemon) {} seconds after it stopped.{restarted}", wait.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// [`stop_for_update`] for this PC's hub, when it is in WSL: what runs before the installer.
pub(crate) fn before_install(paths: &HubPaths, build: &str) -> Result<(), String> {
    let Some(hub) = WslHub::of_this_pc(paths) else { return Ok(()) };
    stop_for_update(&hub, build, &paths.data_dir, &SystemWsl::new(paths, None), DAEMONS_WAIT)
}

/// What the window knows of the hub in WSL.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Seen<'a> {
    /// [`MARKER`]: it was stopped for an update by `from_build`.
    Stopped { from_build: &'a str },
    /// It answered `hub/hello` as `hub_build`.
    Answered { hub_build: &'a str },
}

/// What to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    /// Put the hub of `build` in place, then restart the service.
    Refresh { build: String },
    /// Only start the service again (the update did not go in, or there is no release to take
    /// the hub from).
    Start,
    /// Nothing; the note, if any, says why.
    Leave(Option<String>),
}

/// What a window of `window_build` does about `hub` after an update. Only an older hub is
/// replaced, and only with this window's own build from its release (`can_fetch`).
pub(crate) fn after_update(window_build: &str, can_fetch: bool, hub: &WslHub, seen: Seen<'_>) -> Next {
    use yaac_core::update::build_number;
    let ours = build_number(window_build);
    let refresh = Next::Refresh { build: window_build.to_owned() };
    match seen {
        Seen::Stopped { from_build } => match (ours, build_number(from_build)) {
            (Some(o), Some(f)) if f < o && can_fetch => refresh,
            _ => Next::Start,
        },
        Seen::Answered { hub_build } => match (ours, build_number(hub_build)) {
            (Some(o), Some(h)) if h < o && can_fetch => refresh,
            (Some(o), Some(h)) if h < o => Next::Leave(Some(format!("The hub in {} is YAAC {hub_build}; this window cannot take YAAC {window_build} from the releases.", hub.label()))),
            (None, _) if hub_build != window_build => Next::Leave(Some(format!(
                "This is a development build of YAAC ({window_build}): the hub in {} (YAAC {hub_build}) is updated only from releases.",
                hub.label()
            ))),
            _ => Next::Leave(None),
        },
    }
}

/// Brings the hub in `hub`'s distro to `build` (or, with `None`, only starts it again): the
/// download, the copy into the distro as the bridge's user, and the restart as root. On
/// success the marker goes. Blocking: run it on a worker.
pub(crate) fn refresh(hub: &WslHub, build: Option<&str>, data_dir: &Path, ops: &dyn WslOps, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    if let Some(build) = build {
        let dir = data_dir.join(DOWNLOADS);
        let (file, asset) = ops.fetch_hub(build, &dir, progress)?;
        let placed = ops.run(&wsl_place_hub_argv(&hub.distro, hub.user.as_deref(), &asset.sha256, asset.size), Some(&file), PLACE_TIMEOUT);
        let _ = std::fs::remove_dir_all(&dir);
        placed.map_err(|e| format!("the new hub could not be put in place: {e}"))?;
    }
    ops.run(&wsl_service_restart_argv(&hub.distro), None, SERVICE_TIMEOUT).map_err(|e| format!("its service did not start: {e}"))?;
    remove_marker(data_dir);
    Ok(())
}

/// A worker's news.
pub(crate) enum Msg {
    Progress(u64, u64),
    Done(Result<(), String>),
}

/// A refresh the window shows: running (with its download's progress) or failed.
pub(crate) struct Progress {
    pub hub: WslHub,
    /// `None`: only starting the service again.
    pub build: Option<String>,
    pub state: State,
}

pub(crate) enum State {
    Running { rx: mpsc::Receiver<Msg>, done: u64, total: u64 },
    Failed(String),
}

impl Progress {
    /// Starts [`refresh`] on a worker; `wake` is called with every message.
    pub(crate) fn start(hub: WslHub, build: Option<String>, data_dir: PathBuf, ops: std::sync::Arc<dyn WslOps>, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let (h, b) = (hub.clone(), build.clone());
        let started = std::thread::Builder::new().name("yaac-wsl-hub-update".into()).spawn(move || {
            let mut last = Instant::now() - Duration::from_secs(1);
            let result = refresh(&h, b.as_deref(), &data_dir, ops.as_ref(), &mut |done, total| {
                // A few repaints a second at most.
                if done == total || last.elapsed() >= Duration::from_millis(200) {
                    last = Instant::now();
                    let _ = tx.send(Msg::Progress(done, total));
                    wake();
                }
            });
            let _ = tx.send(Msg::Done(result));
            wake();
        });
        let state = match started {
            Ok(_) => State::Running { rx, done: 0, total: 0 },
            Err(e) => State::Failed(e.to_string()),
        };
        Self { hub, build, state }
    }

    /// Takes the worker's news. `Some` once it has finished.
    pub(crate) fn poll(&mut self) -> Option<Result<(), String>> {
        let State::Running { rx, done, total } = &mut self.state else { return None };
        loop {
            match rx.try_recv() {
                Ok(Msg::Progress(d, t)) => (*done, *total) = (d, t),
                Ok(Msg::Done(result)) => return Some(result),
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => return Some(Err("the update of the hub stopped".into())),
            }
        }
    }

    pub(crate) fn failed(&self) -> bool {
        matches!(self.state, State::Failed(_))
    }

    /// What the window says about it.
    pub(crate) fn line(&self) -> String {
        let label = self.hub.label();
        let to = self.build.as_deref().map(|b| yaac_core::update::build_number(b).map_or_else(|| b.to_owned(), |n| format!("build {n}")));
        match (&self.state, to) {
            (State::Running { done, total, .. }, Some(to)) => {
                let percent = done.saturating_mul(100).checked_div(*total).map(|p| format!(" {}%", p.min(100))).unwrap_or_default();
                format!("Updating the hub in {label} to {to}…{percent}")
            }
            (State::Running { .. }, None) => format!("Starting the hub in {label} again…"),
            (State::Failed(e), Some(to)) => format!("The hub in {label} could not be updated to {to}: {e}"),
            (State::Failed(e), None) => format!("The hub in {label} could not be started again: {e}"),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every `wsl.exe` it is asked to run; never runs one.
    #[derive(Default)]
    pub(crate) struct FakeWsl {
        pub ran: Mutex<Vec<(Vec<String>, Option<Vec<u8>>)>>,
        /// Per call, in order: the answer (`Ok` when the list runs out).
        pub answers: Mutex<Vec<Result<(), String>>>,
        /// How many more times `daemons_running` says yes.
        pub daemons: Mutex<u32>,
        pub no_releases: bool,
        pub fetched: Mutex<Vec<String>>,
        pub fetch_error: Option<String>,
    }

    impl WslOps for FakeWsl {
        fn run(&self, argv: &[String], input: Option<&Path>, _: Duration) -> Result<(), String> {
            self.ran.lock().unwrap().push((argv.to_vec(), input.map(|p| std::fs::read(p).unwrap())));
            let mut answers = self.answers.lock().unwrap();
            if answers.is_empty() { Ok(()) } else { answers.remove(0) }
        }
        fn daemons_running(&self) -> bool {
            let mut n = self.daemons.lock().unwrap();
            let yes = *n > 0;
            *n = n.saturating_sub(1);
            yes
        }
        fn can_fetch(&self) -> bool {
            !self.no_releases
        }
        fn fetch_hub(&self, build: &str, dir: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<(PathBuf, HubAsset), String> {
            self.fetched.lock().unwrap().push(build.to_owned());
            if let Some(e) = &self.fetch_error {
                return Err(e.clone());
            }
            std::fs::create_dir_all(dir).unwrap();
            let file = dir.join("yaac-hub-linux-x86_64");
            std::fs::write(&file, b"hub").unwrap();
            progress(3, 3);
            Ok((file, HubAsset { url: "https://x/hub".into(), sha256: "abc".into(), size: 3 }))
        }
    }

    impl FakeWsl {
        pub(crate) fn argvs(&self) -> Vec<Vec<String>> {
            self.ran.lock().unwrap().iter().map(|(a, _)| a.clone()).collect()
        }
    }

    fn ubuntu() -> WslHub {
        WslHub { distro: "Ubuntu".into(), user: Some("dev".into()) }
    }

    #[test]
    fn the_hub_in_wsl_is_stopped_first_and_the_daemons_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        let ops = FakeWsl { daemons: Mutex::new(2), ..FakeWsl::default() };
        stop_for_update(&ubuntu(), "0.1.0+1063", dir.path(), &ops, Duration::from_secs(5)).unwrap();
        assert_eq!(ops.argvs(), [wsl_service_stop_argv("Ubuntu")], "as root, only the stop");
        assert_eq!(*ops.daemons.lock().unwrap(), 0, "waited until none ran");
        let m = read_marker(dir.path(), &ubuntu()).unwrap();
        assert_eq!(m.from_build, "0.1.0+1063");
        assert!(read_marker(dir.path(), &WslHub { distro: "Debian".into(), user: None }).is_none(), "another hub's");
    }

    #[test]
    fn a_hub_that_cannot_be_stopped_blocks_the_update_and_keeps_running() {
        let dir = tempfile::tempdir().unwrap();
        let ops = FakeWsl { answers: Mutex::new(vec![Err("yaac-hub.service still runs".into())]), ..FakeWsl::default() };
        let e = stop_for_update(&ubuntu(), "0.1.0+1063", dir.path(), &ops, Duration::from_secs(5)).unwrap_err();
        assert_eq!(e, "the hub in Ubuntu (WSL) could not be stopped: yaac-hub.service still runs");
        assert_eq!(ops.argvs(), [wsl_service_stop_argv("Ubuntu"), wsl_service_restart_argv("Ubuntu")]);
        assert!(read_marker(dir.path(), &ubuntu()).is_none());
        // A local-daemon that does not go: started again, said, no update.
        let ops = FakeWsl { daemons: Mutex::new(u32::MAX), ..FakeWsl::default() };
        let e = stop_for_update(&ubuntu(), "0.1.0+1063", dir.path(), &ops, Duration::from_millis(150)).unwrap_err();
        assert!(e.contains("still in use") && e.contains("local-daemon") && e.contains("started again"), "{e}");
        assert_eq!(ops.argvs(), [wsl_service_stop_argv("Ubuntu"), wsl_service_restart_argv("Ubuntu")]);
        assert!(read_marker(dir.path(), &ubuntu()).is_none());
    }

    #[test]
    fn only_an_older_hub_is_replaced_and_only_from_a_release() {
        let hub = ubuntu();
        let refresh = Next::Refresh { build: "0.1.0+1064".into() };
        assert_eq!(after_update("0.1.0+1064", true, &hub, Seen::Answered { hub_build: "0.1.0+1063" }), refresh);
        assert_eq!(after_update("0.1.0+1064", true, &hub, Seen::Stopped { from_build: "0.1.0+1063" }), refresh);
        // The same build: nothing; the update did not go in: start it again.
        assert_eq!(after_update("0.1.0+1064", true, &hub, Seen::Answered { hub_build: "0.1.0+1064" }), Next::Leave(None));
        assert_eq!(after_update("0.1.0+1063", true, &hub, Seen::Stopped { from_build: "0.1.0+1063" }), Next::Start);
        // A newer hub is never replaced from here (the window is the one to update).
        assert_eq!(after_update("0.1.0+1063", true, &hub, Seen::Answered { hub_build: "0.1.0+1064" }), Next::Leave(None));
        // No releases to take it from.
        assert!(matches!(after_update("0.1.0+1064", false, &hub, Seen::Answered { hub_build: "0.1.0+1063" }), Next::Leave(Some(_))));
        assert_eq!(after_update("0.1.0+1064", false, &hub, Seen::Stopped { from_build: "0.1.0+1063" }), Next::Start);
        // A development build: skipped, and said.
        let Next::Leave(Some(note)) = after_update("0.1.0+dev.ab", true, &hub, Seen::Answered { hub_build: "0.1.0+1063" }) else { panic!() };
        assert!(note.contains("development build") && note.contains("Ubuntu (WSL)"), "{note}");
        assert_eq!(after_update("0.1.0+dev.ab", true, &hub, Seen::Stopped { from_build: "0.1.0+1063" }), Next::Start);
        assert_eq!(after_update("0.1.0+1064", true, &hub, Seen::Answered { hub_build: "0.1.0+dev.ab" }), Next::Leave(None));
    }

    #[test]
    fn the_refresh_places_the_window_builds_hub_then_restarts_the_service() {
        let dir = tempfile::tempdir().unwrap();
        let hub = ubuntu();
        write_marker(dir.path(), &Marker { hub: hub.clone(), from_build: "0.1.0+1063".into() }).unwrap();
        let ops = FakeWsl::default();
        let mut last = (0, 0);
        refresh(&hub, Some("0.1.0+1064"), dir.path(), &ops, &mut |d, t| last = (d, t)).unwrap();
        assert_eq!(*ops.fetched.lock().unwrap(), ["0.1.0+1064"]);
        assert_eq!(last, (3, 3));
        let ran = ops.ran.lock().unwrap().clone();
        assert_eq!(ran[0], (wsl_place_hub_argv("Ubuntu", Some("dev"), "abc", 3), Some(b"hub".to_vec())), "the verified file is the input, as the bridge's user");
        assert_eq!(ran[1], (wsl_service_restart_argv("Ubuntu"), None));
        assert_eq!(ran.len(), 2);
        assert!(read_marker(dir.path(), &hub).is_none() && !dir.path().join(DOWNLOADS).exists());

        // Each step's failure is said; nothing after it runs; the marker stays for Retry.
        write_marker(dir.path(), &Marker { hub: hub.clone(), from_build: "0.1.0+1063".into() }).unwrap();
        let ops = FakeWsl { fetch_error: Some("There is no published release of YAAC 0.1.0+1064.".into()), ..FakeWsl::default() };
        assert!(refresh(&hub, Some("0.1.0+1064"), dir.path(), &ops, &mut |_, _| {}).unwrap_err().contains("no published release"));
        assert!(ops.argvs().is_empty());
        let ops = FakeWsl { answers: Mutex::new(vec![Err("the hub copied into WSL does not match its SHA-256".into())]), ..FakeWsl::default() };
        let e = refresh(&hub, Some("0.1.0+1064"), dir.path(), &ops, &mut |_, _| {}).unwrap_err();
        assert_eq!(e, "the new hub could not be put in place: the hub copied into WSL does not match its SHA-256");
        assert_eq!(ops.argvs().len(), 1, "no restart");
        assert!(read_marker(dir.path(), &hub).is_some());
        // Only starting it again.
        let ops = FakeWsl::default();
        refresh(&hub, None, dir.path(), &ops, &mut |_, _| {}).unwrap();
        assert_eq!(ops.argvs(), [wsl_service_restart_argv("Ubuntu")]);
        assert!(ops.fetched.lock().unwrap().is_empty());
    }

    #[test]
    fn the_window_says_how_the_refresh_goes() {
        let ops: std::sync::Arc<dyn WslOps> = std::sync::Arc::new(FakeWsl { fetch_error: Some("offline".into()), ..FakeWsl::default() });
        let dir = tempfile::tempdir().unwrap();
        let mut p = Progress::start(ubuntu(), Some("0.1.0+1064".into()), dir.path().into(), ops, || {});
        assert_eq!(p.line(), "Updating the hub in Ubuntu (WSL) to build 1064…");
        let deadline = Instant::now() + Duration::from_secs(10);
        let result = loop {
            if let Some(r) = p.poll() { break r }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        };
        p.state = State::Failed(result.unwrap_err());
        assert!(p.failed());
        assert_eq!(p.line(), "The hub in Ubuntu (WSL) could not be updated to build 1064: offline");
        let p = Progress { hub: ubuntu(), build: None, state: State::Running { rx: mpsc::channel().1, done: 0, total: 0 } };
        assert_eq!(p.line(), "Starting the hub in Ubuntu (WSL) again…");
    }
}
