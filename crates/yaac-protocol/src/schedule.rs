//! Outbound scheduling and flow control (docs/03-protocol.md#scheduling-and-flow-control).
//!
//! Sans-I/O. The connection hands the [`Scheduler`] things to send, and asks it for the next
//! frame to write whenever the transport can take one:
//!
//! ```text
//! push_message / push_frame / send_data / grant_credit      (any time)
//! loop { let frame = next_frame()?; write it all; on_written(frame.len(), elapsed) }
//! ```
//!
//! Three rules, in both directions:
//!
//! * **Two classes.** Control frames (requests, responses, events) always go first. Bulk (data
//!   frames, history pages) is handed out one chunk at a time, only when no control frame is
//!   waiting. So a control message enqueued at any moment waits behind **at most the one bulk
//!   frame already being written** — provided the writer asks for a frame only when it is
//!   ready to write it, and does not buffer frames ahead.
//! * **Small, adaptive chunks.** Data chunks start at [`CHUNK_MIN`] (16 KiB) and grow, at most
//!   doubling per step, up to [`CHUNK_MAX`] (64 KiB) while measured throughput says a chunk
//!   still leaves the wire within [`TARGET_CHUNK_TIME`]. They shrink again when it does not.
//! * **Credit.** Each outbound data stream may send only as many bytes as its receiver has
//!   granted. A stream without credit is skipped; nothing it holds blocks anything else.
//!
//! Bulk *messages* (history pages, `upload/read` results) cannot be split, so they are bounded
//! at the source: pages are sized by bytes (`maxBytes`). A bulk message counts as one bulk
//! chunk.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use crate::frame::{self, FrameError};
use crate::rpc::Message;
use crate::types::StreamId;

/// Smallest (and initial) data chunk.
pub const CHUNK_MIN: usize = 16 * 1024;
/// Largest data chunk.
pub const CHUNK_MAX: usize = 64 * 1024;
/// The wire time one bulk chunk should take at most. It bounds how long a control message can
/// wait behind the chunk in flight.
pub const TARGET_CHUNK_TIME: Duration = Duration::from_millis(50);

/// The scheduling class of an outbound frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    /// Requests, responses, events. Always first.
    Control,
    /// Data frames and large pages. Handed out one chunk at a time.
    Bulk,
}

/// A flow-control error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FlowError {
    /// The stream is not open.
    #[error("data stream {0} is not open")]
    UnknownStream(StreamId),
    /// The stream is already open.
    #[error("data stream {0} is already open")]
    StreamExists(StreamId),
    /// The peer sent more than the credit granted.
    #[error("data stream {stream}: received {received} bytes with only {credit} bytes of credit")]
    CreditExceeded {
        /// The stream.
        stream: StreamId,
        /// Bytes received in the offending frame.
        received: u64,
        /// Credit that was outstanding.
        credit: u64,
    },
    /// The frame could not be encoded.
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[derive(Debug, Default)]
struct OutStream {
    pending: VecDeque<Vec<u8>>,
    /// Offset into `pending.front()`.
    offset: usize,
    pending_bytes: u64,
    credit: u64,
    /// No more data will be added; drop the stream once drained.
    closing: bool,
}

impl OutStream {
    fn ready(&self) -> bool {
        self.pending_bytes > 0 && self.credit > 0
    }

