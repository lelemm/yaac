//! Request answers through the client: submitted, confirmed, outcome unknown — never re-sent.
mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{NOW, settle};
use yaac_core::client::Client;
use yaac_core::demo::{DEMO_CLIENT, DemoHub, Scenario, threads};
use yaac_core::link::HubLink;
use yaac_core::requests::{CardState, card_state};
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message};
use yaac_protocol::{ApprovalAnswer, ClientId, ItemBody, RequestId, RequestResponse, RequestState};

fn request_state(c: &Client, key: &yaac_core::ThreadKey, id: &str) -> (RequestState, CardState) {
    let t = c.state.transcripts.get(key).unwrap();
    for i in t.items() {
        if let ItemBody::Request(r) = &i.body
            && r.request_id.as_str() == id
        {
            let local = t.local_answers.get(&r.request_id).map(|l| l.summary.as_str());
            return (r.state.clone(), card_state(r, c.state.may_act(key), c.state.client_id.as_ref(), local, "devbox", NOW));
        }
    }
    panic!("no request {id}")
}

#[test]
fn submitted_then_confirmed_by_you() {
    let (mut c, hub) = common::boot();
    let fix = threads::fix();
    c.open(&fix);
    settle(&mut c, &hub);
    let answer = RequestResponse::Approval(ApprovalAnswer { decision: "allow".into(), reason: None });
    c.respond(&fix, &RequestId::new("r-fix"), &answer, "Allow once".into());
    assert_eq!(request_state(&c, &fix, "r-fix").1, CardState::Submitted { summary: "Allow once".into() });
    settle(&mut c, &hub);
    let (state, card) = request_state(&c, &fix, "r-fix");
    assert_eq!(state, RequestState::Resolved);
    assert!(matches!(card, CardState::Confirmed { ref summary, .. } if summary == "Allow once"), "{card:?}");
}

/// A link that loses every `request/respond`: the hub cannot say whether it arrived.
struct Lossy {
    inner: DemoHub,
    injected: Rc<RefCell<Vec<Message>>>,
    responds: Rc<RefCell<u32>>,
}

impl HubLink for Lossy {
    fn send(&mut self, message: Message) {
        if let Message::Request { id, params, .. } = &message
            && params.get("method").and_then(|m| m.as_str()) == Some("request/respond")
        {
            *self.responds.borrow_mut() += 1;
            self.injected.borrow_mut().push(Message::Error {
                id: Some(id.clone()),
                error: ErrorObject::new(ErrorCode::OutcomeUnknown, "lost contact"),
            });
            return;
        }
        self.inner.send(message);
    }
    fn poll(&mut self) -> Option<Message> {
        let next = self.injected.borrow_mut().pop();
        next.or_else(|| self.inner.poll())
    }
}

#[test]
fn a_lost_answer_is_outcome_unknown_and_is_never_resent() {
    let hub = DemoHub::new(Scenario::Fleet, NOW);
    let responds = Rc::new(RefCell::new(0));
    let link = Lossy { inner: hub.clone(), injected: Rc::default(), responds: responds.clone() };
    let mut c = Client::new(Box::new(link), "test", ClientId::new(DEMO_CLIENT));
    c.set_warm(hub.all_threads());
    settle(&mut c, &hub);
    let fix = threads::fix();
    c.open(&fix);
    let answer = RequestResponse::Approval(ApprovalAnswer { decision: "allow".into(), reason: None });
    c.respond(&fix, &RequestId::new("r-fix"), &answer, "Allow once".into());
    settle(&mut c, &hub);
    assert_eq!(request_state(&c, &fix, "r-fix"), (RequestState::Unknown, CardState::OutcomeUnknown));
    // "Check latest status" re-subscribes; it sends no answer.
    c.check_latest(&fix);
    settle(&mut c, &hub);
    assert_eq!(*responds.borrow(), 1);
}

#[test]
fn resolved_elsewhere_and_held_elsewhere() {
    let (mut c, hub) = common::boot();
    hub.play(yaac_core::demo::Scene::RequestStates);
    settle(&mut c, &hub);
    let fix = threads::fix();
    match request_state(&c, &fix, "r-fix").1 {
        CardState::ResolvedElsewhere { text } => assert_eq!(text, "Allowed in another program on devbox · 2m ago"),
        other => panic!("{other:?}"),
    }
    assert_eq!(request_state(&c, &fix, "r-q4").1, CardState::Expired);
    assert_eq!(request_state(&c, &fix, "r-q5").1, CardState::OutcomeUnknown);
    assert!(matches!(request_state(&c, &fix, "r-q2").1, CardState::Submitted { ref summary } if summary.contains("••••••••") && summary.starts_with("Patch the decoder")));
    let hs = threads::handshake();
    assert_eq!(request_state(&c, &hs, "r-hs").1, CardState::Pending { actionable: false });
    // Answering a held thread is refused locally: nothing is sent.
    let answer = RequestResponse::Approval(ApprovalAnswer { decision: "allow".into(), reason: None });
    c.respond(&hs, &RequestId::new("r-hs"), &answer, "Allow once".into());
    assert_eq!(request_state(&c, &hs, "r-hs").0, RequestState::Pending);
}
