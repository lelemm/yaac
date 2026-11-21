//! The steps of SSH Setup, Verify, and Revoke (docs/02-architecture.md#setting-up-an-ssh-connection).
//! Each is a few short `ssh` (or `ssh-keyscan`, `ssh-keygen`) processes, run by the core on
//! its I/O thread while it holds the server's setup token ([`super::Gate`]).
//!
//! ```text
//! reach      ssh -G (resolve the alias) · ssh-keyscan (the host key, to confirm)
//! trust      the confirmed key → YAAC's known_hosts
//! probe      ssh BatchMode=yes "echo …"                 works → access was already in place
//! sign_in    ssh-keygen (YAAC's own key) · ssh with the password or key file → install script:
//!            appends the line, then waits for KEEP or REMOVE on its standard input
//! verify     ssh -i <YAAC's key> IdentitiesOnly=yes "echo …"  → KEEP, else REMOVE
//! revoke     ssh -i <YAAC's key> → revoke script (removes the line holding that key)
//! ```

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use yaac_transport::bootstrap::shell_quote;
use yaac_transport::ssh::SshAuth;
use zeroize::Zeroizing;

use super::askpass::Askpass;
use super::{Ident, SshDefaults};
use crate::protocol::codes;

/// The script that installs and removes YAAC's line on the server.
const REMOTE: &str = include_str!("remote.sh");

/// How long one session may take.
const STEP: Duration = Duration::from_secs(90);

/// A step failed: an error code ([`codes::SSH_FAILED`], [`codes::SSH_UNREACHABLE`]) and the
/// message to show as-is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fail {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

fn failed(message: impl Into<String>) -> Fail {
    Fail { code: codes::SSH_FAILED, message: message.into() }
}

fn unreachable(message: impl Into<String>) -> Fail {
    Fail { code: codes::SSH_UNREACHABLE, message: message.into() }
}

/// The last meaningful lines of `ssh`'s standard error.
fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('@')).collect();
    lines[lines.len().saturating_sub(3)..].join(" ")
}

/// An `ssh` failure in plain words: host-key trouble and refusals are explained, and anything
/// that means the server was never reached is marked unreachable.
fn classify(stderr: &[u8], refused: &str) -> Fail {
    let said = tail(stderr);
    let full = String::from_utf8_lossy(stderr);
    if full.contains("Permission denied") || full.contains("Too many authentication failures") {
        return failed(format!("{refused} ({said})"));
    }
    if let Some(plain) = yaac_transport::ssh::explain(&full) {
        return failed(plain);
    }
    let unreached = [
        "Could not resolve",
        "Connection refused",
        "timed out",
        "No route to host",
        "Network is unreachable",
        "Connection closed",
        "Connection reset",
        "not known",
        "kex_exchange_identification",
    ];
    if said.is_empty() || unreached.iter().any(|u| full.contains(u)) {
        return unreachable(if said.is_empty() { "the server could not be reached".to_owned() } else { said });
    }
    failed(said)
}

fn quiet(program: &std::ffi::OsStr) -> Command {
    let mut c = Command::new(program);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000);
    c
}

async fn output(mut c: Command, what: &str) -> Result<std::process::Output, Fail> {
    match tokio::time::timeout(STEP, c.output()).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(failed(format!("cannot run {what}: {e}"))),
        Err(_) => Err(unreachable(format!("{what} did not finish within {STEP:?}"))),
    }
}

/// A host key read from the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Scanned {
    /// The known-hosts name (`host`, or `[host]:port`).
    pub(crate) name: String,
    /// `ssh-ed25519`.
    pub(crate) keytype: String,
    /// The base64 key.
    pub(crate) blob: String,
    /// `ED25519`.
    pub(crate) algorithm: String,
    /// `SHA256:…`.
    pub(crate) fingerprint: String,
}

/// `SHA256:<unpadded base64 of the SHA-256 of the key>`, as `ssh-keygen -l` prints it.
pub(crate) fn fingerprint(blob: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(blob.trim()).ok()?;
    Some(format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(&raw))))
}

