//! Setting up an SSH connection (docs/02-architecture.md#setting-up-an-ssh-connection; screens
//! 14, 22, 15, 16): the step machine the settings panel is drawn from, and the backend trait
//! that performs the steps.
//!
//! ```text
//! Form ─Setup─► reach ─► ConfirmHost ─► probe ─┬─ works ─────────────────► Ready (already in place)
//!                                              └─ needs a password ─► SignIn
//! SignIn ─Connect─► sign_in ─► install_key ─► verify ─┬─ works ─► Ready (set up by YAAC)
//!                                                    └─ fails ─► remove_key ─► Failed
//! Revoke ─► revoke ─┬─ removed ─► Revoked
//!                   └─ unreachable ─► CouldNotRevoke (the complete line, Copy, "I removed it myself")
//! ```
//!
//! The machine is pure: it calls the [`SshSetupBackend`] and applies its [`StepReport`]s.
//! **The backend never blocks**: each call starts one step and its typed result arrives later
//! through [`SshSetupBackend::poll`]. The real backend is the hub's (it owns every connection
//! to a target, and Setup's short sessions run only while the target has no live one): the
//! window sends each step as a `hub/ssh/*` request ([`crate::client::Client::ssh_request`]);
//! the demo uses the scripted [`crate::demo::ssh::ScriptedSsh`]. With the hub, `sign_in` is
//! the sign-in *and* the install (one session, so the password is used once) and
//! `install_key` reports what it installed.
//!
//! **The password is never stored.** It lives in a [`Secret`] — a zeroised buffer owned by the
//! dialog — and is moved into the backend exactly once, inside a [`Credential`]. Nothing here
//! keeps it: not the machine, not a step line, not a record, not `Debug` output.

use zeroize::Zeroizing;

/// The connection being set up: *user@host:port* and a display name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshConn {
    /// Display name ("legacy").
    pub name: String,
    /// Host name or alias.
    pub host: String,
    /// Port.
    pub port: u16,
    /// User on the server.
    pub user: String,
}

/// The server's host key, shown for confirmation on first contact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKey {
    /// "ED25519".
    pub algorithm: String,
    /// "SHA256:…".
    pub fingerprint: String,
}

/// What the password-less probe found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Logging in with no password worked: access was already in place; YAAC installs nothing.
    AccessInPlace,
    /// It did not: sign in once with a password or a key file.
    SignInNeeded,
}

/// The line YAAC installed in `~/.ssh/authorized_keys` on the server — complete, never
/// abbreviated, because the user may have to delete it by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledKey {
    /// The whole line: `ssh-ed25519 AAAA… yaac:laptop:legacy`.
    pub line: String,
    /// Its comment: `yaac:laptop:legacy`.
    pub comment: String,
}

/// A step failed. The message is the plain-words reason (or the SSH error), shown as-is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepError {
    /// Shown as-is.
    pub message: String,
    /// The server could not be reached at all (revoke: "could not revoke").
    pub unreachable: bool,
}

impl StepError {
    /// A failure with a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), unreachable: false }
    }

    /// The server could not be reached.
    pub fn unreachable(message: impl Into<String>) -> Self {
        Self { message: message.into(), unreachable: true }
    }
}

/// A secret typed by the user (a password or a key passphrase): a zeroised buffer. It is wiped
/// when dropped, has no `Clone`, no `Display`, and its `Debug` shows only a mask.
///
/// The buffer keeps its own capacity: growing it copies into a fresh zeroised buffer and wipes
/// the old one, so no stale copy is left behind by a reallocation.
pub struct Secret(Zeroizing<String>);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(••••••••)")
    }
}

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl Secret {
    /// An empty secret with room for a long passphrase.
    pub fn new() -> Self {
        Self(Zeroizing::new(String::with_capacity(128)))
    }

    /// Append one typed character.
    pub fn push(&mut self, c: char) {
        if self.0.len() + c.len_utf8() > self.0.capacity() {
            let mut bigger = Zeroizing::new(String::with_capacity(self.0.capacity().max(64) * 2));
            bigger.push_str(&self.0);
            // The old buffer is wiped as it drops.
            self.0 = bigger;
        }
        self.0.push(c);
    }

    /// Remove the last character (Backspace).
    pub fn pop(&mut self) {
        self.0.pop();
    }

    /// Remove everything (the buffer is kept, wiped).
    pub fn clear(&mut self) {
        zeroize::Zeroize::zeroize(&mut *self.0);
    }

    /// How many characters it holds (for the mask).
    pub fn chars(&self) -> usize {
        self.0.chars().count()
    }

    /// Empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The secret itself, for the backend's one use (and the dialog's "show" toggle).
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Move the secret out, leaving this one empty. The heap buffer moves; nothing is copied.
    pub fn take(&mut self) -> Secret {
        std::mem::take(self)
    }
}

