//! Voice call state (docs/plan/voice-call.md), without UI: one call at a time per window, its
//! SDP answer for the renderer's peer, and what was said. The renderer owns the media (its own
//! WebRTC peer to the service); only the offer, the answer and transcript text pass through
//! here. Nothing here sends a message or touches the journal: the turns a call starts reach
//! the thread through the target, as any turn does.

use yaac_protocol::{Capability, ThreadEffective, VoiceCallRole};

use crate::key::ThreadKey;

/// Utterances kept per call, oldest dropped first.
pub const MAX_TRANSCRIPT_LINES: usize = 50;

/// Whether a thread can start a call now: its effective capabilities have `voiceCall`. The
/// only test the UI makes before offering a call (invariant 4: never the provider kind).
pub fn offered(effective: Option<&ThreadEffective>) -> bool {
    effective.is_some_and(|e| e.capabilities.contains(&Capability::VoiceCall))
}

/// Where a call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallPhase {
    /// `hub/voiceCall/start` sent, no answer yet.
    Starting,
    /// The target accepted; the SDP answer is awaited (or just arrived).
    Connecting,
    /// The answer arrived: media flows between the renderer's peer and the service.
    Active,
    /// The user hung up; the end is on its way.
    Ending,
}

/// One utterance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallLine {
    pub role: VoiceCallRole,
    /// The whole utterance so far (replaced while it is not final).
    pub text: String,
    pub is_final: bool,
}

/// The running call.
#[derive(Clone, Debug)]
pub struct VoiceCall {
    /// The thread the call talks with.
    pub key: ThreadKey,
    pub phase: CallPhase,
    /// The target's id for the call, once started.
    pub call_id: Option<String>,
    /// The service's SDP answer, once it arrived: the renderer applies it once per call id.
    pub answer: Option<String>,
    /// What was said, oldest first.
    pub transcript: Vec<CallLine>,
    /// Hung up while starting: end as soon as the call id is known.
    pub(crate) end_on_start: bool,
    /// `hub/voiceCall/end` has gone out.
    pub(crate) end_sent: bool,
}

impl VoiceCall {
    pub(crate) fn new(key: ThreadKey) -> Self {
        Self { key, phase: CallPhase::Starting, call_id: None, answer: None, transcript: Vec::new(), end_on_start: false, end_sent: false }
    }

    /// A transcript update: replaces the open utterance of `role`, or starts a new one.
    pub(crate) fn heard(&mut self, role: VoiceCallRole, text: &str, is_final: bool) {
        match self.transcript.iter_mut().rev().find(|l| l.role == role && !l.is_final) {
            Some(line) => {
                line.text = text.to_owned();
                line.is_final = is_final;
            }
            None => self.transcript.push(CallLine { role, text: text.to_owned(), is_final }),
        }
        let over = self.transcript.len().saturating_sub(MAX_TRANSCRIPT_LINES);
        self.transcript.drain(..over);
    }
}

/// A call that is over, for the UI to take once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallOutcome {
    pub key: ThreadKey,
    /// Why it ended, in words for the user; absent after a normal hang-up.
    pub error: Option<String>,
    /// What was said.
    pub transcript: Vec<CallLine>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_protocol::{TargetId, ThreadId};

    #[test]
    fn an_open_utterance_is_replaced_and_a_final_one_closes_it() {
        let mut call = VoiceCall::new(ThreadKey::new(TargetId::new("box"), ThreadId::new("t")));
        call.heard(VoiceCallRole::User, "run", false);
        call.heard(VoiceCallRole::Assistant, "Sure", false);
        call.heard(VoiceCallRole::User, "run the tests", true);
        call.heard(VoiceCallRole::User, "thanks", false);
        let lines: Vec<_> = call.transcript.iter().map(|l| (l.role.clone(), l.text.as_str(), l.is_final)).collect();
        assert_eq!(lines, vec![
            (VoiceCallRole::User, "run the tests", true),
            (VoiceCallRole::Assistant, "Sure", false),
            (VoiceCallRole::User, "thanks", false),
        ]);
        for i in 0..MAX_TRANSCRIPT_LINES + 5 {
            call.heard(VoiceCallRole::Assistant, &i.to_string(), true);
        }
        assert_eq!(call.transcript.len(), MAX_TRANSCRIPT_LINES);
    }
}
