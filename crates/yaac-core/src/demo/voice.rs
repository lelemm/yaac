//! The demo hub's voice input (docs/plan/voice.md): `hub/voice/*` and the audio streams, with a
//! scripted "speech-to-text" that reveals a fixed sentence as audio arrives, so `--demo` has a
//! working microphone and tests can follow a dictation's whole path (start, credit, audio,
//! interim words, stop, final words, ended). Audio is counted, never kept.
use std::collections::{HashMap, VecDeque};

use serde_json::{Value, json};
use yaac_protocol::{StreamId, attach::names};

/// What the demo "hears": a word per quarter second of audio.
pub const SPOKEN: &str = "Add a retry with backoff around the invoice webhook";

/// Streams start here, apart from the terminals' and the tunnels'.
const FIRST_STREAM: u32 = 20_000;
/// Credit granted at start and topped up as audio arrives.
const CREDIT: u64 = 64 * 1024;

#[derive(Debug)]
struct Dictation {
    target: Value,
    format: Value,
    bytes: u64,
    words: usize,
}

/// Every demo dictation.
#[derive(Debug, Default)]
pub(crate) struct Voices {
    next: u32,
    running: HashMap<StreamId, Dictation>,
    /// Notifications for the window (`hub/voice/transcript`, `hub/voice/ended`, `stream/credit`).
    pub(crate) events: VecDeque<(String, Value)>,
}

impl Voices {
    pub(crate) fn owns(&self, stream: StreamId) -> bool {
        self.running.contains_key(&stream)
    }

    /// Answer a `hub/voice/*` request. `format` is the thread's reported format, if it can
    /// dictate.
    pub(crate) fn request(&mut self, method: &str, params: &Value, format: Option<Value>) -> Result<Value, String> {
        let target = params.get("target").cloned().unwrap_or(Value::Null);
        let stream = params.get("stream").and_then(|s| serde_json::from_value::<StreamId>(s.clone()).ok());
        match method {
            names::VOICE_START => {
                let format = format.ok_or("This thread cannot take voice input.")?;
                self.next += 1;
                let stream = StreamId(FIRST_STREAM + self.next);
                self.running.insert(stream, Dictation { target, format: format.clone(), bytes: 0, words: 0 });
                Ok(json!({ "stream": stream, "initialCredit": CREDIT, "format": format }))
            }
            names::VOICE_STOP => {
                if let Some(stream) = stream
                    && let Some(d) = self.running.remove(&stream)
                {
                    let heard = words(&d).max(usize::from(d.bytes > 0));
                    let text = SPOKEN.split(' ').take(heard).collect::<Vec<_>>().join(" ");
                    self.events.push_back((names::VOICE_TRANSCRIPT.into(), json!({ "target": d.target, "stream": stream, "text": text, "final": true })));
                    self.events.push_back((names::VOICE_ENDED.into(), json!({ "target": d.target, "stream": stream })));
                }
                Ok(json!({}))
            }
            names::VOICE_CANCEL => {
                if let Some(stream) = stream
                    && let Some(d) = self.running.remove(&stream)
                {
                    self.events.push_back((names::VOICE_ENDED.into(), json!({ "target": d.target, "stream": stream })));
                }
                Ok(json!({}))
            }
            _ => Err("unknown voice method".into()),
        }
    }

    /// Audio on a dictation's stream: counted, credit returned, words revealed.
    pub(crate) fn data(&mut self, stream: StreamId, bytes: &[u8]) {
        let Some(d) = self.running.get_mut(&stream) else { return };
        d.bytes += bytes.len() as u64;
        self.events.push_back(("stream/credit".into(), json!({ "stream": stream, "bytes": bytes.len() })));
        let now = words(d);
        if now > d.words {
            d.words = now;
            let text = SPOKEN.split(' ').take(now).collect::<Vec<_>>().join(" ");
            self.events.push_back((names::VOICE_TRANSCRIPT.into(), json!({ "target": d.target, "stream": stream, "text": text, "final": false })));
        }
    }
}

/// Words "heard" so far: one per quarter second of audio in the dictation's format.
fn words(d: &Dictation) -> usize {
    let rate = d.format.get("sampleRate").and_then(Value::as_u64).unwrap_or(1).max(1);
    let channels = d.format.get("channels").and_then(Value::as_u64).unwrap_or(1).max(1);
    let quarter = (rate * channels * 2 / 4).max(1);
    usize::try_from(d.bytes / quarter).unwrap_or(usize::MAX).min(SPOKEN.split(' ').count())
}
