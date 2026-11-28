//! This PC's Windows side, from a hub that moved into one of its WSL distros
//! (docs/15-hub-transfer.md#into-this-pcs-wsl).
//!
//! The PC's local target becomes `windowsInterop {exe}` in the moved hub's settings. The hub in
//! WSL reaches it as one child process, as it reaches any target (invariant 1): `<yaac.exe>
//! local-daemon` through WSL interop. On Windows that command starts the local `yaacd
//! connect` bridge exactly as the Windows hub does ([`crate::config::local_config`]: the
//! packaged runtime and bundle, copied into their versioned data directories first) and lends
//! it its standard streams, byte for byte.

use std::process::Stdio;

use tokio::io::AsyncWriteExt as _;
use yaac_transport::{BoxFuture, ChildGuard, ConnectCtx, ConnectError, Connector, LocalConfig, LocalConnector, Stream};

/// `yaac.exe local-daemon`.
pub const LOCAL_DAEMON_COMMAND: &str = "local-daemon";

/// `yaac hub-bridge`: standard streams to the hub's local socket.
pub const HUB_BRIDGE_COMMAND: &str = "hub-bridge";

/// How long `yaac hub-bridge` waits for the hub's socket to accept (the hub may be starting:
/// right after a move, or in a WSL distro that the bridge's own `wsl.exe` has just booted,
/// whose systemd then starts the hub's service).
pub const BRIDGE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Connects to this PC's Windows `yaacd` from inside WSL: `<exe> local-daemon`.
#[derive(Clone, Debug)]
pub struct InteropConnector {
    /// `yaac.exe` as WSL sees it (`/mnt/c/…/yaac.exe`).
    pub exe: String,
}

impl Connector for InteropConnector {
    fn connect<'a>(&'a self, _ctx: &'a ConnectCtx) -> BoxFuture<'a, Result<Stream, ConnectError>> {
        Box::pin(async move {
            if !std::path::Path::new(&self.exe).is_file() {
                return Err(ConnectError::NotInstalled(format!("YAAC for Windows was not found at {}", self.exe)));
            }
            let mut child = tokio::process::Command::new(&self.exe)
                .arg(LOCAL_DAEMON_COMMAND)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .map_err(|source| ConnectError::Spawn { what: self.describe(), source })?;
            let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
                return Err(ConnectError::Other("the child's standard streams were not captured".into()));
            };
            Ok(Stream { reader: Box::new(stdout), writer: Box::new(stdin), child: ChildGuard::process(child) })
        })
    }

    fn describe(&self) -> String {
        format!("windows: {} {LOCAL_DAEMON_COMMAND}", self.exe)
    }
}

/// How long `yaac local-daemon` waits, once its input has closed (the hub in WSL stopped or let
/// go of the target), for the bridge to close its output too. Then it ends, and the bridge with
/// it: nothing of the install's `yaac.exe` stays running after the hub is gone (P423), so the
/// installer can replace it. `yaacd` itself keeps running, and so do its turns (invariant 8).
pub const LOCAL_DAEMON_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// `yaac local-daemon`: starts this computer's local `yaacd connect` bridge as `local`
/// describes it and copies bytes between it and this process's standard streams until either
/// side closes. Returns when the bridge is gone, or [`LOCAL_DAEMON_GRACE`] after the input
/// closed.
pub fn serve_local_daemon(local: LocalConfig) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build().map_err(|e| e.to_string())?;
    let result = rt.block_on(async move {
        let mut local = local;
        // Our standard error goes back to the hub in WSL, which shows it in the target's log.
        local.inherit_stderr = true;
        let connector = LocalConnector::new(local);
        let stream = connector.connect(&ConnectCtx::default()).await.map_err(|e| e.to_string())?;
        let Stream { reader, writer, child } = stream;
        pump(tokio::io::stdin(), tokio::io::stdout(), reader, writer, LOCAL_DAEMON_GRACE).await;
        drop(child);
        Ok(())
    });
    // A read of standard input may still be waiting (the bridge ended first): never wait for it.
    rt.shutdown_background();
    result
}

/// Copies `input` to the bridge's `writer` and the bridge's `reader` to `output`. The bridge
/// closing its output ends it; once `input` closes, the bridge's input is closed and it has
/// `grace` to close its output.
async fn pump(
    mut input: impl tokio::io::AsyncRead + Unpin,
    mut output: impl tokio::io::AsyncWrite + Unpin,
    mut reader: impl tokio::io::AsyncRead + Unpin,
    mut writer: impl tokio::io::AsyncWrite + Unpin,
    grace: std::time::Duration,
) {
    let up = async move {
        let _ = tokio::io::copy(&mut input, &mut writer).await;
        let _ = writer.shutdown().await;
    };
    let down = async move {
        let _ = tokio::io::copy(&mut reader, &mut output).await;
        let _ = output.flush().await;
    };
    tokio::pin!(down);
    let input_closed = tokio::select! {
        () = &mut down => false,
        () = up => true,
    };
    if input_closed {
        let _ = tokio::time::timeout(grace, &mut down).await;
    }
}

/// The named event (`Local\yaac-local-daemon-<hash>`) that every `yaac.exe local-daemon` of the
/// install at `exe` holds while it runs (P423). The window waits for it to be gone before the
/// installer replaces `exe`. The hash is of the path, case-insensitive (FNV-1a).
pub fn local_daemon_event(exe: &std::path::Path) -> String {
    let path = exe.to_string_lossy();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path).replace('/', r"\").to_lowercase();
    let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!(r"Local\yaac-local-daemon-{hash:016x}")
}

