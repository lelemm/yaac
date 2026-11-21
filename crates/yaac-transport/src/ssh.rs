//! The SSH connector (docs/02-architecture.md#ssh): one long-lived system OpenSSH client
//! (`ssh`, `ssh.exe`) per connection, its standard input and output the stream:
//!
//! ```text
//! ssh -T -o BatchMode=yes -o ControlMaster=no -o ControlPath=none -o EscapeChar=none …
//!     [-i <YAAC's key> -o IdentitiesOnly=yes] [-p <port>] [-l <user>] -- <host>
//!     "exec sh -c 'eval \"$(dd bs=1 count=<n> 2>/dev/null)\"' yaac-stage0 'YAAC_…=…'"
//! ```
//!
//! * **One session, one exec channel, no PTY** (`-T`), no escape character, no connection
//!   sharing (`ControlMaster=no`, `ControlPath=none`): nothing else rides on, or is opened
//!   beside, this one connection.
//! * **Never prompts.** `BatchMode=yes`: a connection that needs a password or an unknown host
//!   key fails with a clear message; prompts belong to Setup, in the hub.
//! * **Host keys.** `StrictHostKeyChecking=yes` against YAAC's own known-hosts file (the keys
//!   the user confirmed during Setup) followed by the user's own files, so a key that changes
//!   stops the connection and is never accepted on YAAC's own.
//! * **Stage 0 travels on the stream.** The remote command line passes through the user's
//!   login shell, whatever it is; it is kept to one line of quoted words, and stage 0 itself
//!   ([`crate::bootstrap::STAGE0`]) is the first thing written on standard input.
//!
//! Reconnection is the hub's supervisor's (capped backoff), as for every transport.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::bootstrap::{DaemonBundle, STAGE0, Stage0Timeouts, bootstrap_child, stage0_remote_command};
#[cfg(windows)]
use crate::CREATE_NO_WINDOW;
use crate::runtime::RuntimeSource;
use crate::{BoxFuture, ConnectCtx, ConnectError, Connector, Stream};

/// The default SSH port.
pub const DEFAULT_PORT: u16 = 22;

/// A server as the system `ssh` reaches it: *user@host:port* plus YAAC's fixed options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshTarget {
    /// `ssh` (a path, or a name looked up on `PATH`).
    pub ssh_exe: OsString,
    /// Host name or `~/.ssh/config` alias.
    pub host: String,
    /// Port, when not the default (or the alias's).
    pub port: Option<u16>,
    /// User, when not the default (or the alias's).
    pub user: Option<String>,
    /// YAAC's own known-hosts file, checked before the user's own files. `None`: only the
    /// user's files (and the `ssh` defaults).
    pub known_hosts: Option<PathBuf>,
    /// Extra `-o` options (tests: `-F none`-style isolation is done with these).
    pub options: Vec<String>,
}

/// How a session signs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshAuth<'a> {
    /// Whatever the user's `ssh` is set up with (agent, `~/.ssh/config`, default keys); never
    /// prompts.
    Default,
    /// YAAC's own key and nothing else; never prompts.
    Key(&'a Path),
    /// A password, asked through `SSH_ASKPASS` (the caller sets the environment).
    Password,
    /// A key file the user chose, its passphrase (if any) asked through `SSH_ASKPASS`.
    KeyFile(&'a Path),
}

/// The text of a path for an `ssh` option value (forward slashes on Windows, which `ssh.exe`
/// accepts and its option parser leaves alone), in double quotes.
fn quoted_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    let text = text.replace('\\', "/");
    format!("\"{text}\"")
}

impl SshTarget {
    /// A target with YAAC's defaults.
    pub fn new(host: impl Into<String>, port: Option<u16>, user: Option<String>) -> Self {
        Self {
            ssh_exe: OsString::from(if cfg!(windows) { "ssh.exe" } else { "ssh" }),
            host: host.into(),
            port: port.filter(|p| *p != DEFAULT_PORT),
            user: user.filter(|u| !u.is_empty()),
            known_hosts: None,
            options: Vec::new(),
        }
    }

    /// `user@host:port` for messages.
    pub fn label(&self) -> String {
        let user = self.user.as_ref().map(|u| format!("{u}@")).unwrap_or_default();
        let port = self.port.map(|p| format!(":{p}")).unwrap_or_default();
        format!("{user}{}{port}", self.host)
    }

