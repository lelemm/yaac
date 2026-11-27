//! Voice input in the window controller (docs/plan/voice.md, docs/07-ui.md#voice-input), shared
//! by egui, WPF and Android. `yaac-core` keeps the dictation (start, audio under credit, stop,
//! the words); this decides when the microphone is offered, records it where this build can
//! (`voice_capture`, Windows), and hands the finished words to the composer at its caret. The
//! words are never sent by themselves, and only one dictation runs per window.
//!
//! Every composer — a thread's or a new-thread draft's — has the microphone whenever any
//! instance on any connected target can transcribe (P392); it is not the thread's harness that
//! decides. Without one, the microphone is shown disabled with the reason.

use std::collections::VecDeque;

use serde_json::{Value, json};
use yaac_core::attachments::DraftRef;
use yaac_core::voice::DictationPhase;
use yaac_protocol::transcription::Choice;
use yaac_protocol::{AudioEncoding, VoiceFormat};

use crate::app::YaacApp;
use crate::voice_capture::{self, Capture, Captured};

/// The controller's side of voice input.
#[derive(Default)]
pub(crate) struct Voice {
    /// The microphone, while this controller records it.
    capture: Option<Capture>,
    /// Loudness of the last audio (0..1), for the indicator.
    pub(crate) level: f32,
    /// The last failure, for its thread's composer, until the next start there.
    error: Option<(DraftRef, String)>,
    /// Finished words not yet in a composer: (thread, words), oldest first.
    pending: VecDeque<(DraftRef, String)>,
    /// How many words were handed out: the `insert` ids.
    sequence: u64,
    /// When the running dictation started, for its elapsed time and `maxSeconds`.
    started: Option<(DraftRef, std::time::Instant)>,
}

impl std::fmt::Debug for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voice").field("recording", &self.capture.is_some()).field("error", &self.error).finish_non_exhaustive()
    }
}

/// Where a composer's dictation is, as the renderers name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Idle,
    Starting,
    Listening,
    Stopping,
    Error,
}

impl State {
    pub(crate) fn id(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Starting => "starting",
            State::Listening => "listening",
            State::Stopping => "stopping",
            State::Error => "error",
        }
    }

    pub(crate) fn active(self) -> bool {
        matches!(self, State::Starting | State::Listening)
    }
}

/// Whether `key`'s composer can dictate now, and through whom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Offer {
    /// A transcriber is ready: what to record and "Claude on wsl: Ubuntu".
    Ready { format: VoiceFormat, label: String },
    /// None is: why, in words for the user.
    Unavailable(String),
}

/// Who transcribes for `key`: the running dictation's transcriber, else the same choice the
/// hub will make ([`yaac_core::voice::transcriber`]: any ready instance on any connected
/// target, the composer's own first). Never the provider kind (invariant 4). A format this
/// host cannot produce counts as unavailable.
pub(crate) fn offer(app: &YaacApp, key: &DraftRef) -> Offer {
    let (format, label) = match app.client.dictation().filter(|d| &d.key == key) {
        Some(d) => match d.format.clone() {
            Some(format) => (format, d.label.clone()),
            None => return Offer::Unavailable("The dictation has no audio format.".into()),
        },
        None => match yaac_core::voice::transcriber(&app.client.state, key) {
            Choice::Ready(t) => (t.format, t.label),
            Choice::Unavailable(why) => return Offer::Unavailable(why),
        },
    };
    if format.format == AudioEncoding::Pcm16le && format.channels >= 1 && format.sample_rate > 0 {
        Offer::Ready { format, label }
    } else {
        Offer::Unavailable(format!("{label} wants audio this YAAC cannot record."))
    }
}

/// The format to record for `key`, when it can dictate now.
pub(crate) fn offered(app: &YaacApp, key: &DraftRef) -> Option<VoiceFormat> {
    match offer(app, key) {
        Offer::Ready { format, .. } => Some(format),
        Offer::Unavailable(_) => None,
    }
}

/// The state of `key`'s composer, with the words so far.
pub(crate) fn state<'a>(app: &'a YaacApp, key: &DraftRef) -> (State, Option<&'a str>) {
    match app.client.dictation() {
        Some(d) if &d.key == key => {
            let state = match d.phase {
                DictationPhase::Starting => State::Starting,
                DictationPhase::Recording => State::Listening,
                DictationPhase::Stopping => State::Stopping,
            };
            (state, Some(d.text.as_str()).filter(|t| !t.is_empty()))
        }
        _ if app.voice.error.as_ref().is_some_and(|(k, _)| k == key) => (State::Error, None),
        _ => (State::Idle, None),
    }
}

/// Another thread is dictating (one at a time per window).
fn busy_elsewhere(app: &YaacApp, key: &DraftRef) -> bool {
    app.client.dictation().is_some_and(|d| &d.key != key)
}

