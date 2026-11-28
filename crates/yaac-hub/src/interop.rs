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

/// `yaac local-daemon`: starts this computer's local `yaacd connect` bridge as `local`
/// describes it and copies bytes between it and this process's standard streams until either
/// side closes. Returns when the bridge is gone.
pub fn serve_local_daemon(local: LocalConfig) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_io().build().map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let mut local = local;
        // Our standard error goes back to the hub in WSL, which shows it in the target's log.
        local.inherit_stderr = true;
        let connector = LocalConnector::new(local);
        let stream = connector.connect(&ConnectCtx::default()).await.map_err(|e| e.to_string())?;
        let Stream { mut reader, mut writer, child } = stream;
        let up = async move {
            let mut stdin = tokio::io::stdin();
            let _ = tokio::io::copy(&mut stdin, &mut writer).await;
            let _ = writer.shutdown().await;
        };
        let down = async move {
            let mut stdout = tokio::io::stdout();
            let _ = tokio::io::copy(&mut reader, &mut stdout).await;
            let _ = stdout.flush().await;
        };
        // The bridge closing its output ends it; the hub closing our input only closes the
        // bridge's, which then exits by itself.
        tokio::pin!(down);
        tokio::select! {
            () = &mut down => {}
            () = async { up.await; std::future::pending::<()>().await } => {}
        }
        drop(child);
        Ok(())
    })
}
