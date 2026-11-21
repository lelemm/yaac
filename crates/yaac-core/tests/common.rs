//! Shared helpers for the integration tests.
#![allow(dead_code, unreachable_pub)]

use yaac_core::attention::AttentionMap;
use yaac_core::client::Client;
use yaac_core::demo::{DEMO_CLIENT, DemoHub, Scenario};
use yaac_core::sidebar::{Grouping, RowRight, sections};
use yaac_protocol::ClientId;

pub const NOW: i64 = 1_795_197_600; // 2026-11-20T14:00:00Z

pub fn boot() -> (Client, DemoHub) {
    boot_with(Scenario::Fleet)
}

pub fn boot_with(s: Scenario) -> (Client, DemoHub) {
    let hub = DemoHub::new(s, NOW);
    let mut c = Client::new(Box::new(hub.clone()), "test", ClientId::new(DEMO_CLIENT));
    c.set_warm(hub.all_threads());
    DemoHub::apply_seams(&mut c.state);
    settle(&mut c, &hub);
    (c, hub)
}

pub fn settle(c: &mut Client, hub: &DemoHub) {
    for _ in 0..10_000 {
        let got = c.pump();
        let ticked = hub.tick();
        if !got && !ticked {
            break;
        }
    }
}

pub fn render(c: &Client, g: Grouping) -> Vec<String> {
    render_open(c, g, false)
}

pub fn render_open(c: &Client, g: Grouping, muted_open: bool) -> Vec<String> {
    let map = AttentionMap::compute(&c.state);
    let mut out = Vec::new();
    for s in sections(&c.state, &map, g, "", yaac_core::sidebar::Open { muted: muted_open, archived: muted_open }, NOW, yaac_core::time::Zone::UTC) {
        if let Some(h) = &s.header {
            let mut line = if h.temporary { format!("# _{}_", h.left) } else { format!("# {}", h.left) };
            if let Some(m) = &h.machine {
                line.push_str(&format!(" — {m}"));
            }
            if let Some((_, w)) = &h.connection {
                line.push_str(&format!(" ({w})"));
            }
            out.push(line);
        }
        for r in &s.rows {
            let right = match &r.right {
                RowRight::Age(a) => a.clone(),
                RowRight::Machine { name, muted } => if *muted { format!("{name} muted") } else { name.clone() },
                RowRight::Until(u) => u.clone(),
            };
            let mut line = format!("{} ({}) {}", r.title, r.display.mark.word(), right);
            if let Some(l2) = r.line2.as_ref().filter(|l| !l.branch.is_empty()) {
                line.push_str(&format!(" / {}", l2.branch));
                for (sym, n) in [("+", l2.added), ("~", l2.modified), ("-", l2.deleted), ("^", l2.ahead), ("v", l2.behind)] {
                    if n > 0 {
                        line.push_str(&format!(" {sym}{n}"));
                    }
                }
                if l2.workers > 0 {
                    line.push_str(&format!(" {}w", l2.workers));
                }
            }
            out.push(line);
        }
    }
    out
}
