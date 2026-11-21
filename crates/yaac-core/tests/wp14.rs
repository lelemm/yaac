//! WP14: the thread menu (D109), Delete (D110), Archive (D111), the date field (D112), the
//! unseen mark (D114), narrow rows (D115), ordering (D116), and the Needs-you button (D117).
mod common;

use common::{NOW, boot};
use yaac_core::demo::threads;
use yaac_core::thread_menu::{ArchiveDialog, DeleteDialog, MenuAction, MenuEntry, thread_menu};

fn labels(entries: &[MenuEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| match e {
            MenuEntry::Separator => "—".to_owned(),
            MenuEntry::Item(i) => {
                let mut s = i.label.to_owned();
                if i.danger {
                    s.push_str(" [danger]");
                }
                if !i.enabled {
                    s.push_str(" [off]");
                }
                s
            }
        })
        .collect()
}

#[test]
fn the_thread_menu_has_the_owners_order() {
    let (c, _) = boot();
    // An idle thread in a project, not pinned, not muted.
    let k = threads::billing();
    assert_eq!(
        labels(&thread_menu(&c.state, &k, NOW)),
        ["Copy thread ID", "Rename", "—", "Fork", "Pin", "Mute…", "Archive…", "—", "Delete… [danger]"]
    );
    // The pinned orchestrator: Unpin where Pin was; it is running, so Delete waits.
    let o = threads::orchestrator();
    assert_eq!(
        labels(&thread_menu(&c.state, &o, NOW)),
        ["Copy thread ID", "Rename", "—", "Fork", "Unpin", "Mute…", "Archive…", "—", "Delete… [danger] [off]"]
    );
    let del = thread_menu(&c.state, &o, NOW).into_iter().filter_map(|e| e.item().cloned()).find(|i| i.action == MenuAction::Delete).unwrap();
    assert_eq!(del.reason, Some("Stop the running turn first"));
}

#[test]
fn muted_archived_and_capability_less_threads() {
    let (mut c, _) = boot();
    let k = threads::billing();
    c.set_muted(&k, true);
    assert_eq!(
        labels(&thread_menu(&c.state, &k, NOW)),
        ["Copy thread ID", "Rename", "—", "Fork", "Pin", "Unmute", "Change mute…", "Archive…", "—", "Delete… [danger]"]
    );
    c.archive(&k);
    assert_eq!(labels(&thread_menu(&c.state, &k, NOW)), ["Copy thread ID", "Rename", "—", "Fork", "Unarchive", "—", "Delete… [danger]"]);
    // A harness that cannot rename or archive: the entries are there, unavailable, with why.
    let b = threads::users();
    let inst = c.state.summary(&b).unwrap().instance.clone();
    for t in &mut c.state.targets {
        for p in &mut t.providers {
            if p.instance == inst {
                p.capabilities.retain(|x| !matches!(x, yaac_protocol::Capability::ThreadRename | yaac_protocol::Capability::ThreadArchive));
            }
        }
    }
    c.state.transcripts.remove(&b);
    let menu = thread_menu(&c.state, &b, NOW);
    let rename = menu.iter().filter_map(MenuEntry::item).find(|i| i.action == MenuAction::Rename).unwrap();
    assert!(!rename.enabled && rename.reason.is_some());
    let archive = menu.iter().filter_map(MenuEntry::item).find(|i| i.action == MenuAction::Archive).unwrap();
    assert!(!archive.enabled);
}

#[test]
fn the_delete_dialog_offers_the_harness_only_when_it_can_delete() {
    let (mut c, _) = boot();
    let k = threads::billing();
    let d = DeleteDialog::new(&c.state, &k).unwrap();
    assert_eq!(d.title, "Delete billing-service?");
    let name = c.state.provider_of(&k).unwrap().display_name.clone();
    assert_eq!(d.harness_label.as_deref(), Some(format!("Also delete the session from {name}").as_str()));
    assert!(d.also_in_harness, "checked by default");
    assert!(d.also_in_harness());
    let mut off = d;
    off.also_in_harness = false;
    assert!(!off.also_in_harness());
    assert!(off.body().contains("keeps its session"));
    // Without `threadDelete`, no checkbox: YAAC removes only its own record.
    let inst = c.state.summary(&k).unwrap().instance.clone();
    for t in &mut c.state.targets {
        for p in &mut t.providers {
            if p.instance == inst {
                p.capabilities.retain(|x| *x != yaac_protocol::Capability::ThreadDelete);
            }
        }
    }
    c.state.transcripts.remove(&k);
    let d = DeleteDialog::new(&c.state, &k).unwrap();
    assert_eq!(d.harness_label, None);
    assert!(!d.also_in_harness());
    assert!(d.body().contains("cannot delete its own sessions"));
}

