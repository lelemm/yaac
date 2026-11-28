//! `yaac wsl-hub install|remove`: the hub's systemd system service in one of this PC's WSL
//! distros (docs/15-hub-transfer.md#into-this-pcs-wsl-p416, P416).
//!
//! A hub that moved into WSL runs there as `yaac-hub.service`, which systemd starts whenever
//! the distro boots. Nothing on Windows keeps it running: WSL starts the distro on demand (the
//! window attaching through `wsl.exe … yaac hub-bridge` does) and stops it when no Windows
//! process uses it. `yaacd` in the distro has no root, so the unit is written from here through
//! `wsl.exe -u root`, which WSL allows the Windows user in their own distros without a password
//! or elevation:
//!
//! - **install** — started detached by the old hub just before its tombstone. It waits for that
//!   hub to exit, so no connection to the distro is left when it opens its own short one
//!   (invariant 1), and runs the install script there: it waits for `yaacd` to have placed the
//!   moved files, writes `/etc/systemd/system/yaac-hub.service`, and `systemctl enable --now`.
//!   Output goes to `<data>/wsl-hub.log` on Windows. Nothing is retried (invariant 10): if it
//!   fails, the window offers **Start the hub here again**.
//! - **remove** — **Start the hub here again**: `systemctl disable --now` and the unit goes, so
//!   the distro never starts a second hub.
//!
//! The keeper of P414 (`yaac wsl-hub --distro …` at login) is gone; an entry left from it does
//! nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use yaac_hub::HubPaths;
use yaac_hub::transfer::{WslHubCommand, WslService, parse_wsl_hub_args, wsl_install_args, wsl_service_install_argv, wsl_service_remove_argv};

/// How long the install waits for this PC's old hub to exit.
const OLD_HUB_WAIT: Duration = Duration::from_secs(60);

/// `yaac wsl-hub …` (`args`: after `wsl-hub`).
pub fn run(args: &[String]) -> ExitCode {
    let command = match parse_wsl_hub_args(args) {
        Ok(c) => c,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        WslHubCommand::Legacy => {
            eprintln!("yaac wsl-hub: the hub in WSL runs as a system service there now; nothing to keep running");
            return ExitCode::SUCCESS;
        }
        WslHubCommand::Install { distro, data_dir, unit_file, op } => HubPaths::from_env().map_err(|e| e.to_string()).and_then(|paths| {
            let log = log_file(&paths);
            install_with(&paths, &distro, &data_dir, &unit_file, &op, OLD_HUB_WAIT, |argv| run_wsl(argv, &log))
        }),
        WslHubCommand::Remove { distro } => remove(&distro),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yaac wsl-hub: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Where the old hub leaves the unit's text for the install (this PC's data directory).
fn unit_file(paths: &HubPaths, service: &WslService) -> PathBuf {
    paths.data_dir.join(format!("wsl-hub-{}.service", service.op.as_str()))
}

/// Starts `yaac wsl-hub install` detached, with this process's hub profile: the old hub's
/// [`yaac_hub::transfer::TransferHost::install_wsl_service`].
pub(crate) fn start_install(service: &WslService) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let paths = HubPaths::from_env().map_err(|e| e.to_string())?;
    let file = write_unit(&paths, service)?;
    crate::attach::spawn_detached(&exe, &wsl_install_args(service, &file), &paths).map_err(|e| {
        let _ = std::fs::remove_file(&file);
        format!("cannot start {}: {e}", exe.display())
    })
}

fn write_unit(paths: &HubPaths, service: &WslService) -> Result<PathBuf, String> {
    let file = unit_file(paths, service);
    yaac_hub::paths::ensure_private_dir(&paths.data_dir).map_err(|e| e.to_string())?;
    std::fs::write(&file, &service.unit).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    yaac_hub::paths::make_private_file(&file).map_err(|e| e.to_string())?;
    Ok(file)
}

