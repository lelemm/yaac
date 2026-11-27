//! Voice call (docs/plan/voice-call.md): a speech-to-speech conversation with a thread through
//! its harness's own realtime surface. The client is the media peer; only SDP text and
//! transcript text cross YAAC's wire, never audio.
use super::{OpId, ThreadId};
use crate::macros::open_enum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

open_enum! {
    /// Where a call is.
    pub enum VoiceCallState fallback Unknown {
        /// The harness accepted the start; the SDP answer has not been sent yet.
        Connecting = "connecting",
        /// The answer was sent; media flows between the client and the service.
        Active = "active",
        /// The call is over: the last message for it.
        Ended = "ended",
    }
}

open_enum! {
    /// Who is speaking in a call transcript.
    pub enum VoiceCallRole fallback Unknown {
        /// The user.
        User = "user",
        /// The realtime model.
        Assistant = "assistant",
    }
}

/// `voiceCall/start`: begin a call with a thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCallStartParams {
    /// Operation identity: an uncertain start is shown, never re-sent.
    pub op_id: OpId,
    /// The thread the call talks with (and acts in).
    pub thread_id: ThreadId,
    /// The client's complete (non-trickle) SDP offer: an audio track and the realtime events
    /// data channel.
    pub sdp_offer: String,
}

/// The call that started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCallStartResult {
    /// Opaque, unique on its target.
    pub call_id: String,
}

/// `voiceCall/end`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCallEndParams {
    /// The call.
    pub call_id: String,
}

/// `voiceCall/answer`: the service's SDP answer, once per call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCallAnswerParams {
    /// The call.
    pub call_id: String,
    /// The remote description for the client's peer.
    pub sdp_answer: String,
}

/// `voiceCall/state`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCallStateParams {
    /// The call.
    pub call_id: String,
    /// Where it is now.
    pub state: VoiceCallState,
    /// Why it ended, in words for the user; absent after a normal end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `voiceCall/transcript`: what is being said.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCallTranscriptParams {
    /// The call.
    pub call_id: String,
    /// Who speaks.
    pub role: VoiceCallRole,
    /// The whole current utterance of `role`; it **replaces** the previous `text` for `role`.
    pub text: String,
    /// The utterance is complete; the next message for `role` starts a new one.
    #[serde(rename = "final")]
    pub is_final: bool,
}
