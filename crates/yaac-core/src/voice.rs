//! Voice input state (docs/plan/voice.md), without UI: one dictation at a time per window,
//! its audio held back until the target grants credit, and the words so far. The words go
//! only to the composer, and only when the UI takes them ([`Dictation::text`] while it runs,
//! [`DictationOutcome`] once it is over); nothing here sends a message or touches the journal.
//! Audio lives in memory only while it waits for credit.
//!
//! Dictation is not bound to the composer's thread (P392): any instance that transcribes, on
//! any connected target, serves any composer — a thread's or a new-thread draft's. The hub
//! chooses; [`transcriber`] makes the same choice beforehand, so the microphone shows (or says
//! why not) and names who will transcribe.

use std::collections::VecDeque;

use yaac_protocol::transcription::{self, Candidate, Choice, Context};
use yaac_protocol::{InstanceId, StreamId, TargetId, VoiceFormat};

use crate::attachments::DraftRef;
use crate::state::AppState;

/// Audio waiting for the stream or for credit, at most. Past it new audio is dropped (and
/// counted): the target is not keeping up, and the user sees it end with an error.
pub const MAX_QUEUED_AUDIO: usize = 2 * 1024 * 1024;

/// The composer's own target and instance: a thread's, or what a draft has chosen so far.
pub fn context_of(state: &AppState, draft: &DraftRef) -> (Option<TargetId>, Option<InstanceId>) {
    match draft {
        DraftRef::Thread(key) => (Some(key.target.clone()), state.summary(key).map(|s| s.instance.clone())),
        DraftRef::New(id) => state.new_threads.iter().find(|d| d.id == *id).map_or((None, None), |d| (d.target.clone(), d.instance.clone())),
    }
}

/// Who would transcribe for `draft` now, or why nobody can: the same choice the hub makes
/// ([`yaac_protocol::transcription::choose`]) over the targets this window knows. The only test
/// the UI makes before offering the microphone (invariant 4: never the provider kind).
pub fn transcriber(state: &AppState, draft: &DraftRef) -> Choice {
    let (target, instance) = context_of(state, draft);
    let candidates: Vec<Candidate<'_>> = state
        .targets
        .iter()
        .map(|t| Candidate { target: &t.entry.id, name: t.entry.name.as_str(), connected: t.connected(), providers: &t.providers })
        .collect();
    transcription::choose(Context { target: target.as_ref(), instance: instance.as_ref() }, &candidates)
}

/// Where a dictation is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictationPhase {
    /// `hub/voice/start` sent, no answer yet; audio is queued.
    Starting,
    /// Recording: audio flows as credit allows.
    Recording,
    /// The user stopped; `hub/voice/stop` is sent (or will be, once the start is answered) and
    /// the final words are awaited.
    Stopping,
}

/// The running dictation.
#[derive(Clone, Debug)]
pub struct Dictation {
    /// The composer that gets the words.
    pub key: DraftRef,
    /// The transcriber's target: the hub's answer once started, the window's own choice before.
    pub target: TargetId,
    /// "Claude on wsl: Ubuntu": who transcribes.
    pub label: String,
    pub phase: DictationPhase,
    /// The hub-local stream, once started.
    pub stream: Option<StreamId>,
    /// What to record: the chosen transcriber's, confirmed by the hub's answer.
    pub format: Option<VoiceFormat>,
    /// The words so far: replaced by each transcript, never appended to.
    pub text: String,
    /// The target said `text` will not change again.
    pub is_final: bool,
    /// Audio bytes dropped because too much was waiting.
    pub dropped: u64,
    pub(crate) credit: u64,
    pub(crate) queued: VecDeque<u8>,
    /// Bytes handed to the link so far.
    pub(crate) sent: u64,
    /// `hub/voice/stop` has gone out.
    pub(crate) stop_sent: bool,
    /// Cancelled while starting: cancel as soon as the stream is known.
    pub(crate) cancel_on_start: bool,
}

impl Dictation {
    pub(crate) fn new(key: DraftRef, target: TargetId, label: String, format: VoiceFormat) -> Self {
        Self {
            key,
            target,
            label,
            phase: DictationPhase::Starting,
            stream: None,
            format: Some(format),
            text: String::new(),
            is_final: false,
            dropped: 0,
            credit: 0,
            queued: VecDeque::new(),
            sent: 0,
            stop_sent: false,
            cancel_on_start: false,
        }
    }

    /// Queue recorded audio (in `format`). Never blocks; drops past [`MAX_QUEUED_AUDIO`].
    pub(crate) fn push_audio(&mut self, pcm: &[u8]) {
        if self.phase == DictationPhase::Stopping {
            return;
        }
        let room = MAX_QUEUED_AUDIO.saturating_sub(self.queued.len());
        let take = room.min(pcm.len());
        self.queued.extend(&pcm[..take]);
        self.dropped += (pcm.len() - take) as u64;
    }

    /// The next chunk the credit allows, if any.
    pub(crate) fn next_chunk(&mut self) -> Option<(StreamId, Vec<u8>)> {
        let stream = self.stream?;
        let n = usize::try_from(self.credit).unwrap_or(usize::MAX).min(self.queued.len()).min(yaac_protocol::frame::MAX_DATA_PAYLOAD);
        if n == 0 {
            return None;
        }
        let bytes: Vec<u8> = self.queued.drain(..n).collect();
        self.credit -= n as u64;
        self.sent += n as u64;
        Some((stream, bytes))
    }

    /// Every byte the hub will have received once the queue drains: `voice/stop`'s `bytes`.
    pub(crate) fn total_bytes(&self) -> u64 {
        self.sent + self.queued.len() as u64
    }
}

/// A dictation that is over, for the UI to take once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictationOutcome {
    /// The composer that gets the words.
    pub key: DraftRef,
    /// The words to insert at the caret (empty after a cancel or when nothing was heard).
    pub text: String,
    /// Why it ended early, in words for the user.
    pub error: Option<String>,
}
