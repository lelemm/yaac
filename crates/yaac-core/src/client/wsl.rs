//! The installed WSL distros for Settings → Targets, asked of the hub (`hub/wsl/distros`, the
//! hub's `protocol::names::WSL_DISTROS`). The window never runs `wsl.exe` itself.

use serde::Deserialize;
use serde_json::{Value, json};
use yaac_protocol::rpc::ErrorObject;

use super::{Call, Client, hub_names};
use crate::settings::{DistroPhase, WslDistro, choose_distro};

#[derive(Deserialize)]
struct DistrosWire {
    #[serde(default)]
    distros: Vec<WslDistro>,
}

impl Client {
    /// Ask the hub for the installed distros (the Targets page opened, or the user asked
    /// again). The dropdown says "Loading…" until it answers. A request already out is not
    /// sent twice.
    pub fn load_wsl_distros(&mut self) {
        self.state.wsl_distros.phase = DistroPhase::Loading;
        if self.pending.values().any(|c| *c == Call::WslDistros) {
            return;
        }
        self.request(hub_names::WSL_DISTROS, json!({}), Call::WslDistros);
    }

    /// The user chose a distro in the dropdown (`None` after it was added).
    pub fn choose_wsl_distro(&mut self, name: Option<String>) {
        self.state.wsl_distros.selected = name;
    }

    pub(super) fn on_wsl_distros(&mut self, result: Value) {
        let d = &mut self.state.wsl_distros;
        match serde_json::from_value::<DistrosWire>(result) {
            Ok(w) => {
                d.selected = choose_distro(&w.distros, &self.state.targets, d.selected.as_deref());
                d.list = w.distros;
                d.phase = DistroPhase::Ready;
            }
            Err(e) => d.phase = DistroPhase::Failed(format!("The hub's list of distros could not be read: {e}")),
        }
    }

    pub(super) fn on_wsl_distros_error(&mut self, error: &ErrorObject) {
        self.state.wsl_distros.phase = DistroPhase::Failed(error.message.clone());
    }
}
