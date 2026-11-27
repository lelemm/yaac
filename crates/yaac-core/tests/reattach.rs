mod common;

use common::{NOW, settle};
use serde_json::json;
use yaac_core::client::Client;
use yaac_core::demo::{DEMO_CLIENT, DemoHub, Scenario, threads};
use yaac_core::sending::SendState;
use yaac_protocol::ClientId;
use yaac_protocol::methods::names;

#[test]
fn reattach_uses_the_index_and_transcript_positions_without_retrying_a_send() {
    let hub = DemoHub::new(Scenario::Fleet, NOW);
    let mut client = Client::new(Box::new(hub.clone()), "test", ClientId::new(DEMO_CLIENT));
    settle(&mut client, &hub);
    let key = threads::billing();
    client.open(&key);
    settle(&mut client, &hub);
    let token = client.state.target(&key.target).unwrap().sync_token.clone();
    let position = client.state.transcripts[&key].position.unwrap();
    assert!(!token.as_str().is_empty());

    hub.lose_next_answer();
    client.state.drafts.entry(key.clone()).or_default().text = "An in-flight message".into();
    assert!(client.send(&key));
    client.state.drafts.entry(key.clone()).or_default().text = "Still typing".into();
    let before = hub.requests().len();
    client.rebind(Box::new(hub.clone()));
    assert!(!client.state.hub.attached);
    assert_eq!(client.state.transcripts[&key].position, Some(position));
    assert!(!client.send(&key), "the composer remains a draft until the hello completes");
    settle(&mut client, &hub);

    assert!(client.state.hub.attached);
    assert_eq!(client.state.selected_thread(), Some(&key));
    assert_eq!(client.state.drafts[&key].text, "Still typing");
    let transcript = &client.state.transcripts[&key];
    let after = transcript.position.unwrap();
    assert_eq!(after.epoch, position.epoch);
    assert!(after.seq >= position.seq);
    assert!(transcript.items().iter().any(|item| {
        transcript.pending_of(&item.id).is_some_and(|pending| pending.state == SendState::Unknown)
    }));
    let requests = hub.requests();
    let requests = &requests[before..];
    assert!(requests.iter().any(|(target, method, params)|
        target.as_ref() == Some(&key.target) && method == names::THREAD_SYNC && params["token"] == json!(token)
    ));
    assert!(requests.iter().any(|(target, method, params)|
        target.as_ref() == Some(&key.target) && method == names::THREAD_SUBSCRIBE
            && params["threadId"] == json!(key.thread)
            && params["epoch"] == json!(position.epoch) && params["afterSeq"] == json!(position.seq)
    ));
    assert!(requests.iter().all(|(_, method, _)| !matches!(method.as_str(), names::TURN_START | names::THREAD_READ)));
}

#[test]
fn a_three_entry_transcript_cache_unsubscribes_when_its_oldest_entry_is_evicted() {
    let hub = DemoHub::new(Scenario::Fleet, NOW);
    let mut client = Client::new(Box::new(hub.clone()), "test", ClientId::new(DEMO_CLIENT));
    client.state.transcript_capacity = 3;
    settle(&mut client, &hub);
    let mut keys = [
        threads::billing(), threads::parser(), threads::search(), threads::notes(), threads::fix(),
    ].into_iter();
    let oldest = keys.next().unwrap();
    client.open(&oldest);
    for key in keys {
        client.open(&key);
        settle(&mut client, &hub);
    }
    assert!(!client.state.transcripts.contains_key(&oldest));
    assert_eq!(client.state.transcripts.len(), 4, "three cached transcripts plus the open one");
    assert!(hub.requests().iter().any(|(target, method, params)|
        target.as_ref() == Some(&oldest.target) && method == names::THREAD_UNSUBSCRIBE && params["threadId"] == json!(oldest.thread)
    ));
}
