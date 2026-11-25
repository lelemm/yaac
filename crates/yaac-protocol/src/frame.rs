//! The frame format (docs/03-protocol.md#framing). Part of the frozen core.
//!
//! ```text
//! ┌────────────────┬──────────┬────────────────────────────┐
//! │ length: u32 LE │ kind: u8 │ payload (length − 1 bytes) │
//! └────────────────┴──────────┴────────────────────────────┘
//! ```
//!
//! `length` counts the kind byte and the payload, and may not exceed [`MAX_FRAME_LEN`]
//! (1 MiB). Kind `0x01` is one UTF-8 JSON message; kind `0x02` is `stream: u32 LE` followed by
//! raw bytes. Sans-I/O: [`encode_frame`] appends to a buffer, and [`FrameDecoder`] accepts
//! bytes in arbitrary chunks.

use crate::types::StreamId;

/// The hard limit on `length` (kind byte + payload): 1 MiB.
pub const MAX_FRAME_LEN: usize = 1 << 20;
/// Size of the length prefix.
pub const LENGTH_PREFIX: usize = 4;
/// Size of the stream number at the start of a data payload.
pub const STREAM_PREFIX: usize = 4;
/// The largest payload of a message frame.
pub const MAX_MESSAGE_PAYLOAD: usize = MAX_FRAME_LEN - 1;
/// The largest number of bytes one data frame can carry.
pub const MAX_DATA_PAYLOAD: usize = MAX_FRAME_LEN - 1 - STREAM_PREFIX;

/// Frame kind byte of a JSON message.
pub const KIND_MESSAGE: u8 = 0x01;
/// Frame kind byte of a data frame.
pub const KIND_DATA: u8 = 0x02;

/// One decoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// `0x01`: one JSON message (not yet parsed).
    Message(Vec<u8>),
    /// `0x02`: bytes on a data stream.
    Data {
        /// The stream.
        stream: StreamId,
        /// The bytes.
        bytes: Vec<u8>,
    },
}

impl Frame {
    /// Size of the frame on the wire, prefix included.
    pub fn wire_len(&self) -> usize {
        LENGTH_PREFIX
            + 1
            + match self {
                Frame::Message(payload) => payload.len(),
                Frame::Data { bytes, .. } => STREAM_PREFIX + bytes.len(),
            }
    }

    /// Encodes to a new buffer.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let mut out = Vec::with_capacity(self.wire_len());
        encode_frame(self, &mut out)?;
        Ok(out)
    }
}

/// A framing error. Every one of these is fatal for the stream: framing cannot resynchronise,
/// so the connection must be closed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// `length` is above [`MAX_FRAME_LEN`].
    #[error("frame of {len} bytes exceeds the {max}-byte limit")]
    Oversize {
        /// The length declared or attempted.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// `length` is zero: there is no kind byte.
    #[error("empty frame (length 0)")]
    Empty,
    /// The kind byte is not one this build knows.
    #[error("unknown frame kind 0x{0:02x}")]
    UnknownKind(u8),
    /// A data frame too short to hold its stream number.
    #[error("data frame of {0} payload bytes is too short for a stream number")]
    ShortData(usize),
    /// A negotiated message fragment could not be reassembled.
    #[error("invalid message fragment")]
    InvalidMessageFragment,
}

/// Appends the encoding of `frame` to `out`. Fails (and appends nothing) when it would exceed
/// [`MAX_FRAME_LEN`].
pub fn encode_frame(frame: &Frame, out: &mut Vec<u8>) -> Result<(), FrameError> {
    match frame {
        Frame::Message(payload) => encode_message_frame(payload, out),
        Frame::Data { stream, bytes } => encode_data_frame(*stream, bytes, out),
    }
}

/// Appends a `0x01` frame carrying `payload` (one JSON message).
pub fn encode_message_frame(payload: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
    let len = 1 + payload.len();
    let prefix = length_prefix(len)?;
    out.reserve(LENGTH_PREFIX + len);
    out.extend_from_slice(&prefix);
    out.push(KIND_MESSAGE);
    out.extend_from_slice(payload);
    Ok(())
}

