//! Tunnels carried over a window's attach connection (`hub/preview/tunnel`, P380): the Android
//! client's browser reaches a target this way, because the hub's SOCKS listener is on the PC's
//! loopback. Each tunnel is one `tunnel/open` on the target's connection, relayed byte for
//! byte to an attach data stream. Credit passes through: what the client may send is what the
//! target granted, and what the target may send is what the client granted.

use yaac_protocol::attach::{PreviewTunnelCloseParams, PreviewTunnelParams, PreviewTunnelResult};
use yaac_protocol::methods::{StreamCredit, TunnelClose, TunnelOpen};
use yaac_protocol::{StreamCreditParams, StreamId, TargetId, TunnelCloseParams, TunnelOpenParams};

use super::{ClientKey, Core, ErrorObject, HashMap, Message, Pending, RpcId, Target, Value, attach, names, to_value};

/// At most this many tunnels per window.
const MAX_PER_CLIENT: usize = 256;

#[derive(Debug)]
pub(super) struct Tunnel {
    client: ClientKey,
    target: TargetId,
    remote: StreamId,
}

#[derive(Debug, Default)]
pub(super) struct AttachTunnels {
    pub(super) streams: HashMap<StreamId, Tunnel>,
}

impl Core {
    /// `hub/preview/tunnel`. `None`: answered when the target has connected.
    pub(super) fn attach_tunnel_open(&mut self, client: ClientKey, id: &RpcId, p: PreviewTunnelParams) -> Option<Result<Value, ErrorObject>> {
        if let Err(e) = self.preview_check(&p.target) {
            return Some(Err(e));
        }
        if self.attach_tunnels.streams.values().filter(|t| t.client == client).count() >= MAX_PER_CLIENT {
            return Some(Err(super::err(super::ErrorCode::InvalidParams, "too many tunnels for this window")));
        }
        let pending = Pending::AttachTunnel { client, id: id.clone() };
        let params = TunnelOpenParams { host: p.host, port: p.port, initial_credit: p.initial_credit };
        if self.send::<TunnelOpen>(&p.target, &params, pending) {
            self.flush(&p.target);
            None
        } else {
            Some(Err(super::err(super::ErrorCode::TargetOffline, "target offline")))
        }
    }

    /// The target answered `tunnel/open` for a window's tunnel.
    pub(super) fn attach_tunnel_opened(&mut self, target: &TargetId, client: ClientKey, id: RpcId, outcome: Result<Value, ErrorObject>) {
        let opened = outcome.and_then(|wire| self.decode_typed::<TunnelOpen>(target, wire));
        let reply = match opened {
            Err(error) => Err(error),
            Ok(opened) if !self.clients.contains_key(&client) => {
                self.attach_tunnel_close_remote(target, opened.stream);
                return;
            }
            Ok(opened) => {
                let Some(conn) = self.targets.get_mut(target).and_then(Target::ready_conn) else { return };
                if let Err(e) = conn.session.open_stream(opened.stream, opened.initial_credit) {
                    Err(super::err(super::ErrorCode::InternalError, e.to_string()))
                } else {
                    let stream = self.next_attach_stream();
                    if let Some(conn) = self.targets.get_mut(target).and_then(Target::ready_conn) {
                        conn.attach_tunnels.insert(opened.stream, stream);
                    }
                    self.attach_tunnels.streams.insert(stream, Tunnel { client, target: target.clone(), remote: opened.stream });
                    Ok(to_value(&PreviewTunnelResult { stream, initial_credit: opened.initial_credit }))
                }
            }
        };
        self.reply(client, id, reply);
    }

    /// Bytes from a window on one of its tunnels. Whether it was one (else a terminal's).
    pub(super) fn attach_tunnel_input(&mut self, client: ClientKey, stream: StreamId, bytes: Vec<u8>) -> bool {
        let Some(t) = self.attach_tunnels.streams.get(&stream).filter(|t| t.client == client) else { return false };
        let (target, remote) = (t.target.clone(), t.remote);
        let failed = self.targets.get_mut(&target).and_then(Target::ready_conn).is_none_or(|c| c.session.send_data(remote, bytes).is_err());
        if failed {
            self.attach_tunnel_end(stream, true, true);
        }
        self.flush(&target);
        true
    }

    /// The window granted credit on a tunnel: the target may send that much more.
    pub(super) fn attach_tunnel_client_credit(&mut self, client: ClientKey, credit: &StreamCreditParams) -> bool {
        let Some(t) = self.attach_tunnels.streams.get(&credit.stream).filter(|t| t.client == client) else { return false };
        let (target, remote) = (t.target.clone(), t.remote);
        if let Some(c) = self.targets.get_mut(&target).and_then(Target::ready_conn) {
            let _ = c.session.notify::<StreamCredit>(&StreamCreditParams { stream: remote, bytes: credit.bytes });
        }
        self.flush(&target);
        true
    }

