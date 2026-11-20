//! The sidebar: a flat list under sticky headers, for every grouping combination.
mod common;

use common::{NOW, boot, render, render_open};
use yaac_core::attention::{Attention, AttentionMap};
use yaac_core::demo::{DemoHub, threads};
use yaac_core::hints::RequestKindWord;
use yaac_core::sidebar::{Grouping, SectionKind, hub_status, needs_you, sections};

fn g(machine: bool, project: bool, worktree: bool) -> Grouping {
    Grouping { machine, project, worktree, show_archived: false }
}

#[test]
fn the_default_matches_the_brief() {
    let (c, _) = boot();
    let want = [
        "# Needs you · 2",
        "Fix frame decoder (needs you) devbox",
        "index-rebuild (needs you) gpu-box",
        "# Pinned · 1",
        "Migrate auth module — orchestrator (running) now / main  Δ4 3w",
        "# search — gpu-box",
        "search-service (running) now / auth/search  Δ3 1w",
        "index-rebuild (needs you) 41m / auth/search  Δ3",
        "# api — devbox",
        "CI integration (waiting) 1m / main",
        "billing-service (done) 9m / auth/billing  Δ9  ↑1",
        "# yaac — devbox",
        "Fix frame decoder (needs you) 2m / feature/protocol  Δ7  ↑2",
        "Explain the handshake () 2h / feature/protocol",
        "# api — local",
        "users-service (done) 12m / auth/users  Δ12  ↑3",
        "# docs — staging (offline · last seen 3h ago)",
        "Release notes (offline) 3h / main",
        "# _~/scratch/spike-7_ — devbox",
        "Try the new parser () 1d",
        "# Muted · 4",
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
                let secs = sections(&c.state, &map, grouping, "", true, NOW);
                let groups: Vec<_> = secs.iter().filter(|s| s.kind == SectionKind::Group).collect();
                let rows: usize = groups.iter().map(|s| s.rows.len()).sum();
                let muted: usize = secs.iter().filter(|s| s.kind == SectionKind::Muted).map(|s| s.rows.len()).sum();
                let pinned: usize = secs.iter().filter(|s| s.kind == SectionKind::Pinned).map(|s| s.rows.len()).sum();
                assert_eq!(pinned, 1);
                assert_eq!(rows + muted + pinned, c.state.threads.len(), "{grouping:?}");
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
                assert_eq!(secs.last().unwrap().kind, SectionKind::Muted);
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
    assert!(!flat[5..].iter().any(|l| l.starts_with('#') && !l.starts_with("# Muted")));
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
    let names: Vec<_> = r[muted_at + 1..].iter().map(|l| l.split(" (").next().unwrap()).collect();
    assert_eq!(names, ["search-service", "users-service", "Lint pass", "Old spike"]);
    assert!(r[muted_at + 1..].iter().all(|l| !l.contains(" / ")), "muted rows are one line");
    // Archived threads leave the list; "Show archived" brings them back.
    let mut s = c.state.clone();
    s.threads.get_mut(&threads::billing()).unwrap().summary.archived = true;
    let map = AttentionMap::compute(&s);
    let has = |g: Grouping| sections(&s, &map, g, "", false, NOW).iter().flat_map(|x| x.rows.iter()).any(|r| r.key == threads::billing());
    assert!(!has(Grouping::default()));
    assert!(has(Grouping { show_archived: true, ..Grouping::default() }));
}

#[test]
fn search_filters_title_and_preview() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    let found: Vec<_> = sections(&c.state, &map, Grouping::default(), "REBUILD", false, NOW)
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
    let got: Vec<_> = needs_you(&c.state, &map).into_iter().map(|e| (e.title, e.attention, e.target_name)).collect();
    let a = Attention::Request(RequestKindWord::Approval);
    assert_eq!(
        got,
        [("Fix frame decoder".to_owned(), a, "devbox".to_owned()), ("index-rebuild".to_owned(), a, "gpu-box".to_owned())]
    );
    assert_eq!(hub_status(&c.state), "Hub running · 3 of 4 targets");
}

#[test]
fn pinned_threads_leave_their_project_and_projects_take_new_threads() {
    let (mut c, _) = boot();
    c.set_pinned(&threads::fix(), true);
    let r = render(&c, Grouping::default());
    assert!(r.contains(&"# Pinned · 2".to_owned()));
    assert_eq!(r.iter().filter(|l| l.starts_with("Fix frame decoder")).count(), 2, "Needs you and Pinned: {r:#?}");
    // Creating a project from a temporary one moves its threads in.
    c.create_project(&"devbox".into(), "~/scratch/spike-7", "Parser spike");
    let r = render(&c, Grouping::default());
    assert!(r.contains(&"# Parser spike — devbox".to_owned()), "{r:#?}");
    assert!(!r.iter().any(|l| l.contains("_~/scratch")));
}