/// What the user signs in with, once.
#[derive(Debug)]
pub enum Credential {
    /// A password.
    Password(Secret),
    /// A private key file (PEM), used for this one login only, and its passphrase if it has one.
    KeyFile {
        /// Where the file is on this computer.
        path: String,
        /// The passphrase, if the key has one.
        passphrase: Option<Secret>,
    },
}

/// One step's typed result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepReport {
    /// [`SshSetupBackend::reach`]: the server answered and showed its host key.
    Reached(Result<HostKey, StepError>),
    /// [`SshSetupBackend::probe`]: whether password-less login already works.
    Probed(Result<ProbeOutcome, StepError>),
    /// [`SshSetupBackend::sign_in`]: logged in with what was given (the session stays open for
    /// the install).
    SignedIn(Result<(), StepError>),
    /// [`SshSetupBackend::install_key`]: YAAC's own key was appended for this user.
    KeyInstalled(Result<InstalledKey, StepError>),
    /// [`SshSetupBackend::verify`]: logged in again with YAAC's key alone.
    Verified(Result<(), StepError>),
    /// [`SshSetupBackend::remove_key`]: the line just added was removed again, and its absence
    /// confirmed.
    KeyRemoved(Result<(), StepError>),
    /// [`SshSetupBackend::revoke`]: the installed line was removed and the local key deleted.
    Revoked(Result<(), StepError>),
}

/// Performs the steps of SSH setup and revoke. **No method may block**: each starts one step,
/// and its result arrives through [`SshSetupBackend::poll`]. The machine starts at most one
/// step at a time.
pub trait SshSetupBackend {
    /// Contact the server and read its host key (`Reached`).
    fn reach(&mut self, conn: &SshConn);
    /// Try logging in with no password, accepting exactly `host_key` (`Probed`).
    fn probe(&mut self, conn: &SshConn, host_key: &HostKey);
    /// Log in with what the user gave. The credential is moved in and must be wiped after its
    /// one use (`SignedIn`).
    fn sign_in(&mut self, conn: &SshConn, credential: Credential);
    /// In the signed-in session: generate YAAC's own key pair for this connection and append
    /// its public half to `~/.ssh/authorized_keys` (`KeyInstalled`).
    fn install_key(&mut self, conn: &SshConn);
    /// Log in again with YAAC's key only (`Verified`).
    fn verify(&mut self, conn: &SshConn, key: &InstalledKey);
    /// After a failed verification: remove the line just added and confirm it is gone
    /// (`KeyRemoved`).
    fn remove_key(&mut self, conn: &SshConn, key: &InstalledKey);
    /// Revoke: remove exactly the installed line (matched by the key itself) and delete the
    /// local key pair (`Revoked`).
    fn revoke(&mut self, conn: &SshConn, key: &InstalledKey);
    /// The result of the step in flight, once it has finished.
    fn poll(&mut self) -> Option<StepReport>;
}

/// What YAAC did on a server — recorded per connection, so Revoke knows whether there is
/// anything of YAAC's to remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupRecord {
    /// YAAC installed its key on this date.
    InstalledByYaac {
        /// The installed line.
        key: InstalledKey,
        /// "2026-11-20".
        date: String,
    },
    /// Password-less access already worked; YAAC installed nothing.
    AlreadyInPlace {
        /// When Setup found it.
        date: String,
    },
    /// Never set up.
    NotSetUp,
    /// YAAC's key was removed by Revoke.
    Revoked {
        /// When.
        date: String,
    },
    /// The user removed the line by hand and confirmed it; YAAC deleted its local copy.
    RemovedByHand {
        /// When.
        date: String,
    },
}

