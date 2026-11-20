//! Stage 0, the host side (docs/02-architecture.md#stage-0): drive the tiny shell program on
//! a remote target over the one stream until it hands over to `yaacd connect`, then return the
//! stream positioned at the first byte of the YAAC protocol.
//!
//! ```text
//! target → host   <noise, ignored up to the marker>
//! target → host   YAAC0 os=… arch=… libc=… runtime=<node-V|none> daemon=<builds|none> serving=<build|none|unknown> found=<V|none>
//! host → target   PUT daemon <build> <bytes> <sha256>  + exactly <bytes> bytes   (only if missing)
//! target → host   OK daemon | ERR <reason>
//! host → target   RUN node <build>
//! target → host   OK run | ERR <reason>
//!                 ── from the next byte on, the stream is the YAAC protocol ──
//! ```
//!
//! Transport-independent: the WSL connector uses it now, SSH will. The program itself is
//! [`STAGE0`]; [`stage0_argv`] is how a transport runs it (`sh -c <program> yaac-stage0
//! [YAAC_NAME=value …]`, no login shell, so no profile runs).
//!
//! SEAM(private-runtime): the runtime is a Node.js 22+ already installed for the target user
//! (`runtime=node-<version>`). The `GET runtime` / `PUT runtime` steps for YAAC's own pinned
//! runtime (D17, D25) come with the runtime itself; until then a target without Node.js 22
//! fails with [`ConnectError::NoRuntime`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::ConnectError;

/// The stage 0 program (POSIX sh).
pub const STAGE0: &str = include_str!("stage0.sh");

/// The oldest Node.js major version the daemon runs on (`engines` in `daemon/package.json`).
pub const MIN_NODE_MAJOR: u32 = 22;

/// How much start-up noise is tolerated before the marker.
const MAX_NOISE: usize = 64 * 1024;
/// The longest reply line.
const MAX_LINE: usize = 4096;

/// The `argv` that runs stage 0 on a POSIX target, exporting `env` (names must start with
/// `YAAC_`; others are ignored by the program) before anything else.
pub fn stage0_argv(env: &[(String, String)]) -> Vec<String> {
    let mut argv = vec!["sh".to_owned(), "-c".to_owned(), STAGE0.to_owned(), "yaac-stage0".to_owned()];
    argv.extend(env.iter().map(|(k, v)| format!("{k}={v}")));
    argv
}

/// The daemon bundle this host installs: one JavaScript file and its build id.
#[derive(Clone)]
pub struct DaemonBundle {
    /// The install folder name on the target, `~/.yaac/daemon/<build>/`.
    pub build: String,
    /// `yaacd.mjs`.
    pub bytes: Arc<[u8]>,
    /// Lower-case hex SHA-256 of `bytes`.
    pub sha256: String,
}

impl std::fmt::Debug for DaemonBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonBundle").field("build", &self.build).field("bytes", &self.bytes.len()).field("sha256", &self.sha256).finish()
    }
}

impl DaemonBundle {
    /// A bundle whose build id is `<version>-<first 16 hex of its SHA-256>`, so two different
    /// bundles never share an install folder even when their version is the same.
    pub fn new(version: &str, bytes: impl Into<Arc<[u8]>>) -> Self {
        let bytes = bytes.into();
        let sha256 = sha256_hex(&bytes);
        let version: String = version.chars().map(|c| if c.is_ascii_alphanumeric() || ".+-_".contains(c) { c } else { '_' }).collect();
        Self { build: format!("{version}-{}", &sha256[..16]), bytes, sha256 }
    }
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The marker line's `key=value` tokens. Unknown keys are kept and ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stage0Report {
    /// Every token.
    pub fields: BTreeMap<String, String>,
}

impl Stage0Report {
    /// Parses the text after `YAAC0 `.
    pub fn parse(line: &str) -> Self {
        let fields = line
            .split_ascii_whitespace()
            .filter_map(|t| t.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        Self { fields }
    }

    /// A field, `None` when absent or `none`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str).filter(|v| *v != "none")
    }

