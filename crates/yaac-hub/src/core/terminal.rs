//! Each window gets its own target-side attachment and credit window, on the existing
//! connection. The daemon owns replay and fans the PTY out to those streams.
use super::{
    ClientKey, Core, ErrorCode, ErrorObject, HashMap, HashSet, Message, Pending, RpcId, StreamCreditParams, StreamId, Target, TargetId,
    Value, attach, err, methods, names, parse, to_value,
};
use yaac_protocol::{TerminalAttachResult, TerminalDetachParams};

const OUTPUT_WINDOW: u64 = 1024 * 1024;

#[derive(Debug, Default)]
pub(super) struct Relays {
    next: u32,
    pub(super) streams: HashMap<StreamId, Relay>,
}

#[derive(Debug)]
pub(super) struct Relay {
    client: ClientKey,
    target: TargetId,
    remote: StreamId,
    terminal: String,
    input_credit: u64,
    output_credit: u64,
}

impl Core {
    pub(super) fn terminal_request(
        &mut self,
        client: ClientKey,
        id: &RpcId,
        method: &str,
        params: Value,
    ) -> Option<Result<Value, ErrorObject>> {
        let request = match parse::<attach::TargetTerminal<Value>>(params) {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };
        let method = match method {
            attach::names::TERMINAL_OPEN => names::TERMINAL_OPEN,
            attach::names::TERMINAL_ATTACH => names::TERMINAL_ATTACH,
            attach::names::TERMINAL_LIST => names::TERMINAL_LIST,
            attach::names::TERMINAL_RESIZE => names::TERMINAL_RESIZE,
            attach::names::TERMINAL_CLOSE => names::TERMINAL_CLOSE,
            attach::names::TERMINAL_RENAME => names::TERMINAL_RENAME,
            attach::names::TERMINAL_SHELLS => names::TERMINAL_SHELLS,
            _ => return Some(Err(err(ErrorCode::MethodNotFound, "unknown terminal method"))),
        };
        let Some(target) = self.targets.get_mut(&request.target) else {
            return Some(Err(err(ErrorCode::TargetUnknown, "no such target")));
        };
        let Some(conn) = target.ready_conn() else {
            return Some(Err(err(ErrorCode::TargetOffline, "target offline")));
        };
        if !conn
            .session
            .features()
            .is_some_and(|f| f.allows(&yaac_protocol::Capability::Terminal))
        {
            return Some(Err(err(ErrorCode::CapabilityMissing, "terminal unavailable on this target")));
        }
        let terminal = if method == names::TERMINAL_ATTACH {
            request.params.get("terminal").and_then(Value::as_str).map(str::to_owned)
        } else {
            None
        };
        match conn.session.request_value(method, request.params) {
            Ok(rid) => {
                conn.pending.insert(
                    rid,
                    Pending::Terminal {
                        client,
                        id: id.clone(),
                        terminal,
                    },
                );
                self.flush(&request.target);
                None
            }
            Err(e) => Some(Err(err(ErrorCode::InvalidParams, e.to_string()))),
        }
    }

    pub(super) fn terminal_response(
        &mut self,
        target: &TargetId,
        client: ClientKey,
        id: RpcId,
        method: &str,
        terminal: Option<String>,
        outcome: Result<Value, ErrorObject>,
    ) {
        let outcome = outcome
            .and_then(|wire| self.decode_current(target, method, wire))
            .and_then(|mut result| {
                if !matches!(method, names::TERMINAL_OPEN | names::TERMINAL_ATTACH) {
                    return Ok(result);
                }
                let opened: TerminalAttachResult = parse(result.clone())?;
                let terminal = terminal
                    .or_else(|| result["terminal"].as_str().map(str::to_owned))
                    .ok_or_else(|| err(ErrorCode::InternalError, "missing terminal id"))?;
                let conn = self
                    .targets
                    .get_mut(target)
                    .and_then(Target::ready_conn)
                    .ok_or_else(|| err(ErrorCode::TargetOffline, "target offline"))?;
                if !self.clients.contains_key(&client) {
                    let _ = conn
                        .session
                        .notify::<methods::TerminalDetach>(&TerminalDetachParams { stream: opened.stream });
                    return Ok(result);
                }
                conn.session
                    .open_interactive_stream(opened.stream, opened.initial_credit)
                    .map_err(|e| err(ErrorCode::InternalError, e.to_string()))?;
                loop {
                    self.terminals.next = self.terminals.next.wrapping_add(1);
                    if !self.terminals.streams.contains_key(&StreamId(self.terminals.next)) {
                        break;
                    }
                }
                let stream = StreamId(self.terminals.next);
                self.terminals.streams.insert(
                    stream,
                    Relay {
                        client,
                        target: target.clone(),
                        remote: opened.stream,
                        terminal,
                        input_credit: opened.initial_credit,
                        output_credit: 0,
                    },
                );
                conn.terminal_streams.insert(opened.stream, stream);
                result["stream"] = to_value(&stream);
                Ok(result)
            });
        if let Some(c) = self.clients.get(&client) {
            let message = match outcome {
                Ok(result) => Message::Response { id, result },
                Err(error) => Message::Error { id: Some(id), error },
            };
            let _ = c.tx.terminal_reply(message);
        }
    }

