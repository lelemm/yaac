//! Voice call relay (docs/plan/voice-call.md): `hub/voiceCall/*` from a window becomes
//! `voiceCall/*` on the thread's target connection, and `voiceCall/answer`, `/state` and
//! `/transcript` go back to the starting window only. The call id is the target's, unchanged.
//! The hub keeps only the `(target, callId) → window` mapping: no SDP, no words, no audio
//! (media never passes through YAAC).
use super::{ClientKey, Core, ErrorCode, ErrorObject, HashMap, Message, Pending, RpcId, Target, TargetId, Value, attach, err, names, parse, to_value};
use yaac_protocol::{VoiceCallEndParams, VoiceCallStartResult};

#[derive(Debug, Default)]
pub(super) struct Calls {
    pub(super) by_id: HashMap<(TargetId, String), ClientKey>,
}

/// What a pending `voiceCall/*` request was.
#[derive(Clone, Copy, Debug)]
pub(super) enum CallAsk {
    Start,
    End,
    /// An end the hub sent itself (the window left); nobody is answered.
    Quiet,
}

impl Core {
    pub(super) fn voice_call_request(&mut self, client: ClientKey, id: &RpcId, method: &str, params: Value) -> Option<Result<Value, ErrorObject>> {
        let request = match parse::<attach::TargetVoiceCall<Value>>(params) {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };
        let (method, ask) = match method {
            attach::names::VOICE_CALL_START => (names::VOICE_CALL_START, CallAsk::Start),
            attach::names::VOICE_CALL_END => (names::VOICE_CALL_END, CallAsk::End),
            _ => return Some(Err(err(ErrorCode::MethodNotFound, "unknown voice call method"))),
        };
        if matches!(ask, CallAsk::End) {
            let Some(call) = request.params.get("callId").and_then(Value::as_str).map(str::to_owned) else {
                return Some(Err(err(ErrorCode::InvalidParams, "missing callId")));
            };
            match self.voice_calls.by_id.get(&(request.target.clone(), call)) {
                // Already over (or never started here): nothing to end.
                None => return Some(Ok(serde_json::json!({}))),
                Some(owner) if *owner != client => return Some(Err(err(ErrorCode::InvalidParams, "no such call"))),
                Some(_) => {}
            }
        }
        let Some(target) = self.targets.get_mut(&request.target) else {
            return Some(Err(err(ErrorCode::TargetUnknown, "no such target")));
        };
        let Some(conn) = target.ready_conn() else {
            return Some(Err(err(ErrorCode::TargetOffline, "target offline")));
        };
        if !conn.session.features().is_some_and(|f| f.allows(&yaac_protocol::Capability::VoiceCall)) {
            return Some(Err(err(ErrorCode::CapabilityMissing, "voice calls are unavailable on this target")));
        }
        match conn.session.request_value(method, request.params) {
            Ok(rid) => {
                conn.pending.insert(rid, Pending::VoiceCall { client, id: id.clone(), ask });
                self.flush(&request.target);
                None
            }
            Err(e) => Some(Err(err(ErrorCode::InvalidParams, e.to_string()))),
        }
    }

    pub(super) fn voice_call_response(
        &mut self,
        target: &TargetId,
        client: ClientKey,
        id: RpcId,
        method: &str,
        ask: CallAsk,
        outcome: Result<Value, ErrorObject>,
    ) {
        if matches!(ask, CallAsk::Quiet) {
            return;
        }
        let outcome = outcome.and_then(|wire| self.decode_current(target, method, wire)).and_then(|result| {
            if !matches!(ask, CallAsk::Start) {
                return Ok(result);
            }
            let started: VoiceCallStartResult = parse(result.clone())?;
            if !self.clients.contains_key(&client) {
                // The window left while the start was in flight: hang up on the target.
                self.voice_call_end_quietly(target, client, &started.call_id);
                return Ok(result);
            }
            self.voice_calls.by_id.insert((target.clone(), started.call_id), client);
            Ok(result)
        });
        if let Some(c) = self.clients.get(&client) {
            let message = match outcome {
                Ok(result) => Message::Response { id, result },
                Err(error) => Message::Error { id: Some(id), error },
            };
            // The window's call lane, so the result is ahead of the call's notifications.
            let _ = c.tx.terminal_reply(message);
        }
        self.flush(target);
    }

    /// `voiceCall/answer`, `/state` or `/transcript` from a target, to the window that started it.
    pub(super) fn voice_call_event(&mut self, target: &TargetId, method: &str, mut params: Value) {
        let Some(call) = params.get("callId").and_then(Value::as_str).map(str::to_owned) else { return };
        let key = (target.clone(), call);
        let Some(&client) = self.voice_calls.by_id.get(&key) else { return };
        if method == names::VOICE_CALL_STATE && params.get("state").and_then(Value::as_str) == Some("ended") {
            self.voice_calls.by_id.remove(&key);
        }
        let name = match method {
            names::VOICE_CALL_ANSWER => attach::names::VOICE_CALL_ANSWER,
            names::VOICE_CALL_STATE => attach::names::VOICE_CALL_STATE,
            _ => attach::names::VOICE_CALL_TRANSCRIPT,
        };
        params["target"] = to_value(target);
        self.send_to(client, Message::notification(name, params));
    }

    fn voice_call_end_quietly(&mut self, target: &TargetId, client: ClientKey, call: &str) {
        if let Some(c) = self.targets.get_mut(target).and_then(Target::ready_conn)
            && let Ok(rid) = c.session.request::<yaac_protocol::methods::VoiceCallEnd>(&VoiceCallEndParams { call_id: call.to_owned() })
        {
            c.pending.insert(rid, Pending::VoiceCall { client, id: RpcId::Number(0), ask: CallAsk::Quiet });
        }
    }

    /// A window left: its calls are hung up on their targets.
    pub(super) fn voice_call_window_closed(&mut self, client: ClientKey) {
        let calls: Vec<_> = self.voice_calls.by_id.iter().filter(|(_, c)| **c == client).map(|(k, _)| k.clone()).collect();
        for (target, call) in calls {
            self.voice_calls.by_id.remove(&(target.clone(), call.clone()));
            self.voice_call_end_quietly(&target, client, &call);
            self.flush(&target);
        }
    }

    /// A target's connection dropped: its calls are over (yaacd hangs them up too).
    pub(super) fn voice_call_target_closed(&mut self, target: &TargetId) {
        let calls: Vec<_> = self.voice_calls.by_id.iter().filter(|((t, _), _)| t == target).map(|(k, c)| (k.1.clone(), *c)).collect();
        for (call, client) in calls {
            self.voice_calls.by_id.remove(&(target.clone(), call.clone()));
            let params = serde_json::json!({ "target": target, "callId": call, "state": "ended", "error": "the connection to the target dropped" });
            self.send_to(client, Message::notification(attach::names::VOICE_CALL_STATE, params));
        }
    }
}