    /// Takes up to `max` bytes, limited by credit.
    fn take(&mut self, max: usize) -> Vec<u8> {
        let allowed = usize::try_from(self.credit).unwrap_or(usize::MAX).min(max);
        let mut out = Vec::with_capacity(allowed.min(usize::try_from(self.pending_bytes).unwrap_or(usize::MAX)));
        while out.len() < allowed {
            let Some(front) = self.pending.front() else { break };
            let rest = &front[self.offset..];
            let n = rest.len().min(allowed - out.len());
            out.extend_from_slice(&rest[..n]);
            self.offset += n;
            if self.offset == front.len() {
                self.pending.pop_front();
                self.offset = 0;
            }
        }
        let n = out.len() as u64;
        self.credit -= n;
        self.pending_bytes -= n;
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BulkTurn {
    Message,
    Data,
}

/// The outbound two-class scheduler of one connection.
#[derive(Debug)]
pub struct Scheduler {
    control: VecDeque<Vec<u8>>,
    bulk_messages: VecDeque<Vec<u8>>,
    streams: BTreeMap<StreamId, OutStream>,
    /// The stream served last, for round-robin.
    last_stream: Option<StreamId>,
    turn: BulkTurn,
    chunk: usize,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    /// An empty scheduler with the initial chunk size.
    pub fn new() -> Self {
        Self {
            control: VecDeque::new(),
            bulk_messages: VecDeque::new(),
            streams: BTreeMap::new(),
            last_stream: None,
            turn: BulkTurn::Message,
            chunk: CHUNK_MIN,
        }
    }

    /// Queues an encoded frame.
    pub fn push_frame(&mut self, class: Class, frame: Vec<u8>) {
        match class {
            Class::Control => self.control.push_back(frame),
            Class::Bulk => self.bulk_messages.push_back(frame),
        }
    }

    /// Encodes and queues a message. Fails when it does not fit in one frame (page it).
    pub fn push_message(&mut self, class: Class, message: &Message) -> Result<(), FrameError> {
        let mut out = Vec::new();
        frame::encode_message_frame(&message.to_json(), &mut out)?;
        self.push_frame(class, out);
        Ok(())
    }

    /// Opens an outbound data stream with the receiver's initial credit.
    pub fn open_stream(&mut self, stream: StreamId, initial_credit: u64) -> Result<(), FlowError> {
        if self.streams.contains_key(&stream) {
            return Err(FlowError::StreamExists(stream));
        }
        self.streams.insert(stream, OutStream { credit: initial_credit, ..OutStream::default() });
        Ok(())
    }

    /// Queues bytes on an open stream. They are sent in chunks, under credit, behind control.
    pub fn send_data(&mut self, stream: StreamId, bytes: Vec<u8>) -> Result<(), FlowError> {
        let out = self.streams.get_mut(&stream).ok_or(FlowError::UnknownStream(stream))?;
        out.pending_bytes += bytes.len() as u64;
        if !bytes.is_empty() {
            out.pending.push_back(bytes);
        }
        Ok(())
    }

    /// The receiver granted more credit (`stream/credit`). Unknown streams are ignored: the
    /// grant may cross a cancellation.
    pub fn grant_credit(&mut self, stream: StreamId, bytes: u64) {
        if let Some(out) = self.streams.get_mut(&stream) {
            out.credit = out.credit.saturating_add(bytes);
        }
    }

    /// No more data will be queued; the stream is forgotten once its bytes are sent.
    pub fn close_stream(&mut self, stream: StreamId) {
        if let Some(out) = self.streams.get_mut(&stream) {
            out.closing = true;
            if out.pending_bytes == 0 {
                self.streams.remove(&stream);
            }
        }
    }

    /// Drops a stream and everything still queued on it (`upload/cancel`).
    pub fn cancel_stream(&mut self, stream: StreamId) {
        self.streams.remove(&stream);
    }

    /// Credit left on a stream.
    pub fn credit(&self, stream: StreamId) -> Option<u64> {
        self.streams.get(&stream).map(|s| s.credit)
    }

    /// Bytes queued on a stream and not yet handed out.
    pub fn pending_data(&self, stream: StreamId) -> Option<u64> {
        self.streams.get(&stream).map(|s| s.pending_bytes)
    }

    /// The current data chunk size.
    pub fn chunk_size(&self) -> usize {
        self.chunk
    }

    /// Control frames waiting.
    pub fn pending_control(&self) -> usize {
        self.control.len()
    }

    /// Whether there is nothing that could be sent now (credit-starved data does not count).
    pub fn is_idle(&self) -> bool {
        self.control.is_empty() && self.bulk_messages.is_empty() && !self.streams.values().any(OutStream::ready)
    }

    /// The next frame to write, or `None` when nothing can be sent now. Call this only when the
    /// transport is ready to write the frame; never buffer frames ahead.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        if let Some(frame) = self.control.pop_front() {
            return Some(frame);
        }
        // Alternate between bulk messages and data so neither starves the other.
        let order = match self.turn {
            BulkTurn::Message => [BulkTurn::Message, BulkTurn::Data],
            BulkTurn::Data => [BulkTurn::Data, BulkTurn::Message],
        };
        for turn in order {
            let frame = match turn {
                BulkTurn::Message => self.bulk_messages.pop_front(),
                BulkTurn::Data => self.next_data_frame(),
            };
            if frame.is_some() {
                self.turn = match turn {
                    BulkTurn::Message => BulkTurn::Data,
                    BulkTurn::Data => BulkTurn::Message,
                };
                return frame;
            }
        }
        None
    }