    /// Every argument before the remote command, ending with `-- <host>`.
    pub fn args(&self, auth: SshAuth<'_>) -> Vec<OsString> {
        let mut o: Vec<String> = vec!["-T".into()];
        let mut opt = |v: &str| {
            o.push("-o".into());
            o.push(v.to_owned());
        };
        for fixed in [
            "ControlMaster=no",
            "ControlPath=none",
            "EscapeChar=none",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=3",
            "ConnectTimeout=20",
            "StrictHostKeyChecking=yes",
            "UpdateHostKeys=no",
            "ClearAllForwardings=yes",
            "ForwardAgent=no",
            "ForwardX11=no",
            "PermitLocalCommand=no",
            "RequestTTY=no",
            "LogLevel=ERROR",
        ] {
            opt(fixed);
        }
        if let Some(file) = &self.known_hosts {
            opt(&format!("UserKnownHostsFile={} ~/.ssh/known_hosts ~/.ssh/known_hosts2", quoted_path(file)));
        }
        match auth {
            SshAuth::Default => opt("BatchMode=yes"),
            SshAuth::Key(key) => {
                opt("BatchMode=yes");
                opt("IdentitiesOnly=yes");
                opt(&format!("IdentityFile={}", quoted_path(key)));
            }
            SshAuth::Password => {
                opt("BatchMode=no");
                opt("PubkeyAuthentication=no");
                opt("PreferredAuthentications=keyboard-interactive,password");
                opt("NumberOfPasswordPrompts=1");
            }
            SshAuth::KeyFile(key) => {
                opt("BatchMode=no");
                opt("IdentitiesOnly=yes");
                opt(&format!("IdentityFile={}", quoted_path(key)));
                opt("PreferredAuthentications=publickey");
                opt("PasswordAuthentication=no");
                opt("KbdInteractiveAuthentication=no");
                opt("NumberOfPasswordPrompts=1");
            }
        }
        for extra in &self.options {
            opt(extra);
        }
        if let Some(port) = self.port {
            o.push("-p".into());
            o.push(port.to_string());
        }
        if let Some(user) = &self.user {
            o.push("-l".into());
            o.push(user.clone());
        }
        o.push("--".into());
        o.push(self.host.clone());
        o.into_iter().map(OsString::from).collect()
    }

    /// `ssh <args> <remote>` with piped standard streams, no console window, killed when
    /// dropped. The caller adds any environment (askpass).
    pub fn command(&self, auth: SshAuth<'_>, remote: &str) -> Command {
        let mut c = Command::new(&self.ssh_exe);
        c.args(self.args(auth)).arg(remote);
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        c.creation_flags(CREATE_NO_WINDOW);
        c
    }
}

/// What the system `ssh` said, in plain words, for the failures a user can act on. `None`
/// when the message is better shown as-is.
pub fn explain(stderr: &str) -> Option<String> {
    if stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") || stderr.contains("Host key for") && stderr.contains("has changed") {
        return Some(
            "the server's host key has changed. YAAC never accepts a changed key on its own: if the change is expected, remove the old key from your known_hosts and set the connection up again".into(),
        );
    }
    if stderr.contains("Host key verification failed") || stderr.contains("No ED25519 host key is known") || stderr.contains("host key is known") {
        return Some("the server's host key is not known yet: use Setup in Settings to confirm it".into());
    }
    if stderr.contains("Permission denied") {
        return Some(format!("the server refused the sign-in: password-less access is not set up (use Setup in Settings) ({})", stderr.lines().last().unwrap_or("").trim()));
    }
    None
}

/// One SSH connection.
#[derive(Clone, Debug)]
pub struct SshConfig {
    /// The server.
    pub target: SshTarget,
    /// YAAC's own key for this connection, when YAAC set it up; otherwise the user's `ssh`
    /// defaults (agent, config, keys) are used.
    pub identity: Option<PathBuf>,
    /// The POSIX shell that runs stage 0 on the server.
    pub shell: String,
    /// The daemon to install when the server lacks its build.
    pub bundle: Option<DaemonBundle>,
    /// Variables stage 0 exports before anything runs (`YAAC_` names only).
    pub env: Vec<(String, String)>,
    /// Stage 0 timings.
    pub timeouts: Stage0Timeouts,
    /// Where a server with no runtime gets one.
    pub runtime: Option<RuntimeSource>,
}

impl SshConfig {
    /// A connection to `target` with the defaults.
    pub fn new(target: SshTarget, bundle: Option<DaemonBundle>) -> Self {
        Self { target, identity: None, shell: "sh".into(), bundle, env: Vec::new(), timeouts: Stage0Timeouts::default(), runtime: None }
    }
}

/// Spawns the one `ssh` child and completes stage 0. See the module docs.
#[derive(Clone, Debug)]
pub struct SshConnector {
    config: SshConfig,
}

