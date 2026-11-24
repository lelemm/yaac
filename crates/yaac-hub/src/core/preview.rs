//! The hub's loopback listeners. Tokio tasks own the sockets; the core owns the tunnel
//! streams and moves their bytes through the target session's existing bulk scheduler.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use yaac_protocol::attach::{
    PreviewCloseParams, PreviewKind, PreviewListResult, PreviewListener, PreviewOpenParams, PreviewOpenResult,
    PreviewProxyParams, PreviewProxyResult,
};
use yaac_protocol::methods::{StreamCredit, TunnelClose, TunnelOpen};
use yaac_protocol::schedule::ReceiveWindow;
use yaac_protocol::{Capability, StreamCreditParams, StreamId, TargetId, TunnelCloseParams, TunnelOpenParams};

use super::{Conn, Core, CoreEvent, ErrorCode, ErrorObject, Pending, Target, Value, err, json, to_value};

const WINDOW: u64 = 64 * 1024;
const MAX_CONNECTIONS: usize = 256;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const SUCCESS: u8 = 0x00;
const FAILURE: u8 = 0x01;
const NOT_ALLOWED: u8 = 0x02;
const REFUSED: u8 = 0x05;
const COMMAND_UNSUPPORTED: u8 = 0x07;
const ADDRESS_UNSUPPORTED: u8 = 0x08;

#[derive(Debug)]
pub(crate) enum Event {
    Accepted {
        listener: u64,
        socket: TcpStream,
        received: oneshot::Sender<()>,
    },
    Connect {
        socket: u64,
        destination: Destination,
    },
    Bytes {
        socket: u64,
        bytes: Vec<u8>,
    },
    Written {
        socket: u64,
        bytes: usize,
    },
    Finished {
        socket: u64,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct Destination {
    host: Option<String>,
    port: u16,
}

#[derive(Debug)]
struct Task(JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Debug)]
struct Listener {
    target: TargetId,
    destination: Option<Destination>,
    local_port: u16,
    _task: Task,
}

#[derive(Debug)]
struct Socket {
    listener: u64,
    target: TargetId,
    reply: Option<oneshot::Sender<u8>>,
    permits: mpsc::Sender<usize>,
    reading: bool,
    finished: bool,
    // Bookkeeping in ReceiveWindow bounds the bytes in this queue and the socket writer.
    downstream: Option<mpsc::UnboundedSender<Vec<u8>>>,
    receive: Option<(StreamId, ReceiveWindow)>,
    _task: Task,
}

#[derive(Debug, Default)]
pub(super) struct Previews {
    serial: u64,
    listeners: HashMap<u64, Listener>,
    sockets: HashMap<u64, Socket>,
}

impl Previews {
    fn next_id(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    fn listener_id(&self, target: &TargetId, port: Option<u16>) -> Option<u64> {
        self.listeners.iter().find_map(|(&id, listener)| {
            (&listener.target == target && listener.destination.as_ref().map(|d| d.port) == port).then_some(id)
        })
    }

    /// Reserve at most one chunk per socket. The scheduler owns credit and takes it when
    /// it hands the frame to the target writer. Keep one read or one queued chunk per socket,
    /// even if the target grants a very large window.
    pub(super) fn grant_reads(&mut self, conn: &mut Conn) {
        let scheduler = conn.session.scheduler();
        let mut drained = Vec::new();
        for (&stream, socket) in &conn.preview_streams {
            let Some(state) = self.sockets.get_mut(socket) else {
                continue;
            };
            if scheduler.pending_data(stream) != Some(0) {
                continue;
            }
            if state.finished {
                drained.push((stream, *socket));
                continue;
            }
            let count = scheduler.credit(stream).unwrap_or(0).min(scheduler.chunk_size() as u64) as usize;
            if !state.reading && count > 0 && state.permits.try_send(count).is_ok() {
                state.reading = true;
            }
        }
        for (stream, socket) in drained {
            self.sockets.remove(&socket);
            close_stream(conn, stream);
        }
    }
}

impl Core {
    pub(super) fn preview_check(&self, target: &TargetId) -> Result<(), ErrorObject> {
        let target = self
            .targets
            .get(target)
            .ok_or_else(|| err(ErrorCode::TargetUnknown, "no such target"))?;
        let conn = target
            .conn
            .as_ref()
            .filter(|c| c.ready)
            .ok_or_else(|| err(ErrorCode::TargetOffline, "the target is not connected"))?;
        if !conn
            .session
            .features()
            .is_some_and(|f| f.allows(&Capability::PortForward))
        {
            return Err(err(
                ErrorCode::CapabilityMissing,
                "the target daemon does not support port forwarding (portForward)",
            ));
        }
        Ok(())
    }

