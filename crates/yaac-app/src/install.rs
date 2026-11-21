//! The commands the Windows installer runs (docs/install.md), usable by hand on any platform:
//!
//! ```text
//! yaac quit [--wait-for <exe>]   close the window, stop the hub, wait for both to exit
//! yaac autostart on|off|status   the hub's login entry, pointing at this executable
//! yaac autostart sync [--start]  the entry follows the hub's "always" setting; --start then
//!                                starts the hub, as login would, when the entry is on
//! yaac remove-data               delete YAAC's own host-side folders (settings, cache, keys)
//! ```
//!
//! Every path comes from the same environment the window and the hub read
//! ([`HubPaths::from_env`], [`Autostart::from_env`]), so a test profile (`YAAC_CONFIG_DIR`,
//! `YAAC_DATA_DIR`, `YAAC_RUNTIME_DIR`, `YAAC_AUTOSTART_KEY`) is used throughout and the
//! user's real one is never touched. Nothing here reaches a target: stopping the hub is the
//! ordinary `hub/shutdown` (its stores checkpointed; turns keep running on the targets, whose
//! `yaacd` journals everything, invariant #8).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde_json::json;
use yaac_hub::HubPaths;
use yaac_hub::autostart::{Autostart, autostart_installed, install_autostart, remove_autostart};
use yaac_hub::client::HubClient;
use yaac_hub::settings::Settings;
use yaac_protocol::attach::names;

/// How long each step may take.
const STEP: Duration = Duration::from_secs(20);

/// Runs `yaac <command> …` if `args` names one of these commands.
pub fn dispatch(args: &[String], build: &str) -> Option<ExitCode> {
    let rest = args.get(1..).unwrap_or_default();
    let code = match args.first().map(String::as_str)? {
        "quit" => quit(rest, build),
        "autostart" => autostart(rest),
        "remove-data" => remove_data(),
        _ => return None,
    };
    Some(code)
}

fn paths() -> Result<HubPaths, ExitCode> {
    HubPaths::from_env().map_err(|e| {
        eprintln!("yaac: {e}");
        ExitCode::FAILURE
    })
}

