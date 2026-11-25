//! The two-class scheduler: a control message is never delayed by more than one bulk chunk;
//! credit is respected; chunks adapt; streams share fairly.

// Test code: small index casts and by-value helpers keep the tests readable.
#![allow(clippy::cast_possible_truncation, clippy::needless_pass_by_value, clippy::type_complexity)]

use std::collections::HashMap;
use std::time::Duration;

use proptest::prelude::*;
use serde_json::{Value, json};
use yaac_protocol::StreamId;
use yaac_protocol::frame::{Frame, FrameDecoder};
use yaac_protocol::rpc::{Message, RpcId};
use yaac_protocol::schedule::{CHUNK_MAX, CHUNK_MIN, Class, FlowError, ReceiveWindow, Scheduler};

fn decode_one(bytes: &[u8]) -> Frame {
    let mut decoder = FrameDecoder::new();
    let mut frames = decoder.decode(bytes).unwrap();
    assert_eq!(frames.len(), 1, "next_frame returns exactly one frame");
    frames.remove(0)
}

fn is_control(frame: &Frame) -> Option<i64> {
    let Frame::Message(payload) = frame else { return None };
    let message: Value = serde_json::from_slice(payload).unwrap();
    (message["method"] == "ctl").then(|| message["id"].as_i64().unwrap())
}

#[derive(Clone, Debug)]
enum Op {
    Control,
    BulkPage(usize),
    Data(u8, usize),
    Credit(u8, u32),
    Write,
    Feedback(u16),
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => Just(Op::Control),
        1 => (1usize..60_000).prop_map(Op::BulkPage),
        2 => (0u8..3, 1usize..300_000).prop_map(|(s, n)| Op::Data(s, n)),
        2 => (0u8..3, 1u32..500_000).prop_map(|(s, n)| Op::Credit(s, n)),
        6 => Just(Op::Write),
        1 => (1u16..1000).prop_map(Op::Feedback),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Whatever is queued, a control message enqueued at any moment is handed to the writer
    /// before any bulk frame that was not already being written. The writer asks for a frame
    /// only when it can write it, so the most a control message waits behind is the one bulk
    /// chunk in flight — and that chunk is at most CHUNK_MAX bytes of data, or one page.
    #[test]
    fn control_is_delayed_by_at_most_one_bulk_chunk(ops in proptest::collection::vec(arb_op(), 1..400)) {
        let mut scheduler = Scheduler::new();
        // Start saturated: a big upload with plenty of credit, and large history pages.
        for s in 0..3u32 {
            scheduler.open_stream(StreamId(s), 4 << 20).unwrap();
            scheduler.send_data(StreamId(s), vec![s as u8; 1 << 20]).unwrap();
        }
        for _ in 0..4 {
            let page = Message::Response { id: RpcId::Number(0), result: json!({ "items": "x".repeat(60_000) }) };
            scheduler.push_message(Class::Bulk, &page).unwrap();
        }
        let mut next_id = 0i64;
        // Control ids waiting, with the number of bulk frames handed out since each was queued.
        let mut waiting: HashMap<i64, usize> = HashMap::new();
        let mut largest_page = 60_100;
        let drain = |scheduler: &mut Scheduler, waiting: &mut HashMap<i64, usize>| -> Result<bool, TestCaseError> {
            let Some(bytes) = scheduler.next_frame() else { return Ok(false) };
            let frame = decode_one(&bytes);
            if let Some(id) = is_control(&frame) {
                let behind = waiting.remove(&id).expect("control frame was queued");
                prop_assert_eq!(behind, 0, "a bulk frame overtook a waiting control message");
            } else {
                for count in waiting.values_mut() { *count += 1; }
                if let Frame::Data { bytes, .. } = &frame {
                    prop_assert!(bytes.len() <= CHUNK_MAX, "data chunk of {} bytes", bytes.len());
                }
            }
            Ok(true)
        };
        for op in ops {
            match op {
                Op::Control => {
                    next_id += 1;
                    waiting.insert(next_id, 0);
                    scheduler.push_message(Class::Control, &Message::request(RpcId::Number(next_id), "ctl", json!({}))).unwrap();
                }
                Op::BulkPage(size) => {
                    largest_page = largest_page.max(size + 100);
                    let page = Message::Response { id: RpcId::Number(0), result: json!({ "items": "y".repeat(size) }) };
                    scheduler.push_message(Class::Bulk, &page).unwrap();
                }
                Op::Data(s, n) => scheduler.send_data(StreamId(u32::from(s)), vec![s; n]).unwrap(),
                Op::Credit(s, n) => scheduler.grant_credit(StreamId(u32::from(s)), u64::from(n)),
                Op::Write => { drain(&mut scheduler, &mut waiting)?; }
                Op::Feedback(ms) => scheduler.on_written(CHUNK_MIN, Duration::from_millis(u64::from(ms))),
            }
        }
        while drain(&mut scheduler, &mut waiting)? {}
        prop_assert!(waiting.is_empty(), "every control message was sent");
    }

