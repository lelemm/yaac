//! SSH targets in the hub (docs/02-architecture.md#ssh): the connector for each SSH target,
//! and Setup, Verify, and Revoke (docs/02-architecture.md#setting-up-an-ssh-connection).
//!
//! | Part | What |
//! | --- | --- |
//! | [`SshDefaults`] | Where YAAC's SSH files are, which programs run, and the [`Gate`]. |
//! | [`Ident`] | A server: *user@host:port*, the same for a target and for its setup. |
//! | [`Gate`] | Setup sessions run only while the server has no live connection, and a connection is not opened while a setup session runs (invariant #1). |
//! | [`askpass`] | The password or passphrase reaches `ssh` through `SSH_ASKPASS` = this program, over a one-shot user-only local socket; never through a file. |
//! | [`setup`] | The steps: reach, probe, sign in and install, verify, remove, revoke. |
//!
//! Files, all user-only, under `<data>/ssh/`:
//!
//! | File | What |
//! | --- | --- |
//! | `known_hosts` | Host keys the user confirmed during Setup (checked before the user's own files). |
//! | `keys/<server>/id_ed25519` (+ `.pub`) | YAAC's own key for one server, generated during Setup, deleted by Revoke. |
//!
//! What YAAC did on each server (the record) is in `hub.json` (`ssh`), and on the server in
//! `~/.yaac/ssh/<id>` beside the one line in `authorized_keys`.

pub mod askpass;
pub(crate) mod setup;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use yaac_protocol::attach::SshTransport;
use yaac_transport::{BoxFuture, ConnectCtx, ConnectError, Connector, DaemonBundle, RuntimeSource, SshConfig, SshConnector, SshTarget, Stream};

use crate::protocol::{SshConnParams, SshRecord, SshRecordKind};

/// A server: host (trimmed), port (22 when not given), user (empty: the `ssh` default).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ident {
    /// Host or alias.
    pub host: String,
    /// Port.
    pub port: u16,
    /// User, or empty.
    pub user: String,
}

impl Ident {
    /// From the setup methods' params.
    pub fn from_params(p: &SshConnParams) -> Self {
        Self::new(&p.host, p.port, p.user.as_deref())
    }

    /// From a target's transport.
    pub fn from_transport(t: &SshTransport) -> Self {
        Self::new(&t.host, t.port, t.user.as_deref())
    }

    fn new(host: &str, port: Option<u16>, user: Option<&str>) -> Self {
        Self { host: host.trim().to_owned(), port: port.unwrap_or(yaac_transport::ssh::DEFAULT_PORT), user: user.unwrap_or("").trim().to_owned() }
    }

    /// As the setup methods name it.
    pub fn params(&self) -> SshConnParams {
        SshConnParams { host: self.host.clone(), port: Some(self.port), user: (!self.user.is_empty()).then(|| self.user.clone()) }
    }

    /// `user@host:port`.
    pub fn label(&self) -> String {
        let user = if self.user.is_empty() { String::new() } else { format!("{}@", self.user) };
        format!("{user}{}:{}", self.host, self.port)
    }

    /// A file-name-safe, collision-free folder name for this server.
    pub fn slug(&self) -> String {
        let label = self.label();
        let safe: String = label.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_@".contains(c) { c } else { '_' }).take(60).collect();
        // FNV-1a of the exact label: two labels that sanitise alike still differ.
        let mut h: u32 = 0x811c_9dc5;
        for b in label.bytes() {
            h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
        format!("{safe}-{h:08x}")
    }

    /// Whether a host name is safe to hand to `ssh` (it must not look like an option).
    pub fn valid(&self) -> bool {
        !self.host.is_empty() && !self.host.starts_with('-') && !self.host.contains(char::is_whitespace) && !self.user.starts_with('-') && !self.user.contains(char::is_whitespace)
    }
}

#[derive(Debug, Default)]
struct GateState {
    live: u32,
    setup: bool,
}

/// Who is using a server's SSH access right now: live connections (and connection attempts)
/// or one setup step. Never both.
#[derive(Debug, Default)]
pub struct Gate {
    state: Mutex<HashMap<Ident, GateState>>,
}

/// Held while a connection to a server is open or being opened.
#[derive(Debug)]
pub(crate) struct LiveToken {
    gate: Arc<Gate>,
    ident: Ident,
}

impl Drop for LiveToken {
    fn drop(&mut self) {
        if let Ok(mut s) = self.gate.state.lock()
            && let Some(g) = s.get_mut(&self.ident)
        {
            g.live = g.live.saturating_sub(1);
        }
    }
}

/// Held while a setup step's sessions run.
#[derive(Debug)]
pub(crate) struct SetupToken {
    gate: Arc<Gate>,
    ident: Ident,
}

impl Drop for SetupToken {
    fn drop(&mut self) {
        if let Ok(mut s) = self.gate.state.lock()
            && let Some(g) = s.get_mut(&self.ident)
        {
            g.setup = false;
        }
    }
}

impl Gate {
    fn live(self: &Arc<Self>, ident: &Ident) -> Option<LiveToken> {
        let mut s = self.state.lock().ok()?;
        let g = s.entry(ident.clone()).or_default();
        if g.setup {
            return None;
        }
        g.live += 1;
        Some(LiveToken { gate: self.clone(), ident: ident.clone() })
    }