fn quit(args: &[String], build: &str) -> ExitCode {
    let wait_for = match args {
        [] => None,
        [flag, exe] if flag == "--wait-for" => Some(PathBuf::from(exe)),
        _ => {
            eprintln!("usage: yaac quit [--wait-for <exe>]");
            return ExitCode::from(2);
        }
    };
    let paths = match paths() {
        Ok(p) => p,
        Err(code) => return code,
    };
    let mut ok = true;
    // The window first: it could otherwise start a hub again.
    #[cfg(windows)]
    if let Err(e) = quit_window(&paths) {
        eprintln!("yaac quit: {e}");
        ok = false;
    }
    if let Err(e) = stop_hub(&paths, build) {
        eprintln!("yaac quit: {e}");
        ok = false;
    }
    if let Some(exe) = wait_for
        && !wait_until_replaceable(&exe, STEP)
    {
        eprintln!("yaac quit: {} is still in use", exe.display());
        ok = false;
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// Signals the window's quit event and waits for its single-instance event to disappear (the
/// window process has ended). No window: nothing to do.
#[cfg(windows)]
fn quit_window(paths: &HubPaths) -> Result<(), String> {
    let name = crate::tray::instance_name(paths);
    match yaac_winsys::signal_named_event(&format!("{name}-quit")) {
        Ok(false) => return Ok(()),
        Ok(true) => {}
        Err(e) => return Err(format!("cannot ask the window to close: {e}")),
    }
    let deadline = Instant::now() + STEP;
    while yaac_winsys::named_event_exists(&name) {
        if Instant::now() >= deadline {
            return Err("the window did not close".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// `hub/shutdown` to the running hub, then waits for it to release its lock. No hub: nothing
/// to do.
fn stop_hub(paths: &HubPaths, build: &str) -> Result<(), String> {
    let Ok(client) = HubClient::connect(&paths.socket()) else { return Ok(()) };
    // The same build, or an older hub, accepts `hub/shutdown` after the hello; a newer hub
    // refuses it (an older build never replaces a newer hub, N14), which is reported. The
    // installer runs this with the installed executable, the hub's own build.
    let _ = client.request_blocking(names::HELLO, json!({ "build": build }), STEP);
    client.request_blocking(names::SHUTDOWN, json!({}), STEP).map_err(|e| format!("the hub refused to stop: {}", e.message))?;
    let deadline = Instant::now() + STEP;
    loop {
        if let Ok(lock) = yaac_hub::HubLock::acquire(&paths.lock_file()) {
            drop(lock);
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("the hub did not exit".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits until `exe` can be opened for writing (no running process maps it any more). A
/// missing file is replaceable.
fn wait_until_replaceable(exe: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match std::fs::OpenOptions::new().write(true).open(exe) {
            Ok(_) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) if Instant::now() >= deadline => return false,
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn autostart(args: &[String]) -> ExitCode {
    let usage = || {
        eprintln!("usage: yaac autostart on|off|status | yaac autostart sync [--start]");
        ExitCode::from(2)
    };
    let (cmd, start) = match args {
        [cmd] => (cmd.as_str(), false),
        [cmd, flag] if cmd == "sync" && flag == "--start" => (cmd.as_str(), true),
        _ => return usage(),
    };
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("yaac: cannot find the yaac binary: {e}");
            return ExitCode::FAILURE;
        }
    };
    let entry = match Autostart::from_env(exe) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("yaac autostart: {e}");
            return ExitCode::FAILURE;
        }
    };
    let want = match cmd {
        "on" => Some(true),
        "off" => Some(false),
        "status" => {
            println!("{}", if autostart_installed(&entry) { "on" } else { "off" });
            return ExitCode::SUCCESS;
        }
        "sync" => {
            let paths = match paths() {
                Ok(p) => p,
                Err(code) => return code,
            };
            match Settings::load(&paths.settings_file()) {
                Ok(Some(s)) => Some(s.hub.always),
                // Never run: the window's first run installs the entry and starts the hub, so
                // its first-run page still appears. An entry already there is pointed here.
                Ok(None) => autostart_installed(&entry).then_some(true),
                Err(e) => {
                    eprintln!("yaac autostart: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        _ => return usage(),
    };
    let result = match want {
        Some(true) => install_autostart(&entry),
        Some(false) => remove_autostart(&entry),
        None => Ok(()),
    };
    if let Err(e) = result {
        eprintln!("yaac autostart: {e}");
        return ExitCode::FAILURE;
    }
    println!("{}", if want == Some(true) { "on" } else { "off" });
    if start && want == Some(true) {
        return start_hub();
    }
    ExitCode::SUCCESS
}

/// Starts `yaac hub` detached, as the login entry would (a hub already running makes the new
/// one exit at once).
fn start_hub() -> ExitCode {
    let config = match crate::attach::AttachConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yaac: {e}");
            return ExitCode::FAILURE;
        }
    };
    match crate::attach::spawn_hub(&config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yaac: could not start the hub: {e}");
            ExitCode::FAILURE
        }
    }
}

fn remove_data() -> ExitCode {
    let paths = match paths() {
        Ok(p) => p,
        Err(code) => return code,
    };
    if HubClient::connect(&paths.socket()).is_ok() {
        eprintln!("yaac remove-data: the hub is running; run `yaac quit` first");
        return ExitCode::FAILURE;
    }
    let mut ok = true;
    let separate_runtime = !paths.runtime_dir.starts_with(&paths.data_dir);
    let mut dirs = vec![paths.config_dir, paths.data_dir];
    if separate_runtime {
        dirs.push(paths.runtime_dir);
    }
    for dir in dirs {
        match removable(&dir) {
            Ok(false) => {}
            Ok(true) => match std::fs::remove_dir_all(&dir) {
                Ok(()) => println!("removed {}", dir.display()),
                Err(e) => {
                    eprintln!("yaac remove-data: {}: {e}", dir.display());
                    ok = false;
                }
            },
            Err(why) => {
                eprintln!("yaac remove-data: left {} alone: {why}", dir.display());
                ok = false;
            }
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// Whether `dir` is YAAC's own folder and may be deleted whole: `Ok(false)` when it does not
/// exist; an error when it does not look like YAAC's (a variable pointing somewhere else).
fn removable(dir: &Path) -> Result<bool, String> {
    if !dir.exists() {
        return Ok(false);
    }
    if !dir.is_dir() {
        return Err("not a directory".into());
    }
    if dir.components().count() < 3 {
        return Err("too close to the root of the file system".into());
    }
    let named_yaac = dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case("yaac"));
    let has_ours = ["hub.json", "display.json", "cache.sqlite", "drafts.sqlite", "relay.sqlite", "hub.lock"].iter().any(|f| dir.join(f).exists());
    if named_yaac || has_ours { Ok(true) } else { Err("it holds none of YAAC's files".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_yaacs_own_folders_are_removable() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(removable(&t.path().join("missing")), Ok(false));
        let other = t.path().join("documents");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("notes.txt"), "x").unwrap();
        assert!(removable(&other).is_err(), "a folder without YAAC's files is left alone");
        std::fs::write(other.join("hub.json"), "{}").unwrap();
        assert_eq!(removable(&other), Ok(true));
        let named = t.path().join("yaac");
        std::fs::create_dir(&named).unwrap();
        assert_eq!(removable(&named), Ok(true));
        assert!(removable(Path::new("/")).is_err());
    }

    #[test]
    fn a_free_file_is_replaceable_and_a_missing_one_too() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("yaac.exe");
        assert!(wait_until_replaceable(&f, Duration::ZERO));
        std::fs::write(&f, "x").unwrap();
        assert!(wait_until_replaceable(&f, Duration::ZERO));
    }

    /// `yaac quit` against a stand-in window: it holds the single-instance event and listens on
    /// the quit event as the real one does, and lets go of both when asked.
    #[cfg(windows)]
    #[test]
    fn quit_closes_a_listening_window_and_waits_for_it() {
        let t = tempfile::tempdir().unwrap();
        let paths = HubPaths::under(t.path());
        let name = crate::tray::instance_name(&paths);
        assert!(!yaac_winsys::named_event_exists(&name));
        assert!(quit_window(&paths).is_ok(), "no window: nothing to do");
        let yaac_winsys::InstanceClaim::First(instance) = yaac_winsys::claim_window_instance(&name).unwrap() else { panic!("claimed") };
        let quit = yaac_winsys::listen_named_event(&format!("{name}-quit")).unwrap();
        let window = std::thread::spawn(move || {
            assert!(quit.wait());
            std::thread::sleep(Duration::from_millis(200));
            drop((instance, quit));
        });
        assert!(yaac_winsys::named_event_exists(&name));
        quit_window(&paths).unwrap();
        assert!(!yaac_winsys::named_event_exists(&name), "quit returns once the window has let go");
        window.join().unwrap();
    }
}
