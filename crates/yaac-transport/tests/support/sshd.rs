//! A throwaway OpenSSH server for tests: `/usr/sbin/sshd` run as the current user on a random
//! local port, with a temporary host key and its own `authorized_keys` file
//! (`AuthorizedKeysFile` in a temporary `sshd_config`), so the real `~/.ssh` is never read
//! for authorisation and never written. Password authentication is off (a non-root `sshd`
//! cannot check passwords).
//!
//! Shared by the transport and hub tests (`#[path]` include).

#![allow(dead_code, unreachable_pub, reason = "each test crate uses a different part")]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The server binary.
pub const SSHD: &str = "/usr/sbin/sshd";

/// A running test server. Stopped, and its folder removed, when dropped.
pub struct Sshd {
    /// Everything of the server's: config, host key, `authorized_keys`, a client key.
    pub dir: PathBuf,
    /// The port on 127.0.0.1.
    pub port: u16,
    /// A client key already in `authorized_keys` (the "user's own" key in tests).
    pub client_key: PathBuf,
    /// The server's `authorized_keys`.
    pub authorized_keys: PathBuf,
    /// A known-hosts file holding the server's host key.
    pub known_hosts: PathBuf,
    /// The host key line (`ssh-ed25519 AAAA…`).
    pub host_key: String,
    child: Child,
}

impl std::fmt::Debug for Sshd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sshd").field("port", &self.port).finish_non_exhaustive()
    }
}

fn keygen(path: &Path, comment: &str) {
    let ok = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"]).arg(path).status().expect("ssh-keygen");
    assert!(ok.success(), "ssh-keygen failed");
}

impl Sshd {
    /// Whether the test can run here.
    pub fn available() -> bool {
        Path::new(SSHD).is_file() && Command::new("ssh").arg("-V").stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }

    /// Starts a server. `force_command`, when given, is run instead of every client command
    /// (the original is in `$SSH_ORIGINAL_COMMAND`): the tests' "noisy login profile".
    pub fn start(name: &str, force_command: Option<&str>) -> Self {
        let dir = std::env::temp_dir().join(format!("yaac-sshd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        keygen(&dir.join("host_key"), "test-host");
        let client_key = dir.join("client_key");
        keygen(&client_key, "test-users-own-key");
        let authorized_keys = dir.join("authorized_keys");
        let own = std::fs::read_to_string(client_key.with_extension("pub")).unwrap();
        std::fs::write(&authorized_keys, format!("# the user's own keys\n{own}")).unwrap();
        let host_key = std::fs::read_to_string(dir.join("host_key.pub")).unwrap().split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        for attempt in 0..5 {
            let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let mut config = format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/host_key\nPidFile {d}/sshd.pid\nAuthorizedKeysFile {ak}\n\
                 UsePAM no\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n\
                 PermitUserEnvironment no\nAllowTcpForwarding no\nX11Forwarding no\nLogLevel ERROR\n",
                d = dir.display(),
                ak = authorized_keys.display()
            );
            if let Some(fc) = force_command {
                config.push_str(&format!("ForceCommand {fc}\n"));
            }
            std::fs::write(dir.join("sshd_config"), config).unwrap();
            let mut child = Command::new(SSHD)
                .arg("-D")
                .arg("-f")
                .arg(dir.join("sshd_config"))
                .arg("-E")
                .arg(dir.join("sshd.log"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("start sshd");
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    let known_hosts = dir.join("known_hosts");
                    std::fs::write(&known_hosts, format!("[127.0.0.1]:{port} {host_key}\n")).unwrap();
                    return Self { known_hosts, dir, port, client_key, authorized_keys, host_key, child };
                }
                if child.try_wait().ok().flatten().is_some() {
                    break; // the port was taken meanwhile: try another
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("sshd attempt {attempt} failed: {}", std::fs::read_to_string(dir.join("sshd.log")).unwrap_or_default());
        }
        panic!("sshd did not start");
    }

    /// The `authorized_keys` lines now.
    pub fn authorized_lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.authorized_keys).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    /// Runs `ssh` with the client key and the known host key.
    pub fn ssh(&self, command: &str) -> std::process::Output {
        Command::new("ssh")
            .args(["-T", "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes", "-o", "StrictHostKeyChecking=yes", "-o", "LogLevel=ERROR"])
            .arg("-o")
            .arg(format!("UserKnownHostsFile={}", self.known_hosts.display()))
            .arg("-i")
            .arg(&self.client_key)
            .arg("-p")
            .arg(self.port.to_string())
            .arg("127.0.0.1")
            .arg(command)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
