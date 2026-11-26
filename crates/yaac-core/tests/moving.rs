//! Moving threads to another project or worktree (D144): which groups a row can be dropped
//! on under each grouping and why not, the same-machine rule, the Move dialog's model and
//! texts, a multi-selection, and what the client sends.
mod common;

use common::{NOW, boot, settle};
use yaac_core::ThreadKey;
use yaac_core::attention::AttentionMap;
use yaac_core::demo::threads;
use yaac_core::moving::{Destination, Drop, MoveDialog, OTHER_MACHINE, can_drag, drop_on};
use yaac_core::sidebar::{Grouping, Open, Ordering, Section, SectionKind, section_of, sections};
use yaac_core::state::AppState;
use yaac_core::thread_menu::{MenuAction, thread_menu};
use yaac_core::time::Zone;

fn secs(state: &AppState, g: Grouping) -> Vec<Section> {
    let map = AttentionMap::compute(state);
    sections(state, &map, g, Ordering::default(), "", Open { muted: true, archived: true }, NOW, Zone::UTC)
}

/// The section a thread is listed in under `g`.
fn group_of(state: &AppState, g: Grouping, k: &ThreadKey) -> Section {
    let id = section_of(state, g, k).expect("listed");
    secs(state, g).into_iter().find(|s| s.id == id).expect("drawn")
}

fn project_id(state: &AppState, k: &ThreadKey) -> String {
    state.project_of(k).expect("a project").wire_id.clone()
}

const BY_PROJECT: Grouping = Grouping { machine: true, project: true, worktree: false, show_dismissed: false };

#[test]
fn a_row_drops_on_another_project_of_its_machine_and_nowhere_else() {
    let (c, _) = common::boot_with_independent_threads();
    let st = &c.state;
    let (billing, fix, search) = (threads::billing(), threads::fix(), threads::search());
    // billing-service (devbox, api, Claude) onto "yaac" on devbox: moved at once (its harness
    // keeps its folder, so nothing to confirm).
    let yaac = group_of(st, BY_PROJECT, &fix);
    match drop_on(st, BY_PROJECT, &yaac, std::slice::from_ref(&billing)) {
        Drop::Move { keys, to, label, confirm } => {
            assert_eq!(keys, std::slice::from_ref(&billing));
            assert_eq!(to, Destination { project: project_id(st, &fix), worktree: None });
            assert_eq!(label, "yaac");
            assert!(!confirm);
        }
        other => panic!("{other:?}"),
    }
    // Its own group: nothing happens.
    assert_eq!(drop_on(st, BY_PROJECT, &group_of(st, BY_PROJECT, &billing), std::slice::from_ref(&billing)), Drop::Nothing);
    // Another machine's group: refused, with the hint.
    assert_eq!(drop_on(st, BY_PROJECT, &group_of(st, BY_PROJECT, &search), std::slice::from_ref(&billing)), Drop::Refused(OTHER_MACHINE.into()));
    // Fix frame decoder's harness (Codex) moves its folder: the drop asks first.
    assert!(matches!(drop_on(st, BY_PROJECT, &group_of(st, BY_PROJECT, &billing), std::slice::from_ref(&fix)), Drop::Move { confirm: true, .. }));
    // A thread in use in a terminal cannot be moved.
    assert_eq!(drop_on(st, BY_PROJECT, &group_of(st, BY_PROJECT, &billing), &[threads::handshake()]), Drop::Refused("This thread is in use elsewhere".into()));
    // Pinned, Muted, Archived are not places.
    for s in secs(st, BY_PROJECT).iter().filter(|s| s.kind != SectionKind::Group) {
        assert_eq!(drop_on(st, BY_PROJECT, s, std::slice::from_ref(&billing)), Drop::Nothing, "{}", s.id);
    }
    // A temporary project (a folder with no project yet) is not one either.
    let parser = ThreadKey::new("devbox", "t-parser");
    let temp = group_of(st, BY_PROJECT, &parser);
    assert!(matches!(drop_on(st, BY_PROJECT, &temp, std::slice::from_ref(&billing)), Drop::Refused(w) if w.contains("not a project yet")));
}

