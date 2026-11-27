//! Voice input relay (docs/plan/voice.md): `hub/voice/start` from a window names the composer;
//! the hub chooses the transcriber (P392, [`yaac_protocol::transcription::choose`]) — the
//! composer's own instance, else another on its target, else one on another connected target —
//! and sends `voice/start` on that target's one connection. The window's audio data frames are
//! forwarded under the target's credit, and `voice/transcript` / `voice/ended` go back to the
//! starting window only. Streams are renumbered per window as terminals are. The hub keeps no
//! audio and no words: nothing here is cached, journaled or logged beyond the stream mapping.
use super::{
    ClientKey, Core, ErrorCode, ErrorObject, HashMap, Message, Pending, RpcId, StreamCreditParams, StreamId, Target, TargetId, Value,
    attach, err, names, parse, to_value,
};
use yaac_protocol::attach::{HubVoiceStartParams, HubVoiceStartResult};
use yaac_protocol::transcription::{self, Candidate, Choice, Context};
use yaac_protocol::{InstanceId, ProviderDescriptor, VoiceCancelParams, VoiceStartParams, VoiceStartResult};

#[derive(Debug, Default)]
pub(super) struct Relays {
    pub(super) streams: HashMap<StreamId, Relay>,
}

#[derive(Debug)]
pub(super) struct Relay {
    client: ClientKey,
    target: TargetId,
    remote: StreamId,
    input_credit: u64,
}

/// What a pending `voice/*` request was.
#[derive(Clone, Debug)]
pub(super) enum VoiceAsk {
    /// A start through this instance, which the window is told about.
    Start(Box<(InstanceId, String)>),
    /// Stop or cancel of this hub-local stream.
    Stream,
    /// A cancel the hub sent itself (the window left); nobody is answered.
    Quiet,
}

impl Core {
    /// A target's instances changed: keep what the transcriber choice reads (`replace`: the
    /// whole list, else one updated descriptor).
    pub(super) fn voice_providers(&mut self, target: &TargetId, providers: &[ProviderDescriptor], replace: bool) {
        let Some(conn) = self.targets.get_mut(target).and_then(|t| t.conn.as_mut()) else { return };
        if replace {
            conn.transcribers.clear();
        }
        for p in providers {
            let slim = ProviderDescriptor { models: Vec::new(), disabled_models: Vec::new(), modes: Vec::new(), ..p.clone() };
            match conn.transcribers.iter_mut().find(|x| x.instance == p.instance) {
                Some(x) => *x = slim,
                None => conn.transcribers.push(slim),
            }
        }
    }

    /// The transcriber for a composer, among the targets connected now.
    fn choose_transcriber(&self, p: &HubVoiceStartParams) -> Choice {
        let candidates: Vec<Candidate<'_>> = self
            .targets
            .values()
            .map(|t| {
                let conn = t.conn.as_ref().filter(|c| c.ready);
                Candidate {
                    target: &t.def.id,
                    name: t.def.name.as_str(),
                    connected: conn.is_some(),
                    providers: conn.map_or(&[][..], |c| c.transcribers.as_slice()),
                }
            })
            .collect();
        transcription::choose(Context { target: p.target.as_ref(), instance: p.instance.as_ref() }, &candidates)
    }

    /// `hub/voice/start`: choose, then `voice/start` on the chosen target.
    fn voice_start(&mut self, client: ClientKey, id: &RpcId, params: Value) -> Option<Result<Value, ErrorObject>> {
        let p: HubVoiceStartParams = match parse(params) {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };
        let chosen = match self.choose_transcriber(&p) {
            Choice::Ready(t) => t,
            Choice::Unavailable(why) => return Some(Err(err(ErrorCode::CapabilityMissing, why))),
        };
        // The thread is context for its own target only; another target does not know it.
        let thread_id = p.thread_id.filter(|_| p.target.as_ref() == Some(&chosen.target));
        let wire = VoiceStartParams { op_id: p.op_id, instance: chosen.instance.clone(), thread_id, language: p.language };
        let Some(conn) = self.targets.get_mut(&chosen.target).and_then(Target::ready_conn) else {
            return Some(Err(err(ErrorCode::TargetOffline, "target offline")));
        };
        match conn.session.request_value(names::VOICE_START, to_value(&wire)) {
            Ok(rid) => {
                let ask = VoiceAsk::Start(Box::new((chosen.instance, chosen.label)));
                conn.pending.insert(rid, Pending::Voice { client, id: id.clone(), ask });
                self.flush(&chosen.target);
                None
            }
            Err(e) => Some(Err(err(ErrorCode::InvalidParams, e.to_string()))),
        }
    }