impl SetupRecord {
    /// The key YAAC installed and still holds, if any.
    pub fn installed(&self) -> Option<&InstalledKey> {
        match self {
            SetupRecord::InstalledByYaac { key, .. } => Some(key),
            _ => None,
        }
    }

    /// Whether the connection can be used (it was set up one way or the other).
    pub fn is_set_up(&self) -> bool {
        matches!(self, SetupRecord::InstalledByYaac { .. } | SetupRecord::AlreadyInPlace { .. })
    }
}

/// A step line's mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepMark {
    /// Done (check in a circle).
    Done,
    /// The step in progress or waiting for the user.
    Current,
    /// Tried; not available (a hollow circle) — not a failure.
    Hollow,
    /// Failed (cross in a circle).
    Failed,
}

/// One line of the step list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepLine {
    /// The mark.
    pub mark: StepMark,
    /// "Reached legacy.example.com".
    pub text: String,
    /// Secondary text beneath it (a failure's reason).
    pub detail: Option<String>,
}

/// How the user signs in at the sign-in step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SignInMethod {
    /// A password.
    #[default]
    Password,
    /// A key file.
    KeyFile,
}

/// Where the setup is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Filling in the form. Setup is enabled once host and port are present.
    Form,
    /// A step is running.
    Working,
    /// Screen 14: confirm the host key.
    ConfirmHost(HostKey),
    /// Screen 22: sign in once. `error` is the last sign-in failure (nothing was changed).
    SignIn {
        /// The last failure, shown as-is.
        error: Option<String>,
    },
    /// Screen 15: ready.
    Ready(SetupRecord),
    /// Screen 16, "Could not finish".
    Failed {
        /// Whether YAAC's key was removed again and its absence confirmed.
        cleaned_up: bool,
    },
    /// Contact failed before anything was changed (bad host, refused): the message.
    Unreached(String),
}

/// The setup of one connection.
#[derive(Clone, Debug)]
pub struct SshSetup {
    /// The connection.
    pub conn: SshConn,
    /// Where it is.
    pub phase: Phase,
    /// The step list so far.
    pub steps: Vec<StepLine>,
    /// The method chosen at the sign-in step (kept to grey it as completed afterwards).
    pub method: SignInMethod,
    host_key: Option<HostKey>,
    installed: Option<InstalledKey>,
    today: String,
}

/// Whether the Setup button can be pressed: host and a valid port are present.
pub fn form_ready(host: &str, port: &str) -> bool {
    !host.trim().is_empty() && port.trim().parse::<u16>().is_ok_and(|p| p > 0)
}

impl SshSetup {
    /// A setup for `conn`; `now` (seconds since the epoch) dates the record.
    pub fn new(conn: SshConn, now: i64) -> Self {
        let today = crate::time::format_rfc3339(now).chars().take(10).collect();
        Self { conn, phase: Phase::Form, steps: Vec::new(), method: SignInMethod::Password, host_key: None, installed: None, today }
    }

    /// Date the record in the host's local time zone (D91): `now` is the same instant given
    /// to [`SshSetup::new`].
    pub fn in_zone(mut self, now: i64, zone: crate::time::Zone) -> Self {
        self.today = crate::time::date_in(now, zone);
        self
    }

    fn line(&mut self, mark: StepMark, text: impl Into<String>, detail: Option<String>) {
        self.steps.push(StepLine { mark, text: text.into(), detail });
    }

    /// The step being waited on, as a "current" line (drawn after [`SshSetup::steps`]).
    pub fn current(&self) -> Option<StepLine> {
        let text = match &self.phase {
            Phase::Working => match self.steps.last().map(|l| l.text.as_str()) {
                None => format!("Reaching {}…", self.conn.host),
                Some(t) if t.starts_with("Reached") => "Confirming the server…".to_owned(),
                Some(t) if t.starts_with("Server confirmed") => "Trying to sign in without a password…".to_owned(),
                Some(t) if t.starts_with("Signed in") => format!("Installing YAAC's key for {}…", self.conn.user),
                Some(t) if t.starts_with("Installed") => "Checking that the key works…".to_owned(),
                Some(t) if t.starts_with("Verified") && self.installed.is_some() => "Removing the key again…".to_owned(),
                _ => format!("Signing in to {}…", self.conn.host),
            },
            Phase::ConfirmHost(_) => "Confirm this is the right server".to_owned(),
            Phase::SignIn { .. } => "Sign in once so YAAC can set up password-less access".to_owned(),
            _ => return None,
        };
        Some(StepLine { mark: StepMark::Current, text, detail: None })
    }

