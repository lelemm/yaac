//! The SSH connector against a throwaway local `sshd` (see `support/sshd.rs`): stage 0 runs
//! through the one `ssh` child under `dash` and `bash`, each behind a login that prints noise
//! first; the stream after the hand-over is binary-clean; an unknown or changed host key and
//! a refused sign-in fail with plain words; a server without Node.js gets the private runtime
//! over the same connection.

#![cfg(unix)]

#[path = "support/sshd.rs"]
mod sshd;

use std::path::{Path, PathBuf};
use std::time::Duration;

use sshd::Sshd;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yaac_transport::bootstrap::DaemonBundle;
use yaac_transport::{ConnectCtx, Connector, SshConfig, SshConnector, SshTarget};

const ECHO_DAEMON: &[u8] = b"if (process.argv[2] !== 'connect') process.exit(3);\nprocess.stdin.pipe(process.stdout);\n";

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yaac-ssh-home-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn config(server: &Sshd, home: &Path, shell: &str) -> SshConfig {
    let mut target = SshTarget::new("127.0.0.1", Some(server.port), None);
    target.known_hosts = Some(server.known_hosts.clone());
    let mut c = SshConfig::new(target, Some(DaemonBundle::new("0.0.7", ECHO_DAEMON.to_vec())));
    c.identity = Some(server.client_key.clone());
    c.shell = shell.to_owned();
    c.env.push(("YAAC_HOME".into(), home.display().to_string()));
    c
}

async fn echo(connector: &SshConnector) -> Result<(), String> {
    let mut stream = connector.connect(&ConnectCtx::default()).await.map_err(|e| e.to_string())?;
    let payload: Vec<u8> = (0..=255u8).cycle().take(200_000).collect();
    let writer = async {
        stream.writer.write_all(&payload).await.unwrap();
        stream.writer.flush().await.unwrap();
    };
    let mut back = vec![0u8; payload.len()];
    let reader = tokio::time::timeout(Duration::from_secs(30), stream.reader.read_exact(&mut back));
    let ((), read) = tokio::join!(writer, reader);
    read.expect("echo in time").unwrap();
    assert_eq!(back, payload, "the stream after OK run is binary-clean through ssh");
    Ok(())
}

fn has_node() -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("node").is_file()))
}