#[test]
fn grouped_by_machine_alone_there_is_nothing_to_move_to() {
    let (c, _) = boot();
    let st = &c.state;
    for g in [
        Grouping { machine: true, project: false, worktree: false, show_dismissed: false },
        Grouping { machine: false, project: false, worktree: false, show_dismissed: false },
    ] {
        assert!(!can_drag(g));
        for s in secs(st, g) {
            assert_eq!(drop_on(st, g, &s, &[threads::billing()]), Drop::Nothing);
        }
    }
    assert!(can_drag(BY_PROJECT));
    assert!(can_drag(Grouping { machine: false, project: false, worktree: true, show_dismissed: false }));
}

#[test]
fn without_the_machine_level_a_group_is_still_one_machines_project() {
    let (c, _) = common::boot_with_independent_threads();
    let st = &c.state;
    let g = Grouping { machine: false, project: true, worktree: false, show_dismissed: false };
    // "api" exists on local and on devbox: two groups, each of its own machine.
    let users_api = group_of(st, g, &threads::users());
    assert!(matches!(drop_on(st, g, &users_api, &[threads::fix()]), Drop::Refused(w) if w == OTHER_MACHINE));
    assert!(matches!(drop_on(st, g, &group_of(st, g, &threads::billing()), &[threads::fix()]), Drop::Move { .. }));
}

#[test]
fn by_worktree_a_drop_names_the_checkout_and_the_selection_moves_together() {
    let (mut c, _) = boot();
    // Give devbox's yaac project a repository with a second checkout that a thread is in.
    let fix = threads::fix();
    for p in c.state.projects.iter_mut().filter(|p| p.name == "yaac") {
        p.repo = Some(yaac_core::projects::RepoLocation { repo: "/home/dev/yaac/.git".into(), relative: ".".into(), checkout: None, root: "/home/dev/yaac".into() });
    }
    let hs = threads::handshake();
    for (k, root, name) in [(&fix, "/home/dev/yaac-wt/protocol", Some("protocol")), (&hs, "/home/dev/yaac", None)] {
        let s = &mut c.state.threads.get_mut(k).expect("listed").summary;
        s.repo = Some(yaac_protocol::RepoLocation { repo: "/home/dev/yaac/.git".into(), relative: ".".into(), root: root.into(), worktree_name: name.map(str::to_owned) });
    }
    let st = &c.state;
    let g = Grouping { machine: true, project: true, worktree: true, show_dismissed: false };
    let wt = group_of(st, g, &fix);
    match drop_on(st, g, &wt, &[threads::billing(), threads::ci()]) {
        Drop::Move { keys, to, label, confirm } => {
            assert_eq!(keys, [threads::billing(), threads::ci()], "both, in the order given");
            assert_eq!(to.worktree.as_deref(), Some("/home/dev/yaac-wt/protocol"));
            assert_eq!(label, "yaac · protocol");
            assert!(confirm, "CI integration's harness moves its folder");
        }
        other => panic!("{other:?}"),
    }
    // The project's own checkout is the project itself (no worktree to name).
    match drop_on(st, g, &group_of(st, g, &hs), &[threads::billing()]) {
        Drop::Move { to, .. } => assert_eq!(to.worktree, None),
        other => panic!("{other:?}"),
    }
    // The dialog offers both checkouts, the project's own first.
    let mut d = MoveDialog::new(st, &[threads::billing()]);
    d.set_project(&project_id(st, &fix));
    let w = d.worktrees(st);
    assert_eq!(w.iter().map(|w| (w.label.as_str(), w.own)).collect::<Vec<_>>(), [("feature/protocol", true), ("protocol", false)]);
    d.set_worktree(st, Some("/home/dev/yaac-wt/protocol"));
    assert_eq!(d.worktree_label(st), "protocol");
    assert_eq!(d.folder(st).as_deref(), Some("/home/dev/yaac-wt/protocol"));
    d.set_worktree(st, Some("/home/dev/yaac"));
    assert_eq!(d.worktree, None, "the project's own checkout is no worktree");
    // A selection across machines cannot be dropped anywhere.
    assert_eq!(drop_on(st, g, &wt, &[threads::billing(), threads::search()]), Drop::Refused(OTHER_MACHINE.into()));
}