    /// The one-line preview at the sign-in step.
    pub fn next_preview(&self) -> String {
        format!("YAAC will sign in, install its own key for {}, and check that the key works.", self.conn.user)
    }

    /// Setup: contact the server.
    pub fn start(&mut self, backend: &mut dyn SshSetupBackend) {
        if !matches!(self.phase, Phase::Form | Phase::Unreached(_)) {
            return;
        }
        self.steps.clear();
        self.phase = Phase::Working;
        backend.reach(&self.conn);
    }

    /// "This is the right server": try password-less login, accepting exactly this key.
    pub fn confirm_host(&mut self, backend: &mut dyn SshSetupBackend) {
        let Phase::ConfirmHost(key) = self.phase.clone() else { return };
        self.line(StepMark::Done, "Server confirmed", None);
        self.phase = Phase::Working;
        backend.probe(&self.conn, &key);
    }

    /// "Connect": sign in with what the user gave. The credential is handed to the backend and
    /// not kept.
    pub fn sign_in(&mut self, backend: &mut dyn SshSetupBackend, credential: Credential) {
        if !matches!(self.phase, Phase::SignIn { .. }) {
            return;
        }
        self.method = match &credential {
            Credential::Password(_) => SignInMethod::Password,
            Credential::KeyFile { .. } => SignInMethod::KeyFile,
        };
        self.phase = Phase::Working;
        backend.sign_in(&self.conn, credential);
    }

    /// "Try again" after a failure: back to the sign-in step (screen 22).
    pub fn try_again(&mut self) {
        if matches!(self.phase, Phase::Failed { .. }) {
            self.steps.retain(|l| !(l.text.starts_with("Signed in") || l.text.starts_with("Installed") || l.text.starts_with("Verified") || l.text.starts_with("Removed")));
            self.phase = Phase::SignIn { error: None };
        }
    }

    /// Apply what the backend reported, and start the next step. Returns whether anything
    /// changed.
    pub fn pump(&mut self, backend: &mut dyn SshSetupBackend) -> bool {
        let mut any = false;
        while let Some(report) = backend.poll() {
            any = true;
            self.apply(report, backend);
        }
        any
    }

    fn apply(&mut self, report: StepReport, backend: &mut dyn SshSetupBackend) {
        match report {
            StepReport::Reached(Ok(key)) => {
                self.line(StepMark::Done, format!("Reached {}", self.conn.host), None);
                self.host_key = Some(key.clone());
                self.phase = Phase::ConfirmHost(key);
            }
            StepReport::Reached(Err(e)) => self.phase = Phase::Unreached(e.message),
            StepReport::Probed(Ok(ProbeOutcome::AccessInPlace)) => {
                self.line(StepMark::Done, "Signed in without a password · access was already in place", None);
                self.phase = Phase::Ready(SetupRecord::AlreadyInPlace { date: self.today.clone() });
            }
            StepReport::Probed(Ok(ProbeOutcome::SignInNeeded)) => {
                self.line(StepMark::Hollow, "Tried signing in without a password · not available", None);
                self.phase = Phase::SignIn { error: None };
            }
            StepReport::Probed(Err(e)) => self.phase = Phase::Unreached(e.message),
            StepReport::SignedIn(Ok(())) => {
                let with = match self.method {
                    SignInMethod::Password => "your password",
                    SignInMethod::KeyFile => "your key file",
                };
                self.line(StepMark::Done, format!("Signed in with {with}"), None);
                self.phase = Phase::Working;
                backend.install_key(&self.conn);
            }
            // Nothing was changed on the server: show the SSH error and let the user try again.
            StepReport::SignedIn(Err(e)) => self.phase = Phase::SignIn { error: Some(e.message) },
            StepReport::KeyInstalled(Ok(key)) => {
                self.line(StepMark::Done, format!("Installed YAAC's key for {}", self.conn.user), None);
                self.installed = Some(key.clone());
                self.phase = Phase::Working;
                backend.verify(&self.conn, &key);
            }
            StepReport::KeyInstalled(Err(e)) => {
                self.line(StepMark::Failed, format!("Installed YAAC's key for {}", self.conn.user), Some(e.message));
                self.phase = Phase::Failed { cleaned_up: false };
            }
            StepReport::Verified(Ok(())) => {
                self.line(StepMark::Done, "Verified: signed in with the key alone", None);
                let key = self.installed.take();
                self.phase = Phase::Ready(match key {
                    Some(key) => SetupRecord::InstalledByYaac { key, date: self.today.clone() },
                    None => SetupRecord::AlreadyInPlace { date: self.today.clone() },
                });
            }
            StepReport::Verified(Err(e)) => {
                self.line(StepMark::Failed, "Verified: signed in with the key alone", Some(e.message));
                match self.installed.clone() {
                    Some(key) => {
                        self.phase = Phase::Working;
                        backend.remove_key(&self.conn, &key);
                    }
                    None => self.phase = Phase::Failed { cleaned_up: false },
                }
            }
            StepReport::KeyRemoved(Ok(())) => {
                self.installed = None;
                self.line(StepMark::Done, "Removed the key again · confirmed: nothing of YAAC's is left on the server.", None);
                self.phase = Phase::Failed { cleaned_up: true };
            }
            StepReport::KeyRemoved(Err(e)) => {
                self.line(StepMark::Failed, "Removing the key again", Some(e.message));
                self.phase = Phase::Failed { cleaned_up: false };
            }
            StepReport::Revoked(_) => {}
        }
    }