    /// A setup token, if the server has no live connection and no other setup step.
    pub(crate) fn setup(self: &Arc<Self>, ident: &Ident) -> Option<SetupToken> {
        let mut s = self.state.lock().ok()?;
        let g = s.entry(ident.clone()).or_default();
        if g.setup || g.live > 0 {
            return None;
        }
        g.setup = true;
        Some(SetupToken { gate: self.clone(), ident: ident.clone() })
    }

    /// A setup token, waiting up to `wait` for a closing connection to finish closing.
    pub(crate) async fn setup_within(self: &Arc<Self>, ident: &Ident, wait: Duration) -> Option<SetupToken> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(t) = self.setup(ident) {
                return Some(t);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Whether a connection to the server is open or being opened.
    pub fn is_live(&self, ident: &Ident) -> bool {
        self.state.lock().is_ok_and(|s| s.get(ident).is_some_and(|g| g.live > 0))
    }
}

/// How the hub reaches SSH servers.
#[derive(Clone, Debug)]
pub struct SshDefaults {
    /// `ssh`.
    pub ssh_exe: OsString,
    /// `ssh-keygen` (generates YAAC's key).
    pub keygen_exe: OsString,
    /// `ssh-keyscan` (reads a host key for the user to confirm).
    pub keyscan_exe: OsString,
    /// The program `ssh` runs as `SSH_ASKPASS`. `None`: this executable (`yaac` answers as
    /// the askpass helper when `YAAC_ASKPASS` is set; see [`askpass::client_main`]).
    pub askpass_exe: Option<PathBuf>,
    /// `<data>/ssh`: YAAC's known-hosts file and keys.
    pub dir: PathBuf,
    /// Where the one-shot askpass sockets are made (the hub's private runtime directory).
    pub socket_dir: PathBuf,
    /// The `authorized_keys` file on servers, when not `~/.ssh/authorized_keys` (tests).
    pub authorized_keys: Option<String>,
    /// Extra `-o` options for every session (tests).
    pub options: Vec<String>,
    /// The POSIX shell that runs scripts on servers.
    pub shell: String,
    /// This machine's name, in the comment of YAAC's key (`yaac:<machine>:<id>`).
    pub machine: String,
    /// The daemon to install on servers that lack its build.
    pub bundle: Option<DaemonBundle>,
    /// Variables for stage 0 on the server (`YAAC_` names; `YAAC_HOME` also moves YAAC's
    /// record on the server).
    pub env: Vec<(String, String)>,
    /// Where a server with no runtime gets one.
    pub runtime: Option<RuntimeSource>,
    /// Who is using each server's access.
    pub gate: Arc<Gate>,
}

fn exe(name: &str) -> OsString {
    OsString::from(if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() })
}

impl SshDefaults {
    /// The defaults for a hub whose data directory is `data_dir` and runtime directory
    /// `runtime_dir`, using the system OpenSSH client.
    pub fn new(data_dir: &Path, runtime_dir: &Path, bundle: Option<DaemonBundle>, env: Vec<(String, String)>) -> Self {
        Self {
            ssh_exe: exe("ssh"),
            keygen_exe: exe("ssh-keygen"),
            keyscan_exe: exe("ssh-keyscan"),
            askpass_exe: None,
            dir: data_dir.join("ssh"),
            socket_dir: runtime_dir.to_owned(),
            authorized_keys: None,
            options: Vec::new(),
            shell: "sh".into(),
            machine: crate::config::host_name(),
            bundle,
            env,
            runtime: Some(RuntimeSource::pinned(Some(data_dir.join("runtime-cache")))),
            gate: Arc::new(Gate::default()),
        }
    }

    /// YAAC's known-hosts file.
    pub fn known_hosts(&self) -> PathBuf {
        self.dir.join("known_hosts")
    }

    /// YAAC's private key for a server.
    pub fn key(&self, ident: &Ident) -> PathBuf {
        self.dir.join("keys").join(ident.slug()).join("id_ed25519")
    }

