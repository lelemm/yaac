//! `yaac hub`: the headless hub. Nothing here touches a window, the GPU, or fonts — the part
//! that stays running is the small part (docs/02-architecture.md#footprint-budget).
//!
//! `yaac hub --reclaim`: **Start the hub here again** after the hub moved to another machine
//! (docs/15-hub-transfer.md): deletes the tombstone (`<data>/moved.json`), then runs the hub.
//! A hub with a tombstone does not start otherwise ([`HubError::Moved`], checked under the
//! hub's lock).

use std::process::ExitCode;

use yaac_hub::{HubConfig, HubError, run_hub};

/// Run the hub until it exits (idle with nothing attached and `always` off, `hub/shutdown`, a
/// signal, or a move to another machine). `args`: after `hub`.
pub fn run(build: &str, args: &[String]) -> ExitCode {
    let reclaim = match args {
        [] => false,
        [flag] if flag == "--reclaim" => true,
        _ => {
            eprintln!("usage: yaac hub [--reclaim]");
            return ExitCode::from(2);
        }
    };
    #[cfg_attr(not(all(windows, feature = "desktop")), allow(unused_mut))]
    let mut config = match HubConfig::from_env(build) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yaac hub: {e}");
            return ExitCode::FAILURE;
        }
    };
    if reclaim {
        match yaac_hub::paths::remove_moved_tombstone(&config.paths) {
            Ok(true) => eprintln!("yaac hub: starting the hub here again"),
            Ok(false) => {}
            Err(e) => {
                eprintln!("yaac hub: cannot remove {}: {e}", config.paths.moved_file().display());
                return ExitCode::FAILURE;
            }
        }
    }
    // A newer phone may ask for its build to be installed here (`hub/update/request`): the
    // window's updater does the work, so only where it is enabled (Windows). Elsewhere the hub
    // answers that this host does not support it.
    #[cfg(all(windows, feature = "desktop"))]
    {
        let dir = config.paths.data_dir.join("updates");
        config.host_installer = Some(std::sync::Arc::new(crate::updater::HubInstaller::new(crate::updater::Config::github(dir))));
    }
    // Moving the hub to another machine needs the release lookup (the updater's HTTP client).
    #[cfg(feature = "desktop")]
    {
        let dir = config.paths.data_dir.join("updates");
        config.transfer_host = Some(std::sync::Arc::new(crate::updater::HubTransfer::new(crate::updater::Config::github(dir))));
    }
    match run_hub(config) {
        Ok(_) => ExitCode::SUCCESS,
        Err(HubError::AlreadyRunning(e)) => {
            eprintln!("yaac hub: another hub is already running ({e})");
            ExitCode::SUCCESS
        }
        Err(e @ HubError::Moved(_)) => {
            eprintln!("yaac hub: {e}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("yaac hub: {e}");
            ExitCode::FAILURE
        }
    }
}