/// The install, with the `wsl.exe` run injected (tests never run it): waits up to `lock_wait`
/// for this PC's hub to exit, then runs the install script as root in `distro` with the unit
/// from `unit_file`, which is deleted afterwards.
fn install_with(
    paths: &HubPaths,
    distro: &str,
    data_dir: &str,
    unit_file: &Path,
    op: &yaac_protocol::OpId,
    lock_wait: Duration,
    run: impl FnOnce(&[String]) -> Result<(), String>,
) -> Result<(), String> {
    let deadline = Instant::now() + lock_wait;
    loop {
        match yaac_hub::HubLock::acquire(&paths.lock_file()) {
            Ok(lock) => break drop(lock),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => return Err(format!("this PC's hub did not exit, so the hub's service in {distro} was not set up: {e}")),
        }
    }
    let unit = std::fs::read_to_string(unit_file).map_err(|e| format!("cannot read {}: {e}", unit_file.display()));
    let _ = std::fs::remove_file(unit_file);
    let service = WslService { distro: distro.to_owned(), data_dir: data_dir.to_owned(), unit: unit?, op: op.clone() };
    run(&wsl_service_install_argv(&service)).map_err(|e| format!("the hub's service in {distro} was not set up: {e}"))
}

/// **Start the hub here again** for a hub in `distro`: its service is stopped and removed (as
/// root through `wsl.exe`). Blocks while the hub there stops: never on the UI thread.
pub(crate) fn remove(distro: &str) -> Result<(), String> {
    let paths = HubPaths::from_env().map_err(|e| e.to_string())?;
    let log = log_file(&paths);
    remove_with(distro, |argv| run_wsl(argv, &log))
}

fn remove_with(distro: &str, run: impl FnOnce(&[String]) -> Result<(), String>) -> Result<(), String> {
    run(&wsl_service_remove_argv(distro)).map_err(|e| format!("the hub's service in {distro} could not be removed: {e}"))
}

fn log_file(paths: &HubPaths) -> PathBuf {
    paths.data_dir.join("wsl-hub.log")
}

/// Runs `wsl.exe <argv>` without a console window. Its standard error is kept for the message
/// and appended to `log` with its standard output.
fn run_wsl(argv: &[String], log: &Path) -> Result<(), String> {
    let wsl = wsl_exe();
    let mut cmd = Command::new(&wsl);
    cmd.args(argv).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| format!("cannot run {}: {e}", wsl.display()))?;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        use std::io::Write as _;
        let _ = f.write_all(&out.stdout);
        let _ = f.write_all(&out.stderr);
    }
    if out.status.success() {
        return Ok(());
    }
    // wsl.exe's own messages are UTF-16; the script's are UTF-8.
    let err = String::from_utf8_lossy(&out.stderr).replace('\0', "");
    let err = err.trim();
    Err(if err.is_empty() { format!("wsl.exe exited with {}", out.status) } else { err.to_owned() })
}

fn wsl_exe() -> PathBuf {
    std::env::var_os("YAAC_WSL_EXE").filter(|v| !v.is_empty()).map_or_else(|| "wsl.exe".into(), PathBuf::from)
}

/// How the window reaches a hub in WSL that a tombstone from before `via` (build 1059) names:
/// its distro, and the user of this PC's WSL target for it (from this PC's own `hub.json`, as
/// it was at the hand-over). `None`: the tombstone names no distro.
pub(crate) fn via_from_old_tombstone(paths: &HubPaths) -> Option<yaac_protocol::attach::TargetTransport> {
    use yaac_protocol::attach::{TargetTransport, WslTransport};
    let distro = yaac_hub::paths::moved_tombstone(paths).ok().flatten()?.wsl_distro?;
    let user = yaac_hub::settings::Settings::load(&paths.settings_file()).ok().flatten().and_then(|s| {
        s.targets.iter().find_map(|t| match &t.transport {
            TargetTransport::Wsl(w) if w.distro.eq_ignore_ascii_case(&distro) => Some(w.user.clone()),
            _ => None,
        })
    });
    Some(TargetTransport::Wsl(WslTransport { distro, user: user.flatten() }))
}

/// The P414 keeper's single-instance event for this hub profile (`Local\<pipe name>-wsl-hub`).
#[cfg_attr(not(windows), allow(dead_code))]
fn legacy_keeper_name(paths: &HubPaths) -> String {
    let pipe = paths.socket();
    let tail = pipe.file_name().map_or_else(|| "yaac-hub".into(), |n| n.to_string_lossy().into_owned());
    format!(r"Local\{}-wsl-hub", tail.replace('\\', "-"))
}