/// `yaac hub-bridge`: connects to the hub's local socket (waiting up to `wait` for it to
/// accept) and copies bytes between it and this process's standard streams until either side
/// closes. The PC's window runs it through `wsl.exe` or `ssh` to reach a hub that moved there
/// (docs/15-hub-transfer.md#the-pcs-window), so the hub sees an ordinary local attach: the
/// socket is the user's own, as for every local client.
#[cfg(unix)]
pub fn serve_hub_bridge(socket: &std::path::Path, wait: std::time::Duration) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build().map_err(|e| e.to_string())?;
    rt.block_on(bridge(socket, wait, tokio::io::stdin(), tokio::io::stdout()))
}

/// [`serve_hub_bridge`] between any two streams.
#[cfg(unix)]
async fn bridge(
    socket: &std::path::Path,
    wait: std::time::Duration,
    mut input: impl tokio::io::AsyncRead + Unpin,
    mut output: impl tokio::io::AsyncWrite + Unpin,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + wait;
    let stream = loop {
        match tokio::net::UnixStream::connect(socket).await {
            Ok(s) => break s,
            Err(e) if tokio::time::Instant::now() >= deadline => return Err(format!("no hub at {}: {e}", socket.display())),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    };
    let (mut reader, mut writer) = stream.into_split();
    let up = async move {
        let _ = tokio::io::copy(&mut input, &mut writer).await;
        let _ = writer.shutdown().await;
    };
    let down = async move {
        let _ = tokio::io::copy(&mut reader, &mut output).await;
        let _ = output.flush().await;
    };
    // The hub closing ends it; the window closing only closes the hub's input.
    tokio::pin!(down);
    tokio::select! {
        () = &mut down => {}
        () = async { up.await; std::future::pending::<()>().await } => {}
    }
    Ok(())
}

/// Only a Linux hub is reached through a bridge.
#[cfg(not(unix))]
pub fn serve_hub_bridge(socket: &std::path::Path, wait: std::time::Duration) -> Result<(), String> {
    let _ = (socket, wait);
    Err("the hub bridge runs where the hub listens on a Unix socket".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt as _;

    #[tokio::test]
    async fn the_bridge_waits_for_the_hub_and_copies_both_ways() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("hub.sock");
        // The hub starts listening a moment after the bridge started.
        let path = socket.clone();
        let hub = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            let (mut s, _) = listener.accept().await.unwrap();
            let mut got = [0u8; 5];
            s.read_exact(&mut got).await.unwrap();
            s.write_all(&[b'>', got[0], got[1], got[2], got[3], got[4]]).await.unwrap();
            // The hub closes: the bridge ends.
        });
        let (mut window, window_end) = tokio::io::duplex(64);
        let (window_in, window_out) = tokio::io::split(window_end);
        window.write_all(b"hello").await.unwrap();
        bridge(&socket, std::time::Duration::from_secs(5), window_in, window_out).await.unwrap();
        hub.await.unwrap();
        let mut back = Vec::new();
        window.read_to_end(&mut back).await.unwrap();
        assert_eq!(back, b">hello");
    }

    #[tokio::test]
    async fn no_hub_is_a_plain_error() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _b) = tokio::io::duplex(8);
        let (i, o) = tokio::io::split(a);
        let e = bridge(&dir.path().join("none.sock"), std::time::Duration::from_millis(300), i, o).await.unwrap_err();
        assert!(e.starts_with("no hub at"), "{e}");
    }
}

#[cfg(test)]
mod local_daemon_tests {
    use super::*;

    #[tokio::test]
    async fn local_daemon_ends_soon_after_its_input_closes() {
        // The bridge never closes its output: only the grace ends the pump.
        let (input, mut hub) = tokio::io::duplex(64);
        let (output, _out) = tokio::io::duplex(64);
        let (reader, _bridge_out) = tokio::io::duplex(64);
        let (writer, mut bridge_in) = tokio::io::duplex(64);
        let pumping = tokio::spawn(pump(input, output, reader, writer, std::time::Duration::from_millis(50)));
        hub.write_all(b"hi").await.unwrap();
        let mut got = [0u8; 2];
        tokio::io::AsyncReadExt::read_exact(&mut bridge_in, &mut got).await.unwrap();
        assert_eq!(&got, b"hi");
        drop(hub);
        tokio::time::timeout(std::time::Duration::from_secs(5), pumping).await.expect("ends after the grace").unwrap();
        // The bridge's input was closed first.
        let mut rest = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut bridge_in, &mut rest).await.unwrap();
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn local_daemon_ends_when_the_bridge_closes() {
        let (input, _hub) = tokio::io::duplex(64);
        let (output, mut out) = tokio::io::duplex(64);
        let (reader, mut bridge_out) = tokio::io::duplex(64);
        let (writer, _bridge_in) = tokio::io::duplex(64);
        let pumping = tokio::spawn(pump(input, output, reader, writer, std::time::Duration::from_secs(60)));
        bridge_out.write_all(b"ok").await.unwrap();
        drop(bridge_out);
        tokio::time::timeout(std::time::Duration::from_secs(5), pumping).await.expect("ends with the bridge").unwrap();
        let mut got = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut out, &mut got).await.unwrap();
        assert_eq!(got, b"ok");
    }

    #[test]
    fn one_local_daemon_event_per_install() {
        let a = local_daemon_event(std::path::Path::new(r"C:\Users\me\AppData\Local\Programs\YAAC\yaac.exe"));
        assert!(a.starts_with(r"Local\yaac-local-daemon-") && a.len() == r"Local\yaac-local-daemon-".len() + 16, "{a}");
        assert_eq!(a, local_daemon_event(std::path::Path::new(r"\\?\c:\users\ME\AppData\Local\Programs\YAAC\yaac.exe")), "the same file");
        assert_ne!(a, local_daemon_event(std::path::Path::new(r"C:\Other\YAAC\yaac.exe")));
    }
}
