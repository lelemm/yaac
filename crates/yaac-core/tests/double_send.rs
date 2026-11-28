//! One Send, one operation (docs/07-ui.md#composer). Starting a thread takes seconds; a
//! second click on Send while the first `thread/start` (or `turn/start`) is on its way must not
//! send it again — the owner got two threads with the same message. A lost answer is said and
//! never sent again on its own; a Send again by the user carries the same operation id, so
//! `yaacd` answers with the first thread instead of starting another (invariant 10).

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{NOW, settle};
use serde_json::json;
use yaac_core::client::{Client, DraftStart};
use yaac_core::demo::{DEMO_CLIENT, DemoHub, Scenario, threads};
use yaac_core::link::HubLink;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message};
use yaac_protocol::ClientId;

/// A link that records the op id of every `thread/start` and `turn/start`, holds them until
/// [`Rig::release`] (the demo hub answers them then), or answers the next ones itself with an
/// error.
#[derive(Default)]
struct Wire {
    starts: Vec<(String, String)>,
    held: Vec<Message>,
    hold: bool,
    errors: Vec<ErrorObject>,
    injected: Vec<Message>,
}

struct Recording {
    inner: DemoHub,
    wire: Rc<RefCell<Wire>>,
}

impl HubLink for Recording {
    fn send(&mut self, message: Message) {
        let mut wire = self.wire.borrow_mut();
        if let Message::Request { id, params, .. } = &message
            && let Some(method @ ("thread/start" | "turn/start")) = params.get("method").and_then(|m| m.as_str())
        {
            let op = params.pointer("/params/opId").and_then(|o| o.as_str()).unwrap_or_default();
            wire.starts.push((method.to_owned(), op.to_owned()));
            if !wire.errors.is_empty() {
                let error = wire.errors.remove(0);
                wire.injected.push(Message::Error { id: Some(id.clone()), error });
                return;
            }
            if wire.hold {
                wire.held.push(message);
                return;
            }
        }
        drop(wire);
        self.inner.send(message);
    }
    fn poll(&mut self) -> Option<Message> {
        let next = self.wire.borrow_mut().injected.pop();
        next.or_else(|| self.inner.poll())
    }
}

struct Rig {
    c: Client,
    hub: DemoHub,
    wire: Rc<RefCell<Wire>>,
}

impl Rig {
    fn new() -> Self {
        let hub = DemoHub::new(Scenario::Fleet, NOW);
        let wire = Rc::new(RefCell::new(Wire::default()));
        let link = Recording { inner: hub.clone(), wire: wire.clone() };
        let mut c = Client::new(Box::new(link), "test", ClientId::new(DEMO_CLIENT));
        c.set_warm(hub.all_threads());
        DemoHub::apply_seams(&mut c.state);
        settle(&mut c, &hub);
        Rig { c, hub, wire }
    }

    /// A draft on the demo's local target with this first message, selected.
    fn draft(&mut self, text: &str) -> u64 {
        let id = self.c.new_thread("local".into(), Some("/home/dev/api".into()));
        let d = self.c.state.new_threads.iter_mut().find(|d| d.id == id).unwrap();
        d.draft.text = text.into();
        id
    }

    fn starts(&self, method: &str) -> Vec<String> {
        self.wire.borrow().starts.iter().filter(|(m, _)| m == method).map(|(_, op)| op.clone()).collect()
    }

    /// Let the held requests through to the demo hub, and let everything settle.
    fn release(&mut self) {
        let held = {
            let mut wire = self.wire.borrow_mut();
            wire.hold = false;
            std::mem::take(&mut wire.held)
        };
        for m in held {
            self.hub.clone().send(m);
        }
        settle(&mut self.c, &self.hub);
    }

    fn threads_titled(&self, title: &str) -> usize {
        self.c.state.threads.values().filter(|r| r.summary.title == title).count()
    }
}

#[test]
fn a_second_send_while_the_thread_starts_sends_nothing() {
    let mut r = Rig::new();
    r.wire.borrow_mut().hold = true;
    let id = r.draft("Fix the flaky login test.");
    assert_eq!(r.c.draft_start(id), None);
    assert!(r.c.start_thread(id));
    // At once, before any answer: the composer is "Starting…" and the text is still there.
    assert_eq!(r.c.draft_start(id), Some(DraftStart::Starting));
    assert!(r.c.state.new_threads.iter().any(|d| d.id == id && d.draft.text == "Fix the flaky login test."));
    // The impatient second (and third) click.
    assert!(!r.c.start_thread(id));
    r.c.pump();
    assert!(!r.c.start_thread(id));
    assert_eq!(r.starts("thread/start").len(), 1, "exactly one thread/start");
    assert_eq!(r.c.draft_start(id), Some(DraftStart::Starting), "still starting");

    r.release();
    assert_eq!(r.threads_titled("Fix the flaky login test."), 1, "one thread");
    assert!(!r.c.state.new_threads.iter().any(|d| d.id == id), "the draft became the thread");
    assert_eq!(r.c.draft_start(id), None);
    assert_eq!(r.starts("thread/start").len(), 1);
}