/// `composer.voiceStart`: start dictating into `key`'s composer. This controller records when
/// it can ([`voice_capture::AVAILABLE`]); otherwise the renderer sends `composer.voiceAudio`.
pub(crate) fn start(app: &mut YaacApp, key: &DraftRef) {
    #[cfg(feature = "desktop")]
    if crate::desktop_call::busy(app) {
        return;
    }
    let Some(format) = offered(app, key) else { return };
    if app.client.dictation().is_some() {
        return;
    }
    app.voice.error = None;
    app.voice.level = 0.0;
    if let Err(why) = app.client.voice_start(key, None) {
        app.voice.error = Some((key.clone(), why));
        return;
    }
    app.voice.started = Some((key.clone(), std::time::Instant::now()));
    if voice_capture::AVAILABLE {
        let ctx = app.ctx.clone();
        match voice_capture::start(format.sample_rate, format.channels, move || ctx.request_repaint()) {
            Ok(capture) => app.voice.capture = Some(capture),
            Err(why) => {
                app.client.voice_cancel();
                app.voice.error = Some((key.clone(), why));
            }
        }
    }
}

/// `composer.voiceAudio`: audio the renderer recorded (`pcm16le` in the thread's format).
pub(crate) fn audio(app: &mut YaacApp, key: &DraftRef, pcm: &[u8]) {
    if app.client.dictation().is_some_and(|d| &d.key == key) && app.voice.capture.is_none() {
        app.client.voice_audio(pcm);
    }
}

/// `composer.voiceStop`: no more audio; the words follow. When this controller records, the
/// rest of the recording goes first.
pub(crate) fn stop(app: &mut YaacApp, key: &DraftRef) {
    if !app.client.dictation().is_some_and(|d| &d.key == key) {
        return;
    }
    match &app.voice.capture {
        Some(capture) => capture.stop(),
        None => app.client.voice_stop(),
    }
}

/// `composer.voiceCancel`: drop the dictation and its words.
pub(crate) fn cancel(app: &mut YaacApp, key: &DraftRef) {
    if !app.client.dictation().is_some_and(|d| &d.key == key) {
        return;
    }
    if let Some(capture) = app.voice.capture.take() {
        capture.cancel();
    }
    app.voice.level = 0.0;
    app.client.voice_cancel();
}

