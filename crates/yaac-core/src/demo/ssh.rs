//! **Demo only.** A scripted SSH setup backend: every step answers at once (on the next
//! `poll`), following a script. It never sees a real server, and it keeps nothing it is given:
//! a credential is checked for being non-empty and dropped (wiped) in `sign_in`.

use std::collections::VecDeque;

use crate::ssh_setup::{Credential, HostKey, InstalledKey, ProbeOutcome, SshConn, SshSetupBackend, StepError, StepReport};

/// How the scripted server behaves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SshScript {
    /// A password is needed; it works; the key installs and verifies (screens 14, 22, 15).
    #[default]
    Happy,
    /// Password-less login already works.
    AccessInPlace,
    /// The server accepts passwords but not keys: verification fails and the key is removed
    /// again (screen 16, "Could not finish").
    KeysRefused,
    /// The server cannot be reached: a revoke cannot finish (screen 16, "Could not revoke").
    Unreachable,
}

/// The scripted backend.
#[derive(Debug, Default)]
pub struct ScriptedSsh {
    /// The script.
    pub script: SshScript,
    queue: VecDeque<StepReport>,
    /// How many credentials it was handed (for tests: exactly one per Connect).
    pub sign_ins: usize,
}

/// The fingerprint the demo server shows.
pub const FINGERPRINT: &str = "SHA256:nThbg6kXUpJWGl7E1IGOCspRomTxdCARLviKw6E5SY8";

/// The line the demo installs for a connection named `name`.
pub fn demo_line(name: &str) -> InstalledKey {
    let comment = format!("yaac:laptop:{name}");
    InstalledKey { line: format!("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFq4mW1nJt0Xc8uPz3yKb7Vd2Lh9sRgE5oTa6QwNiYk3 {comment}"), comment }
}

impl ScriptedSsh {
    /// A backend following `script`.
    pub fn new(script: SshScript) -> Self {
        Self { script, queue: VecDeque::new(), sign_ins: 0 }
    }
}

impl SshSetupBackend for ScriptedSsh {
    fn reach(&mut self, conn: &SshConn) {
        let r = if self.script == SshScript::Unreachable {
            Err(StepError::unreachable(format!("{} could not be reached.", conn.host)))
        } else {
            Ok(HostKey { algorithm: "ED25519".into(), fingerprint: FINGERPRINT.into() })
        };
        self.queue.push_back(StepReport::Reached(r));
    }

    fn probe(&mut self, _conn: &SshConn, _host_key: &HostKey) {
        let o = if self.script == SshScript::AccessInPlace { ProbeOutcome::AccessInPlace } else { ProbeOutcome::SignInNeeded };
        self.queue.push_back(StepReport::Probed(Ok(o)));
    }

    fn sign_in(&mut self, _conn: &SshConn, credential: Credential) {
        self.sign_ins += 1;
        let ok = match &credential {
            Credential::Password(p) => !p.is_empty(),
            Credential::KeyFile { path, .. } => !path.trim().is_empty(),
        };
        drop(credential);
        let r = if ok { Ok(()) } else { Err(StepError::new("Permission denied (publickey,password).")) };
        self.queue.push_back(StepReport::SignedIn(r));
    }

    fn install_key(&mut self, conn: &SshConn) {
        self.queue.push_back(StepReport::KeyInstalled(Ok(demo_line(&conn.name))));
    }

    fn verify(&mut self, _conn: &SshConn, _key: &InstalledKey) {
        let r = if self.script == SshScript::KeysRefused { Err(StepError::new("This server does not accept key logins.")) } else { Ok(()) };
        self.queue.push_back(StepReport::Verified(r));
    }

    fn remove_key(&mut self, _conn: &SshConn, _key: &InstalledKey) {
        self.queue.push_back(StepReport::KeyRemoved(Ok(())));
    }

    fn revoke(&mut self, conn: &SshConn, _key: &InstalledKey) {
        let r = if self.script == SshScript::Unreachable {
            Err(StepError::unreachable(format!("{} could not be reached", conn.name)))
        } else {
            Ok(())
        };
        self.queue.push_back(StepReport::Revoked(r));
    }

    fn poll(&mut self) -> Option<StepReport> {
        self.queue.pop_front()
    }
}
