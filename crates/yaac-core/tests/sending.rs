//! D165: a sent message shows at once as an echo, is replaced in place by its own item, and a
//! failed send offers Edit and Retry — Retry only on the user's click, with a new op id; an
//! outcome YAAC cannot know is never sent again.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{NOW, settle};
use yaac_core::client::Client;
use yaac_core::demo::{DEMO_CLIENT, DemoHub, Scenario, threads};
use yaac_core::link::HubLink;
use yaac_core::sending::{SendState, is_echo};
use yaac_core::transcript::Transcript;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message};
use yaac_protocol::{ClientId, ItemBody, ItemId, OpId};

/// A link that answers the next `turn/start`s itself, with these errors, and counts them.
struct Refusing {
    inner: DemoHub,
    errors: Rc<RefCell<Vec<ErrorCode>>>,
    starts: Rc<RefCell<Vec<OpId>>>,
    injected: Rc<RefCell<Vec<Message>>>,
}

impl HubLink for Refusing {
    fn send(&mut self, message: Message) {
        if let Message::Request { id, params, .. } = &message
            && params.get("method").and_then(|m| m.as_str()) == Some("turn/start")
        {
            let op = params.pointer("/params/opId").and_then(|o| o.as_str()).unwrap_or_default();
            self.starts.borrow_mut().push(OpId::new(op));
            if let Some(code) = self.errors.borrow_mut().pop() {
                self.injected.borrow_mut().push(Message::Error { id: Some(id.clone()), error: ErrorObject::new(code, "the harness did not start the turn") });
                return;
            }
        }
        self.inner.send(message);
    }
    fn poll(&mut self) -> Option<Message> {
        let next = self.injected.borrow_mut().pop();
        next.or_else(|| self.inner.poll())
    }
}

struct Rig {
    c: Client,
    hub: DemoHub,
    errors: Rc<RefCell<Vec<ErrorCode>>>,
    starts: Rc<RefCell<Vec<OpId>>>,
}

fn rig() -> Rig {
    let hub = DemoHub::new(Scenario::Fleet, NOW);
    let (errors, starts) = (Rc::new(RefCell::new(Vec::new())), Rc::new(RefCell::new(Vec::new())));
    let link = Refusing { inner: hub.clone(), errors: errors.clone(), starts: starts.clone(), injected: Rc::default() };
    let mut c = Client::new(Box::new(link), "test", ClientId::new(DEMO_CLIENT));
    c.set_warm(hub.all_threads());
    DemoHub::apply_seams(&mut c.state);
    settle(&mut c, &hub);
    c.open(&threads::billing());
    settle(&mut c, &hub);
    Rig { c, hub, errors, starts }
}

fn type_and_send(c: &mut Client, text: &str) {
    let key = threads::billing();
    c.state.drafts.entry(key.clone()).or_default().text = text.into();
    assert!(c.send(&key));
}

fn transcript(c: &Client) -> &Transcript {
    c.state.transcripts.get(&threads::billing()).unwrap()
}

/// The user messages showing this text: (item id, is an echo).
fn bubbles(c: &Client, text: &str) -> Vec<(usize, ItemId)> {
    transcript(c)
        .items()
        .iter()
        .enumerate()
        .filter(|(_, i)| matches!(&i.body, ItemBody::UserMessage(u) if u.parts.iter().any(|p| matches!(p, yaac_protocol::InputPart::Text(t) if t.text == text))))
        .map(|(n, i)| (n, i.id.clone()))
        .collect()
}

#[test]
fn a_sent_message_shows_at_once_and_its_own_item_takes_its_place() {
    let Rig { mut c, hub, .. } = rig();
    let before = transcript(&c).items().len();
    type_and_send(&mut c, "Add a test for the refund race.");
    // The same frame: no pump, no hub answer yet.
    assert!(!c.state.drafts.contains_key(&threads::billing()), "the composer clears");
    let shown = bubbles(&c, "Add a test for the refund race.");
    assert_eq!(shown.len(), 1);
    let (at, echo) = shown[0].clone();
    assert_eq!(at, before, "at the end of the transcript");
    assert!(is_echo(&echo));
    let p = transcript(&c).pending_of(&echo).expect("pending");
    assert_eq!((p.state.clone(), p.note().as_deref()), (SendState::Sending, Some("Sending…")));
    // The target's answer and the message's own item arrive a tick later.
    c.pump();
    assert!(is_echo(&bubbles(&c, "Add a test for the refund race.")[0].1), "nothing has come back yet");
    assert!(hub.tick());
    c.pump();
    let shown = bubbles(&c, "Add a test for the refund race.");
    assert_eq!(shown.len(), 1, "no duplicate");
    assert_eq!(shown[0].0, at, "in place: the same slot");
    assert!(!is_echo(&shown[0].1), "its own item now");
    assert!(transcript(&c).pending().is_empty());
    let ItemBody::UserMessage(u) = &transcript(&c).items()[at].body else { panic!() };
    assert!(u.op_id.is_some(), "the item carries the send's op id");
    settle(&mut c, &hub);
    assert_eq!(bubbles(&c, "Add a test for the refund race.").len(), 1);
}

