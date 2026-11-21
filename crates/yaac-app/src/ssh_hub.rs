//! The SSH setup backend when the window is attached to a hub: every step is a `hub/ssh/*`
//! request (the hub runs the `ssh` sessions; the window never opens one). The backend only
//! queues requests; the window's frame hands them to the [`Client`] and the answers back
//! ([`SshBridge::exchange`]), so no call ever blocks.
//!
//! The password (or passphrase) leaves its zeroised [`yaac_core::ssh_setup::Secret`] only to
//! be written into the one `hub/ssh/signIn` request, which is sent at once.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use eframe::egui;
use serde_json::{Value, json};
use yaac_core::client::{Client, hub_names};
use yaac_core::ssh_setup::{Credential, HostKey, InstalledKey, ProbeOutcome, SshConn, SshSetupBackend, StepError, StepReport};
use yaac_protocol::rpc::{ErrorObject, RpcId};

/// Which step a request was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Reach,
    Probe,
    SignIn,
    Verify,
    RemoveKey,
    Revoke,
}

/// What the backend and the window share.
#[derive(Debug, Default)]
pub(crate) struct SshBridge {
    outbox: VecDeque<(Step, &'static str, Value)>,
    in_flight: HashMap<RpcId, Step>,
    reports: VecDeque<StepReport>,
    /// The key `signIn` installed, for `install_key`.
    installed: Option<InstalledKey>,
    /// Wakes the window so a queued step goes out on the next frame.
    repaint: Option<egui::Context>,
}

impl SshBridge {
    /// A bridge that wakes `ctx` when a step is queued.
    pub(crate) fn new(ctx: egui::Context) -> Self {
        Self { repaint: Some(ctx), ..Self::default() }
    }
}

fn conn_params(c: &SshConn) -> Value {
    json!({ "host": c.host, "port": c.port, "user": (!c.user.trim().is_empty()).then(|| c.user.trim()) })
}

fn with(c: &SshConn, extra: &Value) -> Value {
    let mut v = conn_params(c);
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    v
}

fn step_error(e: &ErrorObject) -> StepError {
    let unreachable = e.code.as_str() == "ssh_unreachable";
    StepError { message: e.message.clone(), unreachable }
}

impl SshBridge {
    /// Send queued steps through `client` and turn its answers into step reports. Returns
    /// whether anything changed.
    pub(crate) fn exchange(bridge: &Rc<RefCell<Self>>, client: &mut Client) -> bool {
        let mut b = bridge.borrow_mut();
        let mut any = false;
        while let Some((step, method, params)) = b.outbox.pop_front() {
            let id = client.ssh_request(method, params);
            b.in_flight.insert(id, step);
        }
        for (id, outcome) in client.take_ssh_replies() {
            let Some(step) = b.in_flight.remove(&id) else { continue };
            any = true;
            let report = b.report(step, outcome);
            b.reports.extend(report);
        }
        any
    }

