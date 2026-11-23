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
                    .map(|l| {
                        let counts = l.diff.map(|d| format!(" +{} −{}", d.added, d.removed)).unwrap_or_default();
                        format!("{}{}{}{counts}", if l.failed { "!" } else { "" }, if l.expanded { "+" } else { "" }, l.text)
                    })
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            Block::Folded { text, failed, .. } => format!("folded[{text}{}]", if failed { " !" } else { "" }),
            Block::TurnChanges { totals, expanded, .. } => {
                format!("changes[{} +{} −{}{}]", totals.text(), totals.added, totals.removed, if expanded { " open" } else { "" })
            }
            Block::Worked { text, .. } => format!("worked[{text}]"),
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
        [
            "task",
            "run[+Thought for 9s | Edited services/billing/src/client.rs +4 −9 | Edited services/billing/Cargo.toml +1 −1 | Deleted services/billing/src/session.rs +0 −6 | Created services/billing/tests/auth.rs +9 −0 | Ran cargo test -p billing · 14 passed]",
            "prose",
            "run[Migrate auth module · orchestrator · delivered]",
            "changes[Changed 4 files +14 −16]",
        ]
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
    assert!(last_run.contains("!Old changelog bot · failed · that thread was deleted on staging"), "{last_run}");
    e.toggle_line(&orch, &"o9".into());
    assert!(shape(&c, &orch, &e).iter().any(|b| b.contains("!+Old changelog bot")));
    e.toggle_line(&orch, &"o9".into());
    assert!(!shape(&c, &orch, &e).iter().any(|b| b.contains("+Old changelog bot")));
    // The delivered receipt is updated in place.
    assert!(s.iter().any(|b| b.contains("Release notes · delivered, after 3h 12m queued")));
}

#[test]
fn a_failed_command_splits_two_independently_folded_runs() {
    let (mut c, hub) = boot();
    let key = threads::billing();
    // Make a long run: duplicate the tool line many times.
    let mut t = c.state.transcripts.get(&key).unwrap().clone();
    let tool = t.items()[3].clone();
    let mut items: Vec<_> = t.items()[..3].to_vec();
    for i in 0..9 {
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
    const { assert!(4 < FOLD_AFTER) };
    assert_eq!(s[1], "folded[9 steps · 8 commands]", "{s:#?}");
    assert_eq!(s[2], "run[!Ran cargo clippy · failed (exit 1)]");
    assert_eq!(s[3], "folded[5 steps · 5 commands]");
    let map = AttentionMap::compute(&c.state);
    let transcript = c.state.transcripts.get(&key).unwrap();
    let blocks = blocks(&c.state, &map, &key, &e);
    let completed = yaac_core::turns::collapse(transcript, &key, &blocks, &Default::default(), &e, None);
    assert!(completed.iter().any(|block| matches!(block, Block::Run { lines } if lines.len() == 1 && lines[0].failed)));
    let mut e = e;
    e.toggle_line(&key, &"extra-4".into());
    assert_eq!(shape(&c, &key, &e)[2], "run[!+Ran cargo clippy · failed (exit 1)]");
    let first = match &blocks[1] {
        Block::Folded { first, .. } => first.clone(),
        _ => unreachable!(),
    };
    e.toggle_run(&key, &first);
    let opened = shape(&c, &key, &e);
    assert!(opened[1].starts_with("run[+Thought"));
    assert_eq!(opened[3], s[3]);
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

#[test]
fn a_file_change_is_one_line_per_file_and_show_all_expands_the_turn() {
    let (c, _) = boot();
    let key = threads::billing();
    let mut e = Expansion::default();
    // One file opened by itself.
    e.toggle_file(&key, &"b3".into(), 1);
    let s = shape(&c, &key, &e);
    assert!(s[1].contains("+Edited services/billing/Cargo.toml +1 −1"), "{}", s[1]);
    assert!(!s[1].contains("+Edited services/billing/src/client.rs"), "{}", s[1]);
    // "show all" opens every diff of the turn.
    e.toggle_turn(&key, &"b-t1".into(), &["b3".into()]);
    let s = shape(&c, &key, &e);
    assert_eq!(s[1].matches("| +").count(), 4, "{}", s[1]);
    assert_eq!(s.last().unwrap(), "changes[Changed 4 files +14 −16 open]");
    // Closing it returns the turn to its collapsed form, including the file opened alone.
    e.toggle_turn(&key, &"b-t1".into(), &["b3".into()]);
    let s = shape(&c, &key, &e);
    assert_eq!(s[1].matches("| +").count(), 0, "{}", s[1]);
    // A turn that changed one file has no summary line; a non-diff output stays one plain line.
    let search = shape(&c, &threads::search(), &Expansion::default());
    assert!(search.iter().any(|b| b.starts_with("changes[Changed 2 files")), "{search:#?}");
}

#[test]
fn a_codex_turn_shows_no_line_for_hidden_reasoning() {
    // Codex sends a reasoning item with an empty summary between almost every command
    // (D148): none is a line, except the one that took 6 s and the one with text.
    let (c, _) = boot();
    let key = threads::parser();
    let mut e = Expansion::default();
    e.toggle_run(&key, &"parser-3".into());
    assert_eq!(
        shape(&c, &key, &e),
        [
            "user",
            "run[Ran ls samples/ | Ran cargo build -p parser | Ran ./target/debug/parser samples/a.frm | Ran ./target/debug/parser samples/b.frm | Thought for 6s | Ran ./target/debug/parser samples/c.frm | +Thought for 3s]",
            "prose",
        ]
    );

    e.toggle_thought(&key, &"parser-11".into());
    assert!(shape(&c, &key, &e)[1].ends_with("| Thought for 3s]"));
    e.toggle_thought(&key, &"parser-11".into());
    assert!(shape(&c, &key, &e)[1].ends_with("| +Thought for 3s]"));

    let map = AttentionMap::compute(&c.state);
    yaac_core::activity::collapse_all(&c.state, &key, &mut e);
    e.toggle_run(&key, &"parser-3".into());
    assert!(shape(&c, &key, &e)[1].ends_with("| Thought for 3s]"));
    yaac_core::activity::expand_all(&c.state, &map, &key, &mut e);
    let expanded = shape(&c, &key, &e);
    assert!(expanded[1].ends_with("| +Thought for 3s]"));
    assert!(expanded[1].contains("| Thought for 6s |"));
}
