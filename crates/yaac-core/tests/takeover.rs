//! Take over (D140): the read-only banner, the thread menu, and the Commander command.

mod common;

use common::{NOW, boot, settle};
use yaac_core::attention::AttentionMap;
use yaac_core::client::Client;
use yaac_core::commands::{Avail, Cmd, Ctx, availability};
use yaac_core::demo::threads;
use yaac_core::header::{Banner, header};
use yaac_core::sidebar::{Grouping, Ordering};
use yaac_core::thread_menu::{MenuAction, TAKE_OVER_TIP, TakeOver, thread_menu};
use yaac_core::transcript::TakeOverRequest;
use yaac_core::ThreadKey;
use yaac_protocol::{ItemBody, NoticeKind};

fn ctx<'a>(state: &'a yaac_core::state::AppState, thread: Option<&'a ThreadKey>) -> Ctx<'a> {
    Ctx {
        state,
        now: NOW,
        thread,
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
        terminal: Default::default(),
    }
}

fn banner(c: &Client, k: &ThreadKey) -> Option<(String, Option<TakeOver>)> {
    let map = AttentionMap::compute(&c.state);
    header(&c.state, &map, k)?.banners.into_iter().find_map(|b| match b {
        Banner::InUseElsewhere { text, take_over } => Some((text, take_over)),
        _ => None,
    })
}

fn menu_take_over(c: &Client, k: &ThreadKey) -> Option<(bool, Option<&'static str>)> {
    thread_menu(&c.state, k, NOW).iter().filter_map(|e| e.item()).find(|i| i.action == MenuAction::TakeOver).map(|i| (i.enabled, i.reason))
}

#[test]
fn the_banner_offers_take_over_beside_fork_with_its_consequence() {
    let (mut c, hub) = boot();
    let hs = threads::handshake();
    c.open(&hs);
    settle(&mut c, &hub);
    let (text, t) = banner(&c, &hs).expect("the read-only banner");
    assert_eq!(text, "This thread is open in another program on devbox. You can watch, but not send.");
    let t = t.expect("Take over is offered");
    assert!(t.enabled && !t.busy);
    assert_eq!(t.tooltip, TAKE_OVER_TIP);
    assert_eq!(t.tooltip, "Continue here. The other program is not stopped and will not see what you send from now on.");
    assert_eq!(menu_take_over(&c, &hs), Some((true, None)));
    assert_eq!(availability(Cmd::TakeOver, &ctx(&c.state, Some(&hs))), Avail::Enabled);

    // A thread nobody else holds has no Take over: the command says why.
    let fix = threads::fix();
    assert_eq!(menu_take_over(&c, &fix), None);
    assert!(matches!(availability(Cmd::TakeOver, &ctx(&c.state, Some(&fix))), Avail::Disabled(_)));
    assert_eq!(availability(Cmd::TakeOver, &ctx(&c.state, None)), Avail::Hidden);
}

#[test]
fn a_holder_that_will_not_let_go_disables_take_over_with_its_reason() {
    let (mut c, hub) = boot();
    let hs = threads::handshake();
    c.open(&hs);
    settle(&mut c, &hub);
    hub.block_take_over(&hs, Some("claude Code runs it as a background session; stop it there first"));
    settle(&mut c, &hub);
    let t = banner(&c, &hs).unwrap().1.unwrap();
    assert!(!t.enabled);
    assert_eq!(t.tooltip, "Claude Code runs it as a background session; stop it there first");
    assert_eq!(menu_take_over(&c, &hs), Some((false, Some("The other program will not hand this thread over now"))));
    assert!(matches!(availability(Cmd::TakeOver, &ctx(&c.state, Some(&hs))), Avail::Disabled(_)));

    // Asked anyway (an out-of-date window): the reason shows, nothing is re-sent.
    c.take_over(&hs);
    settle(&mut c, &hub);
    let t = banner(&c, &hs).unwrap().1.unwrap();
    assert_eq!(t.failure.as_deref(), Some("claude Code runs it as a background session; stop it there first"));
    assert_eq!(hub.requests().iter().filter(|(_, m, _)| m == "thread/takeOver").count(), 1, "never retried");
    assert!(!c.state.may_act(&hs));
}

#[test]
fn take_over_shows_progress_then_the_banner_goes_and_the_composer_unlocks() {
    let (mut c, hub) = boot();
    let hs = threads::handshake();
    c.open(&hs);
    settle(&mut c, &hub);
    c.take_over(&hs);
    // In flight: the button shows progress and cannot be pressed twice.
    let t = banner(&c, &hs).unwrap().1.unwrap();
    assert!(t.busy && !t.enabled);
    assert_eq!(c.state.transcripts.get(&hs).unwrap().take_over, Some(TakeOverRequest::InFlight));
    c.take_over(&hs);
    let sent: Vec<_> = hub.requests().into_iter().filter(|(_, m, _)| m == "thread/takeOver").collect();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].2["opId"].as_str().is_some());
    settle(&mut c, &hub);
    assert!(banner(&c, &hs).is_none(), "the banner goes away");
    assert!(c.state.may_act(&hs));
    assert_eq!(menu_take_over(&c, &hs), None);
    let notice = c.state.transcripts.get(&hs).unwrap().items().iter().find_map(|i| match &i.body {
        ItemBody::Notice(n) if n.notice_kind == NoticeKind::TakenOver => Some(n.text.clone()),
        _ => None,
    });
    assert_eq!(notice.as_deref(), Some("Taken over from a terminal session · the other program is still open and will not see these messages"));
}
