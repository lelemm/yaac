//! The sidebar: a flat list under sticky headers, for every grouping combination.
mod common;

use common::{NOW, boot, render, render_open};
use yaac_core::attention::{Attention, AttentionMap};
use yaac_core::demo::{DemoHub, threads};
use yaac_core::hints::RequestKindWord;
use yaac_core::sidebar::{Grouping, Open, SectionKind, hub_status, needs_you, sections};
use yaac_core::time::Zone;

fn g(machine: bool, project: bool, worktree: bool) -> Grouping {
    Grouping { machine, project, worktree, show_dismissed: false }
}

#[test]
fn the_default_matches_the_brief() {
    let (c, _) = boot();
    let want = [
        "# Needs you · 2",
        "Fix frame decoder (needs you) devbox",
        "index-rebuild (needs you) gpu-box",
        "# Pinned · 1",
        "Migrate auth module — orchestrator (running) now / main ~4 3w",
        "# search — gpu-box",
        "search-service (running) now / auth/search ~3 1w",
        "index-rebuild (needs you) 41m / auth/search ~3",
        "# api — devbox",
        "CI integration (waiting) 1m / main v2",
        "billing-service (done) 9m / auth/billing +2 ~6 -1 ^1",
        "# yaac — devbox",
        "Fix frame decoder (needs you) 2m / feature/protocol +1 ~6 ^2",
        "Explain the handshake () 2h / feature/protocol",
        "# api — local",
        "users-service (done) 12m / auth/users +3 ~8 -1 ^3",
        "# docs — staging (offline · last seen 3h ago)",
        "Release notes (offline) 3h / main",
        "# _~/scratch/spike-7_ — devbox",
        "Try the new parser () 1d",
        "# Muted · 4",
        "# Archived · 3",
    ];
    assert_eq!(render(&c, Grouping::default()), want);
}

#[test]
fn every_combination_yields_flat_sections_holding_each_thread_once() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    for machine in [true, false] {
        for project in [true, false] {
            for worktree in [true, false] {
                let grouping = g(machine, project, worktree);
                let secs = sections(&c.state, &map, grouping, "", Open { muted: true, archived: true }, NOW, Zone::UTC);
                let groups: Vec<_> = secs.iter().filter(|s| s.kind == SectionKind::Group).collect();
                let rows: usize = groups.iter().map(|s| s.rows.len()).sum();
                let muted: usize = secs.iter().filter(|s| s.kind == SectionKind::Muted).map(|s| s.rows.len()).sum();
                let pinned: usize = secs.iter().filter(|s| s.kind == SectionKind::Pinned).map(|s| s.rows.len()).sum();
                let archived: usize = secs.iter().filter(|s| s.kind == SectionKind::Archived).map(|s| s.rows.len()).sum();
                assert_eq!(pinned, 1);
                assert_eq!(archived, 3);
                assert_eq!(rows + muted + pinned + archived, c.state.threads.len(), "{grouping:?}");
                assert_eq!(muted, 4);
                let any = machine || project || worktree;
                assert!(groups.iter().all(|s| s.header.is_some() == any), "{grouping:?}");
                // Needs-you and Muted rows are one line; others have a second line in git.
                assert!(secs.iter().filter(|s| matches!(s.kind, SectionKind::NeedsYou | SectionKind::Muted)).all(|s| s.rows.iter().all(|r| r.line2.is_none())));
                if !any {
                    assert_eq!(groups.len(), 1, "no headers: one list");
                }
                // Rows within a section and sections themselves are newest first.
                for s in &groups {
                    let t: Vec<i64> = s
                        .rows
                        .iter()
                        .map(|r| yaac_core::time::secs(&c.state.summary(&r.key).unwrap().updated_at).unwrap())
                        .collect();
                    assert!(t.windows(2).all(|w| w[0] >= w[1]), "{grouping:?}");
                }
                assert_eq!(secs[0].kind, SectionKind::NeedsYou);
                assert_eq!(secs[secs.len() - 2].kind, SectionKind::Muted);
                assert_eq!(secs.last().unwrap().kind, SectionKind::Archived, "Archived is the last section");
            }
        }
    }
}

#[test]
fn header_text_follows_the_ticks() {
    let (c, _) = boot();
    // Projects are per machine: two "api" projects stay two sections.
    let r = render(&c, g(false, true, false));
    assert_eq!(r.iter().filter(|l| *l == "# api").count(), 2, "{r:#?}");
    let r = render(&c, g(true, true, true));
    assert!(r.contains(&"# yaac · feature/protocol — devbox".to_owned()), "{r:#?}");
    assert!(r.contains(&"# api · auth/billing — devbox".to_owned()));
    let r = render(&c, g(true, false, false));
    assert!(r.contains(&"# devbox".to_owned()));
    let r = render(&c, g(false, false, true));
    assert!(r.contains(&"# main".to_owned()) && r.contains(&"# feature/protocol".to_owned()));
    let flat = render(&c, g(false, false, false));
    assert!(flat[5].starts_with("search-service (running) now"), "{flat:#?}");
    assert!(!flat[5..].iter().any(|l| l.starts_with('#') && !l.starts_with("# Muted") && !l.starts_with("# Archived")));
}