/// Appends a `0x02` frame carrying `bytes` on `stream`.
pub fn encode_data_frame(stream: StreamId, bytes: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
    let len = 1 + STREAM_PREFIX + bytes.len();
    let prefix = length_prefix(len)?;
    out.reserve(LENGTH_PREFIX + len);
    out.extend_from_slice(&prefix);
    out.push(KIND_DATA);
    out.extend_from_slice(&stream.0.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn length_prefix(len: usize) -> Result<[u8; 4], FrameError> {
    if len > MAX_FRAME_LEN {
        return Err(FrameError::Oversize { len, max: MAX_FRAME_LEN });
    }
    // `len <= MAX_FRAME_LEN < u32::MAX`, so this conversion is exact.
    let len = u32::try_from(len).map_err(|_| FrameError::Oversize { len, max: MAX_FRAME_LEN })?;
    Ok(len.to_le_bytes())
}

/// An incremental frame decoder. Feed it bytes as they arrive — split anywhere, several frames
/// per chunk — and take frames out with [`FrameDecoder::next_frame`].
///
/// After an error the decoder is *poisoned*: the same error is returned forever, because the
/// stream cannot be resynchronised.
#[derive(Debug)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    start: usize,
    max_len: usize,
    error: Option<FrameError>,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    /// A decoder with the protocol's limit.
    pub fn new() -> Self {
        Self::with_limit(MAX_FRAME_LEN)
    }

    /// A decoder with a lower limit (never higher than the protocol's).
    pub fn with_limit(max_len: usize) -> Self {
        Self { buf: Vec::new(), start: 0, max_len: max_len.min(MAX_FRAME_LEN), error: None }
    }

    /// Appends received bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.error.is_some() {
            return;
        }
        // Reclaim consumed space before growing, so the buffer stays bounded by one frame plus
        // one chunk.
        if self.start > 0 && (self.start == self.buf.len() || self.start >= self.buf.len() / 2) {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// Bytes received but not yet returned as frames.
    pub fn buffered(&self) -> usize {
        self.buf.len() - self.start
    }

    /// The next complete frame, `Ok(None)` when more bytes are needed, or a fatal error.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let available = &self.buf[self.start..];
        let Some(prefix) = available.get(..LENGTH_PREFIX) else {
            return Ok(None);
        };
        let len = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
        if len == 0 {
            return Err(self.fail(FrameError::Empty));
        }
        if len > self.max_len {
            return Err(self.fail(FrameError::Oversize { len, max: self.max_len }));
        }
        // The kind is checked as soon as it arrives, before waiting for the whole payload.
        let Some(&kind) = available.get(LENGTH_PREFIX) else {
            return Ok(None);
        };
        if kind != KIND_MESSAGE && kind != KIND_DATA {
            return Err(self.fail(FrameError::UnknownKind(kind)));
        }
        let Some(body) = available.get(LENGTH_PREFIX + 1..LENGTH_PREFIX + len) else {
            return Ok(None);
        };
        let frame = if kind == KIND_MESSAGE {
            Frame::Message(body.to_vec())
        } else {
            let Some((stream, bytes)) = body.split_first_chunk::<STREAM_PREFIX>() else {
                let short = body.len();
                return Err(self.fail(FrameError::ShortData(short)));
            };
            Frame::Data { stream: StreamId(u32::from_le_bytes(*stream)), bytes: bytes.to_vec() }
        };
        self.start += LENGTH_PREFIX + len;
        Ok(Some(frame))
    }

    /// Pushes `bytes` and drains every complete frame.
    pub fn decode(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, FrameError> {
        self.push(bytes);
        let mut frames = Vec::new();
        while let Some(frame) = self.next_frame()? {
            frames.push(frame);
        }
        Ok(frames)
    }

    fn fail(&mut self, error: FrameError) -> FrameError {
        self.error = Some(error.clone());
        self.buf = Vec::new();
        self.start = 0;
        error
    }
}
