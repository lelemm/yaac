//! Spikes S2 and the WSL connector, run on a Windows host against a real distro: set
//! `YAAC_TEST_WSL_DISTRO` (and optionally `YAAC_TEST_WSL_USER`); otherwise every test is
//! skipped. Everything written inside the distro goes under `/tmp/yaac-wsl-test-<pid>` and is
//! removed afterwards; the distro is never started by these tests (they skip if it is down).
//!
//! `cargo test -p yaac-transport --test wsl_windows -- --nocapture` prints the measurements.

#![cfg(windows)]

use std::ffi::OsStr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yaac_transport::bootstrap::{DaemonBundle, sha256_hex};
use yaac_transport::wsl::{list_distros, running_distros, wsl_command};
use yaac_transport::{ConnectCtx, ConnectError, Connector, WslConfig, WslConnector};

const SIZE: usize = 16 * 1024 * 1024;

fn distro() -> Option<(String, Option<String>)> {
    let d = std::env::var("YAAC_TEST_WSL_DISTRO").ok().filter(|d| !d.is_empty())?;
    Some((d, std::env::var("YAAC_TEST_WSL_USER").ok().filter(|u| !u.is_empty())))
}

async fn running(d: &str) -> bool {
    running_distros(OsStr::new("wsl.exe")).await.unwrap().iter().any(|r| r.eq_ignore_ascii_case(d))
}

/// Deterministic noise covering every byte value, CR LF, NUL, ^C, ^Z, and invalid UTF-8/16.
fn noise(len: usize, mut seed: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    while v.len() < len {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        v.extend_from_slice(&seed.to_le_bytes());
    }
    v.truncate(len);
    v[..12].copy_from_slice(b"\r\n\0\x03\x1a\xff\xfe\n\r\x1b[0");
    v
}

fn sh(d: &(String, Option<String>), script: &str) -> tokio::process::Command {
    wsl_command(OsStr::new("wsl.exe"), &d.0, d.1.as_deref(), &["sh".into(), "-c".into(), script.into()])
}

#[tokio::test(flavor = "current_thread")]
async fn the_distro_list_decodes() {
    let Some((d, _)) = distro() else { return eprintln!("skipped: YAAC_TEST_WSL_DISTRO not set") };
    let list = list_distros(OsStr::new("wsl.exe")).await.unwrap();
    eprintln!("distros: {list:?}");
    assert!(list.iter().any(|x| x.name.eq_ignore_ascii_case(&d)), "{d} is installed");
}

