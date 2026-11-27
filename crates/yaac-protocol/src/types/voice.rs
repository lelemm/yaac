//! Voice input (docs/plan/voice.md): the host records the microphone and streams raw audio to
//! `yaacd`, which hands it to the harness's own speech-to-text and sends the words back. The
//! words only ever land in the composer; audio is never journaled or written to disk.
use super::{OpId, StreamId, ThreadId};
use crate::macros::open_enum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

open_enum! {
    /// How the audio samples on a voice stream are encoded.
    pub enum AudioEncoding fallback Unknown {
        /// Signed 16-bit little-endian PCM, interleaved when there is more than one channel.
        Pcm16le = "pcm16le",
    }
}

/// The audio a thread's harness wants: reported by the target, never assumed by the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceFormat {
    /// Sample encoding.
    pub format: AudioEncoding,
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// Channel count (1 for every harness so far).
    pub channels: u16,
    /// Longest dictation the harness accepts, in seconds of audio, when it has a limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_seconds: Option<u32>,
}

/// `voice/start`: begin one dictation for a thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStartParams {
    /// Operation identity: an uncertain start is shown, never re-sent.
    pub op_id: OpId,
    /// The thread whose harness transcribes (and whose composer gets the words).
    pub thread_id: ThreadId,
    /// The spoken language as a code (`en`, `pt-BR`), when the user chose one; otherwise the
    /// harness's own setting applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// The dictation's audio stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStartResult {
    /// Connection-local stream for audio, host → target only, on the interactive lane.
    pub stream: StreamId,
    /// Audio bytes granted to the host; more arrive with `stream/credit`.
    pub initial_credit: u64,
    /// What to send on `stream`.
    pub format: VoiceFormat,
}

/// `voice/stop`: no more audio for one dictation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStopParams {
    /// The stream `voice/start` returned.
    pub stream: StreamId,
    /// Every audio byte the host sent on `stream`. Data frames and control messages are
    /// scheduled on different lanes, so this request can overtake the last chunk: the daemon
    /// finishes only once it has received this many bytes (or after a short grace period).
    pub bytes: u64,
}

/// `voice/cancel`: one dictation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCancelParams {
    /// The stream `voice/start` returned.
    pub stream: StreamId,
}

/// `voice/transcript`: the words so far.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTranscriptParams {
    /// The dictation.
    pub stream: StreamId,
    /// The whole transcript of this dictation so far. It **replaces** the previous `text`
    /// (an interim guess may be revised); it is not a delta.
    pub text: String,
    /// The last transcript of this dictation: `text` will not change again.
    #[serde(rename = "final")]
    pub is_final: bool,
}

/// `voice/ended`: the dictation is over and its stream is closed. Always the last message for
/// a stream that `voice/start` opened, whether it was stopped, cancelled, or failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceEndedParams {
    /// The dictation.
    pub stream: StreamId,
    /// Why it ended early, in words for the user; absent after a normal stop or a cancel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
