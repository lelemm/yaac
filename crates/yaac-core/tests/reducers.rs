//! Reducers: replay, deltas, compacted snapshots, resets, and streaming through the client.
mod common;

use common::{boot, settle};
use yaac_core::demo::threads;
use yaac_core::transcript::Transcript;
use yaac_protocol::{
    AgentMessageItem, DeltaField, Epoch, Item, ItemBody, ItemDeltaEvent, ItemEvent, ItemId, ItemStatus, ItemUpdatedEvent,
    JournalPosition, Seq, ThreadEvent, ThreadEventParams, ThreadId, TurnId,
};

fn ev(seq: u64, event: ThreadEvent) -> ThreadEventParams {
    ThreadEventParams { thread_id: ThreadId::new("t"), epoch: Epoch(3), seq: Seq(seq), event }
}

fn agent(id: &str, text: &str, status: ItemStatus) -> Item {
    Item { id: ItemId::new(id), turn_id: TurnId::new("turn"), status, body: ItemBody::AgentMessage(AgentMessageItem { text: text.into() }), started_at: None, completed_at: None }
}

fn text(t: &Transcript, id: &str) -> String {
    match &t.item(&ItemId::new(id)).unwrap().body {
        ItemBody::AgentMessage(a) => a.text.clone(),
        _ => unreachable!(),
    }
}

fn delta(seq: u64, s: &str) -> ThreadEventParams {
    ev(seq, ThreadEvent::ItemDelta(ItemDeltaEvent { item_id: ItemId::new("a"), field: DeltaField::Text, delta: s.into() }))
}

#[test]
fn replay_overlap_is_applied_once() {
    let mut t = Transcript::new();
    assert!(t.apply(&ev(1, ThreadEvent::ItemStarted(ItemEvent { item: agent("a", "", ItemStatus::InProgress) })), 1));
    assert!(t.apply(&delta(2, "Hello"), 2));
    assert!(t.apply(&delta(3, ", world"), 3));
    // A replay after reconnect starts before the position: the overlap is ignored.
    assert!(!t.apply(&delta(2, "Hello"), 4));
    assert!(!t.apply(&delta(3, ", world"), 5));
    assert!(t.apply(&delta(4, "!"), 6));
    assert_eq!(text(&t, "a"), "Hello, world!");
    assert_eq!(t.position, Some(JournalPosition { epoch: Epoch(3), seq: Seq(4) }));
    // Another epoch is not applied (the client re-subscribes for a reset).
    let mut other = delta(9, "x");
    other.epoch = Epoch(4);
    assert!(!t.apply(&other, 7));
}

#[test]
fn item_updated_with_through_seq_replaces_a_range() {
    let mut t = Transcript::new();
    t.apply(&ev(1, ThreadEvent::ItemStarted(ItemEvent { item: agent("a", "", ItemStatus::InProgress) })), 1);
    // seq 2 ..= 6 collapsed into one snapshot.
    let snap = ev(2, ThreadEvent::ItemUpdated(ItemUpdatedEvent { item: agent("a", "all of it", ItemStatus::InProgress), through_seq: Some(Seq(6)) }));
    assert!(t.apply(&snap, 2));
    assert_eq!(t.position.unwrap().seq, Seq(6));
    assert_eq!(text(&t, "a"), "all of it");
    // The original events of that range, if they still arrive, are not applied again.
    assert!(!t.apply(&delta(5, " again"), 3));
    assert!(t.apply(&delta(7, "."), 4));
    assert_eq!(text(&t, "a"), "all of it.");
    // A snapshot that starts inside what we have but reaches beyond is applied.
    let snap = ev(6, ThreadEvent::ItemUpdated(ItemUpdatedEvent { item: agent("a", "final", ItemStatus::Completed), through_seq: Some(Seq(9)) }));
    assert!(t.apply(&snap, 5));
    assert_eq!(t.position.unwrap().seq, Seq(9));
}

#[test]
fn a_reset_holds_live_events_until_the_snapshot_is_complete() {
    let mut t = Transcript::new();
    t.begin_reset();
    assert!(!t.apply(&delta(11, " more"), 1), "held while paging");
    t.loading.as_mut().unwrap().pages.push(agent("a", "snapshot", ItemStatus::InProgress));
    t.finish_reset(JournalPosition { epoch: Epoch(3), seq: Seq(10) }, 2);
    assert_eq!(text(&t, "a"), "snapshot more");
    assert_eq!(t.position.unwrap().seq, Seq(11));
}

#[test]
fn reset_pages_then_replay_after_reconnect_through_the_client() {
    let (mut c, hub) = boot();
    let key = threads::billing();
    let t = c.state.transcripts.get(&key).unwrap();
    assert!(t.is_loaded(), "warm thread loaded by reset + paged thread/read");
    let n = t.items().len();
    assert_eq!(n, 6, "every page arrived, oldest first");
    assert!(matches!(t.items()[0].body, ItemBody::MeshReceived(_)));
    // Reconnect: subscribe from the position — replay, no duplicates.
    c.check_latest(&key);
    settle(&mut c, &hub);
    assert_eq!(c.state.transcripts.get(&key).unwrap().items().len(), n);
}

#[test]
fn a_streamed_reply_grows_by_deltas_and_completes() {
    let (mut c, hub) = boot();
    let key = threads::billing();
    c.open(&key);
    settle(&mut c, &hub);
    c.state.drafts.entry(key.clone()).or_default().text = "Please add a test for the refund race.".into();
    assert!(c.send(&key));
    assert!(!c.state.drafts.contains_key(&key), "the draft is cleared once sent");
    let mut lengths = Vec::new();
    for _ in 0..400 {
        c.pump();
        let t = c.state.transcripts.get(&key).unwrap();
        if let Some(ItemBody::AgentMessage(a)) = t.items().last().map(|i| &i.body) {
            lengths.push(a.text.len());
        }
        if !hub.tick() {
            break;
        }
    }
    c.pump();
    assert!(lengths.windows(2).all(|w| w[0] <= w[1]));
    assert!(lengths.iter().filter(|&&l| l > 0).count() > 10, "it streamed");
    let t = c.state.transcripts.get(&key).unwrap();
    let last = t.items().last().unwrap();
    assert_eq!(last.status, ItemStatus::Completed);
    assert!(t.active_turn.is_none());
}
