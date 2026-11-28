//! `yaac wsl-hub --distro <d> [--user <u>] [--after-transfer <op>]`: the Windows-side keeper of
//! a hub that moved into one of this PC's WSL distros (docs/15-hub-transfer.md#into-this-pcs-wsl,
//! P414).
//!
//! WSL stops a distro that nothing runs in, and `yaacd` there neither starts the moved hub nor
//! installs a start-at-login entry for it (`hubHost/start {startHub: false}`). The keeper
//! does both from Windows:
//!
//! - It is this PC's start-at-login entry while the hub lives in WSL (it replaced `yaac hub`).
//! - It runs `wsl.exe -d <d> --exec sh -c '…exec <data>/bin/yaac hub'`, with no console
//!   window, and lives exactly as long as that hub. When the hub exits, the keeper exits: it
//!   restarts nothing by itself (invariant 10).
//! - Right after the hand-over (`--after-transfer <op>`) it first waits for this PC's old hub
//!   to exit, and the script in the distro waits for `yaacd` to have placed the moved files
//!   (`<data>/transfer-<op>.result` says `placed`).
//! - It refuses to start a hub unless this PC's tombstone says the hub lives in that distro,
//!   so a keeper left over after **Start the hub here again** never runs a second hub.
//! - `yaac quit` (and **Start the hub here again**) signal its quit event: it runs `yaac quit`
//!   in the distro (`hub/shutdown`, stores checkpointed), waits for the hub to exit, and exits.
//!
//! Only one keeper runs per hub profile (a named event, as the window's single instance).

use std::process::ExitCode;

use yaac_hub::HubPaths;
use yaac_hub::autostart::Autostart;
use yaac_hub::transfer::{WslKeeper, keeper_args, parse_keeper_args};

/// `yaac wsl-hub …` (`args`: after `wsl-hub`).
pub fn run(args: &[String]) -> ExitCode {
    let keeper = match parse_keeper_args(args) {
        Ok(k) => k,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    match keep(&keeper) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("yaac wsl-hub: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
/// The keeper's single-instance event for this hub profile (`Local\<pipe name>-wsl-hub`).
pub(crate) fn instance_name(paths: &HubPaths) -> String {
    let pipe = paths.socket();
    let tail = pipe.file_name().map_or_else(|| "yaac-hub".into(), |n| n.to_string_lossy().into_owned());
    format!(r"Local\{}-wsl-hub", tail.replace('\\', "-"))
}

/// The keeper the tombstone describes: its distro, and the user of this PC's WSL target for
/// it (from this PC's own `hub.json`, as it was at the hand-over). `None`: the hub did not move
/// into WSL.
pub(crate) fn from_tombstone(paths: &HubPaths) -> Option<WslKeeper> {
    let distro = yaac_hub::paths::moved_tombstone(paths).ok().flatten()?.wsl_distro?;
    let user = yaac_hub::settings::Settings::load(&paths.settings_file()).ok().flatten().and_then(|s| {
        s.targets.iter().find_map(|t| match &t.transport {
            yaac_protocol::attach::TargetTransport::Wsl(w) if w.distro.eq_ignore_ascii_case(&distro) => Some(w.user.clone()),
            _ => None,
        })
    });
    Some(WslKeeper { distro, user: user.flatten(), op: None })
}

/// The start-at-login entry that runs the keeper (no transfer to wait for).
pub(crate) fn autostart_entry(keeper: &WslKeeper) -> Result<Autostart, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut entry = Autostart::from_env(exe).map_err(|e| e.to_string())?;
    entry.args = keeper_args(&WslKeeper { op: None, ..keeper.clone() });
    Ok(entry)
}

/// Starts the keeper detached, with this process's hub profile.
pub(crate) fn start(keeper: &WslKeeper) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let paths = HubPaths::from_env().map_err(|e| e.to_string())?;
    crate::attach::spawn_detached(&exe, &keeper_args(keeper), &paths).map_err(|e| format!("cannot start {}: {e}", exe.display()))
}

/// The hub's own start-at-login entry again (`yaac hub`), after **Start the hub here again**
/// replaced the keeper.
pub(crate) fn restore_hub_autostart() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let entry = Autostart::from_env(exe).map_err(|e| e.to_string())?;
    yaac_hub::autostart::install_autostart(&entry).map_err(|e| e.to_string())
}