    pub(super) fn terminal_input(&mut self, client: ClientKey, stream: StreamId, bytes: Vec<u8>) {
        let Some(r) = self.terminals.streams.get_mut(&stream).filter(|r| r.client == client) else {
            return;
        };
        let Some(left) = r.input_credit.checked_sub(bytes.len() as u64) else {
            self.detach_terminal(stream);
            return;
        };
        r.input_credit = left;
        let target = r.target.clone();
        if let Some(c) = self.targets.get_mut(&target).and_then(Target::ready_conn) {
            let _ = c.session.send_data(r.remote, bytes);
        }
        self.flush(&target);
    }

    pub(super) fn terminal_output_credit(&mut self, client: ClientKey, credit: &StreamCreditParams) {
        let Some(r) = self.terminals.streams.get_mut(&credit.stream).filter(|r| r.client == client) else {
            return;
        };
        let bytes = credit.bytes.min(OUTPUT_WINDOW - r.output_credit);
        if bytes == 0 {
            return;
        }
        r.output_credit += bytes;
        let target = r.target.clone();
        if let Some(c) = self.targets.get_mut(&target).and_then(Target::ready_conn) {
            let _ = c
                .session
                .notify::<methods::StreamCredit>(&StreamCreditParams { stream: r.remote, bytes });
        }
        self.flush(&target);
    }

    pub(super) fn terminal_input_credit(&mut self, target: &TargetId, p: &StreamCreditParams) {
        if let Some(&stream) = self
            .targets
            .get(target)
            .and_then(|t| t.conn.as_ref())
            .and_then(|c| c.terminal_streams.get(&p.stream))
            && let Some(r) = self.terminals.streams.get_mut(&stream)
        {
            r.input_credit = r.input_credit.saturating_add(p.bytes);
            let client = r.client;
            self.send_to(
                client,
                Message::notification(names::STREAM_CREDIT, to_value(&StreamCreditParams { stream, bytes: p.bytes })),
            );
        }
    }

    pub(super) fn target_data(&mut self, target: &TargetId, remote: StreamId, bytes: Vec<u8>) {
        let Some(&stream) = self
            .targets
            .get(target)
            .and_then(|t| t.conn.as_ref())
            .and_then(|c| c.terminal_streams.get(&remote))
        else {
            self.preview_data(target, remote, bytes);
            return;
        };
        if let Some(r) = self.terminals.streams.get_mut(&stream) {
            let Some(left) = r.output_credit.checked_sub(bytes.len() as u64) else {
                self.detach_terminal(stream);
                return;
            };
            r.output_credit = left;
            if let Some(client) = self.clients.get(&r.client) {
                let _ = client.tx.data(stream, bytes);
            }
        }
    }

    fn detach_terminal(&mut self, stream: StreamId) {
        let Some(r) = self.terminals.streams.remove(&stream) else { return };
        if let Some(c) = self.targets.get_mut(&r.target).and_then(Target::ready_conn) {
            c.terminal_streams.remove(&r.remote);
            c.session.cancel_stream(r.remote);
            let _ = c
                .session
                .notify::<methods::TerminalDetach>(&TerminalDetachParams { stream: r.remote });
        }
        self.flush(&r.target);
    }

    pub(super) fn terminal_window_closed(&mut self, client: ClientKey) {
        let streams: Vec<_> = self
            .terminals
            .streams
            .iter()
            .filter_map(|(&s, r)| (r.client == client).then_some(s))
            .collect();
        for s in streams {
            self.detach_terminal(s);
        }
    }

    pub(super) fn terminal_event(&mut self, target: &TargetId, method: &str, mut params: Value) {
        let Some(terminal) = params["terminal"].as_str() else { return };
        let streams: Vec<_> = self
            .terminals
            .streams
            .iter()
            .filter_map(|(&s, r)| (&r.target == target && r.terminal == terminal).then_some((s, r.client)))
            .collect();
        let exited = method == names::TERMINAL_EXITED;
        let name = match method {
            names::TERMINAL_EXITED => attach::names::TERMINAL_EXITED,
            names::TERMINAL_RENAMED => attach::names::TERMINAL_RENAMED,
            _ => attach::names::TERMINAL_TITLE,
        };
        params["target"] = to_value(target);
        let mut seen = HashSet::new();
        for (s, client) in streams {
            if seen.insert(client) {
                self.send_to(client, Message::notification(name, params.clone()));
            }
            if exited {
                self.detach_terminal(s);
            }
        }
    }
}