    pub(super) fn voice_request(&mut self, client: ClientKey, id: &RpcId, method: &str, params: Value) -> Option<Result<Value, ErrorObject>> {
        if method == attach::names::VOICE_START {
            return self.voice_start(client, id, params);
        }
        let mut request = match parse::<attach::TargetVoice<Value>>(params) {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };
        let (method, ask) = match method {
            attach::names::VOICE_STOP => (names::VOICE_STOP, VoiceAsk::Stream),
            attach::names::VOICE_CANCEL => (names::VOICE_CANCEL, VoiceAsk::Stream),
            _ => return Some(Err(err(ErrorCode::MethodNotFound, "unknown voice method"))),
        };
        if matches!(ask, VoiceAsk::Stream) {
            let local: StreamId = match request.params.get("stream").cloned().map(serde_json::from_value) {
                Some(Ok(s)) => s,
                _ => return Some(Err(err(ErrorCode::InvalidParams, "missing stream"))),
            };
            let Some(r) = self.voice.streams.get(&local).filter(|r| r.client == client && r.target == request.target) else {
                return Some(Err(err(ErrorCode::InvalidParams, "no such dictation")));
            };
            request.params["stream"] = to_value(&r.remote);
        }
        let Some(target) = self.targets.get_mut(&request.target) else {
            return Some(Err(err(ErrorCode::TargetUnknown, "no such target")));
        };
        let Some(conn) = target.ready_conn() else {
            return Some(Err(err(ErrorCode::TargetOffline, "target offline")));
        };
        if !conn.session.features().is_some_and(|f| f.allows(&yaac_protocol::Capability::Transcription)) {
            return Some(Err(err(ErrorCode::CapabilityMissing, "voice input is unavailable on this target")));
        }
        match conn.session.request_value(method, request.params) {
            Ok(rid) => {
                conn.pending.insert(rid, Pending::Voice { client, id: id.clone(), ask });
                self.flush(&request.target);
                None
            }
            Err(e) => Some(Err(err(ErrorCode::InvalidParams, e.to_string()))),
        }
    }

    pub(super) fn voice_response(
        &mut self,
        target: &TargetId,
        client: ClientKey,
        id: RpcId,
        method: &str,
        ask: &VoiceAsk,
        outcome: Result<Value, ErrorObject>,
    ) {
        if matches!(ask, VoiceAsk::Quiet) {
            return;
        }
        let outcome = outcome.and_then(|wire| self.decode_current(target, method, wire)).and_then(|result| {
            let VoiceAsk::Start(chosen) = ask else {
                return Ok(result);
            };
            let mut opened: VoiceStartResult = parse(result.clone())?;
            let stream = self.next_attach_stream();
            let conn = self
                .targets
                .get_mut(target)
                .and_then(Target::ready_conn)
                .ok_or_else(|| err(ErrorCode::TargetOffline, "target offline"))?;
            if !self.clients.contains_key(&client) {
                // The window left while the start was in flight: end it on the target.
                if let Ok(rid) = conn.session.request::<yaac_protocol::methods::VoiceCancel>(&VoiceCancelParams { stream: opened.stream }) {
                    conn.pending.insert(rid, Pending::Voice { client, id: id.clone(), ask: VoiceAsk::Quiet });
                }
                return Ok(result);
            }
            conn.session
                .open_interactive_stream(opened.stream, opened.initial_credit)
                .map_err(|e| err(ErrorCode::InternalError, e.to_string()))?;
            conn.voice_streams.insert(opened.stream, stream);
            self.voice.streams.insert(
                stream,
                Relay { client, target: target.clone(), remote: opened.stream, input_credit: opened.initial_credit },
            );
            opened.stream = stream;
            let (instance, label) = (**chosen).clone();
            Ok(to_value(&HubVoiceStartResult { target: target.clone(), instance, label, opened }))
        });
        if let Some(c) = self.clients.get(&client) {
            let message = match outcome {
                Ok(result) => Message::Response { id, result },
                Err(error) => Message::Error { id: Some(id), error },
            };
            let _ = c.tx.terminal_reply(message);
        }
        self.flush(target);
    }

