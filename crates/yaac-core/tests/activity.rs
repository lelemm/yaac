//! The transcript view model: prose at full size, everything else as muted activity lines;
//! runs; folding; expansion; the task of a spawned thread; cards only for what needs the user.
mod common;

use common::{boot, settle};
use yaac_core::activity::{Block, Expansion, FOLD_AFTER, LineMark, blocks};
use yaac_core::attention::AttentionMap;
use yaac_core::demo::{Scene, threads};
use yaac_core::ThreadKey;

fn shape(c: &yaac_core::client::Client, key: &ThreadKey, e: &Expansion) -> Vec<String> {
    let map = AttentionMap::compute(&c.state);
    blocks(&c.state, &map, key, e)
        .into_iter()
        .map(|b| match b {
            Block::User { .. } => "user".to_owned(),
            Block::Task { .. } => "task".to_owned(),
            Block::Prose { .. } => "prose".to_owned(),
            Block::Card { .. } => "card".to_owned(),
            Block::Handoff { .. } => "handoff".to_owned(),
            Block::Run { lines } => format!(
                "run[{}]",
                lines
                    .iter()
                    .map(|l| format!("{}{}{}", if l.failed { "!" } else { "" }, if l.expanded { "+" } else { "" }, l.text))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            Block::Folded { text, failed, .. } => format!("folded[{text}{}]", if failed { " !" } else { "" }),
        })
        .collect()
}

#[test]
fn the_orchestrator_reads_as_a_conversation_with_one_quiet_run() {
    let (c, _) = boot();
    let s = shape(&c, &threads::orchestrator(), &Expansion::default());
    assert_eq!(
        s,
        [
            "user",
            "prose",
            "run[Started 3 workers · 2 of 3 finished | billing-service · Migration merged. 14 tests pass. One flaky test skipped: tes… | CI integration · delivered | Release notes · queued, staging is offline | search-service · sending… | Waiting for search-service…]",
            "prose",
        ]
    );
}

#[test]
fn a_spawned_thread_shows_its_task_in_full_and_cards_are_never_collapsed() {
    let (c, _) = boot();
    assert_eq!(
        shape(&c, &threads::billing(), &Expansion::default()),
        ["task", "run[Thought for 9s | Edited 6 files | Ran cargo test -p billing — 14 passed]", "prose", "run[Migrate auth module — orchestrator · delivered]"]
    );
    assert_eq!(shape(&c, &threads::index(), &Expansion::default())[2], "card");
}

#[test]
fn clicking_expands_in_place_and_a_failure_is_never_muted() {
    let (mut c, hub) = boot();
    hub.play(Scene::StagingReconnects);
    settle(&mut c, &hub);
    let orch = threads::orchestrator();
    let mut e = Expansion::default();
    let s = shape(&c, &orch, &e);
    let last_run = s.iter().rev().find(|b| b.starts_with("run[")).unwrap();
    assert!(last_run.contains("!Old changelog bot · failed — that thread was deleted on staging"), "{last_run}");
    e.toggle_line(&orch, &"o9".into());
    assert!(shape(&c, &orch, &e).iter().any(|b| b.contains("!+Old changelog bot")));
    e.toggle_line(&orch, &"o9".into());
    assert!(!shape(&c, &orch, &e).iter().any(|b| b.contains("+Old changelog bot")));
    // The delivered receipt is updated in place.
    assert!(s.iter().any(|b| b.contains("Release notes · delivered, after 3h 12m queued")));
}

#[test]
fn a_long_run_folds_into_one_line_that_opens() {
    let (mut c, hub) = boot();
    let key = threads::billing();
    // Make a long run: duplicate the tool line many times.
    let mut t = c.state.transcripts.get(&key).unwrap().clone();
    let tool = t.items()[2].clone();
    let mut items: Vec<_> = t.items()[..3].to_vec();
    for i in 0..10 {
        let mut x = tool.clone();
        x.id = format!("extra-{i}").into();
        if i == 4
            && let yaac_protocol::ItemBody::ToolCall(tc) = &mut x.body
        {
            tc.exit_code = Some(1);
            tc.title = "Ran `cargo clippy`".into();
        }
        items.push(x);
    }
    items.extend(t.items()[3..].iter().cloned());
    t.replace_all(items, 99);
    c.state.transcripts.insert(key.clone(), t);
    settle(&mut c, &hub);
    let e = Expansion::default();
    let s = shape(&c, &key, &e);
    const { assert!(13 > FOLD_AFTER) };
    assert_eq!(s[1], "folded[13 steps · 12 commands !]", "{s:#?}");
    let map = AttentionMap::compute(&c.state);
    let first = match &blocks(&c.state, &map, &key, &e)[1] {
        Block::Folded { first, .. } => first.clone(),
        _ => unreachable!(),
    };
    let mut e = e;
    e.toggle_run(&key, &first);
    let opened = blocks(&c.state, &map, &key, &e);
    match &opened[1] {
        Block::Run { lines } => {
            assert_eq!(lines.len(), 13);
            assert!(lines.iter().any(|l| l.failed && l.mark == LineMark::Tool));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_item_navigated_to_is_expanded_and_highlighted() {
    let (mut c, hub) = boot();
    hub.play(Scene::StagingReconnects);
    settle(&mut c, &hub);
    let notes = threads::notes();
    c.state.focus_item = Some((notes.clone(), "n1".into()));
    let map = AttentionMap::compute(&c.state);
    let b = blocks(&c.state, &map, &notes, &Expansion::default());
    let line = b
        .iter()
        .find_map(|b| match b {
            Block::Run { lines } => lines.iter().find(|l| l.id.as_str() == "n1").cloned(),
            _ => None,
        })
        .unwrap();
    assert!(line.highlighted && line.expanded);
    assert_eq!(line.mark, LineMark::Received);
    assert!(matches!(b[0], Block::User { .. }), "a thread a person started opens with the user");
}