    pub(super) fn preview_proxy(&mut self, params: PreviewProxyParams) -> Result<Value, ErrorObject> {
        let socks_port = self.preview_bind(params.target, None)?;
        Ok(to_value(&PreviewProxyResult { socks_port }))
    }

    pub(super) fn preview_open(&mut self, params: PreviewOpenParams) -> Result<Value, ErrorObject> {
        if params.port == 0 {
            return Err(err(
                ErrorCode::InvalidParams,
                "the target port must be between 1 and 65535",
            ));
        }
        let host = params
            .host
            .as_deref()
            .map(|host| {
                destination_host(host.as_bytes()).ok_or_else(|| {
                    err(ErrorCode::InvalidParams, "the preview host must be a host name or address")
                })
            })
            .transpose()?;
        let local_port = self.preview_bind(
            params.target,
            Some(Destination {
                host,
                port: params.port,
            }),
        )?;
        Ok(to_value(&PreviewOpenResult {
            url: format!("http://127.0.0.1:{local_port}/"),
            local_port,
        }))
    }

    fn preview_bind(&mut self, target: TargetId, destination: Option<Destination>) -> Result<u16, ErrorObject> {
        let ready = self.preview_check(&target);
        let port = destination.as_ref().map(|d| d.port);
        if let Some(id) = self.previews.listener_id(&target, port)
            && (ready.is_ok() || ready.as_ref().is_err_and(|e| e.code == ErrorCode::TargetOffline))
        {
            return Ok(self.previews.listeners[&id].local_port);
        }
        ready?;
        let address = |port| (Ipv4Addr::LOCALHOST, port);
        let listener = std::net::TcpListener::bind(address(port.unwrap_or(0)))
            .or_else(|error| match (port, error.kind()) {
                (Some(_), io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied) => {
                    std::net::TcpListener::bind(address(0))
                }
                _ => Err(error),
            })
            .and_then(|socket| {
                socket.set_nonblocking(true)?;
                TcpListener::from_std(socket)
            })
            .map_err(preview_bind_error)?;
        let local_port = listener.local_addr().map_err(preview_bind_error)?.port();
        let id = self.previews.next_id();
        let task = Task(tokio::spawn(accept(listener, id, self.tx.clone())));
        self.previews.listeners.insert(
            id,
            Listener {
                target,
                destination,
                local_port,
                _task: task,
            },
        );
        Ok(local_port)
    }

    pub(super) fn preview_proxy_close(&mut self, params: PreviewProxyParams) -> Result<Value, ErrorObject> {
        self.preview_close_listener(&params.target, None)
    }

    pub(super) fn preview_close(&mut self, params: PreviewCloseParams) -> Result<Value, ErrorObject> {
        self.preview_close_listener(&params.target, Some(params.port))
    }

    fn preview_close_listener(&mut self, target: &TargetId, port: Option<u16>) -> Result<Value, ErrorObject> {
        if !self.targets.contains_key(target) {
            return Err(err(ErrorCode::TargetUnknown, "no such target"));
        }
        if let Some(id) = self.previews.listener_id(target, port) {
            self.previews.listeners.remove(&id);
            let sockets: Vec<_> = self
                .previews
                .sockets
                .iter()
                .filter_map(|(&socket, s)| (s.listener == id).then_some(socket))
                .collect();
            for socket in sockets {
                self.preview_finish(socket);
            }
            self.flush(target);
        }
        Ok(json!({}))
    }

    pub(super) fn preview_list(&self) -> Value {
        let mut counts = HashMap::<u64, u32>::new();
        for s in self.previews.sockets.values() {
            *counts.entry(s.listener).or_default() += 1;
        }
        let mut list: PreviewListResult = self
            .previews
            .listeners
            .iter()
            .map(|(&id, listener)| PreviewListener {
                target: listener.target.clone(),
                kind: if listener.destination.is_some() {
                    PreviewKind::Forward
                } else {
                    PreviewKind::Proxy
                },
                port: listener.destination.as_ref().map(|d| d.port),
                local_port: listener.local_port,
                connections: counts.get(&id).copied().unwrap_or_default(),
            })
            .collect();
        list.sort_by(|a, b| (&a.target, a.port).cmp(&(&b.target, b.port)));
        to_value(&list)
    }

    pub(super) fn preview_disconnected(&mut self, target: &TargetId) {
        // The entire target session is gone. Drop all local sockets, including handshakes.
        self.previews.sockets.retain(|_, socket| &socket.target != target);
    }

