//! The local connector, against a stand-in runtime (`sh` running a script that echoes its
//! input). The real bundle is exercised by the hub's integration test.

#![cfg(unix)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yaac_transport::{ConnectCtx, ConnectError, Connector, LocalConfig, LocalConnector};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yaac-transport-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn configuration_defaults_and_overrides() {
    let root = Path::new("/repo");
    let none = LocalConfig::from_lookup(root, |_| None);
    assert_eq!(none.runtime, OsString::from("node"));
    assert_eq!(none.bundle, PathBuf::from("/repo/daemon/dist/yaacd.mjs"));

    let set = LocalConfig::from_lookup(root, |key| match key {
        "YAAC_RUNTIME" => Some("/opt/rt/bin/node".into()),
        "YAAC_DAEMON_BUNDLE" => Some("/x/yaacd.mjs".into()),
        _ => None,
    });
    assert_eq!(set.runtime, OsString::from("/opt/rt/bin/node"));
    assert_eq!(set.bundle, PathBuf::from("/x/yaacd.mjs"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_missing_bundle_is_a_clear_error() {
    let config = LocalConfig::from_lookup(Path::new("/nonexistent-root"), |_| None);
    let err = LocalConnector::new(config).connect(&ConnectCtx::default()).await.unwrap_err();
    assert!(matches!(err, ConnectError::NotInstalled(_)), "{err}");
    assert!(err.to_string().contains("YAAC_DAEMON_BUNDLE"));
}

#[tokio::test(flavor = "current_thread")]
async fn the_stream_is_binary_clean_and_dropping_it_kills_the_child() {
    let dir = temp_dir("echo");
    let script = dir.join("echo.sh");
    // `$1` is "connect", as for the real bundle.
    std::fs::write(&script, "test \"$1\" = connect || exit 3\nexec cat\n").unwrap();
    let config = LocalConfig { runtime: "sh".into(), bundle: script, env: vec![], inherit_stderr: true, build: None };
    let connector = LocalConnector::new(config);
    assert!(connector.describe().starts_with("local: sh "));

    let mut stream = connector.connect(&ConnectCtx::default()).await.unwrap();
    let pid = stream.child.pid().expect("running");
    let bytes: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
    let writer = async {
        stream.writer.write_all(&bytes).await.unwrap();
        stream.writer.flush().await.unwrap();
    };
    let mut back = vec![0u8; bytes.len()];
    let reader = stream.reader.read_exact(&mut back);
    let ((), read) = tokio::join!(writer, reader);
    read.unwrap();
    assert_eq!(back, bytes, "every byte value passes through unchanged");

    drop(stream);
    // The child is killed; wait until it is gone (or a zombie awaiting the reaper).
    let status = PathBuf::from(format!("/proc/{pid}/status"));
    for _ in 0..100 {
        let gone = std::fs::read_to_string(&status).map_or(true, |s| s.contains("State:\tZ"));
        if gone {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("child {pid} still running after the stream was dropped");
}
