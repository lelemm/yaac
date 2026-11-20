//! `yaac hub`: the headless hub. Nothing here touches a window, the GPU, or fonts — the part
//! that stays running is the small part (docs/02-architecture.md#footprint-budget).

use std::process::ExitCode;

use yaac_hub::{HubConfig, HubError, run_hub};

/// Run the hub until it exits (idle with nothing attached and `always` off, `hub/shutdown`, or
/// a signal).
pub fn run(build: &str) -> ExitCode {
    let config = match HubConfig::from_env(build) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yaac hub: {e}");
            return ExitCode::FAILURE;
        }
    };
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