    pub(super) fn preview_removed(&mut self, target: &TargetId) {
        self.previews.listeners.retain(|_, listener| &listener.target != target);
    }

    pub(super) fn preview_event(&mut self, event: Event) {
        match event {
            Event::Accepted {
                listener,
                socket,
                received,
            } => {
                self.preview_accept(listener, socket);
                let _ = received.send(());
            }
            Event::Connect { socket, destination } => self.preview_connect(socket, destination),
            Event::Bytes { socket, bytes } => self.preview_send_data(socket, bytes),
            Event::Written { socket, bytes } => self.preview_consumed(socket, bytes),
            Event::Finished { socket } => {
                let Some(state) = self.previews.sockets.get_mut(&socket) else {
                    return;
                };
                if state.receive.is_some() {
                    // A local EOF can arrive while its last chunk is still queued. Let that
                    // chunk reach the target writer before sending tunnel/close.
                    state.finished = true;
                    let target = state.target.clone();
                    self.flush(&target);
                } else {
                    self.preview_finish(socket);
                }
            }
        }
    }

    fn preview_accept(&mut self, id: u64, socket: TcpStream) {
        let Some(listener) = self.previews.listeners.get(&id) else {
            return;
        };
        if self.preview_check(&listener.target).is_err()
            || self
                .previews
                .sockets
                .values()
                .filter(|s| s.target == listener.target)
                .count()
                >= MAX_CONNECTIONS
        {
            return;
        }
        let target = listener.target.clone();
        let destination = listener.destination.clone();
        let key = self.previews.next_id();
        let _ = socket.set_nodelay(true);
        let (reply, outcome) = oneshot::channel();
        let (permits, reads) = mpsc::channel(1);
        let (downstream, writes) = mpsc::unbounded_channel();
        let core = self.tx.clone();
        let task = Task(tokio::spawn(async move {
            let _ = run_socket(key, socket, destination, outcome, reads, writes, &core).await;
            let _ = core.send(CoreEvent::Preview(Event::Finished { socket: key }));
        }));
        self.previews.sockets.insert(
            key,
            Socket {
                listener: id,
                target,
                reply: Some(reply),
                permits,
                reading: false,
                finished: false,
                downstream: Some(downstream),
                receive: None,
                _task: task,
            },
        );
    }

    fn preview_connect(&mut self, socket: u64, destination: Destination) {
        let Some(s) = self.previews.sockets.get(&socket) else {
            return;
        };
        let target = s.target.clone();
        // Limit requests whose local socket left before the open response arrived, too.
        let opening = self.targets.get(&target).and_then(|t| t.conn.as_ref()).map_or(0, |c| {
            c.pending
                .values()
                .filter(|p| matches!(p, Pending::PreviewOpen { .. }))
                .count()
        });
        if opening < MAX_CONNECTIONS
            && self.preview_check(&target).is_ok()
            && self.send::<TunnelOpen>(
                &target,
                &TunnelOpenParams {
                    host: destination.host,
                    port: destination.port,
                    initial_credit: WINDOW,
                },
                Pending::PreviewOpen { socket },
            )
        {
            self.flush(&target);
        } else {
            self.preview_reply(socket, FAILURE);
        }
    }

    fn preview_reply(&mut self, socket: u64, status: u8) -> bool {
        self.previews
            .sockets
            .get_mut(&socket)
            .and_then(|s| s.reply.take())
            .is_some_and(|reply| reply.send(status).is_ok())
    }

    pub(super) fn preview_opened(&mut self, target: &TargetId, socket: u64, outcome: Result<Value, ErrorObject>) {
        let opened = outcome.and_then(|wire| self.decode_typed::<TunnelOpen>(target, wire));
        let result = match opened {
            Ok(result) => result,
            Err(error) => {
                let code = match error.code {
                    ErrorCode::ConnectionRefused => REFUSED,
                    // The target rejected the destination itself.
                    ErrorCode::InvalidParams => NOT_ALLOWED,
                    _ => FAILURE,
                };
                self.preview_reply(socket, code);
                return;
            }
        };
        let stream = result.stream;
        let Some(conn) = self.targets.get_mut(target).and_then(Target::ready_conn) else {
            return;
        };
        if conn.session.open_stream(stream, result.initial_credit).is_err() {
            self.preview_reply(socket, FAILURE);
            return;
        }
        if self.preview_reply(socket, SUCCESS)
            && let Some(s) = self.previews.sockets.get_mut(&socket)
        {
            s.receive = Some((stream, ReceiveWindow::new(stream, WINDOW)));
            if let Some(conn) = self.targets.get_mut(target).and_then(Target::ready_conn) {
                conn.preview_streams.insert(stream, socket);
            }
        } else {
            // The listener or local socket closed while the daemon was connecting.
            self.preview_close_stream(target, stream);
        }
    }

