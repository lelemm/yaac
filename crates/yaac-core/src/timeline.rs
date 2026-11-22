//! The timeline at the transcript's left edge (D153): one short dash per prompt, evenly
//! spaced, the dashes of what is in view highlighted, so the column reads as a map of where
//! the view is in the conversation. Hovering a dash names its prompt; clicking it scrolls
//! there.
//!
//! Pure: the window measures where each prompt is and where the view is, and asks this module
//! which dashes to draw, which to highlight, and which prompt a pointer or a key goes to.

use std::ops::Range;

use yaac_protocol::{ItemBody, ItemId};

use crate::activity::Block;
use crate::transcript::Transcript;

/// Spacing between dashes when there is room.
pub const PITCH: f32 = 8.0;
/// The closest dashes are drawn; with more prompts than fit, neighbours merge into one dash.
pub const MIN_PITCH: f32 = 3.0;
/// A thread with fewer prompts than this has no timeline.
pub const MIN_ENTRIES: usize = 2;
/// Going to a prompt puts its top this far below the top of the view.
pub const MARGIN: f32 = 16.0;
/// The longest prompt title a hover card shows.
pub const TITLE_CHARS: usize = 80;

/// One prompt: a message the user typed, or the task that started a spawned thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The message.
    pub item: ItemId,
    /// Its index in the transcript.
    pub index: usize,
    /// Its first line, ellipsised.
    pub title: String,
    /// When it was sent (seconds since the epoch).
    pub at: Option<i64>,
}

fn title_of(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = crate::message::one_line(line.trim_start_matches('#').trim());
    if line.chars().count() <= TITLE_CHARS {
        return line;
    }
    let mut s: String = line.chars().take(TITLE_CHARS - 1).collect();
    s.push('…');
    s
}

/// The prompts among the blocks shown, oldest first. A message that is only the harness's
/// context (no bubble) and messages between agents are not prompts.
pub fn entries(t: &Transcript, blocks: &[Block]) -> Vec<Entry> {
    let mut out = Vec::new();
    for b in blocks {
        let (Block::User { index } | Block::Task { index }) = b else { continue };
        let Some(item) = t.items().get(*index) else { continue };
        let text = match &item.body {
            ItemBody::UserMessage(u) if crate::message::context_only(u) => continue,
            ItemBody::UserMessage(u) => {
                let typed = crate::message::copy_text(u);
                if typed.trim().is_empty() { "Attachments".to_owned() } else { typed }
            }
            ItemBody::MeshReceived(r) => r.message.text.clone(),
            _ => continue,
        };
        let at = item.started_at.as_ref().or(item.completed_at.as_ref()).and_then(crate::time::secs);
        out.push(Entry { item: item.id.clone(), index: *index, title: title_of(&text), at });
    }
    out
}

/// One dash: the prompts `first..=last` (one, unless the column is too short for all).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dash {
    /// The first prompt it stands for.
    pub first: usize,
    /// The last.
    pub last: usize,
    /// Its centre, from the top of the column.
    pub y: f32,
}

/// Where the dashes go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// The dashes, top to bottom.
    pub dashes: Vec<Dash>,
    /// The distance between two dashes.
    pub pitch: f32,
}

/// Lay out `n` prompts in a column `height` tall: evenly spaced at [`PITCH`], closer together
/// when they do not fit, and — below [`MIN_PITCH`] — neighbours merged so each dash stands for
/// an equal share. The dashes sit centred in the column. Fewer than [`MIN_ENTRIES`]: none.
pub fn layout(n: usize, height: f32) -> Layout {
    if n < MIN_ENTRIES || height <= 0.0 {
        return Layout::default();
    }
    #[allow(clippy::cast_precision_loss, reason = "counts of prompts are far below 2^24")]
    let fit = height / n as f32;
    let (m, pitch) = if fit >= MIN_PITCH {
        (n, fit.min(PITCH))
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a positive height in points")]
        let m = ((height / MIN_PITCH).floor() as usize).max(1);
        (m, MIN_PITCH)
    };
    #[allow(clippy::cast_precision_loss, reason = "a few hundred dashes")]
    let top = (height - pitch * (m - 1) as f32) / 2.0;
    let dashes = (0..m)
        .map(|i| {
            let first = i * n / m;
            let last = ((i + 1) * n / m).max(first + 1) - 1;
            #[allow(clippy::cast_precision_loss, reason = "a few hundred dashes")]
            let y = top + pitch * i as f32;
            Dash { first, last, y }
        })
        .collect();
    Layout { dashes, pitch }
}

