//! The WSL connector (docs/02-architecture.md#wsl): one long-lived
//! `wsl.exe -d <distro> [-u <user>] --exec sh -c <stage 0> yaac-stage0 [YAAC_NAME=value …]`
//! child per connection. Its standard input and output are the stream; it gets no console
//! window; dropping the stream kills it, which closes the bridge's standard input inside the
//! distro (the daemon itself keeps running, as on every target).
//!
//! A stopped distro is never started behind the user's back (D35). Before spawning, the
//! connector asks `wsl.exe -l --running -q`, which starts nothing, and returns
//! [`ConnectError::TargetDown`] if the distro is not running — unless the user asked to
//! connect now ([`ConnectCtx::allow_start`]).
//!
//! `wsl.exe -l` prints UTF-16LE (UTF-8 when `WSL_UTF8=1`); [`console_text`] decodes either.
//! Its column headings and state words are translated with the Windows display language, so
//! whether a distro runs is read from the untranslated `--running -q` name list, and
//! [`Distro::state`] is only for display.

use std::ffi::{OsStr, OsString};
use std::process::Stdio;

use tokio::process::Command;

use crate::bootstrap::{DaemonBundle, Stage0Timeouts, bootstrap_child, stage0_argv};
#[cfg(windows)]
use crate::CREATE_NO_WINDOW;
use crate::runtime::RuntimeSource;
use crate::{BoxFuture, ConnectCtx, ConnectError, Connector, Stream};

/// Decodes console output that may be UTF-16LE (what `wsl.exe` prints by default) or UTF-8.
pub fn console_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xFF, 0xFE]).unwrap_or(bytes);
    let odd_zeros = bytes.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
    let looks_utf16 = bytes.len() >= 2 && odd_zeros * 2 >= bytes.len() / 2;
    if looks_utf16 {
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// A distro as `wsl.exe -l -v` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Distro {
    /// Its name.
    pub name: String,
    /// The default distro (`*`).
    pub default: bool,
    /// The state word as printed (translated; for display only).
    pub state: String,
    /// WSL version (1 or 2), when printed.
    pub version: Option<u8>,
    /// Whether it is running (from `wsl.exe -l --running -q`, which is never translated).
    pub running: bool,
}

/// Parses `wsl.exe -l -v` text: a heading line, then `[*] <name> <state> <version>` per distro.
pub fn parse_list_verbose(text: &str) -> Vec<Distro> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\0');
            let (default, rest) = match line.strip_prefix('*') {
                Some(rest) => (true, rest.trim_start()),
                None => (false, line),
            };
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            let (version, state, name) = match tokens.as_slice() {
                [name @ .., state, version] if !name.is_empty() => (version.parse().ok(), *state, name.join(" ")),
                _ => return None,
            };
            Some(Distro { name, default, state: state.to_owned(), version, running: false })
        })
        .collect()
}

