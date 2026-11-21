//! Bulk Archive and Mute in the sidebar (D138): the multi-selection, the bulk plan, one
//! `hub/view/setMany` for a bulk mute, and the report of a bulk Archive.
mod common;

use common::{NOW, boot, settle};
use yaac_core::ThreadKey;
use yaac_core::attention::AttentionMap;
use yaac_core::bulk::{BulkArchiveDialog, BulkKind, Click, ListSelection, Outcome, Skip, archive_report, plan};
use yaac_core::demo::threads;
use yaac_core::sidebar::{Grouping, Open, Order, Ordering, sections};
use yaac_core::time::Zone;
use yaac_protocol::Capability;

/// Every listed row, in display order.
fn order(c: &yaac_core::client::Client, ordering: Ordering) -> Vec<ThreadKey> {
    let map = AttentionMap::compute(&c.state);
    sections(&c.state, &map, Grouping::default(), ordering, "", Open::default(), NOW, Zone::UTC).into_iter().flat_map(|s| s.rows.into_iter().map(|r| r.key)).collect()
}

#[test]
fn ctrl_click_toggles_plain_click_opens_and_clears() {
    let (c, _) = boot();
    let o = order(&c, Ordering::default());
    let mut sel = ListSelection::default();
    assert!(!sel.click(&o, &threads::billing(), Click::Toggle, None), "Ctrl+click does not open");
    assert!(!sel.click(&o, &threads::fix(), Click::Toggle, None));
    assert_eq!(sel.len(), 2);
    assert!(sel.contains(&threads::billing()) && sel.contains(&threads::fix()));
    // Ctrl+click again removes it.
    sel.click(&o, &threads::billing(), Click::Toggle, None);
    assert_eq!(sel.in_order(&o), [threads::fix()]);
    // A plain click opens and clears.
    assert!(sel.click(&o, &threads::users(), Click::Plain, None));
    assert!(sel.is_empty());
}

#[test]
fn shift_click_selects_the_range_from_the_anchor_across_groups_in_display_order() {
    let (c, _) = boot();
    let o = order(&c, Ordering::default());
    // search-service (group "search · gpu-box") to billing-service ("api · devbox"): the rows
    // between them, as shown, whatever group they are in.
    let mut sel = ListSelection::default();
    sel.click(&o, &threads::search(), Click::Plain, None);
    sel.click(&o, &threads::billing(), Click::Range, None);
    assert_eq!(sel.in_order(&o), [threads::search(), threads::ci(), threads::billing()]);
    // Upwards from the same anchor replaces the range.
    sel.click(&o, &threads::index(), Click::Range, None);
    assert_eq!(sel.in_order(&o), [threads::index(), threads::search()]);
    // With no anchor yet, a range starts at the open thread.
    let mut fresh = ListSelection::default();
    fresh.click(&o, &threads::ci(), Click::Range, Some(&threads::orchestrator()));
    assert_eq!(fresh.in_order(&o), [threads::orchestrator(), threads::index(), threads::search(), threads::ci()]);
    // Ctrl+click moves the anchor: the next range starts there.
    let mut s = ListSelection::default();
    s.click(&o, &threads::fix(), Click::Toggle, None);
    s.click(&o, &threads::users(), Click::Range, None);
    assert_eq!(s.in_order(&o), [threads::fix(), threads::handshake(), threads::users()]);
}

#[test]
fn the_selection_survives_a_reorder_and_drops_threads_that_leave_the_list() {
    let (c, _) = boot();
    let o = order(&c, Ordering::default());
    let mut sel = ListSelection::default();
    sel.click(&o, &threads::billing(), Click::Toggle, None);
    sel.click(&o, &threads::fix(), Click::Toggle, None);
    // A → Z: the rows move, the selection stays with them.
    let az = order(&c, Ordering { order: Order::AToZ, ..Ordering::default() });
    assert_ne!(az, o);
    sel.retain_listed(&az);
    assert_eq!(sel.len(), 2);
    let pos = |k: &ThreadKey| az.iter().position(|x| x == k).unwrap();
    let want = if pos(&threads::billing()) < pos(&threads::fix()) { [threads::billing(), threads::fix()] } else { [threads::fix(), threads::billing()] };
    assert_eq!(sel.in_order(&az), want, "in the new order");
    // billing-service leaves the list (archived elsewhere, filtered out, or removed).
    let without: Vec<ThreadKey> = az.iter().filter(|k| **k != threads::billing()).cloned().collect();
    sel.retain_listed(&without);
    assert_eq!(sel.in_order(&without), [threads::fix()]);
}

