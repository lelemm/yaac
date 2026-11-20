//! Stage 0 run locally under `dash` and `bash` — the same program the WSL connector runs
//! through `wsl.exe` — with start-up noise before it, a temporary `YAAC_HOME`, and a stand-in
//! daemon bundle (a script that echoes its input), so the hand-over is checked byte for byte.
//!
//! Needs a Node.js 22+ that stage 0 can find (PATH, nvm, …), as on a real target; a test is
//! skipped, not failed, when there is none.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use yaac_transport::ConnectError;
use yaac_transport::bootstrap::{DaemonBundle, STAGE0, Stage0Timeouts, bootstrap};

const ECHO_DAEMON: &[u8] = b"if (process.argv[2] !== 'connect') process.exit(3);\nprocess.stdin.pipe(process.stdout);\n";

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yaac-stage0-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn shell_available(shell: &str) -> bool {
    Path::new(shell).exists()
}

/// `<shell> -c 'echo noise; exec <shell> -c "$0" "$@"' <stage0> yaac-stage0 YAAC_HOME=…`: a
/// login script that prints to standard output before stage 0 starts.
fn spawn(shell: &str, home: &Path, extra_env: &[(&str, &str)], noisy: bool) -> Child {
    let mut c = Command::new(shell);
    if noisy {
        c.arg("-c").arg(format!("echo 'Welcome to a noisy profile'; printf 'motd\\r\\n'; exec {shell} -c \"$0\" yaac-stage0 \"$@\""));
        c.arg(STAGE0);
    } else {
        c.arg("-c").arg(STAGE0).arg("yaac-stage0");
    }
    c.arg(format!("YAAC_HOME={}", home.display()));
    for (k, v) in extra_env {
        c.env(k, v);
    }
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true);
    c.spawn().expect("spawn shell")
}

async fn run(shell: &str, home: &Path, bundle: &DaemonBundle, noisy: bool) -> Result<bool, ConnectError> {
    let mut child = spawn(shell, home, &[], noisy);
    let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let mut done = bootstrap(stdout, stdin, bundle, "test target", Stage0Timeouts::default()).await?;
    // The protocol now: the stand-in daemon echoes every byte, including NUL and CR LF.
    let payload: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
    done.writer.write_all(&payload).await.unwrap();
    done.writer.flush().await.unwrap();
    let mut back = vec![0u8; payload.len()];
    tokio::time::timeout(Duration::from_secs(20), done.reader.read_exact(&mut back)).await.expect("echo in time").unwrap();
    assert_eq!(back, payload, "the stream after OK run is binary-clean");
    drop(done.writer);
    let _ = child.kill().await;
    Ok(done.uploaded)
}

async fn has_node(shell: &str) -> bool {
    let home = temp_dir("probe");
    let mut child = spawn(shell, &home, &[], false);
    let mut out = child.stdout.take().unwrap();
    drop(child.stdin.take());
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(20), out.read_to_end(&mut buf)).await;
    let _ = std::fs::remove_dir_all(&home);
    String::from_utf8_lossy(&buf).contains("runtime=node-")
}

#[tokio::test(flavor = "current_thread")]
async fn installs_once_then_reuses_under_dash_and_bash_with_noise() {
    for shell in ["/bin/dash", "/bin/bash"] {
        if !shell_available(shell) {
            eprintln!("skipped {shell}: not installed");
            continue;
        }
        if !has_node(shell).await {
            eprintln!("skipped {shell}: no Node.js 22+ for stage 0 to find");
            continue;
        }
        let home = temp_dir(shell.trim_start_matches("/bin/"));
        let bundle = DaemonBundle::new("0.0.1", ECHO_DAEMON.to_vec());
        assert!(run(shell, &home, &bundle, true).await.expect("first connection"), "{shell}: first connection uploads");
        assert!(home.join("daemon").join(&bundle.build).join("yaacd.mjs").is_file());
        assert!(!run(shell, &home, &bundle, false).await.expect("second connection"), "{shell}: second connection reuses");
        let entries: Vec<_> = std::fs::read_dir(home.join("daemon")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, vec![std::ffi::OsString::from(&bundle.build)], "{shell}: no temporary or lock left behind");
        std::fs::remove_dir_all(&home).unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn two_simultaneous_installs_leave_one_complete_daemon() {
    if !has_node("/bin/sh").await {
        eprintln!("skipped: no Node.js 22+");
        return;
    }
    let home = temp_dir("race");
    // About 1 MB, so the two uploads overlap.
    let bundle = DaemonBundle::new("0.0.2", [ECHO_DAEMON, &b"// padding\n".repeat(100_000)].concat());
    let (a, b) = tokio::join!(run("/bin/sh", &home, &bundle, false), run("/bin/sh", &home, &bundle, false));
    assert!(a.expect("a") && b.expect("b"), "both saw it missing and uploaded");
    let installed = std::fs::read(home.join("daemon").join(&bundle.build).join("yaacd.mjs")).unwrap();
    assert_eq!(installed.len(), bundle.bytes.len());
    let entries = std::fs::read_dir(home.join("daemon")).unwrap().count();
    assert_eq!(entries, 1, "no temporary or lock left behind");
    std::fs::remove_dir_all(&home).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn a_wrong_hash_is_refused_and_nothing_is_installed() {
    if !has_node("/bin/sh").await {
        eprintln!("skipped: no Node.js 22+");
        return;
    }
    let home = temp_dir("hash");
    let mut bundle = DaemonBundle::new("0.0.3", ECHO_DAEMON.to_vec());
    bundle.sha256 = "0".repeat(64);
    let err = run("/bin/sh", &home, &bundle, false).await.unwrap_err();
    assert!(err.to_string().contains("hash mismatch"), "{err}");
    assert!(!home.join("daemon").join(&bundle.build).exists());
    std::fs::remove_dir_all(&home).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn no_runtime_is_a_specific_error() {
    let home = temp_dir("noruntime");
    // A user with no Node.js anywhere stage 0 looks.
    let fake_home = temp_dir("nohome");
    let mut child = spawn("/bin/sh", &home, &[("HOME", fake_home.to_str().unwrap()), ("PATH", "/usr/bin:/bin")], false);
    if Path::new("/usr/bin/node").exists() || Path::new("/usr/local/bin/node").exists() {
        eprintln!("skipped: a system Node.js is installed");
        return;
    }
    let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let bundle = DaemonBundle::new("0.0.4", ECHO_DAEMON.to_vec());
    let err = bootstrap(stdout, stdin, &bundle, "test target", Stage0Timeouts::default()).await.unwrap_err();
    assert!(matches!(err, ConnectError::NoRuntime(_)), "{err}");
    assert!(err.to_string().contains("Node.js 22 or newer"), "{err}");
    std::fs::remove_dir_all(&home).unwrap();
    std::fs::remove_dir_all(&fake_home).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn a_target_that_never_prints_the_marker_shows_what_it_said() {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("echo 'sh: 1: yaac: not found'; exit 127")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let bundle = DaemonBundle::new("0.0.5", ECHO_DAEMON.to_vec());
    let err = bootstrap(stdout, stdin, &bundle, "test target", Stage0Timeouts::default()).await.unwrap_err();
    assert!(err.to_string().contains("yaac: not found"), "{err}");
}