/// Parses `wsl.exe -l -q` / `-l --running -q`: one name per line.
pub fn parse_list_quiet(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

fn quiet_command(wsl_exe: &OsStr) -> Command {
    let mut c = Command::new(wsl_exe);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// The names of the running distros. Starts nothing.
pub async fn running_distros(wsl_exe: &OsStr) -> Result<Vec<String>, String> {
    let out = quiet_command(wsl_exe)
        .args(["-l", "--running", "-q"])
        .output()
        .await
        .map_err(|e| format!("cannot run {}: {e}", wsl_exe.to_string_lossy()))?;
    // With nothing running it prints a sentence and exits non-zero; that is an empty list.
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_list_quiet(&console_text(&out.stdout)))
}

/// Every installed distro with its state. Starts nothing.
pub async fn list_distros(wsl_exe: &OsStr) -> Result<Vec<Distro>, String> {
    let out = quiet_command(wsl_exe)
        .args(["-l", "-v"])
        .output()
        .await
        .map_err(|e| format!("cannot run {}: {e}", wsl_exe.to_string_lossy()))?;
    if !out.status.success() {
        let text = console_text(if out.stdout.is_empty() { &out.stderr } else { &out.stdout });
        return Err(format!("wsl.exe -l -v failed: {}", text.trim()));
    }
    let running = running_distros(wsl_exe).await?;
    let mut distros = parse_list_verbose(&console_text(&out.stdout));
    for d in &mut distros {
        d.running = running.iter().any(|r| r.eq_ignore_ascii_case(&d.name));
    }
    Ok(distros)
}

/// `wsl.exe -d <distro> [-u <user>] --exec <argv…>` with piped standard streams, no console
/// window, killed when dropped.
pub fn wsl_command(wsl_exe: &OsStr, distro: &str, user: Option<&str>, argv: &[String]) -> Command {
    let mut c = Command::new(wsl_exe);
    c.arg("-d").arg(distro);
    if let Some(user) = user {
        c.arg("-u").arg(user);
    }
    c.arg("--exec").args(argv);
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// One WSL connection.
#[derive(Clone, Debug)]
pub struct WslConfig {
    /// `wsl.exe` (a path, or a name looked up on `PATH`).
    pub wsl_exe: OsString,
    /// The distro.
    pub distro: String,
    /// The Linux user; `None` is the distro's default user.
    pub user: Option<String>,
    /// The daemon to install when the distro lacks its build. `None`: this build carries no
    /// bundle, and connecting fails with a clear message.
    pub bundle: Option<DaemonBundle>,
    /// Variables exported by stage 0 before anything runs (names must start with `YAAC_`):
    /// `YAAC_HOME` for a test home, `YAAC_NODE`, provider switches.
    pub env: Vec<(String, String)>,
    /// Stage 0 timings.
    pub timeouts: Stage0Timeouts,
    /// Where a distro with no runtime gets one (`None`: it needs a Node.js 22+ of its own).
    pub runtime: Option<RuntimeSource>,
}

impl WslConfig {
    /// A connection to `distro` as `user` (or its default user).
    pub fn new(distro: impl Into<String>, user: Option<String>, bundle: Option<DaemonBundle>) -> Self {
        Self {
            wsl_exe: OsString::from("wsl.exe"),
            distro: distro.into(),
            user: user.filter(|u| !u.is_empty()),
            bundle,
            env: Vec::new(),
            timeouts: Stage0Timeouts::default(),
            runtime: None,
        }
    }

    fn what(&self) -> String {
        match &self.user {
            Some(user) => format!("WSL distro {} (user {user})", self.distro),
            None => format!("WSL distro {}", self.distro),
        }
    }
}

/// Spawns the one `wsl.exe` child and completes stage 0. See the module docs.
#[derive(Clone, Debug)]
pub struct WslConnector {
    config: WslConfig,
}

impl WslConnector {
    /// A connector for this configuration.
    pub fn new(config: WslConfig) -> Self {
        Self { config }
    }

    /// The configuration.
    pub fn config(&self) -> &WslConfig {
        &self.config
    }

    async fn open(&self, ctx: &ConnectCtx) -> Result<Stream, ConnectError> {
        let config = &self.config;
        let what = config.what();
        let Some(bundle) = &config.bundle else {
            return Err(ConnectError::NotInstalled(
                "this build of YAAC carries no yaacd bundle to install (build it with `npm run build` in daemon/ before building YAAC)".into(),
            ));
        };
        if !ctx.allow_start {
            let running = running_distros(&config.wsl_exe).await.map_err(ConnectError::Other)?;
            if !running.iter().any(|r| r.eq_ignore_ascii_case(&config.distro)) {
                return Err(ConnectError::TargetDown(format!("WSL distro {} is not running", config.distro)));
            }
        }
        let mut env = config.env.clone();
        if let Some(name) = config.runtime.as_ref().and_then(RuntimeSource::name) {
            env.push(("YAAC_RUNTIME_VERSION".into(), name.to_owned()));
        }
        let argv = stage0_argv(&env);
        let child = wsl_command(&config.wsl_exe, &config.distro, config.user.as_deref(), &argv)
            .spawn()
            .map_err(|source| ConnectError::Spawn { what: self.describe(), source })?;
        bootstrap_child(child, &[], bundle, config.runtime.as_ref(), &what, config.timeouts, |_| None).await
    }
}

impl Connector for WslConnector {
    fn connect<'a>(&'a self, ctx: &'a ConnectCtx) -> BoxFuture<'a, Result<Stream, ConnectError>> {
        Box::pin(self.open(ctx))
    }

    fn describe(&self) -> String {
        let user = self.config.user.as_ref().map(|u| format!(" -u {u}")).unwrap_or_default();
        format!("wsl: {} -d {}{user} --exec sh -c <stage 0>", self.config.wsl_exe.to_string_lossy(), self.config.distro)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        let mut v = vec![0xFF, 0xFE];
        v.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        v
    }

    #[test]
    fn list_verbose_in_utf16_with_a_default_and_a_stopped_distro() {
        let raw = utf16("  NAME            STATE           VERSION\r\n* Ubuntu          Running         2\r\n  Debian          Stopped         2\r\n  Legacy-1        Parado          1\r\n");
        let d = parse_list_verbose(&console_text(&raw));
        assert_eq!(d.len(), 3);
        assert_eq!((d[0].name.as_str(), d[0].default, d[0].state.as_str(), d[0].version), ("Ubuntu", true, "Running", Some(2)));
        assert_eq!((d[1].name.as_str(), d[1].default), ("Debian", false));
        assert_eq!((d[2].name.as_str(), d[2].state.as_str(), d[2].version), ("Legacy-1", "Parado", Some(1)), "a translated state is kept as text");
    }

    #[test]
    fn quiet_lists_in_utf16_and_utf8() {
        assert_eq!(parse_list_quiet(&console_text(&utf16("Ubuntu\r\nDebian\r\n"))), vec!["Ubuntu", "Debian"]);
        assert_eq!(parse_list_quiet(&console_text(b"Ubuntu\nDebian\n")), vec!["Ubuntu", "Debian"]);
        assert!(parse_list_quiet(&console_text(&utf16("\r\n"))).is_empty());
    }

    #[test]
    fn the_command_line() {
        let argv = vec!["sh".to_owned(), "-c".to_owned(), "true".to_owned()];
        let c = wsl_command(OsStr::new("wsl.exe"), "Ubuntu", Some("dev"), &argv);
        let args: Vec<_> = c.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["-d", "Ubuntu", "-u", "dev", "--exec", "sh", "-c", "true"]);
    }
}