/// The prompts in view: prompt `k` covers the transcript from its top (`tops[k]`) to the next
/// prompt's top; those overlapping `view_top..view_bottom` are in view.
pub fn visible(tops: &[f32], view_top: f32, view_bottom: f32) -> Range<usize> {
    if tops.is_empty() {
        return 0..0;
    }
    let first = tops.iter().rposition(|&t| t <= view_top).unwrap_or(0);
    let end = tops.iter().rposition(|&t| t < view_bottom).map_or(first + 1, |l| l + 1).max(first + 1);
    first..end
}

/// Whether a dash stands for a prompt in view.
pub fn lit(d: &Dash, in_view: &Range<usize>) -> bool {
    d.first < in_view.end && in_view.start <= d.last
}

/// The dash under a pointer `y` points below the column's top, if any: the nearest one within
/// half a pitch (at least 4 points, so a dense column is still easy to hit).
pub fn dash_at(l: &Layout, y: f32) -> Option<usize> {
    let reach = (l.pitch / 2.0).max(4.0);
    l.dashes
        .iter()
        .enumerate()
        .map(|(i, d)| (i, (d.y - y).abs()))
        .filter(|(_, dist)| *dist <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Where the view scrolls to show a prompt whose top is at `top`.
pub fn scroll_target(top: f32) -> f32 {
    (top - MARGIN).max(0.0)
}

/// The prompt "Go to previous message" goes to: the start of the prompt the view is in, or —
/// when the view is already at its start — the one before.
pub fn prev(tops: &[f32], view_top: f32) -> Option<usize> {
    tops.iter().rposition(|&t| scroll_target(t) < view_top - 1.0)
}

/// The prompt "Go to next message" goes to: the first one below the view's top.
pub fn next(tops: &[f32], view_top: f32) -> Option<usize> {
    tops.iter().position(|&t| scroll_target(t) > view_top + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::build::{agent, done, message, received, user};
    use yaac_protocol::{Address, InputPart, MeshMessageKind, MeshPeer, TargetId, TextRole, ThreadId, UserMessageItem};

    #[test]
    fn one_dash_per_prompt_evenly_spaced_and_centred() {
        let l = layout(5, 100.0);
        assert_eq!(l.dashes.len(), 5);
        assert!((l.pitch - PITCH).abs() < f32::EPSILON);
        let ys: Vec<f32> = l.dashes.iter().map(|d| d.y).collect();
        assert_eq!(ys, [34.0, 42.0, 50.0, 58.0, 66.0]);
        assert!(l.dashes.iter().enumerate().all(|(i, d)| d.first == i && d.last == i));
        assert!(layout(1, 100.0).dashes.is_empty(), "hidden below two prompts");
        assert!(layout(0, 100.0).dashes.is_empty());
    }

    #[test]
    fn a_dense_thread_compresses_then_merges() {
        // 60 prompts in 300 points: closer together, still one each.
        let l = layout(60, 300.0);
        assert_eq!(l.dashes.len(), 60);
        assert!((l.pitch - 5.0).abs() < 1e-4);
        assert!(l.dashes.last().unwrap().y <= 300.0);
        // 5000 prompts: 100 dashes, each an equal share, covering every prompt once.
        let l = layout(5000, 300.0);
        assert_eq!(l.dashes.len(), 100);
        assert_eq!(l.dashes[0].first, 0);
        assert_eq!(l.dashes.last().unwrap().last, 4999);
        for w in l.dashes.windows(2) {
            assert_eq!(w[0].last + 1, w[1].first);
        }
        assert!(l.dashes.iter().all(|d| d.last - d.first + 1 == 50));
        // Uneven shares still cover everything.
        let l = layout(101, 30.0);
        assert_eq!(l.dashes.len(), 10);
        assert_eq!(l.dashes.last().unwrap().last, 100);
    }

    #[test]
    fn what_is_in_view_is_highlighted() {
        let tops = [0.0, 400.0, 900.0, 1000.0, 2000.0];
        assert_eq!(visible(&tops, 0.0, 300.0), 0..1);
        assert_eq!(visible(&tops, 450.0, 950.0), 1..3);
        assert_eq!(visible(&tops, 950.0, 1500.0), 2..4);
        assert_eq!(visible(&tops, 2500.0, 3000.0), 4..5);
        assert_eq!(visible(&[], 0.0, 100.0), 0..0);
        let l = layout(5, 100.0);
        let lit_now: Vec<bool> = l.dashes.iter().map(|d| lit(d, &(1..3))).collect();
        assert_eq!(lit_now, [false, true, true, false, false]);
        // A merged dash is lit when any of its prompts is in view.
        let l = layout(5000, 300.0);
        assert!(lit(&l.dashes[1], &(99..100)));
        assert!(!lit(&l.dashes[1], &(100..101)));
    }

    #[test]
    fn the_pointer_finds_the_nearest_dash() {
        let l = layout(5, 100.0);
        assert_eq!(dash_at(&l, 42.0), Some(1));
        assert_eq!(dash_at(&l, 45.5), Some(1));
        assert_eq!(dash_at(&l, 46.5), Some(2));
        assert_eq!(dash_at(&l, 10.0), None);
        assert_eq!(dash_at(&l, 80.0), None);
        let dense = layout(60, 120.0);
        assert!(dense.pitch < 8.0);
        assert!(dash_at(&dense, dense.dashes[30].y + 1.0) == Some(30));
    }

    #[test]
    fn previous_and_next_message() {
        let tops = [12.0, 400.0, 900.0];
        // Reading inside the second prompt: back goes to its start, then to the first.
        assert_eq!(prev(&tops, 500.0), Some(1));
        assert_eq!(prev(&tops, 400.0 - MARGIN), Some(0));
        assert_eq!(prev(&tops, 0.0), None);
        assert_eq!(next(&tops, 0.0), Some(1));
        assert_eq!(next(&tops, 400.0 - MARGIN), Some(2));
        assert_eq!(next(&tops, 900.0 - MARGIN), None);
    }

    #[test]
    fn prompts_are_the_users_messages_and_a_spawned_threads_task() {
        let mut t = Transcript::new();
        let a = Address::new(&TargetId::new("local"), &ThreadId::new("o"));
        let b = Address::new(&TargetId::new("local"), &ThreadId::new("w"));
        let peer = MeshPeer { address: a.clone(), title: None, provider: None, model: None, target_name: None, reachable: None };
        let long = "x".repeat(200);
        let items = vec![
            done("task", "t0", received(message("m", &a, &b, MeshMessageKind::Message, "Migrate users.\nThen report.", crate::time::timestamp(5)), peer)),
            done("u1", "t1", user("## Why does decode() drop\nthe last frame?", &[], &[])),
            done("a1", "t1", agent("Because.")),
            done("ctx", "t2", yaac_protocol::ItemBody::UserMessage(UserMessageItem { parts: vec![InputPart::text_as("reminder", TextRole::Context, None)], op_id: None })),
            done("u2", "t3", user(&long, &[], &[])),
        ];
        t.replace_all(items, 1);
        let blocks = vec![
            Block::Task { index: 0 },
            Block::User { index: 1 },
            Block::Prose { index: 2 },
            Block::User { index: 3 },
            Block::User { index: 4 },
        ];
        let e = entries(&t, &blocks);
        let titles: Vec<&str> = e.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles[..2], ["Migrate users.", "Why does decode() drop"]);
        assert_eq!(e.len(), 3, "a context-only message is not a prompt");
        assert_eq!(e[2].title.chars().count(), TITLE_CHARS);
        assert!(e[2].title.ends_with('…'));
        assert_eq!(e[1].index, 1);
    }
}
