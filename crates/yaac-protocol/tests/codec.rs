//! Frame codec: round trip, split and coalesced reads, oversize frames, garbage input.

// Test code: small index casts and by-value helpers keep the tests readable.
#![allow(clippy::cast_possible_truncation, clippy::needless_pass_by_value, clippy::type_complexity)]

use proptest::prelude::*;
use yaac_protocol::StreamId;
use yaac_protocol::frame::{
    Frame, FrameDecoder, FrameError, KIND_DATA, KIND_MESSAGE, MAX_DATA_PAYLOAD, MAX_FRAME_LEN, MAX_MESSAGE_PAYLOAD,
    encode_data_frame, encode_frame, encode_message_frame,
};

fn arb_frame() -> impl Strategy<Value = Frame> {
    prop_oneof![
        proptest::collection::vec(any::<u8>(), 0..2048).prop_map(Frame::Message),
        (any::<u32>(), proptest::collection::vec(any::<u8>(), 0..4096))
            .prop_map(|(s, bytes)| Frame::Data { stream: StreamId(s), bytes }),
    ]
}

fn encode_all(frames: &[Frame]) -> Vec<u8> {
    let mut out = Vec::new();
    for frame in frames {
        encode_frame(frame, &mut out).unwrap();
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Any sequence of frames, delivered in arbitrary chunks, decodes to the same sequence.
    #[test]
    fn round_trip_with_arbitrary_splits(frames in proptest::collection::vec(arb_frame(), 0..12),
                                        cuts in proptest::collection::vec(any::<usize>(), 0..40)) {
        let bytes = encode_all(&frames);
        let mut points: Vec<usize> = cuts.iter().map(|c| if bytes.is_empty() { 0 } else { c % bytes.len() }).collect();
        points.push(0);
        points.push(bytes.len());
        points.sort_unstable();
        points.dedup();
        let mut decoder = FrameDecoder::new();
        let mut decoded = Vec::new();
        for pair in points.windows(2) {
            decoded.extend(decoder.decode(&bytes[pair[0]..pair[1]]).unwrap());
        }
        prop_assert_eq!(decoded, frames);
        prop_assert_eq!(decoder.buffered(), 0);
    }

    /// One byte at a time.
    #[test]
    fn round_trip_byte_by_byte(frames in proptest::collection::vec(arb_frame(), 0..4)) {
        let bytes = encode_all(&frames);
        let mut decoder = FrameDecoder::new();
        let mut decoded = Vec::new();
        for b in &bytes {
            decoded.extend(decoder.decode(std::slice::from_ref(b)).unwrap());
        }
        prop_assert_eq!(decoded, frames);
    }

    /// Garbage never panics: it decodes to frames or to an error, and after an error the
    /// decoder stays failed.
    #[test]
    fn garbage_never_panics(chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..64), 0..16)) {
        let mut decoder = FrameDecoder::new();
        let mut failed: Option<FrameError> = None;
        for chunk in &chunks {
            match decoder.decode(chunk) {
                Ok(frames) => {
                    prop_assert!(failed.is_none(), "frames after an error");
                    for frame in frames {
                        prop_assert!(frame.wire_len() <= MAX_FRAME_LEN + 4);
                    }
                }
                Err(err) => {
                    if let Some(previous) = &failed { prop_assert_eq!(previous, &err); }
                    failed = Some(err);
                }
            }
        }
    }

    /// The length prefix is little-endian and counts the kind byte.
    #[test]
    fn layout_is_exact(payload in proptest::collection::vec(any::<u8>(), 0..300), stream in any::<u32>()) {
        let mut out = Vec::new();
        encode_message_frame(&payload, &mut out).unwrap();
        prop_assert_eq!(&out[..4], &((payload.len() + 1) as u32).to_le_bytes()[..]);
        prop_assert_eq!(out[4], KIND_MESSAGE);
        prop_assert_eq!(&out[5..], &payload[..]);
        let mut out = Vec::new();
        encode_data_frame(StreamId(stream), &payload, &mut out).unwrap();
        prop_assert_eq!(&out[..4], &((payload.len() + 5) as u32).to_le_bytes()[..]);
        prop_assert_eq!(out[4], KIND_DATA);
        prop_assert_eq!(&out[5..9], &stream.to_le_bytes()[..]);
        prop_assert_eq!(&out[9..], &payload[..]);
    }
}