#[test]
fn a_refused_send_offers_edit_and_retry_and_retry_uses_a_new_op_id() {
    let Rig { mut c, hub, errors, starts } = rig();
    errors.borrow_mut().push(ErrorCode::ProviderUnavailable);
    type_and_send(&mut c, "Try the refund race again.");
    settle(&mut c, &hub);
    let (_, echo) = bubbles(&c, "Try the refund race again.")[0].clone();
    let p = transcript(&c).pending_of(&echo).unwrap().clone();
    assert!(matches!(p.state, SendState::Failed(_)), "{:?}", p.state);
    assert!(p.note().unwrap().starts_with("Not sent:"));
    assert!(p.can_retry() && p.settled());
    assert!(c.state.drafts.get(&threads::billing()).is_none_or(|d| d.is_empty()), "the text stays on the bubble, not back in the composer");
    // Nothing is re-sent on its own.
    settle(&mut c, &hub);
    assert_eq!(starts.borrow().len(), 1);
    // Retry: the same message under a new op id, in the same bubble.
    assert!(c.retry_send(&threads::billing(), &echo));
    assert_eq!(starts.borrow().len(), 2);
    assert_ne!(starts.borrow()[0], starts.borrow()[1], "a new op id");
    assert_eq!(transcript(&c).pending_of(&echo).unwrap().state, SendState::Sending);
    settle(&mut c, &hub);
    let shown = bubbles(&c, "Try the refund race again.");
    assert_eq!(shown.len(), 1);
    assert!(!is_echo(&shown[0].1), "the retry arrived and replaced the echo");
}

#[test]
fn edit_puts_a_failed_message_back_in_the_composer() {
    let Rig { mut c, hub, errors, starts } = rig();
    errors.borrow_mut().push(ErrorCode::ProviderUnavailable);
    type_and_send(&mut c, "Fix the typo.");
    settle(&mut c, &hub);
    let (_, echo) = bubbles(&c, "Fix the typo.")[0].clone();
    assert!(c.edit_send(&threads::billing(), &echo));
    assert!(bubbles(&c, "Fix the typo.").is_empty(), "the bubble goes");
    assert_eq!(c.state.drafts.get(&threads::billing()).map(|d| d.text.as_str()), Some("Fix the typo."));
    assert_eq!(starts.borrow().len(), 1, "Edit sends nothing");
}

#[test]
fn an_unknown_outcome_says_so_and_is_never_sent_again() {
    let Rig { mut c, hub, errors, starts } = rig();
    errors.borrow_mut().push(ErrorCode::OutcomeUnknown);
    type_and_send(&mut c, "Did this arrive?");
    settle(&mut c, &hub);
    let (_, echo) = bubbles(&c, "Did this arrive?")[0].clone();
    let p = transcript(&c).pending_of(&echo).unwrap().clone();
    assert_eq!(p.state, SendState::Unknown);
    assert!(p.note().unwrap().contains("may or may not have arrived"));
    assert!(!p.can_retry(), "no Retry for an outcome YAAC cannot know");
    assert!(!c.retry_send(&threads::billing(), &echo));
    settle(&mut c, &hub);
    assert_eq!(starts.borrow().len(), 1, "never re-sent");
    // Edit and Dismiss are offered; Dismiss only removes the bubble.
    assert!(c.dismiss_send(&threads::billing(), &echo));
    assert!(bubbles(&c, "Did this arrive?").is_empty());
    assert_eq!(starts.borrow().len(), 1);
}

#[test]
fn an_echo_survives_a_snapshot_and_a_queued_message_leaves_the_transcript() {
    let Rig { mut c, hub, .. } = rig();
    type_and_send(&mut c, "Still here?");
    let (_, echo) = bubbles(&c, "Still here?")[0].clone();
    // A snapshot that does not hold the message yet keeps the echo at the end.
    let mut t = transcript(&c).clone();
    let items: Vec<_> = t.items().iter().filter(|i| !is_echo(&i.id)).cloned().collect();
    t.replace_all(items, 99);
    assert_eq!(t.items().last().map(|i| &i.id), Some(&echo));
    // Queued instead of started: the queue strip shows it, the transcript does not.
    let op = t.pending_of(&echo).unwrap().op.clone();
    t.remove_pending(&op, 100);
    assert!(t.items().iter().all(|i| !is_echo(&i.id)));
    assert_eq!(t.index_of(&echo), None);
    settle(&mut c, &hub);
}