impl SshConnector {
    /// A connector for this configuration.
    pub fn new(config: SshConfig) -> Self {
        Self { config }
    }

    /// The configuration.
    pub fn config(&self) -> &SshConfig {
        &self.config
    }

    async fn open(&self) -> Result<Stream, ConnectError> {
        let config = &self.config;
        let what = format!("SSH server {}", config.target.label());
        let Some(bundle) = &config.bundle else {
            return Err(ConnectError::NotInstalled(
                "this build of YAAC carries no yaacd bundle to install (build it with `npm run build` in daemon/ before building YAAC)".into(),
            ));
        };
        let mut env = config.env.clone();
        if let Some(name) = config.runtime.as_ref().and_then(RuntimeSource::name) {
            env.push(("YAAC_RUNTIME_VERSION".into(), name.to_owned()));
        }
        let auth = config.identity.as_deref().map_or(SshAuth::Default, SshAuth::Key);
        let child = config
            .target
            .command(auth, &stage0_remote_command(&config.shell, &env))
            .spawn()
            .map_err(|source| ConnectError::Spawn { what: self.describe(), source })?;
        bootstrap_child(child, STAGE0.as_bytes(), bundle, config.runtime.as_ref(), &what, config.timeouts, explain).await
    }
}

impl Connector for SshConnector {
    fn connect<'a>(&'a self, _ctx: &'a ConnectCtx) -> BoxFuture<'a, Result<Stream, ConnectError>> {
        Box::pin(self.open())
    }

    fn describe(&self) -> String {
        let key = if self.config.identity.is_some() { " with YAAC's key" } else { "" };
        format!("ssh: {} {}{key} <stage 0>", self.config.target.ssh_exe.to_string_lossy(), self.config.target.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_normal_connection_never_prompts_and_never_shares() {
        let mut t = SshTarget::new("devbox", Some(2222), Some("dev".into()));
        t.known_hosts = Some(PathBuf::from("/data/ssh/known_hosts"));
        let args = strings(t.args(SshAuth::Key(Path::new("/data/ssh/keys/k/id_ed25519"))));
        for want in ["BatchMode=yes", "ControlMaster=no", "ControlPath=none", "EscapeChar=none", "StrictHostKeyChecking=yes", "IdentitiesOnly=yes"] {
            assert!(args.iter().any(|a| a == want), "{want} in {args:?}");
        }
        assert_eq!(args[0], "-T");
        assert!(args.contains(&"UserKnownHostsFile=\"/data/ssh/known_hosts\" ~/.ssh/known_hosts ~/.ssh/known_hosts2".to_owned()));
        assert_eq!(&args[args.len() - 6..], ["-p", "2222", "-l", "dev", "--", "devbox"]);
    }

    #[test]
    fn the_default_port_and_an_empty_user_are_left_to_ssh() {
        let t = SshTarget::new("h", Some(22), Some(String::new()));
        let args = strings(t.args(SshAuth::Default));
        assert!(!args.contains(&"-p".to_owned()) && !args.contains(&"-l".to_owned()));
        assert_eq!(t.label(), "h");
    }

    #[test]
    fn password_sign_in_turns_keys_off_and_allows_one_prompt() {
        let args = strings(SshTarget::new("h", None, None).args(SshAuth::Password));
        assert!(args.contains(&"BatchMode=no".to_owned()) && args.contains(&"PubkeyAuthentication=no".to_owned()));
        assert!(args.contains(&"NumberOfPasswordPrompts=1".to_owned()));
    }

    #[test]
    fn the_remote_command_is_one_line_of_quoted_words() {
        let env = vec![("YAAC_HOME".to_owned(), "/tmp/a b".to_owned()), ("YAAC_X".to_owned(), "it's".to_owned()), ("PATH".to_owned(), "/x".to_owned())];
        let cmd = stage0_remote_command("sh", &env);
        assert!(!cmd.contains('\n'));
        assert!(cmd.starts_with("exec sh -c 'eval \"$(dd bs=1 count="), "{cmd}");
        assert!(cmd.ends_with(" yaac-stage0 'YAAC_HOME=/tmp/a b'"), "a quote or a non-YAAC name is dropped: {cmd}");
    }

    #[test]
    fn plain_words_for_host_key_failures() {
        assert!(explain("@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@").unwrap().contains("never accepts a changed key"));
        assert!(explain("Host key verification failed.").unwrap().contains("Setup"));
        assert!(explain("user@h: Permission denied (publickey).").unwrap().contains("not set up"));
        assert!(explain("ssh: connect to host h port 22: Connection refused").is_none());
    }
}