    fn preview_send_data(&mut self, socket: u64, bytes: Vec<u8>) {
        let Some(s) = self.previews.sockets.get_mut(&socket) else {
            return;
        };
        s.reading = false;
        let Some((stream, _)) = s.receive else { return };
        let target = s.target.clone();
        if let Some(conn) = self.targets.get_mut(&target).and_then(Target::ready_conn)
            && conn.session.send_data(stream, bytes).is_err()
        {
            self.preview_finish(socket);
        }
        self.flush(&target);
    }

    pub(super) fn preview_data(&mut self, target: &TargetId, stream: StreamId, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let Some(socket) = self.preview_stream_socket(target, stream) else {
            return;
        };
        let Some(s) = self.previews.sockets.get_mut(&socket) else {
            return;
        };
        if let Some((_, window)) = s.receive.as_mut()
            && window.on_received(bytes.len()).is_ok()
            && s.downstream.as_ref().is_some_and(|tx| tx.send(bytes).is_ok())
        {
            return;
        }
        self.preview_finish(socket);
    }

    fn preview_consumed(&mut self, socket: u64, bytes: usize) {
        let Some(s) = self.previews.sockets.get_mut(&socket) else {
            return;
        };
        let Some((stream, window)) = s.receive.as_mut() else {
            return;
        };
        let Some(bytes) = window.on_consumed(bytes) else {
            return;
        };
        let target = s.target.clone();
        if let Some(conn) = self.targets.get_mut(&target).and_then(Target::ready_conn) {
            let _ = conn
                .session
                .notify::<StreamCredit>(&StreamCreditParams { stream: *stream, bytes });
            self.flush(&target);
        }
    }

    fn preview_stream_socket(&self, target: &TargetId, stream: StreamId) -> Option<u64> {
        self.targets
            .get(target)?
            .conn
            .as_ref()?
            .preview_streams
            .get(&stream)
            .copied()
    }

    pub(super) fn preview_remote_close(&mut self, target: &TargetId, stream: StreamId) {
        let Some(socket) = self.preview_stream_socket(target, stream) else {
            return;
        };
        if let Some(conn) = self.targets.get_mut(target).and_then(Target::ready_conn) {
            conn.preview_streams.remove(&stream);
            conn.session.cancel_stream(stream);
        }
        if let Some(s) = self.previews.sockets.get_mut(&socket) {
            s.receive = None;
            // Let the local socket writer drain frames that arrived before the close.
            s.downstream.take();
        }
    }

    fn preview_finish(&mut self, socket: u64) {
        let Some(s) = self.previews.sockets.remove(&socket) else {
            return;
        };
        if let Some((stream, _)) = s.receive {
            self.preview_close_stream(&s.target, stream);
            self.flush(&s.target);
        }
    }

    fn preview_close_stream(&mut self, target: &TargetId, stream: StreamId) {
        if let Some(conn) = self.targets.get_mut(target).and_then(Target::ready_conn) {
            close_stream(conn, stream);
        }
    }
}

fn close_stream(conn: &mut Conn, stream: StreamId) {
    conn.preview_streams.remove(&stream);
    conn.session.cancel_stream(stream);
    let _ = conn
        .session
        .notify::<TunnelClose>(&TunnelCloseParams { stream, reason: None });
}

fn preview_bind_error(error: io::Error) -> ErrorObject {
    err(
        ErrorCode::InternalError,
        format!("cannot bind preview listener on 127.0.0.1: {error}"),
    )
}

async fn accept(listener: TcpListener, id: u64, core: mpsc::UnboundedSender<CoreEvent>) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            // Resource exhaustion can make accept fail repeatedly.
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        // Keep at most one undelivered socket per listener in the core's queue.
        let (received, wait) = oneshot::channel();
        if core
            .send(CoreEvent::Preview(Event::Accepted {
                listener: id,
                socket,
                received,
            }))
            .is_err()
            || wait.await.is_err()
        {
            return;
        }
    }
}