#[test]
fn the_move_dialog_says_what_happens_to_the_working_directory() {
    let (c, _) = boot();
    let st = &c.state;
    // Claude: the placement only.
    let d = MoveDialog::new(st, &[threads::billing()]);
    assert_eq!(d.title, "Move “billing-service”");
    assert_eq!(d.project_label(st), "yaac", "the first project it is not in");
    assert_eq!(d.projects(st).iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["api", "yaac"], "its machine's projects only");
    assert_eq!(d.sentence(st), "Only where YAAC lists it changes; the agent keeps working in /home/dev/api.");
    assert!(d.can_move(st) && !d.changes_folder(st));
    assert_eq!(d.worktree_label(st), "None");
    // Codex: the folder follows, from the next turn; the running turn is said.
    let d = MoveDialog::new(st, &[threads::fix()]);
    assert_eq!(d.project_label(st), "api");
    assert_eq!(d.sentence(st), "The agent will work in /home/dev/api from its next turn. A running turn finishes where it is.");
    assert!(d.changes_folder(st));
    // Already there: nothing to move.
    let mut d = MoveDialog::new(st, &[threads::billing()]);
    d.set_project(&project_id(st, &threads::billing()));
    assert!(!d.can_move(st));
    assert_eq!(d.sentence(st), "It is already there.");
    // In use elsewhere, archived, other machines: disabled with the reason.
    assert_eq!(MoveDialog::new(st, &[threads::handshake()]).blocked.as_deref(), Some("This thread is in use elsewhere"));
    let archived = ThreadKey::new("local", "t-arch-1");
    assert_eq!(MoveDialog::new(st, std::slice::from_ref(&archived)).blocked.as_deref(), Some("An archived thread cannot be moved"));
    assert!(MoveDialog::new(st, &[threads::billing(), threads::search()]).blocked.is_some_and(|b| b.contains(OTHER_MACHINE)));
    // A selection: one sentence for both kinds, and who is left out.
    let d = MoveDialog::new(st, &[threads::billing(), threads::ci(), threads::handshake()]);
    assert_eq!(d.title, "Move 3 threads");
    assert_eq!(d.keys, [threads::billing(), threads::ci()]);
    assert_eq!(d.skipped.as_deref(), Some("1 of 3 cannot be moved (in use elsewhere); it stays where it is"));
    assert_eq!(
        d.sentence(st),
        "1 agent will work in /home/dev/yaac from its next turn. For 1, only where YAAC lists it changes; it keeps its folder. A running turn finishes where it is."
    );
    assert_eq!(d.moves(st).len(), 2);
}

#[test]
fn the_menu_offers_move_after_fork_and_says_why_not() {
    let (c, _) = boot();
    let st = &c.state;
    let labels = |k: &ThreadKey| thread_menu(st, k, NOW).into_iter().filter_map(|e| e.item().map(|i| (i.action, i.enabled, i.reason))).collect::<Vec<_>>();
    let m = labels(&threads::billing());
    let at = m.iter().position(|(a, _, _)| *a == MenuAction::Move).expect("Move…");
    assert_eq!(m[at - 1].0, MenuAction::Fork);
    assert!(m[at].1);
    let hs = labels(&threads::handshake());
    assert!(hs.iter().any(|(a, on, why)| *a == MenuAction::Move && !on && *why == Some("This thread is in use elsewhere")));
    // Archived: no Move at all (it is only under Archived).
    assert!(labels(&ThreadKey::new("local", "t-arch-1")).iter().all(|(a, _, _)| *a != MenuAction::Move));
}