/// Whether a keeper runs for this process's hub profile (never outside Windows).
pub(crate) fn running() -> bool {
    #[cfg(windows)]
    if let Ok(paths) = HubPaths::from_env() {
        return yaac_winsys::named_event_exists(&instance_name(&paths));
    }
    false
}

/// Asks a running keeper to stop its hub (`yaac quit` in the distro) and waits until it has
/// exited. `Ok(false)`: no keeper runs. Blocks for up to about half a minute: never on the UI
/// thread.
#[cfg(windows)]
pub(crate) fn stop(paths: &HubPaths) -> Result<bool, String> {
    use std::time::{Duration, Instant};
    let name = instance_name(paths);
    match yaac_winsys::signal_named_event(&format!("{name}-quit")) {
        Ok(false) => return Ok(false),
        Ok(true) => {}
        Err(e) => return Err(format!("cannot ask the WSL hub's keeper to stop: {e}")),
    }
    let deadline = Instant::now() + QUIT_WAIT + Duration::from_secs(10);
    while yaac_winsys::named_event_exists(&name) {
        if Instant::now() >= deadline {
            return Err("the hub in WSL did not stop".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(true)
}

/// No keeper outside Windows.
#[cfg(not(windows))]
pub(crate) fn stop(paths: &HubPaths) -> Result<bool, String> {
    let _ = paths;
    Ok(false)
}

/// How long the hub in the distro may take to stop after `yaac quit` there.
#[cfg(windows)]
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

#[cfg(windows)]
fn keep(keeper: &WslKeeper) -> Result<ExitCode, String> {
    use std::os::windows::process::CommandExt as _;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let paths = HubPaths::from_env().map_err(|e| e.to_string())?;
    let name = instance_name(&paths);
    if yaac_winsys::named_event_exists(&name) {
        eprintln!("yaac wsl-hub: the hub in WSL is already kept running");
        return Ok(ExitCode::SUCCESS);
    }
    let _instance = yaac_winsys::listen_named_event(&name).map_err(|e| e.to_string())?;
    let quit = yaac_winsys::listen_named_event(&format!("{name}-quit")).map_err(|e| e.to_string())?;
    if from_tombstone(&paths).is_none_or(|k| !k.distro.eq_ignore_ascii_case(&keeper.distro)) {
        // Start the hub here again took the hub back (or it never moved there): never two hubs.
        eprintln!("yaac wsl-hub: the hub does not live in {} (no move recorded on this PC)", keeper.distro);
        return Ok(ExitCode::SUCCESS);
    }
    if keeper.op.is_some() {
        // Right after the hand-over: this PC's old hub exits once every client heard where the
        // hub went; `yaacd` places the files when its connection closes.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match yaac_hub::HubLock::acquire(&paths.lock_file()) {
                Ok(lock) => break drop(lock),
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
                Err(e) => return Err(format!("this PC's hub did not exit: {e}")),
            }
        }
    }
    let wsl = wsl_exe();
    let log = || {
        std::fs::OpenOptions::new().create(true).append(true).open(paths.data_dir.join("wsl-hub.log")).map_or_else(|_| Stdio::null(), Stdio::from)
    };
    let mut child = Command::new(&wsl)
        .args(yaac_hub::transfer::keeper_wsl_argv(keeper))
        .stdin(Stdio::null())
        .stdout(log())
        .stderr(log())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", wsl.display()))?;
    // `yaac quit`: stop the hub in the distro the ordinary way, then let it exit.
    let quitting = Arc::new(AtomicBool::new(false));
    {
        let (quitting, keeper, wsl) = (quitting.clone(), keeper.clone(), wsl.clone());
        std::thread::Builder::new()
            .name("yaac-wsl-hub-quit".into())
            .spawn(move || {
                if !quit.wait() {
                    return;
                }
                let stopped = Command::new(&wsl)
                    .args(yaac_hub::transfer::keeper_quit_argv(&keeper))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW)
                    .status();
                if let Err(e) = stopped {
                    eprintln!("yaac wsl-hub: cannot stop the hub in {}: {e}", keeper.distro);
                }
                quitting.store(true, Ordering::SeqCst);
            })
            .map_err(|e| e.to_string())?;
    }
    let mut quit_at: Option<Instant> = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(if status.success() { ExitCode::SUCCESS } else { ExitCode::FAILURE }),
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
        if quitting.load(Ordering::SeqCst) {
            let at = *quit_at.get_or_insert_with(Instant::now);
            if at.elapsed() >= QUIT_WAIT {
                // The hub did not exit after `yaac quit`: its wsl.exe goes, the distro idles.
                let _ = child.kill();
                let _ = child.wait();
                return Ok(ExitCode::FAILURE);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(windows)]
fn wsl_exe() -> std::path::PathBuf {
    std::env::var_os("YAAC_WSL_EXE").filter(|v| !v.is_empty()).map_or_else(|| "wsl.exe".into(), std::path::PathBuf::from)
}

#[cfg(not(windows))]
fn keep(keeper: &WslKeeper) -> Result<ExitCode, String> {
    let _ = keeper;
    Err("the WSL keeper runs on Windows only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keeper_follows_the_tombstone() {
        let dir = tempfile::tempdir().unwrap();
        let paths = HubPaths::under(dir.path());
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        assert_eq!(from_tombstone(&paths), None, "no move");
        let mut t = yaac_hub::paths::MovedTombstone { hub_id: "h".into(), address: "100.64.0.9:47380".into(), host_name: "ubuntu".into(), at_ms: 1, wsl_distro: None, via: None };
        yaac_hub::paths::write_moved_tombstone(&paths, &t).unwrap();
        assert_eq!(from_tombstone(&paths), None, "moved, but not into WSL");
        t.wsl_distro = Some("Ubuntu".into());
        yaac_hub::paths::write_moved_tombstone(&paths, &t).unwrap();
        assert_eq!(from_tombstone(&paths), Some(WslKeeper { distro: "Ubuntu".into(), user: None, op: None }));
        // The user comes from this PC's own WSL target for that distro.
        let mut s = yaac_hub::settings::Settings::first_run(false, true);
        s.targets.push(yaac_hub::settings::TargetDef {
            id: yaac_protocol::TargetId::new("w"),
            name: yaac_protocol::TargetName::new("w"),
            transport: serde_json::from_value(serde_json::json!({ "kind": "wsl", "distro": "ubuntu", "user": "dev" })).unwrap(),
            enabled: true,
            extra: Default::default(),
        });
        s.save(&paths.settings_file()).unwrap();
        assert_eq!(from_tombstone(&paths).unwrap().user.as_deref(), Some("dev"));
    }

    #[test]
    fn the_login_entry_runs_the_keeper_without_the_transfer() {
        let keeper = WslKeeper { distro: "Ubuntu".into(), user: None, op: Some(yaac_protocol::OpId::new("op-1")) };
        let entry = autostart_entry(&keeper).unwrap();
        assert_eq!(entry.args, ["wsl-hub", "--distro", "Ubuntu"]);
    }

    #[test]
    fn one_keeper_per_hub_profile() {
        let a = instance_name(&HubPaths::under(std::path::Path::new("/tmp/a")));
        assert!(a.starts_with(r"Local\") && a.ends_with("-wsl-hub"), "{a}");
    }
}