    /// The `ssh` target for a server, with YAAC's known-hosts file.
    pub fn target(&self, ident: &Ident) -> SshTarget {
        let mut t = SshTarget::new(&ident.host, Some(ident.port), (!ident.user.is_empty()).then(|| ident.user.clone()));
        t.ssh_exe.clone_from(&self.ssh_exe);
        t.known_hosts = Some(self.known_hosts());
        t.options.clone_from(&self.options);
        t
    }

    /// `ssh` arguments that run `remote` on a server as the hub would reach it: YAAC's own key
    /// for it when that key is here (else the user's `ssh` defaults), YAAC's known hosts, never
    /// a prompt. The PC's window runs `yaac hub-bridge` this way on a server the hub moved to.
    pub fn command_args(&self, transport: &SshTransport, remote: &str) -> Vec<OsString> {
        let ident = Ident::from_transport(transport);
        let key = self.key(&ident);
        let auth = if key.is_file() { yaac_transport::ssh::SshAuth::Key(&key) } else { yaac_transport::ssh::SshAuth::Default };
        let mut args = self.target(&ident).args(auth);
        args.push(remote.into());
        args
    }

    /// The `YAAC_HOME` the server's daemon uses, when moved (tests).
    pub(crate) fn target_home(&self) -> Option<&str> {
        self.env.iter().find(|(k, _)| k == "YAAC_HOME").map(|(_, v)| v.as_str())
    }

    /// The connector for an SSH target: YAAC's own key when YAAC set the server up (and the
    /// key is there), else the user's `ssh` defaults.
    pub fn connector(&self, transport: &SshTransport, record: Option<&SshRecord>) -> Arc<dyn Connector> {
        let ident = Ident::from_transport(transport);
        let key = self.key(&ident);
        let mut config = SshConfig::new(self.target(&ident), self.bundle.clone());
        if record.is_some_and(|r| r.kind == SshRecordKind::InstalledByYaac) && key.is_file() {
            config.identity = Some(key);
        }
        config.shell.clone_from(&self.shell);
        config.env.clone_from(&self.env);
        config.runtime.clone_from(&self.runtime);
        Arc::new(GatedConnector { inner: SshConnector::new(config), gate: self.gate.clone(), ident })
    }
}

/// An SSH connector that takes a [`LiveToken`] for as long as its stream lives, so no setup
/// session runs beside it.
struct GatedConnector {
    inner: SshConnector,
    gate: Arc<Gate>,
    ident: Ident,
}

impl Connector for GatedConnector {
    fn connect<'a>(&'a self, ctx: &'a ConnectCtx) -> BoxFuture<'a, Result<Stream, ConnectError>> {
        Box::pin(async move {
            let Some(token) = self.gate.live(&self.ident) else {
                return Err(ConnectError::Other(format!("SSH setup is running for {}; connecting once it finishes", self.ident.label())));
            };
            let mut stream = self.inner.connect(ctx).await?;
            stream.child = std::mem::take(&mut stream.child).and_on_drop(move || drop(token));
            Ok(stream)
        })
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// A user-only directory (created if missing).
pub(crate) fn private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_and_its_setup_name_the_same_server() {
        let t = SshTransport { host: " devbox ".into(), port: None, user: Some("dev".into()) };
        let p = SshConnParams { host: "devbox".into(), port: Some(22), user: Some("dev".into()) };
        assert_eq!(Ident::from_transport(&t), Ident::from_params(&p));
        let other = SshConnParams { host: "devbox".into(), port: Some(22), user: None };
        assert_ne!(Ident::from_transport(&t), Ident::from_params(&other), "another user is another target");
        assert_eq!(Ident::from_params(&p).label(), "dev@devbox:22");
    }

    #[test]
    fn slugs_are_safe_and_distinct() {
        let a = Ident { host: "a/b".into(), port: 22, user: String::new() };
        let b = Ident { host: "a_b".into(), port: 22, user: String::new() };
        assert!(a.slug().chars().all(|c| c.is_ascii_alphanumeric() || ".-_@".contains(c)));
        assert_ne!(a.slug(), b.slug());
        assert!(!Ident { host: "-oProxyCommand=x".into(), port: 22, user: String::new() }.valid());
    }

    #[test]
    fn setup_and_live_connections_exclude_each_other() {
        let gate = Arc::new(Gate::default());
        let id = Ident { host: "h".into(), port: 22, user: String::new() };
        let live = gate.live(&id).unwrap();
        assert!(gate.setup(&id).is_none(), "no setup beside a live connection");
        drop(live);
        let setup = gate.setup(&id).unwrap();
        assert!(gate.live(&id).is_none(), "no connection beside a setup step");
        assert!(gate.setup(&id).is_none(), "one setup step at a time");
        drop(setup);
        assert!(gate.live(&id).is_some());
    }
}