    fn next_data_frame(&mut self) -> Option<Vec<u8>> {
        // Round-robin: the first ready stream after the one served last.
        let after = self.last_stream;
        let pick = self
            .streams
            .iter()
            .filter(|(id, s)| s.ready() && after.is_none_or(|last| **id > last))
            .map(|(id, _)| *id)
            .next()
            .or_else(|| self.streams.iter().find(|(_, s)| s.ready()).map(|(id, _)| *id))?;
        let chunk = self.chunk;
        let out = self.streams.get_mut(&pick)?;
        let bytes = out.take(chunk);
        let drained = out.closing && out.pending_bytes == 0;
        if drained {
            self.streams.remove(&pick);
        }
        self.last_stream = Some(pick);
        let mut frame = Vec::with_capacity(frame::LENGTH_PREFIX + 1 + frame::STREAM_PREFIX + bytes.len());
        // A chunk is at most CHUNK_MAX, far below the frame limit.
        frame::encode_data_frame(pick, &bytes, &mut frame).ok()?;
        Some(frame)
    }

    /// Reports that `bytes` were written in `elapsed`. Adapts the data chunk size: it grows (at
    /// most doubling) while a chunk would still leave the wire within [`TARGET_CHUNK_TIME`],
    /// and shrinks when it would not. Writes too small to measure are ignored.
    pub fn on_written(&mut self, bytes: usize, elapsed: Duration) {
        if bytes < CHUNK_MIN / 2 {
            return;
        }
        let secs = elapsed.as_secs_f64().max(1e-6);
        let throughput = bytes as f64 / secs; // bytes per second
        let ideal = throughput * TARGET_CHUNK_TIME.as_secs_f64();
        let next = if ideal >= (self.chunk * 2) as f64 {
            self.chunk * 2
        } else if ideal < self.chunk as f64 {
            self.chunk / 2
        } else {
            self.chunk
        };
        self.chunk = next.clamp(CHUNK_MIN, CHUNK_MAX);
    }
}

/// The receiving side of one data stream: tracks credit granted and decides when to grant
/// more. Credit is returned only once received bytes are **consumed** (written to disk), so a
/// slow consumer slows the sender instead of filling memory.
#[derive(Clone, Debug)]
pub struct ReceiveWindow {
    stream: StreamId,
    window: u64,
    /// Credit the sender still holds.
    outstanding: u64,
    /// Consumed bytes not yet granted back.
    consumed: u64,
}

impl ReceiveWindow {
    /// A window of `window` bytes. The full window is the initial credit to announce.
    pub fn new(stream: StreamId, window: u64) -> Self {
        Self { stream, window, outstanding: window, consumed: 0 }
    }

    /// The initial credit to announce (e.g. in `upload/begin`'s result).
    pub fn initial_credit(&self) -> u64 {
        self.window
    }

    /// Records a received data frame. Fails if the sender exceeded its credit.
    pub fn on_received(&mut self, bytes: usize) -> Result<(), FlowError> {
        let bytes = bytes as u64;
        if bytes > self.outstanding {
            return Err(FlowError::CreditExceeded { stream: self.stream, received: bytes, credit: self.outstanding });
        }
        self.outstanding -= bytes;
        Ok(())
    }

    /// Records consumed bytes. Returns credit to grant (`stream/credit`) once at least half
    /// the window has been consumed.
    pub fn on_consumed(&mut self, bytes: usize) -> Option<u64> {
        self.consumed += bytes as u64;
        if self.consumed * 2 >= self.window && self.consumed > 0 {
            let grant = self.consumed;
            self.consumed = 0;
            self.outstanding += grant;
            Some(grant)
        } else {
            None
        }
    }
}