#[test]
fn muted_and_archived() {
    let (mut c, _) = boot();
    let (mute, unmute) = DemoHub::mute_scene();
    for k in &mute {
        c.set_muted(k, true);
    }
    for k in &unmute {
        c.set_muted(k, false);
    }
    let r = render_open(&c, Grouping::default(), true);
    // A muted, blocked thread stays under Needs you, marked "muted", and nowhere else.
    assert!(r.contains(&"index-rebuild (needs you) gpu-box muted".to_owned()), "{r:#?}");
    assert_eq!(r.iter().filter(|l| l.starts_with("index-rebuild")).count(), 1);
    let muted_at = r.iter().position(|l| l == "# Muted · 4").unwrap();
    let archived_at = r.iter().position(|l| l == "# Archived · 3").unwrap();
    let names: Vec<_> = r[muted_at + 1..archived_at].iter().map(|l| l.split(" (").next().unwrap()).collect();
    assert_eq!(names, ["search-service", "users-service", "Lint pass", "Old spike"]);
    assert!(r[muted_at + 1..archived_at].iter().all(|l| !l.contains(" / ")), "muted rows are one line");
    // A timed mute shows when it ends, in the host's time.
    assert!(r.contains(&"Lint pass (done) until 20:30".to_owned()), "{r:#?}");
    // Archived is the last section, under Muted.
    assert_eq!(r[archived_at + 1..].iter().map(|l| l.split(" (").next().unwrap()).collect::<Vec<_>>(), ["Spike: protobuf codec", "Old CI flake hunt", "Rename the crates"]);
    // Archiving lists a thread only under Archived, and clears its pin and mute; unarchiving
    // does not restore them.
    let users = threads::users();
    c.set_pinned(&users, true);
    c.archive(&users);
    assert!(!c.state.is_pinned(&users) && !c.state.is_muted(&users, NOW));
    let map = AttentionMap::compute(&c.state);
    let secs = sections(&c.state, &map, Grouping::default(), "", Open { muted: true, archived: true }, NOW, Zone::UTC);
    let holding: Vec<_> = secs.iter().filter(|s| s.rows.iter().any(|r| r.key == users)).map(|s| s.kind.clone()).collect();
    assert_eq!(holding, [SectionKind::Archived]);
    c.unarchive(&users);
    assert!(!c.state.is_pinned(&users) && !c.state.is_muted(&users, NOW));
    // Collapsed: the header and its count only.
    let secs = sections(&c.state, &map, Grouping::default(), "", Open::default(), NOW, Zone::UTC);
    let a = secs.last().unwrap();
    assert_eq!((a.kind.clone(), a.collapsed, a.rows.len(), a.count), (SectionKind::Archived, true, 0, 3), "users-service is back in its project");
}

#[test]
fn a_timed_mute_ends_by_itself_and_dismissed_threads_hide() {
    let (mut c, _) = boot();
    let fix = threads::fix();
    let ci = threads::ci();
    c.mute_until(&ci, Some(NOW + 600));
    assert!(c.state.is_muted(&ci, NOW));
    assert!(!c.state.is_muted(&ci, NOW + 600), "no timer: it simply stops applying");
    assert_eq!(c.state.next_unmute(NOW), Some(NOW + 600).min(c.state.next_unmute(NOW)));
    c.set_muted(&ci, false);
    // Dismiss hides a thread in the window only; "Show dismissed" lists it.
    c.set_dismissed(&ci, true);
    let map = AttentionMap::compute(&c.state);
    let has = |g: Grouping| sections(&c.state, &map, g, "", Open::default(), NOW, Zone::UTC).iter().flat_map(|x| x.rows.iter()).any(|r| r.key == ci);
    assert!(!has(Grouping::default()));
    assert!(has(Grouping { show_dismissed: true, ..Grouping::default() }));
    // A blocked thread is never hidden from Needs you, dismissed or muted.
    c.set_dismissed(&fix, true);
    c.set_muted(&fix, true);
    assert!(yaac_core::sidebar::needs_you(&c.state, &map, NOW).iter().any(|e| e.key == fix));
}

#[test]
fn search_filters_title_and_preview() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let found: Vec<_> = sections(&c.state, &map, Grouping::default(), "REBUILD", Open::default(), NOW, Zone::UTC)
        .into_iter()
        .filter(|s| s.kind == SectionKind::Group)
        .flat_map(|s| s.rows)
        .map(|r| r.title)
        .collect();
    assert_eq!(found, ["search-service", "index-rebuild"]);
}

#[test]
fn needs_you_lists_every_machine_most_recent_first() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let got: Vec<_> = needs_you(&c.state, &map, NOW).into_iter().map(|e| (e.title, e.attention, e.target_name)).collect();
    let a = Attention::Request(RequestKindWord::Approval);
    assert_eq!(
        got,
        [("Fix frame decoder".to_owned(), a, "devbox".to_owned()), ("index-rebuild".to_owned(), a, "gpu-box".to_owned())]
    );
    assert_eq!(hub_status(&c.state), "Hub running · 3 of 4 targets");
}

#[test]
fn pinned_threads_leave_their_project_and_projects_take_new_threads() {
    let (mut c, hub) = boot();
    c.set_pinned(&threads::fix(), true);
    let r = render(&c, Grouping::default());
    assert!(r.contains(&"# Pinned · 2".to_owned()));
    assert_eq!(r.iter().filter(|l| l.starts_with("Fix frame decoder")).count(), 2, "Needs you and Pinned: {r:#?}");
    // Creating a project from a temporary one moves its threads in.
    c.create_project(&"devbox".into(), "~/scratch/spike-7", "Parser spike", None);
    common::settle(&mut c, &hub);
    let r = render(&c, Grouping::default());
    assert!(r.contains(&"# Parser spike — devbox".to_owned()), "{r:#?}");
    assert!(!r.iter().any(|l| l.contains("_~/scratch")));
}
