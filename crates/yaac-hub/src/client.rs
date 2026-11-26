//! The window side of the attach protocol: connect to the hub's socket (or, from another
//! device, to its Tailscale listener: [`HubClient::connect_tcp`]), then exchange
//! [`Message`]s without ever blocking the caller.
//!
//! ```text
//! let link = HubClient::connect(&HubPaths::from_env()?.socket())?;   // fails if no hub runs
//! link.send(Message::request(RpcId::Number(1), "hub/hello", json!({ "build": BUILD })))?;
//! // every frame:
//! while let Some(message) = link.try_recv() { … }
//! ```
//!
//! Two small threads per client do the socket I/O (on Windows, one thread running the pipe on
//! a small async runtime, since a synchronous pipe handle serialises reads and writes):
//! `send` only queues, and `try_recv` only looks at what has arrived. An optional waker runs whenever a message arrives, so a GUI can
//! request a repaint instead of polling. Starting the hub is the caller's concern; see
//! [`connect_or_start`].
//!
//! The shape matches the window's `HubLink` trait (`send` queues, a non-blocking receive):
//! `impl HubLink for HubClient { fn send(&mut self, m) { let _ = HubClient::send(self, m); }
//! fn poll(&mut self) -> Option<Message> { self.try_recv() } }`.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use yaac_protocol::StreamId;
use yaac_protocol::frame::Frame;
use yaac_protocol::message_chunks;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message, RpcId};

/// Sending failed: the connection to the hub is gone.
#[derive(Debug, thiserror::Error)]
#[error("the hub connection is closed")]
pub struct Disconnected;

/// A connection to a hub.
pub struct HubClient {
    outgoing: crate::attach_writer::Sender,
    incoming: mpsc::Receiver<Message>,
    data: mpsc::Receiver<(StreamId, Vec<u8>)>,
    held: Mutex<VecDeque<Message>>,
    connected: Arc<AtomicBool>,
    next_id: AtomicI64,
    closer: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Drop for HubClient {
    /// Closes the socket, so the hub sees the window detach at once (the reader thread holds
    /// a handle to it and would otherwise keep it open).
    fn drop(&mut self) {
        if let Some(close) = self.closer.take() {
            close();
        }
    }
}

impl std::fmt::Debug for HubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubClient").field("connected", &self.is_connected()).finish_non_exhaustive()
    }
}

impl HubClient {
    /// Connects to the hub's socket. Fails at once if no hub is listening.
    pub fn connect(socket: &Path) -> std::io::Result<Self> {
        Self::connect_with_waker(socket, || {})
    }

    /// Connects, and calls `waker` (from the client's reader thread) whenever a message
    /// arrives or the connection closes.
    #[cfg(unix)]
    pub fn connect_with_waker(socket: &Path, waker: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let stream = std::os::unix::net::UnixStream::connect(socket)?;
        let reader = stream.try_clone()?;
        let closer = stream.try_clone()?;
        let mut client = Self::start(reader, stream, waker)?;
        client.closer = Some(Box::new(move || {
            let _ = closer.shutdown(std::net::Shutdown::Both);
        }));
        Ok(client)
    }