/// `yaac quit` while a keeper of an earlier build (P414) still runs a hub in WSL: asks it to
/// stop that hub and waits for it to exit. `Ok(false)`: none runs.
#[cfg(windows)]
pub(crate) fn stop_legacy_keeper(paths: &HubPaths) -> Result<bool, String> {
    let name = legacy_keeper_name(paths);
    match yaac_winsys::signal_named_event(&format!("{name}-quit")) {
        Ok(false) => return Ok(false),
        Ok(true) => {}
        Err(e) => return Err(format!("cannot ask the WSL hub's keeper to stop: {e}")),
    }
    let deadline = Instant::now() + Duration::from_secs(30);
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
pub(crate) fn stop_legacy_keeper(paths: &HubPaths) -> Result<bool, String> {
    let _ = paths;
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use yaac_protocol::OpId;

    fn service() -> WslService {
        WslService { distro: "Ubuntu".into(), data_dir: "/home/me/.local/share/yaac".into(), unit: "[Unit]\nUser=me\n".into(), op: OpId::new("op-1") }
    }

    #[test]
    fn the_install_waits_for_the_old_hub_then_runs_once_as_root() {
        let dir = tempfile::tempdir().unwrap();
        let paths = HubPaths::under(dir.path());
        std::fs::create_dir_all(&paths.runtime_dir).unwrap();
        let s = service();
        let file = write_unit(&paths, &s).unwrap();
        assert_eq!(file, paths.data_dir.join("wsl-hub-op-1.service"));
        let ran: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();

        // The old hub still holds its lock: nothing runs in the distro.
        let lock = yaac_hub::HubLock::acquire(&paths.lock_file()).unwrap();
        let r = ran.clone();
        let e = install_with(&paths, "Ubuntu", &s.data_dir, &file, &s.op, Duration::from_millis(300), move |a| {
            r.lock().unwrap().push(a.to_vec());
            Ok(())
        })
        .unwrap_err();
        assert!(e.contains("did not exit"), "{e}");
        assert!(ran.lock().unwrap().is_empty() && file.exists());
        drop(lock);

        let r = ran.clone();
        install_with(&paths, "Ubuntu", &s.data_dir, &file, &s.op, Duration::from_millis(300), move |a| {
            r.lock().unwrap().push(a.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(*ran.lock().unwrap(), [wsl_service_install_argv(&s)], "the unit text from the file, as root");
        assert!(!file.exists(), "the unit's copy on Windows goes");

        // A failure in the distro is said, never retried.
        let file = write_unit(&paths, &s).unwrap();
        let e = install_with(&paths, "Ubuntu", &s.data_dir, &file, &s.op, Duration::ZERO, |_| Err("systemd is not running".into())).unwrap_err();
        assert_eq!(e, "the hub's service in Ubuntu was not set up: systemd is not running");
    }

    #[test]
    fn start_the_hub_here_again_removes_the_service() {
        let mut seen = Vec::new();
        remove_with("Ubuntu", |a| {
            seen = a.to_vec();
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, wsl_service_remove_argv("Ubuntu"));
        let e = remove_with("Ubuntu", |_| Err("yaac-hub.service still runs".into())).unwrap_err();
        assert!(e.contains("could not be removed: yaac-hub.service still runs"), "{e}");
    }

    #[test]
    fn the_old_keepers_login_entry_does_nothing() {
        let args: Vec<String> = ["--distro", "Ubuntu"].map(str::to_owned).into();
        assert_eq!(run(&args), ExitCode::SUCCESS);
    }

    #[test]
    fn an_old_tombstone_names_the_distro_and_its_user() {
        let dir = tempfile::tempdir().unwrap();
        let paths = HubPaths::under(dir.path());
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        assert_eq!(via_from_old_tombstone(&paths), None, "no move");
        let mut t = yaac_hub::paths::MovedTombstone { hub_id: "h".into(), address: "100.64.0.9:47380".into(), host_name: "ubuntu".into(), at_ms: 1, wsl_distro: None, via: None };
        yaac_hub::paths::write_moved_tombstone(&paths, &t).unwrap();
        assert_eq!(via_from_old_tombstone(&paths), None, "moved, but not into WSL");
        t.wsl_distro = Some("Ubuntu".into());
        yaac_hub::paths::write_moved_tombstone(&paths, &t).unwrap();
        let wsl = |user: Option<&str>| serde_json::json!({ "kind": "wsl", "distro": "Ubuntu", "user": user });
        let via = |p: &HubPaths| serde_json::to_value(via_from_old_tombstone(p)).unwrap();
        assert_eq!(via(&paths)["distro"], "Ubuntu");
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
        assert_eq!(via(&paths), wsl(Some("dev")));
    }

    #[test]
    fn one_legacy_keeper_name_per_hub_profile() {
        let a = legacy_keeper_name(&HubPaths::under(std::path::Path::new("/tmp/a")));
        assert!(a.starts_with(r"Local\") && a.ends_with("-wsl-hub"), "{a}");
    }
}
