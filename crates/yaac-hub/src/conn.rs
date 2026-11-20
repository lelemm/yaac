//! One target's connection: a supervisor that keeps exactly one stream open, reconnecting
//! with capped exponential backoff, and a pump that moves bytes between that stream and the
//! hub core. The protocol session itself lives in the core; the pump only moves bytes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, mpsc, oneshot};
use yaac_protocol::TargetId;
use yaac_transport::{ConnectCtx, ConnectError, Connector, Stream};

use crate::config::Backoff;
use crate::core::CoreEvent;

/// How the core nudges a supervisor: retry now (never boots a stopped machine), or start —
/// one attempt that may boot it (the user's **Start it**).
#[derive(Debug, Default)]
pub(crate) struct Wake {
    notify: Notify,
    start: AtomicBool,
}

impl Wake {
    /// Skip the backoff.
    pub(crate) fn retry(&self) {
        self.notify.notify_one();
    }

    /// Skip the backoff, and let the next attempt start the target's machine.
    pub(crate) fn start(&self) {
        self.start.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }
}

/// What happened on a connection, tagged by generation (one per stream opened).
#[derive(Debug)]
pub(crate) enum ConnEvent {
    /// Trying to connect.
    Connecting,
    /// The connector failed; retrying after the delay.
    Failed(String),
    /// The target's machine is not running and is not started behind the user's back (a
    /// stopped WSL distro, D35). Checked again after the delay — which starts nothing — and
    /// connected as soon as it runs, or at once when the user asks to start it.
    Down(String),
    /// A stream is open: write frames to `writer`; send on `kill` to drop it.
    Opened { writer: mpsc::UnboundedSender<Vec<u8>>, kill: oneshot::Sender<()> },
    /// Bytes from the target.
    Bytes(Vec<u8>),
    /// The stream ended.
    Closed(String),
}

/// Runs until aborted (the target is disabled or removed). Aborting drops the stream, which
/// kills the child.
pub(crate) async fn supervise(
    supervisor: u64,
    target: TargetId,
    name: String,
    connector: Arc<dyn Connector>,
    backoff: Backoff,
    wake: Arc<Wake>,
    core: mpsc::UnboundedSender<CoreEvent>,
) {
    let mut ctx = ConnectCtx { target_id: target.as_str().to_owned(), target_name: name, allow_start: false };
    let mut delay = backoff.initial;
    let mut generation = 0u64;
    loop {
        generation += 1;
        let send = |event| core.send(CoreEvent::Conn { target: target.clone(), supervisor, generation, event }).is_ok();
        if !send(ConnEvent::Connecting) {
            return;
        }
        let attempt = connector.connect(&ctx).await;
        // Permission to boot the target covers one attempt: the one the user asked for.
        ctx.allow_start = false;
        match attempt {
            Err(ConnectError::TargetDown(why)) => {
                if !send(ConnEvent::Down(why)) {
                    return;
                }
            }
            Err(error) => {
                if !send(ConnEvent::Failed(error.to_string())) {
                    return;
                }
            }
            Ok(stream) => {
                let (writer, frames) = mpsc::unbounded_channel();
                let (kill, killed) = oneshot::channel();
                if !send(ConnEvent::Opened { writer, kill }) {
                    return;
                }
                let (reason, got_bytes) = pump(stream, frames, killed, |bytes| send(ConnEvent::Bytes(bytes))).await;
                if !send(ConnEvent::Closed(reason)) {
                    return;
                }
                if got_bytes {
                    delay = backoff.initial;
                }
            }
        }
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = wake.notify.notified() => {
                delay = backoff.initial;
                // Only **Start it** may boot the target's machine, for this one attempt.
                ctx.allow_start = wake.start.swap(false, Ordering::SeqCst);
            }
        }
        delay = (delay * 2).min(backoff.max);
    }
}

/// Moves bytes until the stream ends or is killed. Returns why it ended, and whether the
/// target sent anything at all.
async fn pump(
    stream: Stream,
    mut frames: mpsc::UnboundedReceiver<Vec<u8>>,
    killed: oneshot::Receiver<()>,
    mut on_bytes: impl FnMut(Vec<u8>) -> bool,
) -> (String, bool) {
    let Stream { mut reader, mut writer, child } = stream;
    let mut got_bytes = false;
    let read = async {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => return "the target closed the connection".to_owned(),
                Ok(n) => {
                    got_bytes = true;
                    if !on_bytes(buf[..n].to_vec()) {
                        return "the hub is stopping".to_owned();
                    }
                }
                Err(e) => return format!("read failed: {e}"),
            }
        }
    };
    let write = async {
        while let Some(frame) = frames.recv().await {
            if let Err(e) = writer.write_all(&frame).await {
                return format!("write failed: {e}");
            }
            if let Err(e) = writer.flush().await {
                return format!("write failed: {e}");
            }
        }
        "the hub dropped the connection".to_owned()
    };
    let reason = tokio::select! {
        reason = read => reason,
        reason = write => reason,
        _ = killed => "the connection was dropped".to_owned(),
    };
    drop(child);
    (reason, got_bytes)
}
