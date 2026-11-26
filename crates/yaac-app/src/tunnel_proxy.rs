//! The Android browser's way to a target (P380, docs/11-preview.md#android-proposed-p380): a
//! SOCKS endpoint on the phone's loopback, bound to one target. Each `CONNECT` becomes one
//! `hub/preview/tunnel`, carried over the attach connection; nothing else is opened.
//!
//! Threads own the sockets (an accept loop, and per connection a reader and a writer); the
//! controller loop owns the tunnels and the credit. The reader asks for a permit before each
//! read, so at most one chunk per connection waits in the controller; the hub's credit decides
//! when it goes out. What arrives from the hub is written by the writer, and the bytes written
//! are granted back to the hub as `stream/credit`.
//!
//! No authentication (the owner's choice, as on the desktop): another app on the phone can use
//! the endpoint while it exists. It exists only while browser tabs do.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde_json::json;
use yaac_core::client::Client;
use yaac_protocol::attach::{self, PreviewTunnelResult};
use yaac_protocol::rpc::RpcId;
use yaac_protocol::{StreamId, TargetId};

/// What the phone may receive on a tunnel before granting more.
const WINDOW: u64 = 256 * 1024;
const CHUNK: usize = 16 * 1024;
const HANDSHAKE: Duration = Duration::from_secs(10);

pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

enum Event {
    Connect { conn: u64, host: String, port: u16, reply: mpsc::Sender<Reply>, socket: TcpStream, permits: mpsc::SyncSender<()> },
    Bytes { conn: u64, bytes: Vec<u8> },
    Written { conn: u64, bytes: usize },
    Closed { conn: u64 },
}

/// The SOCKS answer: success, or a SOCKS5 reply code.
enum Reply {
    Ready,
    Failed(u8),
}

struct Conn {
    reply: Option<mpsc::Sender<Reply>>,
    socket: Option<TcpStream>,
    stream: Option<StreamId>,
    /// What the hub lets us send now.
    credit: u64,
    queued: VecDeque<Vec<u8>>,
    permits: Option<mpsc::SyncSender<()>>,
    to_socket: Option<mpsc::Sender<Vec<u8>>>,
    written: u64,
}

/// One endpoint, bound to one target.
pub(crate) struct TunnelProxy {
    target: TargetId,
    port: u16,
    stop: Arc<AtomicBool>,
    events: mpsc::Receiver<Event>,
    /// For writer threads: where they report the bytes written.
    wake_written: (mpsc::Sender<Event>, Wake),
    conns: HashMap<u64, Conn>,
    by_stream: HashMap<StreamId, u64>,
}

impl std::fmt::Debug for TunnelProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelProxy").field("target", &self.target).field("port", &self.port).finish_non_exhaustive()
    }
}

impl TunnelProxy {
    /// Binds `127.0.0.1:<any>` for `target`.
    pub(crate) fn start(target: TargetId, wake: Wake) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let (tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let wake_written = (tx.clone(), wake.clone());
        std::thread::Builder::new().name("yaac-socks".into()).spawn(move || {
            let mut next = 0u64;
            for socket in listener.incoming() {
                if stopping.load(Ordering::Acquire) {
                    return;
                }
                let Ok(socket) = socket else { continue };
                next += 1;
                let (tx, wake) = (tx.clone(), wake.clone());
                let conn = next;
                let _ = std::thread::Builder::new().name("yaac-socks-conn".into()).spawn(move || serve(conn, socket, &tx, &wake));
            }
        })?;
        Ok(Self { target, port, stop, events, wake_written, conns: HashMap::new(), by_stream: HashMap::new() })
    }

    pub(crate) fn target(&self) -> &TargetId {
        &self.target
    }

    /// The proxy URL a WebView uses (`socks://` is SOCKS5 in Chromium's proxy rules).
    pub(crate) fn url(&self) -> String {
        format!("socks://127.0.0.1:{}", self.port)
    }