    /// Connects to the hub's named pipe, refusing one that belongs to another user, and calls
    /// `waker` whenever a message arrives or the connection closes.
    #[cfg(windows)]
    pub fn connect_with_waker(socket: &Path, waker: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        use tokio::net::windows::named_pipe::ClientOptions;
        const ERROR_PIPE_BUSY: i32 = 231;
        let runtime = tokio::runtime::Builder::new_current_thread().enable_io().build()?;
        let pipe = {
            let _context = runtime.enter();
            // Every instance busy: the hub is between accepting one window and offering the
            // next instance. Retry briefly.
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match ClientOptions::new().open(socket.as_os_str()) {
                    Ok(pipe) => break pipe,
                    Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        if !yaac_winsys::owned_by_current_user(&pipe)? {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the hub pipe belongs to another user"));
        }
        let connected = Arc::new(AtomicBool::new(true));
        let (out_tx, mut out_rx) = crate::attach_writer::channel();
        let (data_tx, data) = mpsc::channel();
        let (in_tx, in_rx) = mpsc::channel::<Message>();
        let close = Arc::new(tokio::sync::Notify::new());
        let (alive, closed) = (connected.clone(), close.clone());
        std::thread::Builder::new().name("yaac-hub-client-pipe".into()).spawn(move || {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            runtime.block_on(async move {
                let (mut reader, mut writer) = tokio::io::split(pipe);
                let read = async {
                    let mut decoder = message_chunks::Decoder::new(true);
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        let n = match reader.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        decoder.push(&buf[..n]);
                        if !deliver(&mut decoder, &in_tx, &data_tx, &waker) {
                            break;
                        }
                    }
                };
                let write = async {
                    let mut frame = Vec::new();
                    while let Some(message) = out_rx.recv().await {
                        if message.encode(&mut frame).is_err() {
                            continue;
                        }
                        if writer.write_all(&frame).await.is_err() || writer.flush().await.is_err() {
                            break;
                        }
                    }
                };
                tokio::select! {
                    () = read => {}
                    () = write => {}
                    () = closed.notified() => {}
                }
                alive.store(false, Ordering::SeqCst);
                waker();
            });
        })?;
        Ok(Self {
            outgoing: out_tx,
            incoming: in_rx,
            data,
            held: Mutex::new(VecDeque::new()),
            connected,
            next_id: AtomicI64::new(1),
            closer: Some(Box::new(move || close.notify_one())),
        })
    }

    #[cfg(not(any(unix, windows)))]
    pub fn connect_with_waker(_socket: &Path, _waker: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "the hub client is not available on this platform yet"))
    }

    /// Connects to a hub's Tailscale attach listener (docs/02-architecture.md#attaching-over-tailscale)
    /// — the Android client's way in. The same frames and methods as the local socket; the hub
    /// closes the connection at once unless this device is on the hub host's own Tailscale login.
    /// Calls `waker` whenever a message arrives or the connection closes.
    pub fn connect_tcp(addr: std::net::SocketAddr, waker: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
        stream.set_nodelay(true)?;
        crate::tailnet::keep_alive(&socket2::SockRef::from(&stream))?;
        let reader = stream.try_clone()?;
        let closer = stream.try_clone()?;
        let mut client = Self::start(reader, stream, waker)?;
        client.closer = Some(Box::new(move || {
            let _ = closer.shutdown(std::net::Shutdown::Both);
        }));
        Ok(client)
    }

    fn start(
        mut reader: impl Read + Send + 'static,
        mut writer: impl Write + Send + 'static,
        waker: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let connected = Arc::new(AtomicBool::new(true));
        let (out_tx, mut out_rx) = crate::attach_writer::channel();
        let (data_tx, data) = mpsc::channel();
        let (in_tx, in_rx) = mpsc::channel::<Message>();
        let alive = connected.clone();
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        std::thread::Builder::new().name("yaac-hub-client-write".into()).spawn(move || {
            let mut frame = Vec::new();
            while let Some(message) = runtime.block_on(out_rx.recv()) {
                if message.encode(&mut frame).is_err() {
                    continue;
                }
                if writer.write_all(&frame).and_then(|()| writer.flush()).is_err() {
                    alive.store(false, Ordering::SeqCst);
                    break;
                }
            }
        })?;
        let alive = connected.clone();
        std::thread::Builder::new().name("yaac-hub-client-read".into()).spawn(move || {
            let mut decoder = message_chunks::Decoder::new(true);
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                decoder.push(&buf[..n]);
                if !deliver(&mut decoder, &in_tx, &data_tx, &waker) {
                    break;
                }
            }
            alive.store(false, Ordering::SeqCst);
            waker();
        })?;
        Ok(Self {
            outgoing: out_tx,
            incoming: in_rx,
            data,
            held: Mutex::new(VecDeque::new()),
            connected,
            next_id: AtomicI64::new(1),
            closer: None,
        })
    }

    /// Queues a message for the hub. Never blocks.
    pub fn send(&self, message: Message) -> Result<(), Disconnected> {
        if !self.is_connected() {
            return Err(Disconnected);
        }
        if matches!(&message, Message::Response { .. }) {
            // Replies to the hub's browser calls can carry screenshots and page snapshots.
            self.outgoing.bulk_reply(message).map_err(|_| Disconnected)
        } else {
            self.outgoing.send(message).map_err(|_| Disconnected)
        }
    }

    /// Queues terminal input within the stream's available credit.
    pub fn send_data(&self, stream: StreamId, bytes: Vec<u8>) -> Result<(), Disconnected> {
        use yaac_protocol::schedule::CHUNK_MIN;
        if !self.is_connected() {
            return Err(Disconnected);
        }
        if bytes.len() <= CHUNK_MIN {
            return self.outgoing.data(stream, bytes).map_err(|_| Disconnected);
        }
        for chunk in bytes.chunks(CHUNK_MIN) {
            self.outgoing.data(stream, chunk.to_vec()).map_err(|_| Disconnected)?;
        }
        Ok(())
    }

    /// Receives terminal output. Return credit once the consumer accepts these bytes.
    pub fn try_recv_data(&self) -> Option<(StreamId, Vec<u8>)> {
        self.data.try_recv().ok()
    }

    /// Waits for terminal bytes on a worker thread.
    pub fn recv_data_timeout(&self, timeout: Duration) -> Result<(StreamId, Vec<u8>), mpsc::RecvTimeoutError> {
        self.data.recv_timeout(timeout)
    }

    /// The next message from the hub, if one has arrived. Never blocks.
    pub fn try_recv(&self) -> Option<Message> {
        if let Some(held) = self.held.lock().ok().and_then(|mut h| h.pop_front()) {
            return Some(held);
        }
        self.incoming.try_recv().ok()
    }

    /// Closes the connection now (also done on drop).
    pub fn close(&mut self) {
        if let Some(close) = self.closer.take() {
            close();
        }
    }

    /// Whether the connection is still open (messages already received can still be read).
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// A fresh request id (this client's own id space).
    pub fn next_id(&self) -> RpcId {
        RpcId::Number(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Sends a request and **blocks** until its answer arrives — for tools, tests, and a
    /// command line, never for a GUI thread. Other messages that arrive meanwhile are kept for
    /// [`HubClient::try_recv`].
    pub fn request_blocking(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, ErrorObject> {
        let id = self.next_id();
        self.send(Message::request(id.clone(), method, params))
            .map_err(|_| ErrorObject::new(ErrorCode::InternalError, "the hub connection is closed"))?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ErrorObject::new(ErrorCode::InternalError, format!("no answer to {method} within {timeout:?}")));
            }
            match self.incoming.recv_timeout(left.min(Duration::from_millis(50))) {
                Ok(Message::Response { id: rid, result }) if rid == id => return Ok(result),
                Ok(Message::Error { id: Some(rid), error }) if rid == id => return Err(error),
                Ok(other) => {
                    if let Ok(mut held) = self.held.lock() {
                        held.push_back(other);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(ErrorObject::new(ErrorCode::InternalError, "the hub connection closed"));
                }
            }
        }
    }
}