    /// A stream never sends more than its credit, and its bytes arrive in order.
    #[test]
    fn credit_is_never_exceeded_and_order_is_kept(grants in proptest::collection::vec(1u32..100_000, 1..30),
                                                  sizes in proptest::collection::vec(1usize..50_000, 1..30)) {
        let mut scheduler = Scheduler::new();
        let stream = StreamId(9);
        scheduler.open_stream(stream, 0).unwrap();
        let mut queued = Vec::new();
        for (i, size) in sizes.iter().enumerate() {
            let chunk: Vec<u8> = (0..*size).map(|j| ((i * 31 + j) % 251) as u8).collect();
            queued.extend_from_slice(&chunk);
            scheduler.send_data(stream, chunk).unwrap();
        }
        let mut granted: u64 = 0;
        let mut received = Vec::new();
        for grant in grants {
            granted += u64::from(grant);
            scheduler.grant_credit(stream, u64::from(grant));
            while let Some(bytes) = scheduler.next_frame() {
                let Frame::Data { stream: s, bytes } = decode_one(&bytes) else { panic!("only data queued") };
                prop_assert_eq!(s, stream);
                received.extend_from_slice(&bytes);
                prop_assert!(received.len() as u64 <= granted, "sent {} with {} credit", received.len(), granted);
            }
        }
        let expected = (granted as usize).min(queued.len());
        prop_assert_eq!(&received[..], &queued[..expected]);
    }
}

#[test]
fn control_jumps_a_saturated_queue_immediately() {
    let mut scheduler = Scheduler::new();
    scheduler.open_stream(StreamId(1), u64::MAX).unwrap();
    scheduler.send_data(StreamId(1), vec![0; 20 << 20]).unwrap();
    // The writer is busy with one chunk…
    let in_flight = decode_one(&scheduler.next_frame().unwrap());
    assert!(matches!(in_flight, Frame::Data { ref bytes, .. } if bytes.len() == CHUNK_MIN));
    // …a Stop is queued…
    scheduler.push_message(Class::Control, &Message::request(RpcId::Number(1), "ctl", json!({}))).unwrap();
    // …and it is the very next frame.
    assert_eq!(is_control(&decode_one(&scheduler.next_frame().unwrap())), Some(1));
}

#[test]
fn chunks_start_small_grow_to_the_maximum_and_shrink_again() {
    let mut scheduler = Scheduler::new();
    assert_eq!(scheduler.chunk_size(), CHUNK_MIN);
    // A fast link: 16 KiB in 1 ms.
    for _ in 0..10 {
        scheduler.on_written(scheduler.chunk_size(), Duration::from_millis(1));
    }
    assert_eq!(scheduler.chunk_size(), CHUNK_MAX, "grows to the cap and no further");
    scheduler.open_stream(StreamId(1), u64::MAX).unwrap();
    scheduler.send_data(StreamId(1), vec![0; 1 << 20]).unwrap();
    let Frame::Data { bytes, .. } = decode_one(&scheduler.next_frame().unwrap()) else { panic!() };
    assert_eq!(bytes.len(), CHUNK_MAX);
    // A slow link: 64 KiB in 2 s (≈ 256 kbit/s).
    for _ in 0..10 {
        scheduler.on_written(scheduler.chunk_size(), Duration::from_secs(2));
    }
    assert_eq!(scheduler.chunk_size(), CHUNK_MIN, "shrinks to the floor and no further");
    // Growth is gradual: at most doubling per report.
    let mut scheduler = Scheduler::new();
    scheduler.on_written(CHUNK_MIN, Duration::from_micros(1));
    assert_eq!(scheduler.chunk_size(), CHUNK_MIN * 2);
    // Tiny writes are not measured.
    scheduler.on_written(10, Duration::from_secs(10));
    assert_eq!(scheduler.chunk_size(), CHUNK_MIN * 2);
}