    /// Socket events: new `CONNECT`s become tunnel requests (their ids returned, to be answered
    /// through [`TunnelProxy::opened`]); bytes go out under credit; closes are told to the hub.
    pub(crate) fn pump(&mut self, client: &mut Client) -> Vec<(RpcId, u64)> {
        let mut requests = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Connect { conn, host, port, reply, socket, permits } => {
                    let rpc = client.preview_request(attach::names::PREVIEW_TUNNEL, json!({
                        "target": self.target, "host": host, "port": port, "initialCredit": WINDOW,
                    }));
                    self.conns.insert(conn, Conn {
                        reply: Some(reply), socket: Some(socket), stream: None, credit: 0, queued: VecDeque::new(),
                        permits: Some(permits), to_socket: None, written: 0,
                    });
                    requests.push((rpc, conn));
                }
                Event::Bytes { conn, bytes } => {
                    if let Some(c) = self.conns.get_mut(&conn) {
                        c.queued.push_back(bytes);
                    }
                }
                Event::Written { conn, bytes } => {
                    if let Some(c) = self.conns.get_mut(&conn) {
                        c.written += bytes as u64;
                        if c.written >= WINDOW / 4
                            && let Some(stream) = c.stream
                        {
                            client.notify(yaac_protocol::methods::names::STREAM_CREDIT, json!({ "stream": stream, "bytes": c.written }));
                            c.written = 0;
                        }
                    }
                }
                Event::Closed { conn } => self.close(client, conn, true),
            }
        }
        for (stream, bytes) in client.take_tunnel_data() {
            if let Some(c) = self.by_stream.get(&stream).and_then(|conn| self.conns.get(conn))
                && let Some(tx) = &c.to_socket
            {
                let _ = tx.send(bytes);
            }
        }
        for (method, params) in client.take_tunnel_notes() {
            let Some(stream) = params.get("stream").and_then(|s| serde_json::from_value::<StreamId>(s.clone()).ok()) else { continue };
            let Some(&conn) = self.by_stream.get(&stream) else { continue };
            if method == attach::names::PREVIEW_TUNNEL_CLOSE {
                self.close(client, conn, false);
            } else if let Some(c) = self.conns.get_mut(&conn) {
                c.credit = c.credit.saturating_add(params["bytes"].as_u64().unwrap_or(0));
            }
        }
        let conns: Vec<u64> = self.conns.keys().copied().collect();
        for conn in conns {
            self.flush(client, conn);
        }
        requests
    }

    /// The hub answered a `hub/preview/tunnel`.
    pub(crate) fn opened(&mut self, client: &mut Client, conn: u64, result: Result<serde_json::Value, String>, code: Option<&str>) {
        let Some(c) = self.conns.get_mut(&conn) else {
            // The socket went away meanwhile: close the tunnel it got.
            if let Ok(r) = result.and_then(|v| serde_json::from_value::<PreviewTunnelResult>(v).map_err(|e| e.to_string())) {
                client.preview_request(attach::names::PREVIEW_TUNNEL_CLOSE, json!({ "stream": r.stream }));
            }
            return;
        };
        let opened = result.and_then(|v| serde_json::from_value::<PreviewTunnelResult>(v).map_err(|e| e.to_string()));
        let Ok(r) = opened else {
            let status = match code {
                Some("connection_refused") => 0x05,
                Some("invalid_params") => 0x02,
                _ => 0x01,
            };
            if let Some(reply) = c.reply.take() {
                let _ = reply.send(Reply::Failed(status));
            }
            self.conns.remove(&conn);
            return;
        };
        client.track_tunnel(r.stream);
        c.stream = Some(r.stream);
        c.credit = r.initial_credit;
        self.by_stream.insert(r.stream, conn);
        let (to_socket, from_controller) = mpsc::channel::<Vec<u8>>();
        if let Some(socket) = c.socket.take() {
            spawn_writer(conn, socket, from_controller, self.wake_written.clone());
        }
        c.to_socket = Some(to_socket);
        if let Some(reply) = c.reply.take() {
            let _ = reply.send(Reply::Ready);
        }
        self.flush(client, conn);
    }

    /// Send what the credit allows; then let the reader take the next chunk.
    fn flush(&mut self, client: &mut Client, conn: u64) {
        let Some(c) = self.conns.get_mut(&conn) else { return };
        let Some(stream) = c.stream else { return };
        while c.credit > 0 {
            let Some(mut chunk) = c.queued.pop_front() else { break };
            let take = usize::try_from(c.credit).unwrap_or(usize::MAX).min(chunk.len());
            if take < chunk.len() {
                let rest = chunk.split_off(take);
                c.queued.push_front(rest);
            }
            c.credit -= take as u64;
            client.send_tunnel_data(stream, chunk);
        }
        if c.queued.is_empty()
            && let Some(p) = &c.permits
        {
            let _ = p.try_send(());
        }
    }

    fn close(&mut self, client: &mut Client, conn: u64, tell_hub: bool) {
        let Some(c) = self.conns.remove(&conn) else { return };
        if let Some(stream) = c.stream {
            self.by_stream.remove(&stream);
            client.forget_tunnel(stream);
            if tell_hub {
                client.preview_request(attach::names::PREVIEW_TUNNEL_CLOSE, json!({ "stream": stream }));
            }
        }
        // Dropping the senders ends the writer, which shuts the socket.
    }

    /// Ends every tunnel and the listener.
    pub(crate) fn stop(mut self, client: &mut Client) {
        let conns: Vec<u64> = self.conns.keys().copied().collect();
        for conn in conns {
            self.close(client, conn, true);
        }
        self.stop.store(true, Ordering::Release);
        // Unblock the accept loop.
        let _ = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)), Duration::from_millis(200));
    }
}