    /// Audio from a window. Whether `stream` was one of its dictations.
    pub(super) fn voice_input(&mut self, client: ClientKey, stream: StreamId, bytes: Vec<u8>) -> bool {
        let Some(r) = self.voice.streams.get_mut(&stream).filter(|r| r.client == client) else {
            return false;
        };
        let Some(left) = r.input_credit.checked_sub(bytes.len() as u64) else {
            // Overran its credit: the dictation is cancelled, and the window told.
            let target = r.target.clone();
            self.voice_end(stream, Some("the audio arrived faster than the target accepts it".into()), true);
            self.flush(&target);
            return true;
        };
        r.input_credit = left;
        let (target, remote) = (r.target.clone(), r.remote);
        if let Some(c) = self.targets.get_mut(&target).and_then(Target::ready_conn) {
            let _ = c.session.send_data(remote, bytes);
        }
        self.flush(&target);
        true
    }

    /// Credit from the target on one of its voice streams. Whether it was one.
    pub(super) fn voice_input_credit(&mut self, target: &TargetId, p: &StreamCreditParams) -> bool {
        let Some(&stream) = self.targets.get(target).and_then(|t| t.conn.as_ref()).and_then(|c| c.voice_streams.get(&p.stream)) else {
            return false;
        };
        if let Some(r) = self.voice.streams.get_mut(&stream) {
            r.input_credit = r.input_credit.saturating_add(p.bytes);
            let client = r.client;
            self.send_to(client, Message::notification(names::STREAM_CREDIT, to_value(&StreamCreditParams { stream, bytes: p.bytes })));
        }
        true
    }

    /// `voice/transcript` or `voice/ended` from a target, to the window that started it.
    pub(super) fn voice_event(&mut self, target: &TargetId, method: &str, mut params: Value) {
        let Ok(remote) = serde_json::from_value::<StreamId>(params["stream"].clone()) else { return };
        let Some(&stream) = self.targets.get(target).and_then(|t| t.conn.as_ref()).and_then(|c| c.voice_streams.get(&remote)) else {
            return;
        };
        let Some(client) = self.voice.streams.get(&stream).map(|r| r.client) else { return };
        if method == names::VOICE_ENDED {
            self.voice.streams.remove(&stream);
            if let Some(c) = self.targets.get_mut(target).and_then(Target::ready_conn) {
                c.voice_streams.remove(&remote);
                c.session.cancel_stream(remote);
            }
        }
        let name = if method == names::VOICE_ENDED { attach::names::VOICE_ENDED } else { attach::names::VOICE_TRANSCRIPT };
        params["stream"] = to_value(&stream);
        params["target"] = to_value(target);
        self.send_to(client, Message::notification(name, params));
    }

    /// Ends a dictation from the hub's side: cancels it on the target if `cancel`, tells the
    /// window with `hub/voice/ended` if it is still there.
    fn voice_end(&mut self, stream: StreamId, error: Option<String>, cancel: bool) {
        let Some(r) = self.voice.streams.remove(&stream) else { return };
        if let Some(c) = self.targets.get_mut(&r.target).and_then(Target::ready_conn) {
            c.voice_streams.remove(&r.remote);
            c.session.cancel_stream(r.remote);
            if cancel
                && let Ok(rid) = c.session.request::<yaac_protocol::methods::VoiceCancel>(&VoiceCancelParams { stream: r.remote })
            {
                c.pending.insert(rid, Pending::Voice { client: r.client, id: RpcId::Number(0), ask: VoiceAsk::Quiet });
            }
        }
        if self.clients.contains_key(&r.client) {
            let mut params = serde_json::json!({ "target": r.target, "stream": stream });
            if let Some(e) = error {
                params["error"] = Value::String(e);
            }
            self.send_to(r.client, Message::notification(attach::names::VOICE_ENDED, params));
        }
    }

    /// A window left: its dictations are cancelled on their targets.
    pub(super) fn voice_window_closed(&mut self, client: ClientKey) {
        let streams: Vec<_> = self.voice.streams.iter().filter_map(|(&s, r)| (r.client == client).then_some((s, r.target.clone()))).collect();
        for (s, target) in streams {
            self.voice_end(s, None, true);
            self.flush(&target);
        }
    }

    /// A target's connection dropped: its dictations are over (the daemon ends them too).
    pub(super) fn voice_target_closed(&mut self, target: &TargetId) {
        let streams: Vec<_> = self.voice.streams.iter().filter_map(|(&s, r)| (&r.target == target).then_some(s)).collect();
        for s in streams {
            self.voice_end(s, Some("the connection to the target dropped".into()), false);
        }
    }
}