#[test]
fn streams_take_turns_and_pages_alternate_with_data() {
    let mut scheduler = Scheduler::new();
    for s in [1, 2] {
        scheduler.open_stream(StreamId(s), u64::MAX).unwrap();
        scheduler.send_data(StreamId(s), vec![0; CHUNK_MIN * 3]).unwrap();
    }
    let order: Vec<u32> = (0..4)
        .map(|_| match decode_one(&scheduler.next_frame().unwrap()) {
            Frame::Data { stream, .. } => stream.0,
            Frame::Message(_) => 0,
        })
        .collect();
    assert_eq!(order, vec![1, 2, 1, 2]);
    scheduler.push_message(Class::Bulk, &Message::notification("page", json!({}))).unwrap();
    scheduler.push_message(Class::Bulk, &Message::notification("page", json!({}))).unwrap();
    let kinds: Vec<&str> = (0..4)
        .map(|_| match decode_one(&scheduler.next_frame().unwrap()) {
            Frame::Data { .. } => "data",
            Frame::Message(_) => "page",
        })
        .collect();
    assert_eq!(kinds, vec!["page", "data", "page", "data"]);
}

#[test]
fn starved_streams_block_nothing_and_close_and_cancel_work() {
    let mut scheduler = Scheduler::new();
    scheduler.open_stream(StreamId(1), 0).unwrap();
    scheduler.send_data(StreamId(1), vec![1; 100]).unwrap();
    assert!(scheduler.is_idle(), "credit-starved data is not sendable");
    assert!(scheduler.next_frame().is_none());
    scheduler.push_message(Class::Bulk, &Message::notification("page", json!({}))).unwrap();
    assert!(matches!(decode_one(&scheduler.next_frame().unwrap()), Frame::Message(_)));
    scheduler.grant_credit(StreamId(1), 60);
    scheduler.close_stream(StreamId(1));
    let Frame::Data { bytes, .. } = decode_one(&scheduler.next_frame().unwrap()) else { panic!() };
    assert_eq!(bytes.len(), 60);
    assert_eq!(scheduler.pending_data(StreamId(1)), Some(40), "a closing stream is kept until drained");
    scheduler.grant_credit(StreamId(1), 100);
    let _ = scheduler.next_frame().unwrap();
    assert_eq!(scheduler.pending_data(StreamId(1)), None, "drained and closed: forgotten");
    // Cancel drops queued bytes.
    scheduler.open_stream(StreamId(2), 1000).unwrap();
    scheduler.send_data(StreamId(2), vec![0; 500]).unwrap();
    scheduler.cancel_stream(StreamId(2));
    assert!(scheduler.next_frame().is_none());
    // Errors.
    assert!(matches!(scheduler.send_data(StreamId(3), vec![1]), Err(FlowError::UnknownStream(_))));
    scheduler.open_stream(StreamId(4), 0).unwrap();
    assert!(matches!(scheduler.open_stream(StreamId(4), 0), Err(FlowError::StreamExists(_))));
    // Credit for an unknown stream is ignored (it may cross a cancellation).
    scheduler.grant_credit(StreamId(99), 10);
}

#[test]
fn receive_window_grants_after_consumption_and_detects_overruns() {
    let mut window = ReceiveWindow::new(StreamId(5), 64 * 1024);
    assert_eq!(window.initial_credit(), 64 * 1024);
    window.on_received(40 * 1024).unwrap();
    // Received but not yet written to disk: no credit back.
    assert_eq!(window.on_consumed(16 * 1024), None);
    // Half the window consumed: grant exactly what was consumed.
    assert_eq!(window.on_consumed(16 * 1024), Some(32 * 1024));
    window.on_received(56 * 1024).unwrap();
    let err = window.on_received(1).unwrap_err();
    assert!(matches!(err, FlowError::CreditExceeded { received: 1, credit: 0, .. }));
}

#[test]
fn interactive_stream_overtakes_bulk_and_alternates_with_control() {
    let mut scheduler = Scheduler::new();
    scheduler.open_stream(StreamId(1), u64::MAX).unwrap();
    scheduler.send_data(StreamId(1), vec![0; CHUNK_MAX]).unwrap();
    scheduler.open_interactive_stream(StreamId(2), 1).unwrap();
    scheduler.send_data(StreamId(2), vec![1, 2]).unwrap();
    scheduler.push_frame(Class::Control, vec![7]);
    scheduler.push_frame(Class::Control, vec![8]);
    scheduler.on_written(CHUNK_MAX, Duration::from_millis(1));
    assert_eq!(scheduler.next_frame().unwrap(), vec![7]);
    assert!(matches!(decode_one(&scheduler.next_frame().unwrap()), Frame::Data { stream: StreamId(2), .. }));
    assert_eq!(scheduler.next_frame().unwrap(), vec![8]);
    assert!(matches!(decode_one(&scheduler.next_frame().unwrap()), Frame::Data { stream: StreamId(1), bytes } if bytes.len() == CHUNK_MIN));
}