    /// Installed daemon builds.
    pub fn daemons(&self) -> Vec<&str> {
        self.get("daemon").map(|d| d.split(',').filter(|b| !b.is_empty()).collect()).unwrap_or_default()
    }
}

/// Timings for one bootstrap.
#[derive(Clone, Copy, Debug)]
pub struct Stage0Timeouts {
    /// For the marker (a cold WSL distro can take several seconds to boot).
    pub marker: Duration,
    /// For each reply.
    pub reply: Duration,
}

impl Default for Stage0Timeouts {
    fn default() -> Self {
        Self { marker: Duration::from_secs(60), reply: Duration::from_secs(120) }
    }
}

/// What a finished bootstrap hands back: the stream at the first protocol byte.
pub struct Bootstrapped<R, W> {
    /// The target's report.
    pub report: Stage0Report,
    /// Whether the daemon was uploaded on this connection.
    pub uploaded: bool,
    /// Bytes from the target, starting at the first protocol byte.
    pub reader: tokio::io::Chain<std::io::Cursor<Vec<u8>>, R>,
    /// Bytes to the target.
    pub writer: W,
}

impl<R, W> std::fmt::Debug for Bootstrapped<R, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bootstrapped").field("report", &self.report).field("uploaded", &self.uploaded).finish_non_exhaustive()
    }
}

struct Lines<R> {
    reader: R,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> Lines<R> {
    /// The next `\n`-terminated line (without it), reading at most `limit` bytes for it.
    async fn line(&mut self, limit: usize) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=i).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
            if self.buf.len() > limit {
                return Ok(None);
            }
            let mut chunk = [0u8; 8192];
            let n = self.reader.read(&mut chunk).await?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

/// What the target printed, for an error message: decoded (Windows tools print UTF-16LE),
/// trimmed, and cut to its last 600 characters.
fn shown(bytes: &[u8]) -> String {
    let text = crate::wsl::console_text(bytes).replace('\0', "");
    let text = text.trim();
    let count = text.chars().count();
    if count > 600 { format!("…{}", text.chars().skip(count - 600).collect::<String>()) } else { text.to_owned() }
}

/// Runs stage 0 to completion. `what` names the target in error messages ("WSL distro
/// Ubuntu"); `bundle` is installed when the target lacks its build.
pub async fn bootstrap<R, W>(
    reader: R,
    mut writer: W,
    bundle: &DaemonBundle,
    what: &str,
    timeouts: Stage0Timeouts,
) -> Result<Bootstrapped<R, W>, ConnectError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = Lines { reader, buf: Vec::new() };
    let io = |e: std::io::Error, step: &str| ConnectError::Other(format!("{what}: {step}: {e}"));

    // 1. Discard noise up to the marker, keeping it to show if the marker never comes.
    let mut noise = Vec::new();
    let report = tokio::time::timeout(timeouts.marker, async {
        loop {
            match lines.line(MAX_NOISE).await {
                Ok(Some(line)) => {
                    if let Some(rest) = line.strip_prefix(b"YAAC0 ") {
                        return Ok(Stage0Report::parse(&String::from_utf8_lossy(rest)));
                    }
                    noise.extend_from_slice(&line);
                    noise.push(b'\n');
                    if noise.len() > MAX_NOISE {
                        return Err(ConnectError::Other(format!("{what}: no YAAC0 marker in the first {MAX_NOISE} bytes; got: {}", shown(&noise))));
                    }
                }
                Ok(None) => return Err(ConnectError::Other(format!("{what}: no YAAC0 marker; got: {}", shown(&lines.buf)))),
                Err(e) => {
                    noise.extend_from_slice(&lines.buf);
                    let got = if noise.is_empty() { String::new() } else { format!("; it printed: {}", shown(&noise)) };
                    return Err(ConnectError::Other(format!("{what}: the bootstrap ended before reporting ({e}){got}")));
                }
            }
        }
    })
    .await
    .map_err(|_| ConnectError::Other(format!("{what}: no YAAC0 marker within {:?}; got: {}", timeouts.marker, shown(&noise))))??;