/// Every frame: the microphone's audio to the hub, the end of a dictation to the composer.
pub(crate) fn upkeep(app: &mut YaacApp) {
    // Leaving the thread drops a dictation still listening (its words were never asked for);
    // one already finishing keeps its words, which go to the end of that draft.
    if let Some(d) = app.client.dictation()
        && d.phase != DictationPhase::Stopping
        && app.open_composer().as_ref() != Some(&d.key)
    {
        let key = d.key.clone();
        cancel(app, &key);
    }
    // The thread's limit: stop by itself (the target would end it there anyway).
    if let Some(d) = app.client.dictation()
        && d.phase != DictationPhase::Stopping
        && let Some(max) = offered(app, &d.key).and_then(|f| f.max_seconds)
        && elapsed(app, &d.key).is_some_and(|secs| secs >= u64::from(max))
    {
        let key = d.key.clone();
        stop(app, &key);
    }
    if app.client.dictation().is_some_and(|d| d.phase != DictationPhase::Stopping) {
        // The elapsed time on the button moves once a second.
        app.ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
    if let Some(capture) = &app.voice.capture {
        let mut done = false;
        let mut failed = None;
        let mut audio = Vec::new();
        while let Ok(event) = capture.events.try_recv() {
            match event {
                Captured::Audio(pcm) => audio.push(pcm),
                Captured::Level(level) => app.voice.level = level,
                Captured::Failed(why) => failed = Some(why),
                Captured::Done => done = true,
            }
        }
        for pcm in audio {
            app.client.voice_audio(&pcm);
        }
        if let Some(why) = failed {
            // The words heard so far still arrive; say why it ended.
            app.voice.capture = None;
            if let Some(key) = app.client.dictation().map(|d| d.key.clone()) {
                app.voice.error = Some((key, why));
            }
            app.client.voice_stop();
        } else if done {
            app.voice.capture = None;
            app.client.voice_stop();
        }
    }
    if app.client.dictation().is_none() && app.voice.capture.take().is_some() {
        // Ended on the target's side (or cancelled): stop recording.
        app.voice.level = 0.0;
    }
    if let Some(outcome) = app.client.take_dictation_outcome() {
        app.voice.level = 0.0;
        app.voice.started = None;
        app.voice.capture = None;
        if let Some(why) = outcome.error {
            app.voice.error = Some((outcome.key.clone(), why));
        }
        let text = outcome.text.trim();
        if !text.is_empty() {
            app.voice.pending.push_back((outcome.key, text.to_owned()));
        }
    }
}

/// Whole seconds since `key`'s dictation started.
fn elapsed(app: &YaacApp, key: &DraftRef) -> Option<u64> {
    app.voice.started.as_ref().filter(|(k, _)| k == key).map(|(_, at)| at.elapsed().as_secs())
}

/// `m:ss`.
fn clock(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The next finished words for `key`, with the space they need after `before` (the text
/// before the caret).
pub(crate) fn take_for(app: &mut YaacApp, key: &DraftRef, before: &str) -> Option<String> {
    let i = app.voice.pending.iter().position(|(k, _)| k == key)?;
    let (_, text) = app.voice.pending.remove(i)?;
    Some(spaced(before, &text))
}

/// Words for composers that are not open: they go to the end of that draft, so nothing is
/// lost (a new-thread draft that is gone takes nothing). Returns the drafts changed.
pub(crate) fn settle_elsewhere(app: &mut YaacApp, open: Option<&DraftRef>) -> Vec<DraftRef> {
    let mut changed = Vec::new();
    let mut keep = VecDeque::new();
    while let Some((key, text)) = app.voice.pending.pop_front() {
        if Some(&key) == open {
            keep.push_back((key, text));
            continue;
        }
        let state = &mut app.client.state;
        let draft = match &key {
            DraftRef::Thread(k) => Some(state.drafts.entry(k.clone()).or_default()),
            DraftRef::New(id) => state.new_threads.iter_mut().find(|d| d.id == *id).map(|d| &mut d.draft),
        };
        let Some(draft) = draft else { continue };
        let joined = spaced(&draft.text, &text);
        draft.text.push_str(&joined);
        changed.push(key);
    }
    app.voice.pending = keep;
    changed
}

/// `text` with a space in front when it follows a word.
fn spaced(before: &str, text: &str) -> String {
    if before.chars().last().is_some_and(|c| !c.is_whitespace()) {
        format!(" {text}")
    } else {
        text.to_owned()
    }
}

/// The next `insert` id for the renderers (`{id, text}` at the caret, once).
pub(crate) fn insert_id(app: &mut YaacApp) -> String {
    app.voice.sequence += 1;
    format!("voice-{}", app.voice.sequence)
}

/// The composer frame's `voice` object. Every composer has one: ready, it names who
/// transcribes; with no transcriber anywhere, its button is disabled with the reason.
pub(crate) fn view(app: &YaacApp, key: &DraftRef) -> Value {
    let capture = if voice_capture::AVAILABLE { "controller" } else { "renderer" };
    let (format, label) = match offer(app, key) {
        Offer::Ready { format, label } => (format, label),
        Offer::Unavailable(why) => {
            return json!({
                "state": State::Idle.id(),
                "partial": null,
                "error": app.voice.error.as_ref().filter(|(k, _)| k == key).map(|(_, why)| why),
                "status": "",
                "time": null,
                "format": null,
                "transcriber": null,
                "unavailable": why,
                "capture": capture,
                "button": { "id": "composer.voiceStart", "label": "Dictate", "enabled": false, "detail": why },
            });
        }
    };
    let (state, partial) = state(app, key);
    let elsewhere = busy_elsewhere(app, key);
    #[cfg(feature = "desktop")]
    let in_call = crate::desktop_call::busy(app);
    #[cfg(not(feature = "desktop"))]
    let in_call = false;
    let (id, button, enabled) = match state {
        State::Starting | State::Listening => ("composer.voiceStop", "Stop dictation", true),
        State::Stopping => ("composer.voiceStop", "Finishing…", false),
        State::Idle | State::Error => ("composer.voiceStart", "Dictate", !elsewhere && !in_call),
    };
    let detail = if elsewhere {
        "Another composer is taking dictation".to_owned()
    } else if in_call {
        "The microphone is in use by a voice call".to_owned()
    } else if state.active() {
        "Stop and put the words in the message".to_owned()
    } else {
        format!("Speak; {label} turns it into words for the message, not sent")
    };
    // Listening: the elapsed time, and near the transcriber's limit the time left.
    let secs = elapsed(app, key).filter(|_| state == State::Listening);
    let left = secs.zip(format.max_seconds).map(|(s, max)| u64::from(max).saturating_sub(s)).filter(|left| *left <= 15);
    let time = secs.map(|s| match left {
        Some(left) => format!("{} · {} left", clock(s), clock(left)),
        None => clock(s),
    });
    let status = match state {
        _ if partial.is_some() => partial.unwrap_or_default().to_owned(),
        State::Starting => format!("Starting dictation with {label}…"),
        State::Listening => format!("Dictating with {label}… {}", time.as_deref().unwrap_or_default()).trim_end().to_owned(),
        State::Stopping => "Finishing…".to_owned(),
        State::Idle | State::Error => String::new(),
    };
    json!({
        "state": state.id(),
        "partial": partial,
        "error": app.voice.error.as_ref().filter(|(k, _)| k == key).map(|(_, why)| why),
        "status": status,
        "time": time,
        "format": format,
        "transcriber": label,
        // Who records: this controller (WPF on Windows), or the renderer (Android).
        "capture": capture,
        "button": { "id": id, "label": button, "enabled": enabled, "detail": detail },
    })
}

/// Tests: words as if a dictation had just finished for `key`.
#[cfg(test)]
pub(crate) fn finished(app: &mut YaacApp, key: &DraftRef, words: &str) {
    app.voice.pending.push_back((key.clone(), words.to_owned()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_after_a_word_get_a_space() {
        assert_eq!(spaced("fix the", "bug"), " bug");
        assert_eq!(spaced("", "bug"), "bug");
        assert_eq!(spaced("line\n", "bug"), "bug");
    }

    #[test]
    fn the_clock_reads_minutes_and_seconds() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(75), "1:15");
    }
}