/// Hands every complete message in `decoder` to the reader side. False when the connection
/// should end (garbage from the hub, or nobody listening any more).
fn deliver(decoder: &mut message_chunks::Decoder, in_tx: &mpsc::Sender<Message>, data_tx: &mpsc::Sender<(StreamId, Vec<u8>)>, waker: &impl Fn()) -> bool {
    loop {
        match decoder.next_frame() {
            Ok(Some(Frame::Message(payload))) => {
                let Ok(message) = Message::from_json(&payload) else { return false };
                if in_tx.send(message).is_err() {
                    return false;
                }
                waker();
            }
            Ok(Some(Frame::Data { stream, bytes })) => {
                if data_tx.send((stream, bytes)).is_err() { return false; }
                waker();
            }
            Ok(None) => return true,
            Err(_) => return false,
        }
    }
}

/// Connects to a running hub, or calls `start` (which should launch `yaac hub` detached) and
/// waits up to `timeout` for it to listen. `start` is called at most once.
pub fn connect_or_start(
    socket: &Path,
    start: impl FnOnce() -> std::io::Result<()>,
    timeout: Duration,
) -> std::io::Result<HubClient> {
    if let Ok(client) = HubClient::connect(socket) {
        return Ok(client);
    }
    start()?;
    let deadline = Instant::now() + timeout;
    loop {
        match HubClient::connect(socket) {
            Ok(client) => return Ok(client),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}