    fn attach_stream_of(&self, target: &TargetId, remote: StreamId) -> Option<StreamId> {
        self.targets.get(target)?.conn.as_ref()?.attach_tunnels.get(&remote).copied()
    }

    /// The target granted credit on a tunnel: the window may send that much more.
    pub(super) fn attach_tunnel_target_credit(&mut self, target: &TargetId, p: &StreamCreditParams) -> bool {
        let Some(stream) = self.attach_stream_of(target, p.stream) else { return false };
        if let Some(t) = self.attach_tunnels.streams.get(&stream) {
            let client = t.client;
            self.send_to(client, Message::notification(names::STREAM_CREDIT, to_value(&StreamCreditParams { stream, bytes: p.bytes })));
        }
        true
    }

    /// Bytes from the target on a tunnel. Whether it was one.
    pub(super) fn attach_tunnel_data(&mut self, target: &TargetId, remote: StreamId, bytes: Vec<u8>) -> bool {
        let Some(stream) = self.attach_stream_of(target, remote) else { return false };
        if let Some(t) = self.attach_tunnels.streams.get(&stream)
            && let Some(c) = self.clients.get(&t.client)
        {
            let _ = c.tx.data(stream, bytes);
        }
        true
    }

    /// The target closed a tunnel. Whether it was one.
    pub(super) fn attach_tunnel_remote_closed(&mut self, target: &TargetId, remote: StreamId) -> bool {
        let Some(stream) = self.attach_stream_of(target, remote) else { return false };
        self.attach_tunnel_end(stream, false, true);
        true
    }

    /// `hub/preview/tunnelClose` from the window.
    pub(super) fn attach_tunnel_client_close(&mut self, client: ClientKey, p: &PreviewTunnelCloseParams) {
        if self.attach_tunnels.streams.get(&p.stream).is_some_and(|t| t.client == client) {
            self.attach_tunnel_end(p.stream, true, false);
        }
    }

    /// Forget a tunnel; tell the target and/or the window.
    fn attach_tunnel_end(&mut self, stream: StreamId, tell_target: bool, tell_client: bool) {
        let Some(t) = self.attach_tunnels.streams.remove(&stream) else { return };
        if let Some(conn) = self.targets.get_mut(&t.target).and_then(|x| x.conn.as_mut()) {
            conn.attach_tunnels.remove(&t.remote);
            conn.session.cancel_stream(t.remote);
            if tell_target {
                let _ = conn.session.notify::<TunnelClose>(&TunnelCloseParams { stream: t.remote, reason: None });
            }
        }
        if tell_client {
            self.send_to(t.client, Message::notification(attach::names::PREVIEW_TUNNEL_CLOSE, to_value(&PreviewTunnelCloseParams { stream })));
        }
        self.flush(&t.target);
    }

    fn attach_tunnel_close_remote(&mut self, target: &TargetId, remote: StreamId) {
        if let Some(conn) = self.targets.get_mut(target).and_then(Target::ready_conn) {
            conn.session.cancel_stream(remote);
            let _ = conn.session.notify::<TunnelClose>(&TunnelCloseParams { stream: remote, reason: None });
        }
        self.flush(target);
    }

    /// A window left: close its tunnels on the targets.
    pub(super) fn attach_tunnels_client_gone(&mut self, client: ClientKey) {
        let gone: Vec<_> = self.attach_tunnels.streams.iter().filter(|(_, t)| t.client == client).map(|(s, _)| *s).collect();
        for s in gone {
            self.attach_tunnel_end(s, true, false);
        }
    }

    /// A target's connection closed: every tunnel on it is gone; tell the windows.
    pub(super) fn attach_tunnels_target_gone(&mut self, target: &TargetId) {
        let gone: Vec<_> = self.attach_tunnels.streams.iter().filter(|(_, t)| &t.target == target).map(|(s, t)| (*s, t.client)).collect();
        for (stream, client) in gone {
            self.attach_tunnels.streams.remove(&stream);
            self.send_to(client, Message::notification(attach::names::PREVIEW_TUNNEL_CLOSE, to_value(&PreviewTunnelCloseParams { stream })));
        }
    }

    /// An attach stream number unused by this hub's terminals and tunnels.
    pub(super) fn next_attach_stream(&mut self) -> StreamId {
        loop {
            self.terminals.next = self.terminals.next.wrapping_add(1);
            let s = StreamId(self.terminals.next);
            if s.0 != 0 && !self.terminals.streams.contains_key(&s) && !self.attach_tunnels.streams.contains_key(&s) {
                return s;
            }
        }
    }
}