#[test]
fn a_refused_start_gives_the_draft_back_and_a_send_again_is_a_new_operation() {
    let mut r = Rig::new();
    r.wire.borrow_mut().errors.push(ErrorObject::new(ErrorCode::InternalError, "the harness would not start"));
    let id = r.draft("Write the release notes.");
    assert!(r.c.start_thread(id));
    assert_eq!(r.c.draft_start(id), Some(DraftStart::Starting));
    settle(&mut r.c, &r.hub);
    // Nothing started: editable again, with the reason.
    assert_eq!(r.c.draft_start(id), None);
    assert!(r.c.state.new_threads.iter().any(|d| d.id == id && d.draft.text == "Write the release notes."));
    assert!(r.c.errors.iter().any(|e| e.text.contains("The thread was not started")));
    assert_eq!(r.starts("thread/start").len(), 1, "not sent again on its own");
    // The user sends again: a new operation, which starts it.
    assert!(r.c.start_thread(id));
    settle(&mut r.c, &r.hub);
    let ops = r.starts("thread/start");
    assert_eq!(ops.len(), 2);
    assert_ne!(ops[0], ops[1], "a refused operation is not reused");
    assert_eq!(r.threads_titled("Write the release notes."), 1);
}

#[test]
fn a_lost_answer_is_said_never_resent_and_a_send_again_reuses_the_operation_id() {
    let mut r = Rig::new();
    r.wire.borrow_mut().errors.push(ErrorObject::new(ErrorCode::OutcomeUnknown, "connection lost"));
    let id = r.draft("Bump the API version.");
    assert!(r.c.start_thread(id));
    settle(&mut r.c, &r.hub);
    assert_eq!(r.c.draft_start(id), Some(DraftStart::Unknown), "the composer says it");
    assert!(r.c.errors.iter().any(|e| e.text.contains("Could not confirm whether the thread started")));
    assert!(r.c.state.new_threads.iter().any(|d| d.id == id), "the text stays");
    settle(&mut r.c, &r.hub);
    assert_eq!(r.starts("thread/start").len(), 1, "never sent again on its own");
    // The user's Send again carries the same operation id: yaacd answers with the thread the
    // first one started, if it did, rather than starting a second.
    assert!(r.c.start_thread(id));
    assert_eq!(r.c.draft_start(id), Some(DraftStart::Starting));
    settle(&mut r.c, &r.hub);
    let ops = r.starts("thread/start");
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0], ops[1], "the same operation id");
    assert_eq!(r.c.draft_start(id), None);
    assert_eq!(r.threads_titled("Bump the API version."), 1);
}

#[test]
fn yaacd_not_knowing_frees_the_operation_id() {
    // yaacd stopped mid-start: it would answer `outcome_unknown` to that id for ever, so the
    // draft is not tied to it (the message says to check the thread list).
    let mut r = Rig::new();
    let verdict = ErrorObject { code: ErrorCode::OutcomeUnknown, message: "thread/start may or may not have taken effect".into(), data: Some(json!({ "opId": "x" })) };
    r.wire.borrow_mut().errors.push(verdict);
    let id = r.draft("Rotate the keys.");
    assert!(r.c.start_thread(id));
    settle(&mut r.c, &r.hub);
    assert_eq!(r.c.draft_start(id), None);
    assert!(r.c.errors.iter().any(|e| e.text.contains("Check the thread list before sending again")));
    assert!(r.c.start_thread(id));
    settle(&mut r.c, &r.hub);
    let ops = r.starts("thread/start");
    assert_ne!(ops[0], ops[1]);
}

#[test]
fn a_second_send_in_a_thread_waits_for_the_first_answer() {
    let mut r = Rig::new();
    let key = threads::billing();
    r.c.open(&key);
    settle(&mut r.c, &r.hub);
    r.wire.borrow_mut().hold = true;
    r.c.state.drafts.entry(key.clone()).or_default().text = "First.".into();
    assert!(r.c.send(&key));
    assert!(r.c.sending(&key));
    // Typed and sent again before the answer: nothing goes, and the text stays.
    r.c.state.drafts.entry(key.clone()).or_default().text = "Second.".into();
    assert!(!r.c.send(&key));
    assert!(!r.c.send_now(&key));
    assert_eq!(r.starts("turn/start").len(), 1, "exactly one turn/start");
    assert_eq!(r.c.state.drafts.get(&key).map(|d| d.text.as_str()), Some("Second."));
    r.release();
    assert!(!r.c.sending(&key));
    assert!(r.c.send(&key), "sendable once the first is answered");
    assert_eq!(r.starts("turn/start").len(), 2);
}