    /// The key that is still on the server after a failure whose cleanup did not finish.
    pub fn leftover(&self) -> Option<&InstalledKey> {
        match self.phase {
            Phase::Failed { cleaned_up: false } => self.installed.as_ref(),
            _ => None,
        }
    }

    /// The host key being confirmed (screen 14).
    pub fn host_key(&self) -> Option<&HostKey> {
        self.host_key.as_ref()
    }
}

/// Revoking what YAAC installed on one server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevokePhase {
    /// Removing the line.
    Working,
    /// Removed: "Only YAAC's own key was removed. Your other keys were not touched."
    Revoked,
    /// The server could not be reached: the key is still there. YAAC keeps its copy of the
    /// key so it can finish later.
    CouldNotRevoke {
        /// Why.
        reason: String,
    },
}

/// A revoke in progress or finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revoke {
    /// The connection.
    pub conn: SshConn,
    /// The line being removed (shown in full when it could not be).
    pub key: InstalledKey,
    /// Where it is.
    pub phase: RevokePhase,
    today: String,
}

impl Revoke {
    /// Start revoking `key` on `conn`. The caller has closed the target's live connection
    /// first (the hub does, in the real backend).
    pub fn start(conn: SshConn, key: InstalledKey, now: i64, backend: &mut dyn SshSetupBackend) -> Self {
        backend.revoke(&conn, &key);
        Self { conn, key, phase: RevokePhase::Working, today: crate::time::format_rfc3339(now).chars().take(10).collect() }
    }

    /// Date the record in the host's local time zone (D91).
    pub fn in_zone(mut self, now: i64, zone: crate::time::Zone) -> Self {
        self.today = crate::time::date_in(now, zone);
        self
    }

    /// "Try again".
    pub fn retry(&mut self, backend: &mut dyn SshSetupBackend) {
        if matches!(self.phase, RevokePhase::CouldNotRevoke { .. }) {
            self.phase = RevokePhase::Working;
            backend.revoke(&self.conn, &self.key);
        }
    }

    /// Apply the backend's answer. Returns the record the connection now has, when it changed.
    pub fn pump(&mut self, backend: &mut dyn SshSetupBackend) -> Option<SetupRecord> {
        let mut out = None;
        while let Some(report) = backend.poll() {
            if let StepReport::Revoked(r) = report {
                match r {
                    Ok(()) => {
                        self.phase = RevokePhase::Revoked;
                        out = Some(SetupRecord::Revoked { date: self.today.clone() });
                    }
                    Err(e) => self.phase = RevokePhase::CouldNotRevoke { reason: e.message },
                }
            }
        }
        out
    }

    /// "I removed it myself": the user confirmed the line is gone; YAAC deletes its local copy
    /// of the key. Returns the new record.
    pub fn removed_by_hand(&mut self) -> SetupRecord {
        self.phase = RevokePhase::Revoked;
        SetupRecord::RemovedByHand { date: self.today.clone() }
    }
}
