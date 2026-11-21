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

/// A stand-in daemon that says its name, then echoes.
fn named_daemon(name: &str) -> Vec<u8> {
    format!("if (process.argv[2] !== 'connect') process.exit(3);\nprocess.stdout.write('{name}\\n');\nprocess.stdin.pipe(process.stdout);\n").into_bytes()
}

/// Installs `bytes` as build `build` in `home`, as an earlier connection would have.
fn preinstall(home: &Path, build: &str, bytes: &[u8]) {
    let dir = home.join("daemon").join(build);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("yaacd.mjs"), bytes).unwrap();
}

/// Bootstraps with `bundle` and returns the build run and the first line the daemon said.
async fn ran_with(home: &Path, bundle: &DaemonBundle) -> (String, String) {
    let mut child = spawn("/bin/sh", home, &[], false);
    let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let mut done = bootstrap(stdout, stdin, bundle, "test target", Stage0Timeouts::default()).await.expect("bootstrap");
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while tokio::time::timeout(Duration::from_secs(20), done.reader.read_exact(&mut byte)).await.expect("in time").is_ok() && byte[0] != b'\n' {
        line.push(byte[0]);
    }
    drop(done.writer);
    let _ = child.kill().await;
    (done.ran, String::from_utf8_lossy(&line).into_owned())
}

#[tokio::test(flavor = "current_thread")]
async fn daemons_only_move_forward_the_highest_build_number_runs() {
    if !has_node("/bin/sh").await {
        eprintln!("skipped: no Node.js 22+");
        return;
    }
    let home = temp_dir("order");
    // Installed by earlier hubs: build 9 and build 41 (41 > 9 as numbers, not as text), and a
    // development build, which has no order.
    preinstall(&home, "0.0.8+9-aaaaaaaaaaaaaaaa", &named_daemon("nine"));
    preinstall(&home, "0.0.8+41-bbbbbbbbbbbbbbbb", &named_daemon("forty-one"));
    preinstall(&home, "0.0.8+dev-cccccccccccccccc", &named_daemon("dev"));

    // An older hub (build 40) installs its own bundle but runs the newest: 41.
    let older = DaemonBundle::new("0.0.8+40", named_daemon("forty"));
    let (ran, said) = ran_with(&home, &older).await;
    assert_eq!((ran.as_str(), said.as_str()), ("0.0.8+41-bbbbbbbbbbbbbbbb", "forty-one"));
    assert!(home.join("daemon").join(&older.build).join("yaacd.mjs").is_file(), "its own build is still installed alongside");

    // A newer hub (build 42) runs its own.
    let newer = DaemonBundle::new("0.0.8+42", named_daemon("forty-two"));
    let (ran, said) = ran_with(&home, &newer).await;
    assert_eq!((ran, said.as_str()), (newer.build.clone(), "forty-two"));

    // A development bundle runs itself, as before build numbers.
    let dev = DaemonBundle::new("0.0.8", named_daemon("mine"));
    let (ran, said) = ran_with(&home, &dev).await;
    assert_eq!((ran, said.as_str()), (dev.build.clone(), "mine"));
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

// ── The private runtime, from a fake archive served from a temporary folder ──────────────

/// The real Node.js on this machine's `PATH` (the fake runtime is a script that runs it).
fn real_node() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("node")).find(|n| n.is_file()))
}

