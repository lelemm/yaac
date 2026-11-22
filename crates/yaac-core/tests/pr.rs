//! Pull requests in the window (D119, D120): the badge's states, `+N`, its menus, the visible
//! refresh, the Create PR modal, and "Ask the agent to write it", against the demo's forge.
mod common;

use common::{NOW, boot, settle};
use yaac_core::demo::threads;
use yaac_core::pr::{self, AgentDraft, BadgeClick, BadgeHint, badge_menu, badge_of, click, parse_agent_draft};
use yaac_protocol::{ChecksState, ForgeAccess, ForgePrsResult, PrState};

fn all_visible(c: &mut yaac_core::client::Client, hub: &yaac_core::demo::DemoHub) {
    let keys: Vec<_> = c.state.threads.keys().cloned().collect();
    c.forge_visible(&keys, NOW);
    settle(c, hub);
}

#[test]
fn badges_open_draft_more_merged_create_disabled_and_absent() {
    let (mut c, hub) = boot();
    // Nothing until the target has answered: no badge flickers in.
    assert_eq!(badge_of(&c.state, &threads::users()), None);
    all_visible(&mut c, &hub);

    let users = badge_of(&c.state, &threads::users()).unwrap();
    assert_eq!(users.number_text(), "33342");
    assert_eq!(users.more_text(), None);
    assert_eq!(users.hint, None);
    assert_eq!(users.checks, Some(ChecksState::Failing));
    assert_eq!(click(&users), BadgeClick::Open("https://github.com/acme/platform/pull/33342".into()));

    // Two from one branch: the open one (a draft) stands; +1 lists the closed one.
    let billing = badge_of(&c.state, &threads::billing()).unwrap();
    assert_eq!(billing.number_text(), "33343");
    assert_eq!(billing.more_text().as_deref(), Some("+1"));
    assert_eq!(billing.hint, Some(BadgeHint::Draft));
    assert_eq!(billing.others[0].number, 33301);
    assert_eq!(pr::describe(&billing.others[0]), "#33301 · closed");

    // Merged: shown with its hint; passing checks draw nothing.
    let search = badge_of(&c.state, &threads::search()).unwrap();
    assert_eq!(search.hint, Some(BadgeHint::Merged));
    assert_eq!(search.checks, None);

    // No pull request yet: no badge (D143); creating one is in the thread's menu.
    assert_eq!(badge_of(&c.state, &threads::fix()), None);
    assert_eq!(pr::create_entry(&c.state, &threads::fix()), Some(Ok(())));
    let menu = yaac_core::thread_menu::thread_menu(&c.state, &threads::fix(), 0);
    assert!(menu.iter().any(|e| matches!(e, yaac_core::thread_menu::MenuEntry::Item(i) if i.action == yaac_core::thread_menu::MenuAction::CreatePr && i.enabled)));

    // az signed out on that machine: no badge either; the menu entry says why.
    assert_eq!(badge_of(&c.state, &threads::ci()), None);
    assert_eq!(pr::create_entry(&c.state, &threads::ci()), Some(Err("The forge's command-line tool on this target is missing or not signed in")));

    // On the default branch with no pull request: no badge.
    assert_eq!(badge_of(&c.state, &threads::orchestrator()), None);
}

#[test]
fn badge_menu_create_and_copy_link() {
    let (mut c, hub) = boot();
    all_visible(&mut c, &hub);
    let users = badge_of(&c.state, &threads::users()).unwrap();
    let menu = badge_menu(&users);
    assert_eq!(menu.iter().map(|i| (i.label, i.enabled)).collect::<Vec<_>>(), [("Create a new PR", true), ("Copy link", true)]);
    // The default branch: offered, but not usable.
    assert_eq!(pr::create_entry(&c.state, &threads::orchestrator()), Some(Err("This is the repository's default branch")));
}

#[test]
fn visible_rows_are_asked_once_then_only_when_due() {
    let (mut c, hub) = boot();
    let k = threads::users();
    assert_eq!(pr::due(&c.state, std::slice::from_ref(&k), NOW), vec![k.clone()]);
    c.forge_visible(std::slice::from_ref(&k), NOW);
    // In flight: not asked again.
    assert!(pr::due(&c.state, std::slice::from_ref(&k), NOW).is_empty());
    settle(&mut c, &hub);
    assert!(pr::due(&c.state, std::slice::from_ref(&k), NOW + 60).is_empty());
    assert_eq!(pr::due(&c.state, std::slice::from_ref(&k), NOW + pr::VISIBLE_REFRESH_SECS), vec![k.clone()]);
    // A new branch on the row: asked at once.
    if let Some(r) = c.state.threads.get_mut(&k)
        && let Some(g) = r.summary.git.as_mut()
    {
        g.branch = Some("auth/users-2".into());
    }
    assert_eq!(pr::due(&c.state, std::slice::from_ref(&k), NOW + 60), vec![k.clone()]);
}