    fn report(&mut self, step: Step, outcome: Result<Value, ErrorObject>) -> Option<StepReport> {
        let e = |x: &ErrorObject| step_error(x);
        Some(match step {
            Step::Reach => StepReport::Reached(outcome.as_ref().map_err(e).map(|v| HostKey {
                algorithm: v["algorithm"].as_str().unwrap_or_default().to_owned(),
                fingerprint: v["fingerprint"].as_str().unwrap_or_default().to_owned(),
            })),
            Step::Probe => StepReport::Probed(outcome.as_ref().map_err(e).map(|v| {
                if v["inPlace"].as_bool() == Some(true) { ProbeOutcome::AccessInPlace } else { ProbeOutcome::SignInNeeded }
            })),
            Step::SignIn => match outcome {
                Ok(v) => {
                    self.installed = Some(InstalledKey {
                        line: v["line"].as_str().unwrap_or_default().to_owned(),
                        comment: v["comment"].as_str().unwrap_or_default().to_owned(),
                    });
                    StepReport::SignedIn(Ok(()))
                }
                Err(x) => StepReport::SignedIn(Err(e(&x))),
            },
            Step::Verify => StepReport::Verified(outcome.as_ref().map(|_| ()).map_err(e)),
            Step::RemoveKey => StepReport::KeyRemoved(outcome.as_ref().map(|_| ()).map_err(e)),
            Step::Revoke => StepReport::Revoked(outcome.as_ref().map(|_| ()).map_err(e)),
        })
    }
}

/// The backend the settings panel drives.
#[derive(Debug, Clone)]
pub(crate) struct HubSsh(pub Rc<RefCell<SshBridge>>);

impl HubSsh {
    fn send(&self, step: Step, method: &'static str, params: Value) {
        let mut b = self.0.borrow_mut();
        b.outbox.push_back((step, method, params));
        if let Some(ctx) = &b.repaint {
            ctx.request_repaint();
        }
    }
}

impl SshSetupBackend for HubSsh {
    fn reach(&mut self, c: &SshConn) {
        self.send(Step::Reach, hub_names::SSH_REACH, conn_params(c));
    }
    fn probe(&mut self, c: &SshConn, k: &HostKey) {
        self.send(Step::Probe, hub_names::SSH_PROBE, with(c, &json!({ "fingerprint": k.fingerprint })));
    }
    fn sign_in(&mut self, c: &SshConn, credential: Credential) {
        let params = match &credential {
            Credential::Password(secret) => with(c, &json!({ "password": secret.expose() })),
            Credential::KeyFile { path, passphrase } => with(c, &json!({ "keyFile": path, "passphrase": passphrase.as_ref().map(|p| p.expose()) })),
        };
        // The credential is wiped here; the request goes out on the next frame.
        drop(credential);
        self.send(Step::SignIn, hub_names::SSH_SIGN_IN, params);
    }
    fn install_key(&mut self, _c: &SshConn) {
        // The hub installed the key in the sign-in session (the password is used once).
        let mut b = self.0.borrow_mut();
        let report = match b.installed.take() {
            Some(key) => StepReport::KeyInstalled(Ok(key)),
            None => StepReport::KeyInstalled(Err(StepError::new("the hub did not say which key it installed"))),
        };
        b.reports.push_back(report);
    }
    fn verify(&mut self, c: &SshConn, _k: &InstalledKey) {
        self.send(Step::Verify, hub_names::SSH_VERIFY, conn_params(c));
    }
    fn remove_key(&mut self, c: &SshConn, _k: &InstalledKey) {
        self.send(Step::RemoveKey, hub_names::SSH_REMOVE_KEY, conn_params(c));
    }
    fn revoke(&mut self, c: &SshConn, _k: &InstalledKey) {
        self.send(Step::Revoke, hub_names::SSH_REVOKE, conn_params(c));
    }
    fn poll(&mut self) -> Option<StepReport> {
        self.0.borrow_mut().reports.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_become_step_reports_and_sign_in_carries_the_installed_key() {
        let mut b = SshBridge::default();
        let key = b.report(Step::Reach, Ok(json!({ "algorithm": "ED25519", "fingerprint": "SHA256:x" })));
        assert_eq!(key, Some(StepReport::Reached(Ok(HostKey { algorithm: "ED25519".into(), fingerprint: "SHA256:x".into() }))));
        let e = ErrorObject::new(yaac_protocol::rpc::ErrorCode::from_wire("ssh_unreachable"), "down");
        assert_eq!(b.report(Step::Revoke, Err(e)), Some(StepReport::Revoked(Err(StepError::unreachable("down")))));
        b.report(Step::SignIn, Ok(json!({ "line": "ssh-ed25519 AAAA yaac:h:1", "comment": "yaac:h:1" })));
        let bridge = Rc::new(RefCell::new(b));
        let mut backend = HubSsh(bridge);
        backend.install_key(&SshConn { name: "n".into(), host: "h".into(), port: 22, user: String::new() });
        assert!(matches!(backend.poll(), Some(StepReport::KeyInstalled(Ok(k))) if k.comment == "yaac:h:1"));
    }
}