#[test]
fn archive_asks_first() {
    let (c, _) = boot();
    let d = ArchiveDialog::new(&c.state, &threads::billing()).unwrap();
    assert_eq!(d.title, "Archive billing-service?");
    assert!(d.body.contains("Unarchive"));
}

// ── D114: the unseen mark ────────────────────────────────────────────────────────────────

use yaac_core::attention::AttentionMap;
use yaac_core::sidebar::{Grouping, NeedsCycle, Open, Order, Ordering, SectionKind, Steady, natural_cmp, needs_you, sections, title_width};
use yaac_core::state::ViewPatch;
use yaac_core::time::Zone;

fn unseen(c: &mut yaac_core::client::Client, k: &yaac_core::ThreadKey) {
    c.set_view(k, ViewPatch { unseen: Some(true), ..Default::default() });
}

#[test]
fn the_unseen_mark_lives_in_the_hub_clears_on_open_and_yields_to_louder_marks() {
    let (mut c, hub) = boot();
    let billing = threads::billing();
    unseen(&mut c, &billing);
    common::settle(&mut c, &hub);
    let map = AttentionMap::compute(&c.state);
    assert!(yaac_core::attention::display(&c.state, &map, &billing).unseen);
    // Needs you, running, failed and paused win: the mark is kept, but not drawn.
    for (k, why) in [(threads::fix(), "needs you"), (threads::orchestrator(), "running")] {
        unseen(&mut c, &k);
        assert!(c.state.is_unseen(&k));
        assert!(!yaac_core::attention::display(&c.state, &map, &k).unseen, "{why}");
    }
    // A restart: a new window reads it back from the hub (`hub/view/get`).
    let mut again = yaac_core::client::Client::new(Box::new(hub.clone()), "test", yaac_protocol::ClientId::new(yaac_core::demo::DEMO_CLIENT));
    common::settle(&mut again, &hub);
    assert!(again.state.is_unseen(&billing));
    // Opening it clears it, here and in the hub.
    again.open(&billing);
    common::settle(&mut again, &hub);
    assert!(!again.state.is_unseen(&billing));
    let mut third = yaac_core::client::Client::new(Box::new(hub.clone()), "test", yaac_protocol::ClientId::new(yaac_core::demo::DEMO_CLIENT));
    common::settle(&mut third, &hub);
    assert!(!third.state.is_unseen(&billing));
    // A thread shown on screen is seen even when the mark arrives later.
    third.set_showing(vec![threads::ci()]);
    unseen(&mut c, &threads::ci());
    common::settle(&mut c, &hub);
    third.state.apply_view(&serde_json::json!({ "views": [{ "target": "devbox", "threadId": threads::ci().thread, "unseen": true }] }));
    third.set_showing(vec![threads::ci(), threads::billing()]);
    assert!(!third.state.is_unseen(&threads::ci()));
}