fn algorithm(keytype: &str) -> String {
    match keytype {
        "ssh-ed25519" => "ED25519".into(),
        t if t.starts_with("ecdsa-") => "ECDSA".into(),
        "ssh-rsa" => "RSA".into(),
        t => t.to_uppercase(),
    }
}

/// Contact the server: resolve what `ssh` would connect to (`ssh -G` honours
/// `~/.ssh/config`), then read its host keys and pick the strongest.
pub(crate) async fn reach(d: &SshDefaults, id: &Ident) -> Result<Scanned, Fail> {
    let mut g = quiet(&d.ssh_exe);
    g.arg("-G");
    for o in &d.options {
        g.arg("-o").arg(o);
    }
    g.arg("-p").arg(id.port.to_string());
    if !id.user.is_empty() {
        g.arg("-l").arg(&id.user);
    }
    g.arg("--").arg(&id.host);
    let out = output(g, "ssh -G").await?;
    if !out.status.success() {
        return Err(failed(format!("ssh does not accept this host: {}", tail(&out.stderr))));
    }
    let config = String::from_utf8_lossy(&out.stdout);
    let get = |key: &str| config.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(' '))).map(str::trim);
    let hostname = get("hostname").unwrap_or(&id.host).to_owned();
    let port: u16 = get("port").and_then(|p| p.parse().ok()).unwrap_or(id.port);
    let alias = get("hostkeyalias").filter(|a| !a.is_empty() && *a != "none").unwrap_or(&hostname).to_owned();
    if hostname.starts_with('-') {
        return Err(failed("invalid host name"));
    }
    let mut scan = quiet(&d.keyscan_exe);
    scan.args(["-T", "10", "-t", "ed25519,ecdsa,rsa", "-p"]).arg(port.to_string()).arg(&hostname);
    let out = output(scan, "ssh-keyscan").await?;
    let text = String::from_utf8_lossy(&out.stdout);
    let keys: Vec<(String, String)> = text
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (_host, t, b) = (f.next()?, f.next()?, f.next()?);
            Some((t.to_owned(), b.to_owned()))
        })
        .collect();
    let pick = ["ssh-ed25519", "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp384", "ecdsa-sha2-nistp521", "ssh-rsa"]
        .iter()
        .find_map(|want| keys.iter().find(|(t, _)| t == want))
        .or_else(|| keys.first());
    let Some((keytype, blob)) = pick.cloned() else {
        let why = tail(&out.stderr);
        return Err(unreachable(if why.is_empty() { format!("{} did not answer on port {port}", id.host) } else { why }));
    };
    let fingerprint = fingerprint(&blob).ok_or_else(|| failed("the server sent an unreadable host key"))?;
    let name = if port == 22 { alias } else { format!("[{alias}]:{port}") };
    Ok(Scanned { name, algorithm: algorithm(&keytype), keytype, blob, fingerprint })
}

/// Trust a confirmed host key: add it to YAAC's known-hosts file (never the user's).
pub(crate) fn trust(d: &SshDefaults, key: &Scanned) -> Result<(), Fail> {
    super::private_dir(&d.dir).map_err(|e| failed(format!("cannot create {}: {e}", d.dir.display())))?;
    let file = d.known_hosts();
    let line = format!("{} {} {}", key.name, key.keytype, key.blob);
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line);
    text.push('\n');
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, text).and_then(|()| crate::paths::make_private_file(&tmp)).and_then(|()| std::fs::rename(&tmp, &file)).map_err(|e| failed(format!("cannot write {}: {e}", file.display())))
}

/// The probe command: prints a marker, so a login that only seems to work does not count.
const PROBE: &str = "echo YAAC-PROBE-OK";