#[test]
fn esc_clears_ctrl_a_selects_every_listed_row_and_shift_arrows_extend() {
    let (c, _) = boot();
    let o = order(&c, Ordering::default());
    let mut sel = ListSelection::default();
    sel.select_all(&o);
    assert_eq!(sel.in_order(&o), o);
    sel.clear();
    assert!(sel.is_empty());
    // Shift+↓ twice from the open thread, then Shift+↑ once.
    let open = threads::search();
    sel.extend(&o, 1, Some(&open));
    sel.extend(&o, 1, Some(&open));
    assert_eq!(sel.in_order(&o), [threads::search(), threads::ci(), threads::billing()]);
    sel.extend(&o, -1, Some(&open));
    assert_eq!(sel.in_order(&o), [threads::search(), threads::ci()]);
    // Past the top it stops there.
    for _ in 0..20 {
        sel.extend(&o, -1, Some(&open));
    }
    assert_eq!(sel.in_order(&o).first(), o.first());
    assert_eq!(sel.in_order(&o).last(), Some(&threads::search()));
}

#[test]
fn the_archive_plan_skips_by_capability_and_state_with_reasons() {
    let (mut c, _) = boot();
    // billing-service's harness cannot archive (a capability, never the provider's kind).
    let billing = threads::billing();
    let inst = c.state.summary(&billing).unwrap().instance.clone();
    for t in c.state.targets.iter_mut().filter(|t| t.entry.id == billing.target) {
        for p in t.providers.iter_mut().filter(|p| p.instance == inst) {
            p.capabilities.retain(|x| *x != Capability::ThreadArchive);
        }
    }
    if let Some(t) = c.state.transcripts.get_mut(&billing)
        && let Some(e) = t.effective.as_mut()
    {
        e.capabilities.retain(|x| *x != Capability::ThreadArchive);
    }
    let keys = [threads::users(), billing.clone(), threads::search(), threads::handshake(), threads::fix(), threads::notes(), threads::ci()];
    let p = plan(&c.state, &keys, BulkKind::Archive, NOW);
    assert_eq!(p.eligible, [threads::users()]);
    let why = |k: &ThreadKey| p.skipped.iter().find(|(x, _)| x == k).map(|(_, s)| *s);
    assert_eq!(why(&billing), Some(Skip::NoArchive));
    assert_eq!(why(&threads::search()), Some(Skip::Running));
    assert_eq!(why(&threads::handshake()), Some(Skip::InUse), "in use in a terminal: read-only here");
    assert_eq!(why(&threads::fix()), Some(Skip::NeedsYou), "a pending approval is never archived away");
    assert_eq!(why(&threads::notes()), Some(Skip::Offline), "staging is offline");
    assert_eq!(why(&threads::ci()), Some(Skip::Waiting));
    assert_eq!(
        p.skipped_line().as_deref(),
        Some(
            "1 of 7 can be archived; 1 cannot be archived by its harness; 1 is on a machine that is not connected; 1 is running; 1 is waiting on another agent; 1 is waiting for you; 1 is in use elsewhere"
        )
    );
    // The confirmation counts what will be archived and says why the rest are not.
    let d = BulkArchiveDialog::new(&c.state, &[threads::users(), threads::search(), threads::orchestrator()], NOW);
    assert_eq!(d.title, "Archive 1 thread?");
    assert_eq!(d.skipped.as_deref(), Some("1 of 3 can be archived; 2 are running"));
    let none = BulkArchiveDialog::new(&c.state, &[threads::search()], NOW);
    assert!(!none.can_archive());
    assert_eq!(none.skipped.as_deref(), Some("None of these 1 can be archived; 1 is running"));
    // Mute takes any listed thread; Unmute only the muted ones.
    let m = plan(&c.state, &keys, BulkKind::Mute, NOW);
    assert_eq!(m.eligible.len(), keys.len());
    let u = plan(&c.state, &keys, BulkKind::Unmute, NOW);
    assert!(u.eligible.is_empty() && u.skipped.iter().all(|(_, s)| *s == Skip::NotMuted));
}