#[test]
fn delete_removes_the_thread_from_the_list_and_its_view_preferences() {
    let (mut c, hub) = boot();
    let k = threads::billing();
    c.set_pinned(&k, true);
    c.open(&k);
    common::settle(&mut c, &hub);
    c.delete(&k, true);
    common::settle(&mut c, &hub);
    assert!(c.state.summary(&k).is_none());
    assert!(!c.state.view.contains_key(&k));
    assert_ne!(c.state.selection, yaac_core::state::Selection::Thread(k.clone()));
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

// ── D115: narrow rows ────────────────────────────────────────────────────────────────────

#[test]
fn a_narrow_row_shortens_its_title_and_keeps_every_mark() {
    let marks = [16.0, 15.0, 40.0, 30.0, 24.0];
    assert_eq!(title_width(300.0, &marks, 20.0), 155.0);
    // Narrower: the title gives way, the marks keep their room.
    assert_eq!(title_width(140.0, &marks, 20.0), 0.0);
    assert!(title_width(160.0, &marks, 20.0) < title_width(200.0, &marks, 20.0));
}

// ── D116: ordering ───────────────────────────────────────────────────────────────────────

fn group_titles(c: &yaac_core::client::Client, o: Ordering, id_part: &str) -> Vec<String> {
    let map = AttentionMap::compute(&c.state);
    sections(&c.state, &map, Grouping::default(), o, "", Open::default(), NOW, Zone::UTC)
        .into_iter()
        .find(|s| s.kind == SectionKind::Group && s.header.as_ref().is_some_and(|h| h.left == id_part && h.machine.as_deref() == Some("devbox")))
        .map(|s| s.rows.into_iter().map(|r| r.title).collect())
        .unwrap_or_default()
}

#[test]
fn ordering_table() {
    let (mut c, hub) = boot();
    // "api — devbox": CI integration (1m), billing-service (9m).
    let on = Ordering::default();
    assert_eq!(group_titles(&c, on, "api"), ["CI integration", "billing-service"]);
    assert_eq!(group_titles(&c, Ordering { order: Order::AToZ, ..on }, "api"), ["billing-service", "CI integration"]);
    assert_eq!(group_titles(&c, Ordering { order: Order::ZToA, ..on }, "api"), ["CI integration", "billing-service"]);
    // "yaac — devbox": Fix frame decoder needs the user, Explain the handshake is older.
    let yaac = |c: &yaac_core::client::Client, o| group_titles(c, o, "yaac");
    assert_eq!(yaac(&c, Ordering { order: Order::ZToA, ..on }), ["Fix frame decoder", "Explain the handshake"], "floated");
    assert_eq!(yaac(&c, Ordering { order: Order::AToZ, needs_first: false, ..on }), ["Explain the handshake", "Fix frame decoder"]);
    // Unseen floats below needs you, above the rest; off, it falls back into the order.
    unseen(&mut c, &threads::billing());
    common::settle(&mut c, &hub);
    let az = Ordering { order: Order::ZToA, ..on };
    assert_eq!(group_titles(&c, az, "api"), ["billing-service", "CI integration"]);
    assert_eq!(group_titles(&c, Ordering { unseen_first: false, ..az }, "api"), ["CI integration", "billing-service"]);
    // Seen: it drops back.
    c.open(&threads::billing());
    common::settle(&mut c, &hub);
    assert_eq!(group_titles(&c, az, "api"), ["CI integration", "billing-service"]);
    // Natural, case-insensitive titles.
    let mut t = vec!["task 10", "Task 2", "alpha", "Beta", "task 1"];
    t.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(t, ["alpha", "Beta", "task 1", "Task 2", "task 10"]);
    // There is no Needs-you section any more.
    let map = AttentionMap::compute(&c.state);
    let secs = sections(&c.state, &map, Grouping::default(), on, "", Open::default(), NOW, Zone::UTC);
    assert!(secs.iter().all(|s| s.header.as_ref().is_none_or(|h| !h.left.starts_with("Needs you"))));
}

#[test]
fn an_answered_thread_drops_back_into_the_order() {
    let (mut c, hub) = boot();
    let o = Ordering { order: Order::AToZ, ..Ordering::default() };
    let yaac = |c: &yaac_core::client::Client| group_titles(c, o, "yaac");
    assert_eq!(yaac(&c), ["Fix frame decoder", "Explain the handshake"]);
    // Answer the approval that keeps it on top.
    let fix = threads::fix();
    c.open(&fix);
    common::settle(&mut c, &hub);
    let answer = yaac_protocol::RequestResponse::Approval(yaac_protocol::ApprovalAnswer { decision: "allow".into(), reason: None });
    c.respond(&fix, &yaac_protocol::RequestId::new("r-fix"), &answer, "Allow once".into());
    common::settle(&mut c, &hub);
    assert_eq!(yaac(&c), ["Explain the handshake", "Fix frame decoder"]);
}

#[test]
fn a_reorder_waits_while_the_pointer_is_over_the_list() {
    let (mut c, hub) = boot();
    let o = Ordering { order: Order::ZToA, ..Ordering::default() };
    let map = AttentionMap::compute(&c.state);
    let mut steady = Steady::default();
    let api = |secs: &[yaac_core::sidebar::Section]| -> Vec<String> {
        secs.iter().find(|s| s.header.as_ref().is_some_and(|h| h.left == "api" && h.machine.as_deref() == Some("devbox"))).unwrap().rows.iter().map(|r| r.title.clone()).collect()
    };
    let first = steady.apply(sections(&c.state, &map, Grouping::default(), o, "", Open::default(), NOW, Zone::UTC), false);
    assert_eq!(api(&first), ["CI integration", "billing-service"]);
    unseen(&mut c, &threads::billing());
    common::settle(&mut c, &hub);
    let map = AttentionMap::compute(&c.state);
    let fresh = || sections(&c.state, &map, Grouping::default(), o, "", Open::default(), NOW, Zone::UTC);
    // Hovered: the rows stay where they are.
    assert_eq!(api(&steady.apply(fresh(), true)), ["CI integration", "billing-service"]);
    assert_eq!(api(&steady.apply(fresh(), true)), ["CI integration", "billing-service"]);
    // The pointer left: the chosen order comes back.
    assert_eq!(api(&steady.apply(fresh(), false)), ["billing-service", "CI integration"]);
}

// ── D117: the Needs-you button ───────────────────────────────────────────────────────────

#[test]
fn the_needs_you_button_cycles_drops_resolved_threads_and_disappears() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let mut list = needs_you(&c.state, &map, NOW);
    assert_eq!(list.len(), 2);
    let mut cycle = NeedsCycle::default();
    let a = cycle.next(&list).unwrap();
    let b = cycle.next(&list).unwrap();
    assert_ne!(a, b);
    assert_eq!(cycle.next(&list).unwrap(), a, "it wraps");
    // `a` is answered: it leaves the cycle.
    list.retain(|e| e.key != a);
    assert_eq!(cycle.next(&list).unwrap(), b);
    assert_eq!(cycle.next(&list).unwrap(), b);
    list.clear();
    assert_eq!(cycle.next(&list), None, "no button");
    assert_eq!(NeedsCycle::tooltip(2), "Next thread that needs you (2)");
}