/// `<dir>/node-v22.99.0-test.tar.gz` holding `node-v22.99.0-test/bin/node`, a script that
/// runs the real Node.js, and a `RuntimeSource` offering it for this machine at a `file://`
/// URL (or, with `broken`, at a URL nothing can download).
fn fake_runtime(dir: &Path, node: &Path, broken: bool) -> yaac_transport::RuntimeSource {
    let top = dir.join("pkg").join("node-v22.99.0-test").join("bin");
    std::fs::create_dir_all(&top).unwrap();
    let script = top.join("node");
    std::fs::write(&script, format!("#!/bin/sh\nexec '{}' \"$@\"\n", node.display())).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let archive = dir.join("node-v22.99.0-test.tar.gz");
    let ok = std::process::Command::new("tar").arg("-czf").arg(&archive).arg("-C").arg(dir.join("pkg")).arg("node-v22.99.0-test").status().unwrap();
    assert!(ok.success());
    let sha256 = yaac_transport::runtime::sha256_file(&archive).unwrap();
    let arch = String::from_utf8(std::process::Command::new("uname").arg("-m").output().unwrap().stdout).unwrap().trim().to_owned();
    let libc = if Path::new("/lib").read_dir().unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("ld-musl-")) { "musl" } else { "glibc" };
    let url = if broken { format!("file://{}/missing.tar.gz", dir.display()) } else { format!("file://{}", archive.display()) };
    yaac_transport::RuntimeSource {
        artifacts: vec![yaac_transport::runtime::RuntimeArtifact {
            name: "node-22.99.0-test".into(),
            os: "Linux".into(),
            arch: vec![arch],
            libc: libc.into(),
            url,
            sha256,
        }],
        cache_dir: Some(dir.join("host-cache")),
        curl: "curl".into(),
        target_download: true,
    }
}

/// Stage 0 with the system Node.js switched off (`YAAC_SYSTEM_NODE=0`), so the target has no
/// runtime until one is installed.
async fn run_private(home: &Path, source: &yaac_transport::RuntimeSource) -> Result<yaac_transport::bootstrap::Bootstrapped<tokio::process::ChildStdout, tokio::process::ChildStdin>, ConnectError> {
    let mut c = Command::new("/bin/sh");
    c.arg("-c").arg(STAGE0).arg("yaac-stage0").arg(format!("YAAC_HOME={}", home.display())).arg("YAAC_SYSTEM_NODE=0");
    c.arg(format!("YAAC_RUNTIME_VERSION={}", source.name().unwrap()));
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true);
    let mut child = c.spawn().unwrap();
    let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let bundle = DaemonBundle::new("0.0.6", ECHO_DAEMON.to_vec());
    let done = yaac_transport::bootstrap::bootstrap_with(stdout, stdin, &bundle, Some(source), "test target", Stage0Timeouts::default()).await;
    // The child must outlive the stream in the test; it is killed when dropped otherwise.
    std::mem::drop(tokio::spawn(async move { let _ = child.wait().await; }));
    done
}

