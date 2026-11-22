//! The rate-limit panel (D161) through the client and the demo hub: read on attaching, the
//! panel built from it, and refreshes asked at most every five minutes while it is drawn.

mod common;

use common::{NOW, boot, boot_with, settle};
use yaac_core::demo::Scenario;
use yaac_core::limits::{self, Segment};
use yaac_core::time::Zone;
use yaac_protocol::attach;

fn asks(hub: &yaac_core::demo::DemoHub) -> Vec<bool> {
    hub.requests()
        .into_iter()
        .filter(|(t, m, _)| t.is_none() && m == attach::names::RATE_LIMITS_GET)
        .map(|(_, _, p)| p["refresh"].as_bool().unwrap_or(false))
        .collect()
}

#[test]
fn the_accounts_are_read_on_attaching_and_make_the_panel() {
    let (c, hub) = boot();
    assert_eq!(asks(&hub), vec![false]);
    let names = |t: &yaac_protocol::TargetId| c.state.target(t).map_or_else(|| t.to_string(), |r| r.entry.name.to_string());
    let p = limits::panel(&c.state.rate_limits.accounts, &names, NOW, Zone::UTC);
    assert_eq!(p.groups.len(), 2);
    let claude = &p.groups[0];
    assert_eq!(claude.kind.as_str(), "claude");
    assert_eq!(claude.bars.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), vec!["100% · 53m", "55% · 2d"]);
    assert_eq!(claude.bars[1].segments, [Segment::Full, Segment::Full, Segment::Partial, Segment::Empty, Segment::Empty]);
    assert!(claude.bars[0].near && !claude.bars[1].near);
    assert!(claude.tooltip.contains("Used on local, devbox"), "{}", claude.tooltip);
    assert_eq!(p.groups[1].bars[0].text, "55% · 4d");
}

#[test]
fn a_refresh_is_asked_only_every_five_minutes_while_the_panel_is_drawn() {
    let (mut c, hub) = boot();
    c.rate_limits_shown(NOW);
    c.rate_limits_shown(NOW + 60);
    settle(&mut c, &hub);
    assert_eq!(asks(&hub), vec![false]);
    c.rate_limits_shown(NOW + limits::REFRESH_SECS);
    c.rate_limits_shown(NOW + limits::REFRESH_SECS + 1);
    settle(&mut c, &hub);
    assert_eq!(asks(&hub), vec![false, true]);
}

#[test]
fn a_fresh_install_has_no_panel() {
    let (c, _hub) = boot_with(Scenario::Fresh);
    assert!(limits::panel(&c.state.rate_limits.accounts, &|t| t.to_string(), NOW, Zone::UTC).groups.is_empty());
}