// ── D118: the agents panel's relations ───────────────────────────────────────────────────

#[test]
fn card_docked_and_pill_share_one_grouping_by_relation() {
    use yaac_core::agents::{Relation, SpawnedBy, agents_panel, clusters};
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    for k in [threads::orchestrator(), threads::search(), threads::index(), threads::ci()] {
        let Some(panel) = agents_panel(&c.state, &map, &k) else { continue };
        let cl = clusters(&panel, None);
        // In order, and only the relations that have someone.
        let want: Vec<Relation> = [
            (Relation::SpawnedBy, matches!(panel.spawned_by, SpawnedBy::Agent(_))),
            (Relation::Workers, !panel.workers.is_empty()),
            (Relation::InContact, !panel.contacts.is_empty()),
        ]
        .into_iter()
        .filter_map(|(r, has)| has.then_some(r))
        .collect();
        assert_eq!(cl.iter().map(|c| c.relation).collect::<Vec<_>>(), want, "{k:?}");
        // Every row once, in the panel's order: the same rows the docked panel lists.
        let flat: Vec<_> = cl.iter().flat_map(|c| c.rows.iter().map(|r| r.title.clone())).collect();
        let all: Vec<_> = panel.all().map(|r| r.title.clone()).collect();
        assert_eq!(flat, all);
        // The pill's cap trims, keeping one icon per relation and counting the rest.
        let capped = clusters(&panel, Some(3));
        assert!(capped.iter().all(|c| !c.rows.is_empty()));
        assert_eq!(capped.iter().map(|c| c.rows.len() + c.more).sum::<usize>(), all.len());
        if all.len() > 3 {
            assert!(capped.iter().map(|c| c.rows.len() + usize::from(c.more > 0)).sum::<usize>() <= 3.max(capped.len() * 2));
        }
    }
    // The orchestrator was started by the user: no "Spawned by" cluster in the pill.
    let panel = agents_panel(&c.state, &map, &threads::orchestrator()).unwrap();
    assert!(clusters(&panel, None).iter().all(|c| c.relation != Relation::SpawnedBy));
}