#[tokio::test(flavor = "current_thread")]
async fn stage0_runs_through_ssh_under_dash_and_bash_behind_a_noisy_login() {
    if !Sshd::available() || !has_node() {
        return eprintln!("skipped: needs {} and Node.js", sshd::SSHD);
    }
    // The login prints to standard output before the command runs, like a chatty profile.
    let cases = [
        ("dash", "/bin/dash", r#"/bin/dash -c 'echo "Welcome to a noisy dash login"; printf "motd\r\n"; eval "$SSH_ORIGINAL_COMMAND"'"#),
        ("bash", "/bin/bash", r#"/bin/bash -c 'echo "Last login: yesterday"; echo -e "\x1b[1mbold\x1b[0m"; eval "$SSH_ORIGINAL_COMMAND"'"#),
    ];
    for (name, shell, login) in cases {
        if !Path::new(shell).exists() {
            eprintln!("skipped {name}: not installed");
            continue;
        }
        let server = Sshd::start(name, Some(login));
        let home = temp_home(name);
        let connector = SshConnector::new(config(&server, &home, shell));
        echo(&connector).await.unwrap_or_else(|e| panic!("{name}: first connection: {e}"));
        assert!(home.join("daemon").read_dir().unwrap().count() == 1, "{name}: installed once");
        echo(&connector).await.unwrap_or_else(|e| panic!("{name}: second connection: {e}"));
        assert_eq!(server.authorized_lines().len(), 2, "{name}: authorized_keys untouched");
        std::fs::remove_dir_all(&home).unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_and_changed_host_keys_and_a_refused_key_fail_in_plain_words() {
    if !Sshd::available() {
        return eprintln!("skipped: needs {}", sshd::SSHD);
    }
    let server = Sshd::start("keys", None);
    let home = temp_home("keys");

    let mut unknown = config(&server, &home, "sh");
    let empty = server.dir.join("empty_known_hosts");
    std::fs::write(&empty, "").unwrap();
    unknown.target.known_hosts = Some(empty);
    // Keep the user's own known_hosts out of it: only YAAC's (empty) file and nothing else.
    unknown.target.options.push("GlobalKnownHostsFile=/dev/null".into());
    let e = SshConnector::new(unknown).connect(&ConnectCtx::default()).await.unwrap_err().to_string();
    assert!(e.contains("not known yet") || e.contains("host key"), "{e}");

    let mut changed = config(&server, &home, "sh");
    let other = server.dir.join("other_known_hosts");
    std::fs::write(&other, format!("[127.0.0.1]:{} ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBV0dJ/eLRkzwSbmXZJ0RVkf3cOVlgvQyWZchnI9H+Fj\n", server.port)).unwrap();
    changed.target.known_hosts = Some(other);
    let e = SshConnector::new(changed).connect(&ConnectCtx::default()).await.unwrap_err().to_string();
    assert!(e.contains("host key has changed") && e.contains("never accepts a changed key"), "{e}");

    let mut refused = config(&server, &home, "sh");
    let stranger = server.dir.join("stranger");
    assert!(std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&stranger).status().unwrap().success());
    refused.identity = Some(stranger);
    let e = SshConnector::new(refused).connect(&ConnectCtx::default()).await.unwrap_err().to_string();
    assert!(e.contains("refused the sign-in"), "{e}");
    assert!(!home.exists(), "nothing ran on the server");
}

#[tokio::test(flavor = "current_thread")]
async fn a_server_without_node_gets_the_private_runtime_over_the_same_connection() {
    let Some(node) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("node")).find(|n| n.is_file())) else {
        return eprintln!("skipped: no node");
    };
    if !Sshd::available() {
        return eprintln!("skipped: needs {}", sshd::SSHD);
    }
    let server = Sshd::start("runtime", None);
    let home = temp_home("runtime");
    // A fake "pinned" archive whose node runs the real one, offered only through the host
    // (the target may not download), so it crosses the one ssh stream with PUT runtime.
    let pkg = server.dir.join("pkg/node-v22.99.0-test/bin");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("node"), format!("#!/bin/sh\nexec '{}' \"$@\"\n", node.display())).unwrap();
    std::fs::set_permissions(pkg.join("node"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let archive = server.dir.join("rt.tar.gz");
    assert!(std::process::Command::new("tar").arg("-czf").arg(&archive).arg("-C").arg(server.dir.join("pkg")).arg(".").status().unwrap().success());
    let arch = String::from_utf8(std::process::Command::new("uname").arg("-m").output().unwrap().stdout).unwrap().trim().to_owned();
    let mut c = config(&server, &home, "sh");
    c.env.push(("YAAC_SYSTEM_NODE".into(), "0".into()));
    c.runtime = Some(yaac_transport::RuntimeSource {
        artifacts: vec![yaac_transport::runtime::RuntimeArtifact {
            name: "node-22.99.0-test".into(),
            os: "Linux".into(),
            arch: vec![arch],
            libc: "glibc".into(),
            url: format!("file://{}", archive.display()),
            sha256: yaac_transport::runtime::sha256_file(&archive).unwrap(),
        }],
        cache_dir: Some(server.dir.join("host-cache")),
        curl: "curl".into(),
        target_download: false,
    });
    let connector = SshConnector::new(c);
    echo(&connector).await.expect("connection with the runtime upload");
    assert!(home.join("runtime/node-22.99.0-test/bin/node").is_file());
    echo(&connector).await.expect("second connection reuses it");
    std::fs::remove_dir_all(&home).unwrap();
}
