//! Optional fragmentation of oversized JSON messages over reserved data stream zero.

use crate::frame::{self, Frame, FrameDecoder, FrameError};
use crate::types::StreamId;

const STREAM: StreamId = StreamId(0);
const HEADER: usize = 9;
const CHUNK: usize = 16 * 1024;
const MAX_MESSAGE: usize = 32 * 1024 * 1024;
const LANES: usize = 2;

/// Encodes one logical message, keeping its fragments in the same FIFO lane.
pub fn encode(payload: &[u8], lane: Option<u8>) -> Result<Vec<Vec<u8>>, FrameError> {
    if payload.len() <= frame::MAX_MESSAGE_PAYLOAD || lane.is_none() {
        let mut bytes = Vec::new();
        frame::encode_message_frame(payload, &mut bytes)?;
        return Ok(vec![bytes]);
    }
    if payload.len() > MAX_MESSAGE {
        return Err(FrameError::Oversize {
            len: payload.len(),
            max: MAX_MESSAGE,
        });
    }
    let lane = lane
        .filter(|lane| usize::from(*lane) < LANES)
        .ok_or(FrameError::InvalidMessageFragment)?;
    let total = (payload.len() as u32).to_le_bytes();
    let mut frames = Vec::new();
    for (index, part) in payload.chunks(CHUNK).enumerate() {
        let mut bytes = Vec::with_capacity(HEADER + part.len());
        bytes.push(lane);
        bytes.extend_from_slice(&total);
        bytes.extend_from_slice(&((index * CHUNK) as u32).to_le_bytes());
        bytes.extend_from_slice(part);
        let mut frame = Vec::new();
        frame::encode_data_frame(STREAM, &bytes, &mut frame)?;
        frames.push(frame);
    }
    Ok(frames)
}

#[derive(Debug)]
struct Partial {
    total: usize,
    payload: Vec<u8>,
}

/// Physical frame decoder plus optional message reassembly. A reassembly error is fatal.
#[derive(Debug)]
pub struct Decoder {
    physical: FrameDecoder,
    enabled: bool,
    partial: [Option<Partial>; LANES],
}

impl Decoder {
    /// The attach link is same-build. The daemon link enables chunks at `initialize`.
    pub fn new(enabled: bool) -> Self {
        Self {
            physical: FrameDecoder::new(),
            enabled,
            partial: [None, None],
        }
    }

    /// Enables the negotiated extension.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Queues transport bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.physical.push(bytes);
    }

    /// The next complete logical message or ordinary data frame.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        while let Some(frame) = self.physical.next_frame()? {
            let Frame::Data {
                stream: STREAM,
                bytes,
            } = frame
            else {
                return Ok(Some(frame));
            };
            if !self.enabled {
                return Ok(Some(Frame::Data {
                    stream: STREAM,
                    bytes,
                }));
            }
            if let Some(payload) = self.accept(&bytes)? {
                return Ok(Some(Frame::Message(payload)));
            }
        }
        Ok(None)
    }

    fn accept(&mut self, bytes: &[u8]) -> Result<Option<Vec<u8>>, FrameError> {
        let Some((header, payload)) = bytes.split_at_checked(HEADER) else {
            return Err(FrameError::InvalidMessageFragment);
        };
        let Some(slot) = self.partial.get_mut(usize::from(header[0])) else {
            return Err(FrameError::InvalidMessageFragment);
        };
        let total = u32::from_le_bytes(header[1..5].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
        if total <= frame::MAX_MESSAGE_PAYLOAD
            || total > MAX_MESSAGE
            || offset >= total
            || payload.len() != CHUNK.min(total - offset)
            || (offset == 0) != slot.is_none()
        {
            return Err(FrameError::InvalidMessageFragment);
        }
        let partial = slot.get_or_insert_with(|| Partial {
            total,
            payload: Vec::new(),
        });
        if partial.total != total || partial.payload.len() != offset {
            return Err(FrameError::InvalidMessageFragment);
        }
        partial.payload.extend_from_slice(payload);
        Ok((partial.payload.len() == total).then(|| slot.take().unwrap().payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_lanes_and_data_can_interleave() {
        let bulk = vec![b'b'; frame::MAX_FRAME_LEN + 1];
        let control = vec![b'c'; frame::MAX_FRAME_LEN + 2];
        let mut bulk_frames = encode(&bulk, Some(1)).unwrap().into_iter();
        let control_frames = encode(&control, Some(0)).unwrap();
        let mut decoder = Decoder::new(true);
        decoder.push(&bulk_frames.next().unwrap());
        assert!(decoder.next_frame().unwrap().is_none());
        let mut data = Vec::new();
        frame::encode_data_frame(StreamId(9), b"terminal", &mut data).unwrap();
        decoder.push(&data);
        assert_eq!(
            decoder.next_frame().unwrap(),
            Some(Frame::Data {
                stream: StreamId(9),
                bytes: b"terminal".to_vec()
            })
        );
        for frame in control_frames {
            decoder.push(&frame);
        }
        assert_eq!(decoder.next_frame().unwrap(), Some(Frame::Message(control)));
        for frame in bulk_frames {
            decoder.push(&frame);
        }
        assert_eq!(decoder.next_frame().unwrap(), Some(Frame::Message(bulk)));
    }

    #[test]
    fn a_fragment_out_of_order_is_rejected() {
        let frames = encode(&vec![b'a'; frame::MAX_FRAME_LEN], Some(0)).unwrap();
        let mut decoder = Decoder::new(true);
        decoder.push(&frames[1]);
        assert_eq!(
            decoder.next_frame().unwrap_err(),
            FrameError::InvalidMessageFragment
        );
    }
}