    // 2. The runtime. SEAM(private-runtime): only a system Node.js 22+ for now.
    let Some(runtime) = report.get("runtime").filter(|r| r.starts_with("node-")) else {
        let found = report.get("found").map_or_else(
            || "no Node.js was found".to_owned(),
            |v| format!("only Node.js {v} was found"),
        );
        return Err(ConnectError::NoRuntime(format!(
            "{what}: YAAC needs Node.js {MIN_NODE_MAJOR} or newer on the target for now, and {found} (looked on PATH, in ~/.nvm, ~/.volta, fnm, asdf, /usr/local/bin and /usr/bin; set YAAC_NODE to point at one)"
        )));
    };
    let _ = runtime;

    // 3. The daemon, in-band, only if this build is missing.
    let mut uploaded = false;
    if !report.daemons().contains(&bundle.build.as_str()) {
        let header = format!("PUT daemon {} {} {}\n", bundle.build, bundle.bytes.len(), bundle.sha256);
        writer.write_all(header.as_bytes()).await.map_err(|e| io(e, "sending the daemon"))?;
        writer.write_all(&bundle.bytes).await.map_err(|e| io(e, "sending the daemon"))?;
        writer.flush().await.map_err(|e| io(e, "sending the daemon"))?;
        expect(&mut lines, "OK daemon", what, "installing the daemon", timeouts.reply).await?;
        uploaded = true;
    }

    // 4. Hand over.
    writer.write_all(format!("RUN node {}\n", bundle.build).as_bytes()).await.map_err(|e| io(e, "starting the daemon"))?;
    writer.flush().await.map_err(|e| io(e, "starting the daemon"))?;
    expect(&mut lines, "OK run", what, "starting the daemon", timeouts.reply).await?;

    let Lines { reader, buf } = lines;
    Ok(Bootstrapped { report, uploaded, reader: std::io::Cursor::new(buf).chain(reader), writer })
}

async fn expect<R: AsyncRead + Unpin>(lines: &mut Lines<R>, ok: &str, what: &str, step: &str, timeout: Duration) -> Result<(), ConnectError> {
    let line = tokio::time::timeout(timeout, lines.line(MAX_LINE))
        .await
        .map_err(|_| ConnectError::Other(format!("{what}: {step}: no answer within {timeout:?}")))?
        .map_err(|e| ConnectError::Other(format!("{what}: {step}: the target closed the stream ({e})")))?
        .ok_or_else(|| ConnectError::Other(format!("{what}: {step}: an over-long reply")))?;
    let line = String::from_utf8_lossy(&line);
    if line == ok {
        return Ok(());
    }
    match line.strip_prefix("ERR ") {
        Some(reason) => Err(ConnectError::Other(format!("{what}: {step}: {reason}"))),
        None => Err(ConnectError::Other(format!("{what}: {step}: unexpected reply {:?}", shown(line.as_bytes())))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_line_is_key_value_tokens_and_unknown_keys_are_kept() {
        let r = Stage0Report::parse("os=Linux arch=x86_64 libc=glibc runtime=node-22.1.0 daemon=a,b serving=none found=none later=1");
        assert_eq!(r.get("os"), Some("Linux"));
        assert_eq!(r.daemons(), vec!["a", "b"]);
        assert_eq!(r.get("serving"), None);
        assert_eq!(r.get("later"), Some("1"));
        assert!(Stage0Report::parse("daemon=none").daemons().is_empty());
    }

    #[test]
    fn a_bundle_build_carries_its_hash() {
        let b = DaemonBundle::new("0.1.0", b"hello\n".to_vec());
        assert_eq!(b.sha256, "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03");
        assert_eq!(b.build, "0.1.0-5891b5b522d5df08");
        assert_eq!(DaemonBundle::new("1 2/3", b"x".to_vec()).build.split('-').next(), Some("1_2_3"));
    }
}
