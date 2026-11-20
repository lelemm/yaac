//! The attach server: a user-only local socket speaking [`yaac_protocol::attach`] in the same
//! frames as the YAAC protocol. Several windows may attach at once; each is one task pair
//! (reader, writer) feeding the hub core.
//!
//! The platform part is [`Listener`]: a Unix socket now (mode `0600` in a `0700` directory).
//! A Windows named pipe restricted to the current user slots in behind the same two calls.

use std::path::{Path, PathBuf};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use yaac_protocol::frame::{Frame, FrameDecoder, encode_message_frame};
use yaac_protocol::rpc::Message;

use crate::core::{ClientKey, CoreEvent};

/// The platform listener.
#[derive(Debug)]
pub(crate) struct Listener {
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    path: PathBuf,
}

/// The socket path is too long for the platform, or binding failed.
#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    /// The path does not fit in `sun_path`.
    #[error("the hub socket path is too long ({0} bytes): set YAAC_RUNTIME_DIR to a shorter directory")]
    PathTooLong(usize),
    /// Binding failed.
    #[error("cannot listen on {path}: {source}")]
    Io {
        /// The socket.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// Not built for this platform yet.
    #[error("the attach server is not available on this platform yet")]
    Unsupported,
}

impl Listener {
    /// Binds the socket. The caller holds the hub lock, so an existing file is stale.
    #[cfg(unix)]
    pub(crate) fn bind(path: &Path) -> Result<Self, ListenError> {
        let len = path.as_os_str().len();
        if len > crate::paths::MAX_SOCKET_PATH {
            return Err(ListenError::PathTooLong(len));
        }
        let io = |source| ListenError::Io { path: path.display().to_string(), source };
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e)),
        }
        let inner = tokio::net::UnixListener::bind(path).map_err(io)?;
        crate::paths::make_private_file(path).map_err(io)?;
        Ok(Self { inner, path: path.to_owned() })
    }

    #[cfg(not(unix))]
    pub(crate) fn bind(_path: &Path) -> Result<Self, ListenError> {
        Err(ListenError::Unsupported)
    }

    /// Accepts the next window.
    #[cfg(unix)]
    async fn accept(&self) -> std::io::Result<(impl AsyncRead + Send + Unpin + 'static, impl AsyncWrite + Send + Unpin + 'static)> {
        let (stream, _) = self.inner.accept().await?;
        Ok(stream.into_split())
    }

    #[cfg(not(unix))]
    async fn accept(&self) -> std::io::Result<(tokio::io::Empty, tokio::io::Sink)> {
        Err(std::io::Error::other("unsupported"))
    }

    /// Removes the socket file.
    pub(crate) fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Accepts windows until the hub stops. Returns the writer tasks so the hub can let them
/// flush their last messages before it exits.
pub(crate) async fn serve(listener: &Listener, core: mpsc::UnboundedSender<CoreEvent>, writers: mpsc::UnboundedSender<JoinHandle<()>>) {
    let mut next: ClientKey = 0;
    loop {
        let Ok((reader, writer)) = listener.accept().await else { continue };
        next += 1;
        let client = next;
        let (tx, rx) = mpsc::unbounded_channel();
        if core.send(CoreEvent::Attached { client, tx }).is_err() {
            return;
        }
        let _ = writers.send(tokio::spawn(write_loop(writer, rx)));
        tokio::spawn(read_loop(client, reader, core.clone()));
    }
}

async fn read_loop(client: ClientKey, mut reader: impl AsyncRead + Unpin, core: mpsc::UnboundedSender<CoreEvent>) {
    let mut decoder = FrameDecoder::new();
    let mut buf = vec![0u8; 64 * 1024];
    'outer: loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        decoder.push(&buf[..n]);
        loop {
            match decoder.next_frame() {
                Ok(Some(Frame::Message(payload))) => match Message::from_json(&payload) {
                    Ok(message) => {
                        if core.send(CoreEvent::FromClient { client, message }).is_err() {
                            return;
                        }
                    }
                    Err(_) => break 'outer, // a window that sends garbage is dropped
                },
                Ok(Some(Frame::Data { .. })) => {} // no data streams on the attach protocol yet
                Ok(None) => break,
                Err(_) => break 'outer,
            }
        }
    }
    let _ = core.send(CoreEvent::Detached { client });
}

async fn write_loop(mut writer: impl AsyncWrite + Unpin, mut rx: mpsc::UnboundedReceiver<Message>) {
    let mut frame = Vec::new();
    while let Some(message) = rx.recv().await {
        frame.clear();
        if encode_message_frame(&message.to_json(), &mut frame).is_err() {
            continue; // larger than a frame: a hub bug; drop it rather than the window
        }
        if writer.write_all(&frame).await.is_err() || writer.flush().await.is_err() {
            break;
        }
    }
    let _ = writer.shutdown().await;
}