#[tokio::test(flavor = "current_thread")]
async fn pipes_through_wsl_exe_are_binary_clean_both_ways() {
    let Some(d) = distro() else { return eprintln!("skipped: YAAC_TEST_WSL_DISTRO not set") };
    if !running(&d.0).await {
        return eprintln!("skipped: {} is not running (the tests never start it)", d.0);
    }

    // Host → target: the target hashes what arrives.
    let data = noise(SIZE, 0x9E37_79B9_7F4A_7C15);
    let want = sha256_hex(&data);
    let mut child = sh(&d, "sha256sum | cut -d ' ' -f 1").spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let t = Instant::now();
    let writer = async {
        stdin.write_all(&data).await.unwrap();
        drop(stdin);
    };
    let mut got = String::new();
    let reader = stdout.read_to_string(&mut got);
    let ((), r) = tokio::join!(writer, reader);
    r.unwrap();
    eprintln!("host → target: {SIZE} bytes in {:?}", t.elapsed());
    assert_eq!(got.trim(), want, "host → target");
    let _ = child.wait().await;

    // Target → host: the target sends random bytes and their hash on standard error.
    let script = format!(
        "f=$(mktemp) && head -c {SIZE} /dev/urandom >\"$f\" && sha256sum \"$f\" | cut -d ' ' -f 1 >&2 && cat \"$f\"; rm -f \"$f\""
    );
    let out = sh(&d, &script).output().await.unwrap();
    assert_eq!(out.stdout.len(), SIZE, "target → host: byte count");
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), sha256_hex(&out.stdout), "target → host");

    // Both at once through `cat`, the shape of a live connection.
    let data = noise(SIZE, 42);
    let mut child = sh(&d, "cat").spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let t = Instant::now();
    let writer = async {
        for chunk in data.chunks(64 * 1024) {
            stdin.write_all(chunk).await.unwrap();
        }
        drop(stdin);
    };
    let mut back = Vec::with_capacity(SIZE);
    let reader = stdout.read_to_end(&mut back);
    let ((), r) = tokio::join!(writer, reader);
    r.unwrap();
    eprintln!("echo through cat: {SIZE} bytes each way in {:?}", t.elapsed());
    assert_eq!(back.len(), SIZE, "echo: byte count");
    assert_eq!(sha256_hex(&back), sha256_hex(&data), "echo");
    let _ = child.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn round_trip_latency() {
    let Some(d) = distro() else { return eprintln!("skipped: YAAC_TEST_WSL_DISTRO not set") };
    if !running(&d.0).await {
        return eprintln!("skipped: {} is not running", d.0);
    }
    let mut child = sh(&d, "cat").spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut samples = Vec::new();
    let mut buf = [0u8; 64];
    for i in 0..300u32 {
        let msg = [u8::try_from(i % 251).unwrap(); 64];
        let t = Instant::now();
        stdin.write_all(&msg).await.unwrap();
        stdin.flush().await.unwrap();
        stdout.read_exact(&mut buf).await.unwrap();
        samples.push(t.elapsed());
        assert_eq!(buf, msg);
    }
    samples.sort();
    let pick = |percent: usize| samples[(samples.len() - 1) * percent / 100];
    eprintln!("64-byte round trip through wsl.exe + cat: min {:?}, median {:?}, p95 {:?}, max {:?}", samples[0], pick(50), pick(95), samples[samples.len() - 1]);
    drop(stdin);
    let _ = child.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_connector_installs_and_hands_over_in_a_temporary_home() {
    let Some(d) = distro() else { return eprintln!("skipped: YAAC_TEST_WSL_DISTRO not set") };
    if !running(&d.0).await {
        return eprintln!("skipped: {} is not running", d.0);
    }
    let home = format!("/tmp/yaac-wsl-test-{}", std::process::id());
    let echo = b"if (process.argv[2] !== 'connect') process.exit(3);\nprocess.stdin.pipe(process.stdout);\n".to_vec();
    let mut config = WslConfig::new(&d.0, d.1.clone(), Some(DaemonBundle::new("0.0.0-test", echo)));
    config.env.push(("YAAC_HOME".into(), home.clone()));
    let connector = WslConnector::new(config);
    for round in 0..2 {
        let t = Instant::now();
        let mut stream = connector.connect(&ConnectCtx::default()).await.unwrap_or_else(|e| panic!("connect: {e}"));
        eprintln!("connect #{round} (stage 0{}) took {:?}", if round == 0 { " with upload" } else { "" }, t.elapsed());
        let payload = noise(1 << 20, 7 + round);
        let (mut r, mut w) = (stream.reader, stream.writer);
        let writer = async {
            w.write_all(&payload).await.unwrap();
            w.flush().await.unwrap();
            w
        };
        let mut back = vec![0u8; payload.len()];
        let reader = tokio::time::timeout(Duration::from_secs(60), r.read_exact(&mut back));
        let (_w, got) = tokio::join!(writer, reader);
        got.expect("echo in time").unwrap();
        assert_eq!(back, payload, "the stream after stage 0 is binary-clean");
        stream.child.kill();
    }
    // A user that does not exist is a clear error, not a hang.
    let mut bad = WslConfig::new(&d.0, Some("no-such-user-yaac".into()), connector.config().bundle.clone());
    bad.env.push(("YAAC_HOME".into(), home.clone()));
    let err = WslConnector::new(bad).connect(&ConnectCtx::default()).await.unwrap_err();
    eprintln!("unknown user: {err}");
    assert!(matches!(err, ConnectError::Other(_)), "{err}");

    let out = sh(&d, &format!("rm -rf '{home}' && echo gone")).output().await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "gone");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unknown_distro_is_down_not_started() {
    if distro().is_none() {
        return eprintln!("skipped: YAAC_TEST_WSL_DISTRO not set");
    }
    let config = WslConfig::new("yaac-no-such-distro", None, Some(DaemonBundle::new("0", b"x".to_vec())));
    let err = WslConnector::new(config).connect(&ConnectCtx::default()).await.unwrap_err();
    assert!(matches!(err, ConnectError::TargetDown(_)), "{err}");
    assert_eq!(err.to_string(), "WSL distro yaac-no-such-distro is not running");
}
