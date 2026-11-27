//! `yaac hub`: the headless hub. Nothing here touches a window, the GPU, or fonts — the part
//! that stays running is the small part (docs/02-architecture.md#footprint-budget).

use std::process::ExitCode;

use yaac_hub::{HubConfig, HubError, run_hub};

/// Run the hub until it exits (idle with nothing attached and `always` off, `hub/shutdown`, or
/// a signal).
pub fn run(build: &str) -> ExitCode {
    let mut config = match HubConfig::from_env(build) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yaac hub: {e}");
            return ExitCode::FAILURE;
        }
    };
    // A newer phone may ask for its build to be installed here (`hub/update/request`): the
    // window's updater does the work, so only where it is enabled (Windows). Elsewhere the hub
    // answers that this host does not support it.
    if crate::updater::ENABLED {
        let dir = config.paths.data_dir.join("updates");
        config.host_installer = Some(std::sync::Arc::new(crate::updater::HubInstaller::new(crate::updater::Config::github(dir))));
    }
    match run_hub(config) {
        Ok(_) => ExitCode::SUCCESS,
        Err(HubError::AlreadyRunning(e)) => {
            eprintln!("yaac hub: another hub is already running ({e})");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("yaac hub: {e}");
            ExitCode::FAILURE
        }
    }
}