/// One accepted connection: the SOCKS handshake, then its reader. The writer starts once the
/// tunnel is open ([`spawn_writer`]).
fn serve(conn: u64, mut socket: TcpStream, events: &mpsc::Sender<Event>, wake: &Wake) {
    let _ = socket.set_read_timeout(Some(HANDSHAKE));
    let Ok((version, host, port)) = handshake(&mut socket) else { return };
    let (reply_tx, reply_rx) = mpsc::channel();
    let (permit_tx, permits) = mpsc::sync_channel::<()>(1);
    let Ok(writer) = socket.try_clone() else { return };
    if events.send(Event::Connect { conn, host, port, reply: reply_tx, socket: writer, permits: permit_tx }).is_err() {
        return;
    }
    wake();
    let answer = reply_rx.recv_timeout(Duration::from_secs(30)).unwrap_or(Reply::Failed(0x01));
    let ok = matches!(answer, Reply::Ready);
    let response: Vec<u8> = match (version, answer) {
        (5, Reply::Ready) => vec![5, 0, 0, 1, 0, 0, 0, 0, 0, 0],
        (5, Reply::Failed(code)) => vec![5, code, 0, 1, 0, 0, 0, 0, 0, 0],
        (_, Reply::Ready) => vec![0, 0x5a, 0, 0, 0, 0, 0, 0],
        (_, Reply::Failed(_)) => vec![0, 0x5b, 0, 0, 0, 0, 0, 0],
    };
    if socket.write_all(&response).is_err() || !ok {
        let _ = events.send(Event::Closed { conn });
        wake();
        return;
    }
    let _ = socket.set_read_timeout(None);
    // The reader: one chunk, then wait for the controller's permit (it has sent what it can).
    let mut buf = vec![0u8; CHUNK];
    loop {
        match socket.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if events.send(Event::Bytes { conn, bytes: buf[..n].to_vec() }).is_err() {
                    break;
                }
                wake();
                if permits.recv().is_err() {
                    break;
                }
            }
        }
    }
    let _ = events.send(Event::Closed { conn });
    wake();
}

/// Writes what the hub sent; each write is reported back so the hub gets the credit again.
fn spawn_writer(conn: u64, mut socket: TcpStream, from_controller: mpsc::Receiver<Vec<u8>>, written: (mpsc::Sender<Event>, Wake)) {
    let _ = std::thread::Builder::new().name("yaac-socks-write".into()).spawn(move || {
        let (events, wake) = written;
        for bytes in from_controller {
            if socket.write_all(&bytes).is_err() {
                break;
            }
            let _ = events.send(Event::Written { conn, bytes: bytes.len() });
            wake();
        }
        let _ = socket.shutdown(Shutdown::Both);
    });
}

/// SOCKS5 (no authentication, `CONNECT`) or SOCKS4/4a: the destination.
fn handshake(s: &mut TcpStream) -> std::io::Result<(u8, String, u16)> {
    let mut b = [0u8; 2];
    s.read_exact(&mut b)?;
    match b[0] {
        5 => {
            let mut methods = vec![0u8; usize::from(b[1])];
            s.read_exact(&mut methods)?;
            if !methods.contains(&0) {
                s.write_all(&[5, 0xff])?;
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            s.write_all(&[5, 0])?;
            let mut head = [0u8; 4];
            s.read_exact(&mut head)?;
            if head[1] != 1 {
                s.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0])?;
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            let host = match head[3] {
                1 => {
                    let mut a = [0u8; 4];
                    s.read_exact(&mut a)?;
                    std::net::Ipv4Addr::from(a).to_string()
                }
                3 => {
                    let mut len = [0u8; 1];
                    s.read_exact(&mut len)?;
                    let mut name = vec![0u8; usize::from(len[0])];
                    s.read_exact(&mut name)?;
                    String::from_utf8(name).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
                }
                4 => {
                    let mut a = [0u8; 16];
                    s.read_exact(&mut a)?;
                    std::net::Ipv6Addr::from(a).to_string()
                }
                _ => {
                    s.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0])?;
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
            };
            let mut port = [0u8; 2];
            s.read_exact(&mut port)?;
            Ok((5, host, u16::from_be_bytes(port)))
        }
        4 if b[1] == 1 => {
            let mut rest = [0u8; 6];
            s.read_exact(&mut rest)?;
            let port = u16::from_be_bytes([rest[0], rest[1]]);
            let ip = [rest[2], rest[3], rest[4], rest[5]];
            let _user = read_cstr(s)?;
            // SOCKS4a: 0.0.0.x (x ≠ 0) means a host name follows.
            let host = if ip[0] == 0 && ip[1] == 0 && ip[2] == 0 && ip[3] != 0 { read_cstr(s)? } else { std::net::Ipv4Addr::from(ip).to_string() };
            Ok((4, host, port))
        }
        _ => Err(std::io::ErrorKind::InvalidData.into()),
    }
}

fn read_cstr(s: &mut TcpStream) -> std::io::Result<String> {
    let mut out = Vec::new();
    let mut b = [0u8; 1];
    loop {
        s.read_exact(&mut b)?;
        if b[0] == 0 || out.len() > 255 {
            break;
        }
        out.push(b[0]);
    }
    String::from_utf8(out).map_err(|_| std::io::ErrorKind::InvalidData.into())
}