#[test]
fn forge_prs_changed_updates_every_row_of_the_branch() {
    let (mut c, hub) = boot();
    all_visible(&mut c, &hub);
    let k = threads::users();
    let mut r: ForgePrsResult = c.state.prs.of(&k).unwrap().clone();
    r.prs[0].state = PrState::Merged;
    c.state.prs.apply_changed(&k.target, r);
    assert_eq!(badge_of(&c.state, &k).unwrap().hint, Some(BadgeHint::Merged));
    // A branch the forge says nothing about (another repo) changes nothing.
    c.state.prs.apply_changed(&k.target, ForgePrsResult { root: "/x".into(), branch: None, access: ForgeAccess::default(), default_branch: None, prs: vec![], at: None, error: None });
    assert!(badge_of(&c.state, &k).is_some());
}

#[test]
fn the_create_modal_prefills_warns_and_creates() {
    let (mut c, hub) = boot();
    all_visible(&mut c, &hub);
    let k = threads::fix();
    c.open_create_pr(&k);
    assert_eq!(c.state.create_pr.as_ref().unwrap().create_blocked().as_deref(), Some("Reading the branch…"));
    settle(&mut c, &hub);
    let d = c.state.create_pr.as_ref().unwrap();
    assert_eq!(d.base, "main");
    assert_eq!(d.title, "Fix the frame decoder's off-by-one");
    assert!(d.body.contains("two frames back to back"));
    assert!(d.branches().contains(&"release/1.4".to_owned()));
    assert_eq!(d.push_notice().as_deref(), Some("Create will push feature/protocol first: it is not on the remote yet."));
    assert_eq!(d.uncommitted_warning().as_deref(), Some("3 files have uncommitted changes · they are not part of the pull request."));
    assert_eq!(d.create_blocked(), None);

    // The target branch can be the branch itself only by mistake.
    c.state.create_pr.as_mut().unwrap().base = "feature/protocol".into();
    assert_eq!(c.state.create_pr.as_ref().unwrap().create_blocked().as_deref(), Some("The target branch is the branch itself"));
    c.set_create_pr_base("release/1.4");
    settle(&mut c, &hub);
    let d = c.state.create_pr.as_mut().unwrap();
    assert_eq!(d.base, "release/1.4");
    d.title.clear();
    assert_eq!(d.create_blocked().as_deref(), Some("A title is needed"));
    d.title = "Fix the decoder".into();
    d.draft = true;
    assert!(c.submit_create_pr());
    assert!(c.state.create_pr.as_ref().unwrap().creating);
    settle(&mut c, &hub);
    assert!(c.state.create_pr.is_none(), "closed once created");
    let b = badge_of(&c.state, &k).unwrap();
    assert_eq!(b.number_text(), "33350");
    assert_eq!(b.hint, Some(BadgeHint::Draft));
}

#[test]
fn ask_the_agent_fills_the_modal_from_its_reply() {
    let (mut c, hub) = boot();
    // A thread whose agent is busy cannot be asked.
    c.open_create_pr(&threads::fix());
    settle(&mut c, &hub);
    assert_eq!(c.state.create_pr.as_ref().unwrap().ask_blocked(&c.state).as_deref(), Some("The agent is busy with a turn"));
    let k = threads::billing();
    c.open_create_pr(&k);
    settle(&mut c, &hub);
    assert_eq!(c.state.create_pr.as_ref().unwrap().ask_blocked(&c.state), None);
    assert!(c.ask_agent_for_pr());
    assert_eq!(c.state.create_pr.as_ref().unwrap().agent, Some(AgentDraft::Sending));
    for _ in 0..200 {
        settle(&mut c, &hub);
        c.poll_create_pr();
        if c.state.create_pr.as_ref().unwrap().agent.is_none() {
            break;
        }
    }
    let d = c.state.create_pr.as_ref().unwrap();
    assert_eq!(d.agent, None);
    assert_eq!(d.title, "Fix the frame decoder's off-by-one on the last frame");
    assert!(d.body.starts_with("The length prefix already counts the kind byte"));
    assert!(d.edited);
    // The request is a user message in the thread, visible in its transcript.
    let t = c.state.transcripts.get(&k).unwrap();
    assert!(t.items().iter().any(|i| matches!(&i.body, yaac_protocol::ItemBody::UserMessage(u) if format!("{:?}", u.parts).contains("Write a title and a description"))));
}

#[test]
fn agent_replies_are_read_tolerantly() {
    assert_eq!(parse_agent_draft("Title: Add X\n\nDescription:\nBody line\n- a"), Some(("Add X".into(), "Body line\n- a".into())));
    assert_eq!(parse_agent_draft("```\n**Title:** `Add X`\n**Description:** One line.\n```"), Some(("Add X".into(), "One line.".into())));
    assert_eq!(parse_agent_draft("## Title\nAdd Y\n\n## Description\nWhy."), Some(("Add Y".into(), "Why.".into())));
    assert_eq!(parse_agent_draft("Sure! Here you go."), None);
}
