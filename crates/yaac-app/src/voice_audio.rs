//! Voice input's audio path on the host (docs/plan/voice.md): whatever the microphone gives
//! (any rate, any channel count, floating point) to what the thread's harness wants (the
//! target's `VoiceFormat`: `pcm16le` at its rate, mono so far), cut into ~100 ms chunks.
//! Plain values, no device: the capture thread (`voice_capture`) feeds it, the tests drive it.
//! Audio lives here only in memory, on its way to the hub; it is never written anywhere.
#![cfg_attr(not(all(windows, feature = "desktop")), allow(dead_code))]
// Sample arithmetic: positions and samples are small and bounded, clamped where it matters.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss, clippy::cast_possible_wrap)]

/// Length of one chunk sent to the hub.
pub(crate) const CHUNK_MS: u32 = 100;

/// Mix interleaved frames of `channels` samples down to the first `out_channels`
/// (for mono: the mean of every channel).
pub(crate) fn mix(samples: &[f32], channels: usize, out_channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    let out_channels = out_channels.max(1);
    if channels == out_channels {
        return samples.to_vec();
    }
    let frames = samples.len() / channels;
    let mut out = Vec::with_capacity(frames * out_channels);
    for frame in samples.chunks_exact(channels) {
        if out_channels == 1 {
            out.push(frame.iter().sum::<f32>() / channels as f32);
        } else {
            out.extend((0..out_channels).map(|c| frame[c.min(channels - 1)]));
        }
    }
    out
}

/// Streaming linear resampler for one channel; state carries across [`Resampler::push`]
/// calls so chunk boundaries neither click nor lose samples. Equal rates pass through.
#[derive(Debug)]
pub(crate) struct Resampler {
    step: f64,
    /// Where the next output sample falls, in input samples from the next input's first.
    t: f64,
    /// The previous input's last sample: index -1 of the next input.
    prev: f32,
    same: bool,
}

impl Resampler {
    pub(crate) fn new(from: u32, to: u32) -> Self {
        let (from, to) = (from.max(1), to.max(1));
        Self { step: f64::from(from) / f64::from(to), t: 0.0, prev: 0.0, same: from == to }
    }

    pub(crate) fn push(&mut self, input: &[f32]) -> Vec<f32> {
        if self.same {
            return input.to_vec();
        }
        let Some(&last) = input.last() else { return Vec::new() };
        let count = input.len();
        let at = |i: isize| if i < 0 { self.prev } else { input[(i as usize).min(count - 1)] };
        let mut out = Vec::with_capacity(((count as f64 - self.t) / self.step) as usize + 2);
        let mut t = self.t;
        while t <= (count - 1) as f64 {
            let i = t.floor() as isize;
            let frac = (t - i as f64) as f32;
            let (a, b) = (at(i), at(i + 1));
            out.push(a + (b - a) * frac);
            t += self.step;
        }
        self.t = t - count as f64;
        self.prev = last;
        out
    }
}

/// One sample as signed 16-bit, clamped.
pub(crate) fn to_i16(sample: f32) -> i16 {
    (sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

/// Loudness in 0..1 (RMS, scaled so ordinary speech sits near the middle).
pub(crate) fn level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    (mean.sqrt() * 4.0).min(1.0)
}

/// Device samples in, `pcm16le` chunks of [`CHUNK_MS`] at the harness's rate out.
#[derive(Debug)]
pub(crate) struct Pipeline {
    device_channels: usize,
    channels: usize,
    resampler: Resampler,
    chunk_bytes: usize,
    pending: Vec<u8>,
}

impl Pipeline {
    /// `channels` is the harness's channel count; only mono is resampled exactly (every
    /// harness so far), more channels are resampled as one interleaved line.
    pub(crate) fn new(device_rate: u32, device_channels: u16, rate: u32, channels: u16) -> Self {
        let channels = usize::from(channels.max(1));
        let frames = (rate.max(1) * CHUNK_MS / 1000).max(1) as usize;
        Self {
            device_channels: usize::from(device_channels.max(1)),
            channels,
            resampler: Resampler::new(device_rate, rate),
            chunk_bytes: frames * channels * 2,
            pending: Vec::new(),
        }
    }

    /// Whole chunks ready to send, and the loudness of `samples` (interleaved device frames).
    pub(crate) fn push(&mut self, samples: &[f32]) -> (Vec<Vec<u8>>, f32) {
        let mono = mix(samples, self.device_channels, self.channels);
        let loud = level(&mono);
        for s in self.resampler.push(&mono) {
            self.pending.extend_from_slice(&to_i16(s).to_le_bytes());
        }
        let mut out = Vec::new();
        while self.pending.len() >= self.chunk_bytes {
            let rest = self.pending.split_off(self.chunk_bytes);
            out.push(std::mem::replace(&mut self.pending, rest));
        }
        (out, loud)
    }

    /// What is left when recording stops, so the last words are not cut.
    pub(crate) fn flush(&mut self) -> Option<Vec<u8>> {
        (!self.pending.is_empty()).then(|| std::mem::take(&mut self.pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_mixes_to_mono_by_the_mean() {
        assert_eq!(mix(&[1.0, 0.0, 0.5, 0.5], 2, 1), vec![0.5, 0.5]);
        assert_eq!(mix(&[0.25, 0.75], 1, 1), vec![0.25, 0.75]);
    }

    #[test]
    fn resampling_keeps_the_ratio_across_chunks() {
        let mut down = Resampler::new(48_000, 16_000);
        let total: usize = (0..100).map(|_| down.push(&[0.1; 480]).len()).sum();
        assert!(total.abs_diff(16_000) <= 1, "{total}");
        let mut up = Resampler::new(16_000, 44_100);
        let total: usize = (0..10).map(|_| up.push(&[0.0; 1600]).len()).sum();
        // Up to one input sample waits for the next chunk.
        assert!((44_097..=44_100).contains(&total), "{total}");
    }

    #[test]
    fn a_tone_resampled_in_pieces_matches_one_piece() {
        let tone: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.0576).sin() * 0.3).collect();
        let whole = Resampler::new(48_000, 16_000).push(&tone);
        let mut split = Resampler::new(48_000, 16_000);
        let pieces: Vec<f32> = tone.chunks(333).flat_map(|c| split.push(c)).collect();
        assert_eq!(whole.len(), pieces.len());
        assert!(whole.iter().zip(&pieces).all(|(a, b)| (a - b).abs() < 1e-5));
    }

    #[test]
    fn samples_become_clamped_little_endian_i16() {
        assert_eq!(to_i16(1.5), i16::MAX);
        assert_eq!(to_i16(-1.0), i16::MIN);
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(0.5).to_le_bytes(), [0x00, 0x40]);
    }

    #[test]
    fn the_pipeline_sends_tenth_second_chunks_at_the_harness_rate() {
        // A 48 kHz stereo microphone, a 16 kHz mono harness.
        let mut p = Pipeline::new(48_000, 2, 16_000, 1);
        let (chunks, loud) = p.push(&[0.5; 9600 + 200]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 3200);
        assert!(loud > 0.5);
        assert_eq!(p.flush().map(|rest| rest.len()), Some(68));
        assert_eq!(p.flush(), None);
        assert_eq!(level(&[]), 0.0);
    }
}