/// Try signing in without a password: with `key` (YAAC's own) only, or with the user's `ssh`
/// defaults. `Ok(true)`: it worked; `Ok(false)`: the server refused.
pub(crate) async fn probe(d: &SshDefaults, id: &Ident, key: Option<&Path>) -> Result<bool, Fail> {
    let auth = key.map_or(SshAuth::Default, SshAuth::Key);
    let mut c = d.target(id).command(auth, PROBE);
    c.stdin(Stdio::null());
    let out = output(c, "ssh").await?;
    if out.status.success() && String::from_utf8_lossy(&out.stdout).contains("YAAC-PROBE-OK") {
        return Ok(true);
    }
    let full = String::from_utf8_lossy(&out.stderr);
    if full.contains("Permission denied") || full.contains("Too many authentication failures") {
        return Ok(false);
    }
    Err(classify(&out.stderr, "the server refused the sign-in"))
}

/// What the user signs in with, once. Wiped when dropped.
#[derive(Default)]
pub(crate) struct Credential {
    pub(crate) password: Option<Zeroizing<String>>,
    pub(crate) key_file: Option<PathBuf>,
    pub(crate) passphrase: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential").field("key_file", &self.key_file).finish_non_exhaustive()
    }
}

/// `exec <shell> -c 'eval "$(dd …)"' yaac-ssh '<arg>' …`, the script itself first on stdin.
fn script_command(shell: &str, args: &[&str]) -> String {
    let mut cmd = format!("exec {shell} -c 'eval \"$(dd bs=1 count={} 2>/dev/null)\"' yaac-ssh", REMOTE.len());
    for a in args {
        cmd.push(' ');
        cmd.push_str(&shell_quote(a));
    }
    cmd
}

/// The setup id in a comment `yaac:<machine>:<id>`.
fn id_of(comment: &str) -> &str {
    comment.rsplit(':').next().unwrap_or("")
}

/// The install session, kept open after YAAC's line was appended, until the host decides.
pub(crate) struct Installing {
    child: Child,
    stdin: ChildStdin,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
    /// The line appended.
    pub(crate) line: String,
    /// Its comment.
    pub(crate) comment: String,
    key: PathBuf,
}

impl std::fmt::Debug for Installing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Installing").field("comment", &self.comment).finish_non_exhaustive()
    }
}

async fn reply(lines: &mut tokio::io::Lines<BufReader<ChildStdout>>, prefixes: &[&str]) -> Option<String> {
    let read = async {
        // Skip login noise (bounded).
        for _ in 0..1000 {
            match lines.next_line().await {
                Ok(Some(l)) => {
                    let l = l.trim_end_matches('\r');
                    if l.starts_with("ERR ") || prefixes.iter().any(|p| l.starts_with(p)) {
                        return Some(l.to_owned());
                    }
                }
                _ => return None,
            }
        }
        None
    };
    tokio::time::timeout(STEP, read).await.ok().flatten()
}