async fn echo_works<R: tokio::io::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(reader: &mut R, writer: &mut W) {
    writer.write_all(b"\x00hello\r\n").await.unwrap();
    writer.flush().await.unwrap();
    let mut back = [0u8; 8];
    tokio::time::timeout(Duration::from_secs(20), reader.read_exact(&mut back)).await.expect("echo in time").unwrap();
    assert_eq!(&back, b"\x00hello\r\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_target_without_node_downloads_the_pinned_runtime_itself_then_reuses_it() {
    let Some(node) = real_node() else { return eprintln!("skipped: no node on PATH") };
    let has_curl = std::process::Command::new("sh").arg("-c").arg("command -v curl").status().is_ok_and(|s| s.success());
    if !has_curl {
        return eprintln!("skipped: no curl for the target to download with");
    }
    let dir = temp_dir("rt-get");
    let home = dir.join("home");
    let source = fake_runtime(&dir, &node, false);
    let mut done = run_private(&home, &source).await.expect("first connection");
    assert_eq!(done.report.get("runtime"), None, "no runtime at first");
    assert_eq!(done.runtime, Some(yaac_transport::bootstrap::RuntimeInstall::Downloaded));
    echo_works(&mut done.reader, &mut done.writer).await;
    assert!(home.join("runtime/node-22.99.0-test/bin/node").is_file());
    assert!(!dir.join("host-cache").exists(), "the host fetched nothing");
    let entries: Vec<_> = std::fs::read_dir(home.join("runtime")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("node-22.99.0-test")], "no temporary, download, or lock left behind");
    drop(done);
    let mut again = run_private(&home, &source).await.expect("second connection");
    assert_eq!(again.report.get("private"), Some("node-22.99.0-test"));
    assert_eq!(again.runtime, None, "installed once");
    echo_works(&mut again.reader, &mut again.writer).await;
    drop(again);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn when_the_target_cannot_download_the_host_sends_its_verified_copy() {
    let Some(node) = real_node() else { return eprintln!("skipped: no node on PATH") };
    let dir = temp_dir("rt-put");
    let home = dir.join("home");
    let mut source = fake_runtime(&dir, &node, false);
    // The target is not allowed to download; the host fetches (with curl, from file://).
    source.target_download = false;
    let mut done = run_private(&home, &source).await.expect("connection");
    assert_eq!(done.runtime, Some(yaac_transport::bootstrap::RuntimeInstall::Uploaded));
    echo_works(&mut done.reader, &mut done.writer).await;
    assert_eq!(std::fs::read_dir(dir.join("host-cache")).unwrap().count(), 1, "the host cached one verified archive");
    drop(done);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn a_runtime_whose_checksum_differs_is_never_installed() {
    let Some(node) = real_node() else { return eprintln!("skipped: no node on PATH") };
    let dir = temp_dir("rt-hash");
    let home = dir.join("home");
    let mut source = fake_runtime(&dir, &node, false);
    source.artifacts[0].sha256 = "0".repeat(64);
    let err = run_private(&home, &source).await.unwrap_err();
    assert!(matches!(err, ConnectError::NoRuntime(_)), "{err}");
    assert!(err.to_string().contains("hash mismatch") && err.to_string().contains("pinned checksum"), "both paths refused it: {err}");
    assert!(!home.join("runtime/node-22.99.0-test").exists());
    assert_eq!(std::fs::read_dir(dir.join("host-cache")).unwrap().count(), 0, "nothing unverified is cached");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_download_falls_back_to_the_host_and_reports_both_when_it_fails_too() {
    let Some(node) = real_node() else { return eprintln!("skipped: no node on PATH") };
    let dir = temp_dir("rt-broken");
    let home = dir.join("home");
    let source = fake_runtime(&dir, &node, true);
    let err = run_private(&home, &source).await.unwrap_err();
    assert!(matches!(err, ConnectError::NoRuntime(_)), "{err}");
    let text = err.to_string();
    assert!(text.contains("could not download the runtime") && text.contains("the host could not download"), "{text}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The real pinned Node.js from nodejs.org (about 50 MB): run by hand with
/// `cargo test -p yaac-transport --test stage0 -- --ignored real_pinned`. `YAAC_TEST_HOST_FETCH=1`
/// makes the host fetch it and send it in-band instead of the target downloading it.
#[tokio::test(flavor = "current_thread")]
#[ignore = "downloads the real runtime from nodejs.org"]
async fn real_pinned_node_from_nodejs_org() {
    let dir = temp_dir("rt-real");
    let home = dir.join("home");
    let mut source = yaac_transport::RuntimeSource::pinned(Some(dir.join("host-cache")));
    let host = std::env::var("YAAC_TEST_HOST_FETCH").is_ok_and(|v| v == "1");
    source.target_download = !host;
    let started = std::time::Instant::now();
    let mut done = run_private(&home, &source).await.expect("the pinned runtime installs");
    eprintln!("installed {:?} in {:?}", done.runtime, started.elapsed());
    echo_works(&mut done.reader, &mut done.writer).await;
    let node = home.join(format!("runtime/node-{}/bin/node", yaac_transport::runtime::PINNED_NODE));
    let v = std::process::Command::new(&node).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&v.stdout).trim(), format!("v{}", yaac_transport::runtime::PINNED_NODE));
    drop(done);
    std::fs::remove_dir_all(&dir).unwrap();
}
