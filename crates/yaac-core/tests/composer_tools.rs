mod common;

use common::{boot, settle};
use yaac_core::attachments::DraftRef;
use yaac_core::demo::threads;
use yaac_protocol::{ItemBody, NoticeKind, TargetPath};

#[test]
fn compact_sends_once_on_the_existing_thread_and_keeps_history_and_other_draft_text() {
    let (mut client, hub) = boot();
    let thread = threads::users();
    client.open(&thread);
    settle(&mut client, &hub);
    let items_before = client.state.transcripts[&thread].items().len();
    client.state.drafts.entry(thread.clone()).or_default().text = " /compact ".into();
    assert!(client.send(&thread), "{:?}", client.errors);
    assert!(client.compacting(&thread));
    assert!(!client.send_now(&thread));

    client.state.drafts.get_mut(&thread).unwrap().text = "Keep this draft".into();
    settle(&mut client, &hub);
    assert!(!client.compacting(&thread));
    assert_eq!(client.state.drafts[&thread].text, "Keep this draft");
    let items = client.state.transcripts[&thread].items();
    assert_eq!(items.len(), items_before + 1);
    assert!(items.iter().any(|i| matches!(&i.body, ItemBody::Notice(n) if n.notice_kind == NoticeKind::Compaction)));
    let requests = hub.requests();
    assert!(!requests.iter().any(|(_, method, _)| method == "turn/start"));
    let mut calls = requests.iter().filter(|(_, method, _)| method == "thread/compact");
    let (target, _, params) = calls.next().unwrap();
    assert_eq!(target.as_ref(), Some(&thread.target));
    assert_eq!(params["threadId"].as_str(), Some(thread.thread.as_str()));
    assert!(params["opId"].is_string());
    assert!(calls.next().is_none());
}

#[test]
fn file_search_uses_the_current_draft_target_and_ignores_stale_replies() {
    let (mut client, hub) = boot();
    let target = threads::fix().target;
    let draft = DraftRef::New(client.new_thread(target.clone(), Some(TargetPath::new("/work/checkout"))));
    client.search_composer_files(&draft, "README");
    client.search_composer_files(&draft, "notes");
    settle(&mut client, &hub);
    let search = client.composer_files(&draft).unwrap();
    assert_eq!(search.params.query, "notes");
    assert_eq!(search.result.as_ref().unwrap().as_ref().unwrap().entries[0].path.as_str(), "/work/checkout/docs/design notes.md");
    let requests = hub.requests();
    assert!(requests.iter().any(|(t, m, p)| t.as_ref() == Some(&target) && m == "fs/search" && p["cwd"] == "/work/checkout"));
    assert!(client.composer_files(&DraftRef::Thread(threads::fix())).is_none());
}

#[test]
fn uncertain_compaction_keeps_the_command_and_does_not_retry() {
    let (mut client, hub) = boot();
    let thread = threads::users();
    client.open(&thread);
    settle(&mut client, &hub);
    client.state.drafts.entry(thread.clone()).or_default().text = "/compact".into();
    hub.lose_next_answer();
    assert!(client.send(&thread));
    settle(&mut client, &hub);
    assert!(!client.compacting(&thread));
    assert_eq!(client.state.drafts[&thread].text, "/compact");
    assert!(client.errors.iter().any(|error| error.text.contains("lost contact")));
    assert_eq!(hub.requests().iter().filter(|(_, m, _)| m == "thread/compact").count(), 1);
}