async fn run_socket(
    key: u64,
    mut socket: TcpStream,
    destination: Option<Destination>,
    outcome: oneshot::Receiver<u8>,
    mut permits: mpsc::Receiver<usize>,
    mut downstream: mpsc::UnboundedReceiver<Vec<u8>>,
    core: &mpsc::UnboundedSender<CoreEvent>,
) -> io::Result<()> {
    let proxy = destination.is_none();
    let destination = match destination {
        Some(destination) => destination,
        None => match tokio::time::timeout(HANDSHAKE_TIMEOUT, socks_destination(&mut socket)).await? {
            Ok(destination) => destination,
            Err(SocksError::Reply(code)) => return socks_reply(&mut socket, code).await,
            Err(SocksError::Io(error)) => return Err(error),
            Err(SocksError::Closed) => return Ok(()),
        },
    };
    let report = |event| {
        core.send(CoreEvent::Preview(event))
            .map_err(|_| io::ErrorKind::BrokenPipe)
    };
    report(Event::Connect {
        socket: key,
        destination,
    })?;
    let mut outcome = outcome;
    let mut first = [0];
    let status = tokio::select! {
        status = &mut outcome => status.unwrap_or(FAILURE),
        peeked = socket.peek(&mut first) => {
            if peeked? == 0 {
                return Ok(());
            }
            outcome.await.unwrap_or(FAILURE)
        }
    };
    if proxy {
        socks_reply(&mut socket, status).await?;
    }
    if status != SUCCESS {
        return Ok(());
    }
    let (mut reader, mut writer) = socket.split();
    let read = async {
        loop {
            let mut first = [0];
            let count = tokio::select! {
                biased;
                count = permits.recv() => count,
                peeked = reader.peek(&mut first) => {
                    if peeked? == 0 {
                        break;
                    }
                    permits.recv().await
                }
            };
            let Some(count) = count else { break };
            reader.readable().await?;
            let mut bytes = vec![0; count];
            let n = reader.read(&mut bytes).await?;
            if n == 0 {
                break;
            }
            bytes.truncate(n);
            report(Event::Bytes { socket: key, bytes })?;
        }
        Ok(())
    };
    let write = async {
        while let Some(bytes) = downstream.recv().await {
            writer.write_all(&bytes).await?;
            report(Event::Written {
                socket: key,
                bytes: bytes.len(),
            })?;
        }
        writer.shutdown().await
    };
    tokio::select! {
        result = read => result,
        result = write => result,
    }
}

/// Any host name or address, passed on unresolved: the target resolves and reaches it (P323).
/// The target checks it again; this only refuses what cannot be a host at all.
fn destination_host(host: &[u8]) -> Option<String> {
    let host = std::str::from_utf8(host).ok()?;
    (!host.is_empty() && host.len() <= 255 && !host.chars().any(|c| c.is_control() || c.is_whitespace()))
        .then(|| host.to_owned())
}

enum SocksError {
    Reply(u8),
    Io(io::Error),
    Closed,
}

impl From<io::Error> for SocksError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

async fn socks_destination(socket: &mut TcpStream) -> Result<Destination, SocksError> {
    let version = socket.read_u8().await?;
    if version != 5 {
        return Err(SocksError::Closed);
    }
    let n = socket.read_u8().await?;
    let mut methods = vec![0; usize::from(n)];
    socket.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        socket.write_all(&[5, 0xff]).await?;
        return Err(SocksError::Closed);
    }
    socket.write_all(&[5, 0]).await?;
    let mut header = [0; 4];
    socket.read_exact(&mut header).await?;
    if header[0] != 5 || header[2] != 0 {
        return Err(SocksError::Reply(FAILURE));
    }
    if header[1] != 1 {
        return Err(SocksError::Reply(COMMAND_UNSUPPORTED));
    }
    let host = match header[3] {
        1 => {
            let mut octets = [0; 4];
            socket.read_exact(&mut octets).await?;
            Some(Ipv4Addr::from(octets).to_string())
        }
        3 => {
            let n = socket.read_u8().await?;
            let mut domain = vec![0; usize::from(n)];
            socket.read_exact(&mut domain).await?;
            destination_host(&domain)
        }
        4 => {
            let mut octets = [0; 16];
            socket.read_exact(&mut octets).await?;
            Some(Ipv6Addr::from(octets).to_string())
        }
        _ => return Err(SocksError::Reply(ADDRESS_UNSUPPORTED)),
    };
    let port = socket.read_u16().await?;
    let host = host.ok_or(SocksError::Reply(NOT_ALLOWED))?;
    if port == 0 {
        return Err(SocksError::Reply(FAILURE));
    }
    Ok(Destination { host: Some(host), port })
}

async fn socks_reply(socket: &mut TcpStream, status: u8) -> io::Result<()> {
    // The target-side ephemeral bind address is unspecified by tunnel/open.
    socket.write_all(&[5, status, 0, 1, 0, 0, 0, 0, 0, 0]).await
}