#[test]
fn mute_all_is_one_set_many_and_applies_at_once() {
    let (mut c, hub) = boot();
    let keys = [threads::users(), threads::billing(), threads::fix()];
    let before = hub.requests().len();
    c.mute_many(&keys, Some(NOW + 3600));
    settle(&mut c, &hub);
    let sent: Vec<_> = hub.requests().into_iter().skip(before).collect();
    let many: Vec<_> = sent.iter().filter(|(_, m, _)| m == "hub/view/setMany").collect();
    assert_eq!(many.len(), 1, "{sent:?}");
    assert!(sent.iter().all(|(_, m, _)| m != "hub/view/set"), "no per-thread set");
    let entries = many[0].2["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|e| e["muted"] == true && e["mutedUntil"].is_string()));
    for k in &keys {
        assert!(c.state.is_muted(k, NOW), "{k:?}");
    }
    // Unmute selected: one more.
    c.unmute_many(&keys);
    let after: Vec<_> = hub.requests().into_iter().skip(before).filter(|(_, m, _)| m == "hub/view/setMany").collect();
    assert_eq!(after.len(), 2);
    assert!(keys.iter().all(|k| !c.state.is_muted(k, NOW)));
}

#[test]
fn bulk_archive_sends_one_request_per_thread_with_its_own_op_id_and_reports_failures_once() {
    let (mut c, hub) = boot();
    // A thread this window lists but its target no longer knows: its archive is refused.
    let ghost = ThreadKey::new("devbox", "t-ghost");
    let mut s = c.state.summary(&threads::billing()).unwrap().clone();
    s.id = ghost.thread.clone();
    s.title = "Ghost thread".into();
    c.state.upsert_summary(&ghost.target, s);
    // users-service is pinned: archiving clears it, in one setMany, once the answers are in.
    c.set_pinned(&threads::users(), true);
    let before = hub.requests().len();
    c.archive_many(&[threads::users(), threads::billing(), ghost.clone()]);
    assert_eq!(c.archives_in_flight(), 1);
    settle(&mut c, &hub);
    let sent: Vec<_> = hub.requests().into_iter().skip(before).collect();
    let archives: Vec<_> = sent.iter().filter(|(_, m, _)| m == "thread/archive").collect();
    assert_eq!(archives.len(), 3);
    let ops: std::collections::BTreeSet<&str> = archives.iter().map(|(_, _, p)| p["opId"].as_str().unwrap()).collect();
    assert_eq!(ops.len(), 3, "each its own operation id");
    assert!(archives.iter().all(|(t, _, _)| t.as_ref().is_some_and(|t| t.as_str() == "devbox" || t.as_str() == "local")), "routed to each thread's target");
    assert_eq!(c.archives_in_flight(), 0);
    assert!(c.state.summary(&threads::users()).unwrap().archived && c.state.summary(&threads::billing()).unwrap().archived);
    assert!(!c.state.summary(&ghost).unwrap().archived, "a refusal leaves it where it was");
    assert!(!c.state.is_pinned(&threads::users()), "the pin went with it (D93)");
    let many: Vec<_> = sent.iter().filter(|(_, m, _)| m == "hub/view/setMany").collect();
    assert_eq!(many.len(), 1);
    let r = c.bulk_report.clone().expect("one report");
    assert_eq!(r.text, "1 could not be archived: Ghost thread");
    // Nothing is sent again by itself.
    let n = hub.requests().len();
    settle(&mut c, &hub);
    assert_eq!(hub.requests().len(), n);
}

#[test]
fn the_archive_report_names_failed_and_uncertain_threads_and_is_silent_when_all_succeeded() {
    let ok = |t: &str| (t.to_owned(), Outcome::Ok);
    assert_eq!(archive_report(&[ok("a"), ok("b")]), None);
    let r = archive_report(&[
        ok("a"),
        ("billing-service".into(), Outcome::Failed("target offline".into())),
        ("CI integration".into(), Outcome::Failed("no such thread".into())),
        ("Fix frame decoder".into(), Outcome::Uncertain),
    ])
    .unwrap();
    assert_eq!(r.text, "2 could not be archived: billing-service, CI integration · uncertain: Fix frame decoder");
    assert_eq!(r.detail.len(), 3);
    assert!(r.detail[0].contains("target offline"));
    assert!(r.detail[2].contains("not asked again"));
    let only = archive_report(&[("x".into(), Outcome::Uncertain)]).unwrap();
    assert_eq!(only.text, "uncertain: x");
}
