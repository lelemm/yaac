//! Voice call in the window controller (docs/plan/voice-call.md), shared by egui, WPF and
//! Android. `yaac-core` keeps the call (start, the SDP answer, what was said); the renderer
//! owns the media — its own WebRTC peer, microphone and speaker — and this decides when a call
//! is offered and builds the composer frame's `call` object. One call per window. Leaving the
//! thread does not end a call: it is a conversation with the thread, not with the composer.

use serde_json::{Value, json};
use yaac_core::key::ThreadKey;
use yaac_core::voice_call::{CallLine, CallOutcome, CallPhase};
use yaac_protocol::VoiceCallRole;

use crate::app::YaacApp;

/// The controller's side of a voice call: the last call's end, for its thread, until the next
/// start there.
#[derive(Debug, Default)]
pub(crate) struct VoiceCallUi {
    last: Option<CallOutcome>,
    /// A start the controller refused (or yaac-core did), for its thread.
    refused: Option<(ThreadKey, String)>,
}

/// Where a composer's call is, as the renderers name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Idle,
    Starting,
    Connecting,
    Active,
    Ending,
    Error,
}

impl State {
    pub(crate) fn id(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Starting => "starting",
            State::Connecting => "connecting",
            State::Active => "active",
            State::Ending => "ending",
            State::Error => "error",
        }
    }
}

/// Whether `key` offers a call: the thread's effective capabilities have `voiceCall` (never
/// the provider kind, invariant 4).
pub(crate) fn offered(app: &YaacApp, key: &ThreadKey) -> bool {
    yaac_core::voice_call::offered(app.client.state.transcripts.get(key).and_then(|t| t.effective.as_ref()))
}

/// The state of `key`'s call.
pub(crate) fn state(app: &YaacApp, key: &ThreadKey) -> State {
    match app.client.voice_call() {
        Some(c) if &c.key == key => match c.phase {
            CallPhase::Starting => State::Starting,
            CallPhase::Connecting => State::Connecting,
            CallPhase::Active => State::Active,
            CallPhase::Ending => State::Ending,
        },
        _ if error(app, key).is_some() => State::Error,
        _ => State::Idle,
    }
}

fn error<'a>(app: &'a YaacApp, key: &ThreadKey) -> Option<&'a str> {
    if let Some((k, why)) = &app.voice_call.refused
        && k == key
    {
        return Some(why);
    }
    app.voice_call.last.as_ref().filter(|o| &o.key == key).and_then(|o| o.error.as_deref())
}

/// A call runs in this window on another thread.
fn busy_elsewhere(app: &YaacApp, key: &ThreadKey) -> bool {
    app.client.voice_call().is_some_and(|c| &c.key != key)
}

/// `composer.callStart`: start a call with `key` from the renderer's complete SDP offer, in the
/// renderer's `language` or the window's ([`crate::voice_language`]). The last call's words and
/// error on that thread are cleared first.
pub(crate) fn start(app: &mut YaacApp, key: &ThreadKey, sdp_offer: String, language: Option<&str>) -> Result<(), String> {
    if !offered(app, key) {
        return Err("This thread cannot take a voice call.".into());
    }
    app.voice_call.refused = None;
    if app.voice_call.last.as_ref().is_some_and(|o| &o.key == key) {
        app.voice_call.last = None;
    }
    let started = if app.client.dictation().is_some() {
        Err("Finish the dictation first: the microphone is in use.".to_owned())
    } else {
        let language = crate::voice_language::language(app, language);
        app.client.voice_call_start(key, sdp_offer, language)
    };
    if let Err(why) = &started {
        app.voice_call.refused = Some((key.clone(), why.clone()));
    }
    started
}

/// `composer.callEnd`: hang up `key`'s call. Always allowed.
pub(crate) fn end(app: &mut YaacApp, key: &ThreadKey) {
    if app.client.voice_call().is_some_and(|c| &c.key == key) {
        app.client.voice_call_end();
    }
}

/// Every frame: the end of a call is kept for its composer.
pub(crate) fn upkeep(app: &mut YaacApp) {
    if let Some(outcome) = app.client.take_voice_call_outcome() {
        app.voice_call.last = Some(outcome);
    }
}

fn role(r: &VoiceCallRole) -> &'static str {
    match r {
        VoiceCallRole::Assistant => "assistant",
        _ => "user",
    }
}

fn lines(lines: &[CallLine]) -> Value {
    json!(lines.iter().map(|l| json!({ "role": role(&l.role), "text": l.text, "final": l.is_final })).collect::<Vec<_>>())
}

/// The composer frame's `call` object, when `key` offers a call (or one is running there).
pub(crate) fn view(app: &YaacApp, key: &ThreadKey) -> Option<Value> {
    let running = app.client.voice_call().filter(|c| &c.key == key);
    if !offered(app, key) && running.is_none() {
        return None;
    }
    let state = state(app, key);
    let elsewhere = busy_elsewhere(app, key);
    let dictating = app.client.dictation().is_some();
    let (id, label, enabled, detail) = match state {
        State::Starting | State::Connecting => ("composer.callEnd", "Cancel call", true, "Connecting the call…"),
        State::Active => ("composer.callEnd", "End call", true, "Hang up"),
        State::Ending => ("composer.callEnd", "Ending…", false, "Hanging up"),
        State::Idle | State::Error => (
            "composer.callStart",
            "Call",
            !elsewhere && !dictating,
            if elsewhere {
                "Another thread is on a call"
            } else if dictating {
                "Finish the dictation first"
            } else {
                "Talk with this thread — it can start work here, using your Codex plan"
            },
        ),
    };
    let status = match state {
        State::Starting | State::Connecting => "Connecting…".to_owned(),
        State::Active => "On a call with this thread".to_owned(),
        State::Ending => "Ending the call…".to_owned(),
        State::Error => error(app, key).unwrap_or_default().to_owned(),
        State::Idle => String::new(),
    };
    let transcript = match running {
        Some(c) => lines(&c.transcript),
        None => app.voice_call.last.as_ref().filter(|o| &o.key == key).map_or_else(|| json!([]), |o| lines(&o.transcript)),
    };
    Some(json!({
        "state": state.id(),
        "callId": running.and_then(|c| c.call_id.as_deref()),
        // The renderer sets this as the remote description once per `callId`.
        "answer": running.filter(|c| matches!(c.phase, CallPhase::Connecting | CallPhase::Active)).and_then(|c| c.answer.as_deref()),
        "error": error(app, key),
        "transcript": transcript,
        "status": status,
        "button": { "id": id, "label": label, "enabled": enabled, "detail": detail },
    }))
}