#[test]
fn the_client_sends_one_thread_move_each_moves_the_row_at_once_and_puts_back_a_refusal() {
    let (mut c, hub) = boot();
    let (billing, ci, fix) = (threads::billing(), threads::ci(), threads::fix());
    let yaac = project_id(&c.state, &fix);
    let to = Destination { project: yaac.clone(), worktree: None };
    let before = hub.requests().len();
    let notice = yaac_core::moving::notice(&c.state, &[billing.clone(), ci.clone()], "yaac");
    assert_eq!(notice.text, "Moved 2 threads to yaac");
    c.move_threads(&[(billing.clone(), Some(to.clone())), (ci.clone(), Some(to))], Some(notice));
    // Moved at once, before the answers.
    assert_eq!(project_id(&c.state, &billing), yaac);
    settle(&mut c, &hub);
    let sent: Vec<_> = hub.requests().into_iter().skip(before).filter(|(_, m, _)| m == "thread/move").collect();
    assert_eq!(sent.len(), 2);
    assert_ne!(sent[0].2["opId"], sent[1].2["opId"], "each its own operation id");
    assert_eq!(project_id(&c.state, &billing), yaac);
    assert_eq!(c.state.summary(&ci).unwrap().cwd.as_str(), "/home/dev/yaac", "Codex: the folder followed");
    assert_eq!(c.state.summary(&billing).unwrap().cwd.as_str(), "/home/dev/api", "Claude: it did not");
    assert!(c.move_notice.is_some() && c.bulk_report.is_none());
    // Undo: billing had no placement before, so it is forgotten.
    c.undo_move();
    settle(&mut c, &hub);
    let undo: Vec<_> = hub.requests().into_iter().skip(before + 2).filter(|(_, m, _)| m == "thread/move").collect();
    assert_eq!(undo.len(), 2);
    assert!(undo.iter().all(|(_, _, p)| p.get("project").is_none()));
    assert!(c.state.summary(&billing).unwrap().placement.is_none());
    // A refusal puts the row back and is reported once; nothing is re-sent.
    let was = project_id(&c.state, &billing);
    c.move_threads(&[(billing.clone(), Some(Destination { project: "p99".into(), worktree: None }))], None);
    settle(&mut c, &hub);
    assert_eq!(project_id(&c.state, &billing), was);
    assert_eq!(c.bulk_report.as_ref().map(|r| r.text.as_str()), Some("1 could not be moved: billing-service"));
    let n = hub.requests().len();
    settle(&mut c, &hub);
    assert_eq!(hub.requests().len(), n);
}

#[test]
fn the_commander_lists_the_machines_projects_with_the_current_one_ticked() {
    let (c, _) = boot();
    let fix = threads::fix();
    let ctx = yaac_core::commands::Ctx {
        state: &c.state,
        now: NOW,
        thread: Some(&fix),
        draft: None,
        can_back: false,
        can_forward: false,
        has_tray: false,
        grouping: Grouping::default(),
        ordering: Ordering::default(),
        archived_open: false,
        docked: false,
        theme: yaac_core::prefs::ThemePref::System,
        zoom: (13.5, 11.0, 18.0),
        host_windows: false,
        focused_card: None,
        list_selected: 0,
        input: yaac_core::input::InputMode::Pointer,
        terminal: Default::default(),
        fullscreen_tabs: false,
    };
    let items = yaac_core::commands::step_items(yaac_core::commands::StepKind::MoveTo, &ctx, &[]);
    assert_eq!(items.iter().map(|i| (i.label.as_str(), i.current)).collect::<Vec<_>>(), [("api", false), ("yaac", true)]);
    assert!(yaac_core::commands::availability(yaac_core::commands::Cmd::MoveThread, &ctx).enabled());
}
