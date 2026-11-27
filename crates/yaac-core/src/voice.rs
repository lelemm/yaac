//! Voice input state (docs/plan/voice.md), without UI: one dictation at a time per window,
//! its audio held back until the target grants credit, and the words so far. The words go
//! only to the composer, and only when the UI takes them ([`Dictation::text`] while it runs,
//! [`DictationOutcome`] once it is over); nothing here sends a message or touches the journal.
//! Audio lives in memory only while it waits for credit.

use std::collections::VecDeque;

use yaac_protocol::{Capability, StreamId, ThreadEffective, VoiceFormat};

use crate::key::ThreadKey;

/// Audio waiting for the stream or for credit, at most. Past it new audio is dropped (and
/// counted): the target is not keeping up, and the user sees it end with an error.
pub const MAX_QUEUED_AUDIO: usize = 2 * 1024 * 1024;

/// The format to record for a thread, when it can dictate now: the effective capabilities
/// have `voiceInput` and the target reported the format. The only test the UI makes before
/// showing the microphone (invariant 4: never the provider kind).
pub fn voice_format(effective: Option<&ThreadEffective>) -> Option<&VoiceFormat> {
    let e = effective?;
    if !e.capabilities.contains(&Capability::VoiceInput) {
        return None;
    }
    e.voice_input.as_ref()
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
    /// The thread whose composer gets the words.
    pub key: ThreadKey,
    pub phase: DictationPhase,
    /// The hub-local stream, once started.
    pub stream: Option<StreamId>,
    /// What to record, once started (also known beforehand from the thread's effective
    /// capabilities).
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
    pub(crate) fn new(key: ThreadKey) -> Self {
        Self {
            key,
            phase: DictationPhase::Starting,
            stream: None,
            format: None,
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
    /// The thread whose composer gets the words.
    pub key: ThreadKey,
    /// The words to insert at the caret (empty after a cancel or when nothing was heard).
    pub text: String,
    /// Why it ended early, in words for the user.
    pub error: Option<String>,
}