#[test]
fn coalesced_frames_in_one_chunk() {
    let frames = vec![
        Frame::Message(br#"{"method":"ping","params":{}}"#.to_vec()),
        Frame::Data { stream: StreamId(7), bytes: vec![1, 2, 3] },
        Frame::Message(Vec::new()),
        Frame::Data { stream: StreamId(0), bytes: Vec::new() },
    ];
    let mut decoder = FrameDecoder::new();
    assert_eq!(decoder.decode(&encode_all(&frames)).unwrap(), frames);
}

#[test]
fn largest_frames_are_accepted() {
    let message = Frame::Message(vec![b'x'; MAX_MESSAGE_PAYLOAD]);
    let data = Frame::Data { stream: StreamId(1), bytes: vec![7; MAX_DATA_PAYLOAD] };
    let bytes = encode_all(&[message.clone(), data.clone()]);
    let mut decoder = FrameDecoder::new();
    let mut decoded = Vec::new();
    for chunk in bytes.chunks(65_536) {
        decoded.extend(decoder.decode(chunk).unwrap());
    }
    assert_eq!(decoded, vec![message, data]);
}

#[test]
fn oversize_frames_are_refused_by_the_encoder() {
    let mut out = Vec::new();
    let err = encode_message_frame(&vec![0; MAX_MESSAGE_PAYLOAD + 1], &mut out).unwrap_err();
    assert_eq!(err, FrameError::Oversize { len: MAX_FRAME_LEN + 1, max: MAX_FRAME_LEN });
    assert!(out.is_empty(), "nothing is written on failure");
    let err = encode_data_frame(StreamId(1), &vec![0; MAX_DATA_PAYLOAD + 1], &mut out).unwrap_err();
    assert!(matches!(err, FrameError::Oversize { .. }));
}

#[test]
fn oversize_frames_are_refused_by_the_decoder_before_the_payload_arrives() {
    let mut decoder = FrameDecoder::new();
    let prefix = ((MAX_FRAME_LEN + 1) as u32).to_le_bytes();
    let err = decoder.decode(&prefix).unwrap_err();
    assert!(matches!(err, FrameError::Oversize { .. }));
    // Poisoned: further input is rejected with the same error.
    assert_eq!(decoder.decode(b"anything").unwrap_err(), err);
    assert_eq!(decoder.buffered(), 0, "nothing is kept after a fatal error");
    // u32::MAX too.
    let mut decoder = FrameDecoder::new();
    assert!(matches!(decoder.decode(&u32::MAX.to_le_bytes()).unwrap_err(), FrameError::Oversize { .. }));
    // A lower per-connection limit.
    let mut decoder = FrameDecoder::with_limit(16);
    let mut out = Vec::new();
    encode_message_frame(&[0; 16], &mut out).unwrap();
    assert!(matches!(decoder.decode(&out).unwrap_err(), FrameError::Oversize { len: 17, max: 16 }));
}

#[test]
fn unknown_kind_empty_frame_and_short_data_are_errors() {
    let mut decoder = FrameDecoder::new();
    let mut bytes = 3u32.to_le_bytes().to_vec();
    bytes.push(0x03);
    assert_eq!(decoder.decode(&bytes).unwrap_err(), FrameError::UnknownKind(0x03));

    // The kind is rejected as soon as it arrives, without waiting for the payload.
    let mut decoder = FrameDecoder::new();
    let mut bytes = 1000u32.to_le_bytes().to_vec();
    bytes.push(0xff);
    assert_eq!(decoder.decode(&bytes).unwrap_err(), FrameError::UnknownKind(0xff));

    let mut decoder = FrameDecoder::new();
    assert_eq!(decoder.decode(&0u32.to_le_bytes()).unwrap_err(), FrameError::Empty);

    let mut decoder = FrameDecoder::new();
    let mut bytes = 3u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[KIND_DATA, 1, 2]);
    assert_eq!(decoder.decode(&bytes).unwrap_err(), FrameError::ShortData(2));
}

#[test]
fn a_partial_frame_waits() {
    let frame = Frame::Message(b"{}".to_vec());
    let bytes = frame.encode().unwrap();
    let mut decoder = FrameDecoder::new();
    assert!(decoder.decode(&bytes[..bytes.len() - 1]).unwrap().is_empty());
    assert_eq!(decoder.buffered(), bytes.len() - 1);
    assert_eq!(decoder.decode(&bytes[bytes.len() - 1..]).unwrap(), vec![frame]);
}

#[test]
fn decoder_memory_stays_bounded_over_a_long_stream() {
    let frame = Frame::Data { stream: StreamId(1), bytes: vec![0; 10_000] };
    let bytes = frame.encode().unwrap();
    let mut decoder = FrameDecoder::new();
    for _ in 0..2_000 {
        // Split each frame in two so the buffer always holds a partial frame.
        assert!(decoder.decode(&bytes[..5_000]).unwrap().is_empty());
        assert_eq!(decoder.decode(&bytes[5_000..]).unwrap().len(), 1);
        assert!(decoder.buffered() == 0);
    }
}