fn remove_key_files(key: &Path) {
    if let Some(dir) = key.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Generate YAAC's key for this server, sign in once with what the user gave, and append the
/// key's public half to `authorized_keys`. The secret is wiped as soon as the sign-in is over.
pub(crate) async fn sign_in(d: &SshDefaults, id: &Ident, credential: Credential) -> Result<Installing, Fail> {
    // 1. YAAC's own key, user-only, in the hub's data directory.
    let key = d.key(id);
    remove_key_files(&key);
    let dir = key.parent().unwrap_or(&d.dir).to_owned();
    super::private_dir(&d.dir.join("keys")).and_then(|()| super::private_dir(&dir)).map_err(|e| failed(format!("cannot create {}: {e}", dir.display())))?;
    let comment = format!("yaac:{}:{}", d.machine.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_".contains(c) { c } else { '-' }).collect::<String>(), super::askpass::random_hex(6));
    let mut keygen = quiet(&d.keygen_exe);
    keygen.args(["-q", "-t", "ed25519", "-N", "", "-C"]).arg(&comment).arg("-f").arg(&key);
    let out = output(keygen, "ssh-keygen").await?;
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap_or_default();
    let line = public.trim().to_owned();
    if !out.status.success() || line.split_whitespace().count() < 2 {
        remove_key_files(&key);
        return Err(failed(format!("ssh-keygen could not make YAAC's key: {}", tail(&out.stderr))));
    }

    // 2. Sign in with the credential, once, and run the install script in that session.
    let Credential { password, key_file, passphrase } = credential;
    let (auth, secret, refused) = match (&key_file, password) {
        (Some(file), _) => {
            if !file.is_file() {
                remove_key_files(&key);
                return Err(failed(format!("the key file {} does not exist", file.display())));
            }
            (SshAuth::KeyFile(file), passphrase, "the server did not accept the key file")
        }
        (None, Some(password)) => (SshAuth::Password, Some(password), "the server did not accept the password"),
        (None, None) => {
            remove_key_files(&key);
            return Err(failed("give a password or a key file"));
        }
    };
    let askpass = Askpass::start(secret.unwrap_or_default(), &d.socket_dir).map_err(|e| failed(format!("cannot start the password prompt: {e}")))?;
    let helper = match &d.askpass_exe {
        Some(p) => p.clone(),
        None => std::env::current_exe().map_err(|e| failed(format!("cannot find this program for the password prompt: {e}")))?,
    };
    let ak = d.authorized_keys.clone().unwrap_or_default();
    let home = d.target_home().unwrap_or("").to_owned();
    let mut c = d.target(id).command(auth, &script_command(&d.shell, &["install", &line, &ak, &home, id_of(&comment)]));
    for (k, v) in askpass.env(&helper) {
        c.env(k, v);
    }
    let mut child = c.spawn().map_err(|e| failed(format!("cannot run ssh: {e}")))?;
    let (Some(mut stdin), Some(stdout), Some(mut stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        return Err(failed("ssh's standard streams were not captured"));
    };
    let errors = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = (&mut stderr).take(64 * 1024).read_to_end(&mut buf).await;
        buf
    });
    let _ = stdin.write_all(REMOTE.as_bytes()).await;
    let _ = stdin.flush().await;
    let mut lines = BufReader::new(stdout).lines();
    let got = reply(&mut lines, &["INSTALLED "]).await;
    // The sign-in is over either way: the secret goes now.
    let asked = askpass.used();
    drop(askpass);
    let refused = if !asked && matches!(auth, SshAuth::Password) { "the server did not ask for a password (it may not accept passwords)" } else { refused };
    match got {
        Some(l) if l.starts_with("INSTALLED ") => Ok(Installing { child, stdin, lines, line, comment, key }),
        other => {
            let _ = child.start_kill();
            let stderr = tokio::time::timeout(Duration::from_secs(5), errors).await.ok().and_then(Result::ok).unwrap_or_default();
            remove_key_files(&key);
            Err(match other.as_deref().and_then(|l| l.strip_prefix("ERR ")) {
                Some(reason) => failed(format!("could not install YAAC's key: {reason}")),
                None => classify(&stderr, refused),
            })
        }
    }
}

impl Installing {
    /// Keep the line (the key-only login worked) or remove it again and confirm it is gone.
    /// Removing also deletes YAAC's local key. Returns the server's answer.
    pub(crate) async fn finish(mut self, keep: bool) -> Result<(), Fail> {
        let word: &[u8] = if keep { b"KEEP\n" } else { b"REMOVE\n" };
        let sent = self.stdin.write_all(word).await.and(self.stdin.flush().await);
        let got = if sent.is_ok() { reply(&mut self.lines, &["OK "]).await } else { None };
        let _ = tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await;
        if !keep {
            remove_key_files(&self.key);
        }
        match got.as_deref() {
            Some("OK kept") if keep => Ok(()),
            Some("OK removed" | "OK absent") if !keep => Ok(()),
            Some(l) => Err(failed(l.strip_prefix("ERR ").unwrap_or(l).to_owned())),
            None => Err(unreachable("the setup session ended before the server answered")),
        }
    }
}

/// Revoke: sign in with YAAC's key and remove the line holding exactly that key, then delete
/// the key. `Ok(true)`: a line was removed; `Ok(false)`: it was already gone. The key is kept
/// when the server cannot be reached or no longer accepts it.
pub(crate) async fn revoke(d: &SshDefaults, id: &Ident, line: &str, comment: &str) -> Result<bool, Fail> {
    let key = d.key(id);
    let Some(blob) = line.split_whitespace().nth(1) else { return Err(failed("YAAC's record of the line is unreadable")) };
    if !key.is_file() {
        return Err(unreachable("YAAC's key for this server is missing, so YAAC cannot sign in to remove its line"));
    }
    let ak = d.authorized_keys.clone().unwrap_or_default();
    let home = d.target_home().unwrap_or("").to_owned();
    let mut c = d.target(id).command(SshAuth::Key(&key), &script_command(&d.shell, &["revoke", blob, &ak, &home, id_of(comment)]));
    let mut child = c.spawn().map_err(|e| failed(format!("cannot run ssh: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(REMOTE.as_bytes()).await;
        let _ = stdin.flush().await;
    }
    let out = tokio::time::timeout(STEP, child.wait_with_output()).await.map_err(|_| unreachable("the server did not answer"))?.map_err(|e| failed(e.to_string()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let answer = text.lines().map(|l| l.trim_end_matches('\r')).find(|l| l.starts_with("OK ") || l.starts_with("ERR "));
    match answer {
        Some("OK removed") => {
            remove_key_files(&key);
            Ok(true)
        }
        Some("OK absent") => {
            remove_key_files(&key);
            Ok(false)
        }
        Some(l) => Err(failed(l.strip_prefix("ERR ").unwrap_or(l).to_owned())),
        None => {
            let mut fail = classify(&out.stderr, "the server no longer accepts YAAC's key (the line may already be gone)");
            // Either way the line may still be there and YAAC cannot remove it now: the user
            // is shown the line, and the key is kept until they confirm.
            fail.code = codes::SSH_UNREACHABLE;
            Err(fail)
        }
    }
}

/// Delete YAAC's key for a server (the user removed the line by hand).
pub(crate) fn forget_key(d: &SshDefaults, id: &Ident) {
    remove_key_files(&d.key(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_match_ssh_keygen() {
        // `ssh-keygen -lf` of this key prints SHA256:Fl7ERLXUlA6D6rIBC8yZMxYNcQGZbWX8GaUS+z4xDUM.
        let blob = "AAAAC3NzaC1lZDI1NTE5AAAAIBV0dJ/eLRkzwSbmXZJ0RVkf3cOVlgvQyWZchnI9H+Fj";
        assert!(fingerprint(blob).unwrap().starts_with("SHA256:"));
        assert_eq!(fingerprint(blob).unwrap().len(), "SHA256:".len() + 43);
        assert!(fingerprint("not base64!").is_none());
    }

    #[test]
    fn failures_in_plain_words() {
        assert_eq!(classify(b"ssh: connect to host h port 22: Connection refused\n", "x").code, codes::SSH_UNREACHABLE);
        let refused = classify(b"me@h: Permission denied (password).\n", "the server did not accept the password");
        assert_eq!(refused.code, codes::SSH_FAILED);
        assert!(refused.message.starts_with("the server did not accept the password"));
        assert!(classify(b"Host key verification failed.\n", "x").message.contains("not known yet"));
    }

    #[test]
    fn the_script_command_is_one_line_of_quoted_words() {
        let c = script_command("sh", &["install", "ssh-ed25519 AAAA yaac:m:1", "", "", "1"]);
        assert!(!c.contains('\n'));
        assert!(c.ends_with("yaac-ssh 'install' 'ssh-ed25519 AAAA yaac:m:1' '' '' '1'"), "{c}");
        assert_eq!(id_of("yaac:laptop:ab12"), "ab12");
    }
}
