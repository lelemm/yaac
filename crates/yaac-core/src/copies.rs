//! A worker's request, shown in every ancestor up to the root (P354,
//! docs/10-agent-mesh.md#a-worker-that-is-blocked-must-be-seen).
//!
//! A request raised in a worker — a YAAC mesh worker on any target, or a harness's own
//! sub-agent (D151) — is **one request**, journaled on the worker's thread. Its parent, the
//! parent's parent and so on up to the root show a *copy*: a view of the worker's request
//! item, never an item of their own. Nothing here is on the wire; the window derives copies
//! from what it already has:
//!
//! - the parent links of the merged thread index (`spawnedBy`, on any target), and
//! - the worker's own transcript, which the window follows
//!   only while an ancestor is the open thread and the worker needs the user.
//!
//! Answering a copy is the ordinary `request/respond` for the worker's thread and request id
//! (through the hub for a worker on another target). Every copy reads the worker's one item,
//! so all of them turn resolved together. A copy never opens a turn on the ancestor, adds
//! nothing to its attention, and never dims its composer (D162).
//!
//! **Where copies sit.** An open copy stays at the end of the ancestor's transcript, below
//! everything the ancestor itself has — the ancestor keeps working (D162), and a worker that is
//! blocked must stay in sight. Open copies stack nearest descendant first, then by arrival.
//! Once answered, a copy stays where it was answered: it is placed by the time of its answer,
//! so it does not move at that moment and later items of the ancestor flow below it. A copy is
//! shown resolved only for requests this window showed open as a copy and while it holds the
//! worker's transcript; the worker's own transcript keeps the request and its answer forever.

use std::collections::{BTreeMap, BTreeSet};

use yaac_protocol::{AnsweredBy, ItemBody, ItemId, RequestId, RequestItem, RequestState, ThreadState};

use crate::activity::Block;
use crate::hints::request_is_open;
use crate::key::ThreadKey;
use crate::state::AppState;
use crate::transcript::Transcript;

/// What the window keeps for copies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CopyState {
    /// Reconsider subscriptions after a request, index change, or history snapshot.
    pub(crate) dirty: bool,
    /// The workers whose transcripts the open thread's copies need. Their transcripts are not
    /// evicted while they are listed here.
    pub sources: BTreeSet<ThreadKey>,
    /// Subscriptions opened solely to display a copy. A normal open takes ownership.
    pub(crate) following: BTreeSet<ThreadKey>,
    /// Cached worker transcripts whose copy-only subscription was released.
    pub(crate) detached: BTreeSet<ThreadKey>,
    /// Requests this window has shown open as a copy, by worker and item: once answered they
    /// stay shown, resolved, instead of vanishing.
    pub shown: BTreeSet<(ThreadKey, ItemId)>,
    /// Where this window answered a request from a copy: the ancestor whose copy was used, by
    /// worker and request id.
    pub answered_in: BTreeMap<(ThreadKey, RequestId), ThreadKey>,
}

/// One copy of a worker's request in an ancestor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestCopy {
    /// The thread that raised the request.
    pub worker: ThreadKey,
    /// Its request item, in the worker's transcript.
    pub item: ItemId,
    /// The request id, for `request/respond` on the worker's thread.
    pub request: RequestId,
    /// 1 for a child of the ancestor, 2 for a grandchild, …
    pub depth: usize,
    /// Still waiting for the user (pending, submitted, or outcome unknown).
    pub open: bool,
    /// When it was raised (milliseconds), for the order of open copies.
    pub raised: Option<i64>,
    /// When it was answered (milliseconds), where a resolved copy is placed.
    pub answered: Option<i64>,
}

impl RequestCopy {
    /// The copy's own id in the ancestor's view: its row, its focus. Never the worker's item id
    /// alone — two threads' item ids may be equal.
    pub fn id(&self) -> ItemId {
        copy_id(&self.worker, &self.item)
    }
}

/// The id of the copy of `worker`'s item `item`.
pub fn copy_id(worker: &ThreadKey, item: &ItemId) -> ItemId {
    let address = worker.address();
    ItemId::new(format!("~copy:{}:{address}:{item}", address.as_str().len()))
}

/// A thread's parent, when the parent is listed in this window (on any target).
pub fn parent(state: &AppState, key: &ThreadKey) -> Option<ThreadKey> {
    state.summary(key)?.spawned_by.as_ref().and_then(|a| state.key_of(a))
}

/// A thread's ancestors, nearest first, up to its root.
pub fn ancestors(state: &AppState, key: &ThreadKey) -> Vec<ThreadKey> {
    let mut out: Vec<ThreadKey> = Vec::new();
    let mut at = key.clone();
    while let Some(p) = parent(state, &at) {
        if &p == key || out.contains(&p) {
            break;
        }
        out.push(p.clone());
        at = p;
    }
    out
}

/// How far below `ancestor` a thread is (1: its child), or `None` when it is not beneath it.
pub fn depth_below(state: &AppState, key: &ThreadKey, ancestor: &ThreadKey) -> Option<usize> {
    ancestors(state, key).iter().position(|a| a == ancestor).map(|i| i + 1)
}

/// Whether a thread is known to be waiting for the user on a request: its hints when the
/// window has seen its events, else its summary.
fn needs_user(state: &AppState, key: &ThreadKey) -> bool {
    let Some(rec) = state.threads.get(key) else { return false };
    match state.hints.get(key) {
        Some(h) if h.stamp > rec.stamp => h.open_request().is_some(),
        Some(h) if h.open_request().is_some() => true,
        _ => rec.summary.state == ThreadState::NeedsAttention,
    }
}

/// The descendants of `ancestor` whose requests it may show, with their depth: those waiting
/// for the user, and those still awaiting acknowledgement of an answer. Nearest first.
pub fn sources(state: &AppState, ancestor: &ThreadKey) -> Vec<(ThreadKey, usize)> {
    // A summary can switch to running just before the worker confirms a submitted answer.
    let waiting: BTreeSet<&ThreadKey> = state.copies.shown.iter().filter_map(|(k, id)| {
        let item = state.transcripts.get(k)?.item(id)?;
        matches!(&item.body, ItemBody::Request(r) if request_is_open(&r.state)).then_some(k)
    }).collect();
    let mut out: Vec<(ThreadKey, usize)> = state
        .threads
        .iter()
        .filter(|(k, r)| *k != ancestor && r.summary.spawned_by.is_some())
        .filter(|(k, _)| waiting.contains(k) || needs_user(state, k))
        .filter_map(|(k, _)| depth_below(state, k, ancestor).map(|d| (k.clone(), d)))
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Whether a request is one the user answers: not a tool approval `yaacd` gave itself (P342).
fn for_the_user(r: &RequestItem) -> bool {
    !r.resolution.as_ref().is_some_and(|res| res.by == AnsweredBy::Yaacd)
}

// Journal timestamps may differ only in their fractional seconds.
fn millis(stamp: &yaac_protocol::Timestamp) -> Option<i64> {
    let secs = crate::time::secs(stamp)?;
    let fraction = stamp.as_str().get(19..)?.strip_prefix('.').unwrap_or_default();
    let ms: i64 = fraction.bytes().take(3).take_while(u8::is_ascii_digit)
        .zip([100, 10, 1]).map(|(digit, scale)| i64::from(digit - b'0') * scale).sum();
    Some(secs * 1000 + ms)
}

/// The copies `ancestor` shows: open ones nearest descendant first, then by arrival; then the
/// resolved ones this window showed open, by the time of their answer.
pub fn copies(state: &AppState, ancestor: &ThreadKey) -> Vec<RequestCopy> {
    let mut out = Vec::new();
    for (key, t) in &state.transcripts {
        let Some(depth) = depth_below(state, key, ancestor) else { continue };
        let worker = key.clone();
        let Some(hints) = state.hints.get(key) else { continue };
        let mut indices: Vec<_> = hints.requests.keys().filter_map(|id| t.index_of(id)).collect();
        indices.sort_unstable();
        for index in indices {
            let item = &t.items()[index];
            let ItemBody::Request(r) = &item.body else { continue };
            if !for_the_user(r) {
                continue;
            }
            let open = request_is_open(&r.state);
            if !open && !state.copies.shown.contains(&(worker.clone(), item.id.clone())) {
                continue;
            }
            let answered = r
                .resolution
                .as_ref()
                .and_then(|res| res.at.as_ref())
                .or(item.completed_at.as_ref())
                .and_then(millis);
            out.push(RequestCopy {
                worker: worker.clone(),
                item: item.id.clone(),
                request: r.request_id.clone(),
                depth,
                open,
                raised: item.started_at.as_ref().and_then(millis),
                answered,
            });
        }
    }
    out.sort_by(|a, b| {
        b.open
            .cmp(&a.open)
            .then_with(|| if a.open { a.depth.cmp(&b.depth) } else { std::cmp::Ordering::Equal })
            .then_with(|| if a.open { a.raised.cmp(&b.raised) } else { a.answered.cmp(&b.answered) })
    });
    out
}

/// The copy with this view id in `ancestor`, if it shows one.
pub fn find(state: &AppState, ancestor: &ThreadKey, id: &ItemId) -> Option<RequestCopy> {
    if !id.as_str().starts_with("~copy:") {
        return None;
    }
    copies(state, ancestor).into_iter().find(|c| &c.id() == id)
}

/// The worker's request item and its transcript, for a copy.
pub fn request_of<'a>(state: &'a AppState, copy: &RequestCopy) -> Option<(&'a Transcript, &'a yaac_protocol::Item, &'a RequestItem)> {
    let t = state.transcripts.get(&copy.worker)?;
    let item = t.item(&copy.item)?;
    let ItemBody::Request(r) = &item.body else { return None };
    Some((t, item, r))
}

/// The first open copy in `ancestor` of a request of `worker`: where Needs you and Ctrl+J land
/// when `ancestor` is the open thread.
pub fn open_copy_of(state: &AppState, ancestor: &ThreadKey, worker: &ThreadKey) -> Option<RequestCopy> {
    copies(state, ancestor).into_iter().filter(|c| c.open && &c.worker == worker)
        .min_by_key(|c| request_of(state, c).is_none_or(|(_, _, r)| r.state != RequestState::Pending))
}

/// The pending copies an ancestor shows, as the Commander's request commands see them:
/// (worker, copy id, request item).
pub fn pending<'a>(state: &'a AppState, ancestor: &ThreadKey) -> Vec<(ThreadKey, ItemId, &'a RequestItem)> {
    copies(state, ancestor)
        .into_iter()
        .filter(|c| c.open)
        .filter_map(|c| {
            let (_, _, r) = request_of(state, &c)?;
            (r.state == RequestState::Pending).then(|| (c.worker.clone(), c.id(), r))
        })
        .collect()
}

/// The header of a copy: "From index-rebuild · Codex on gpu-box".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyHeader {
    /// The worker's title: the link that opens it.
    pub title: String,
    /// "Codex on gpu-box" (the harness is left out when its instance is not listed).
    pub rest: String,
}

impl CopyHeader {
    /// The whole line.
    pub fn text(&self) -> String {
        format!("From {} · {}", self.title, self.rest)
    }
}

/// The header of a copy of `worker`'s request.
pub fn header(state: &AppState, worker: &ThreadKey) -> CopyHeader {
    let title = state.summary(worker).map_or_else(|| worker.thread.to_string(), |s| s.title.clone());
    let target = state.target_name(&worker.target);
    let rest = match state.provider_of(worker) {
        Some(p) => format!("{} on {target}", p.display_name),
        None => format!("on {target}"),
    };
    CopyHeader { title, rest }
}

/// Where the user answered a request in this window, said on a card in `viewing` (a copy or
/// the worker's own card): "Answered here", "Answered in search-service". `None` when this
/// window did not answer it (the card's own text says where it was answered then), or when
/// it was answered on the worker's own card and `viewing` is that worker.
pub fn answered_where(state: &AppState, worker: &ThreadKey, item: &RequestItem, viewing: &ThreadKey) -> Option<String> {
    let mine = match &item.state {
        RequestState::Submitted => true,
        RequestState::Resolved => item
            .resolution
            .as_ref()
            .is_some_and(|r| r.by == AnsweredBy::Client && r.client_id.is_some() && r.client_id == state.client_id),
        _ => false,
    };
    if !mine {
        return None;
    }
    let place = state.copies.answered_in.get(&(worker.clone(), item.request_id.clone()))?;
    if place == viewing {
        return (place != worker).then(|| "Answered here".to_owned());
    }
    let title = state.summary(place).map_or_else(|| place.thread.to_string(), |s| s.title.clone());
    Some(format!("Answered in {title}"))
}

/// Milliseconds when a block of `t` was written, from its first item.
fn block_time(b: &Block, t: &Transcript) -> Option<i64> {
    let of = |i: usize| t.items().get(i).and_then(|x| x.started_at.as_ref().or(x.completed_at.as_ref())).and_then(millis);
    let of_turn = |turn: &yaac_protocol::TurnId| t.items().iter().position(|i| &i.turn_id == turn).and_then(of);
    match b {
        Block::User { index } | Block::Task { index } | Block::Prose { index } | Block::Card { index } | Block::Handoff { index } => of(*index),
        Block::Run { lines } => lines.first().and_then(|l| of(l.index)),
        Block::Folded { first, .. } => t.index_of(first).and_then(of),
        Block::TurnChanges { turn, .. } | Block::Worked { turn, .. } => of_turn(turn),
        Block::Copy { .. } => None,
    }
}

/// Put an ancestor's copies among its blocks (after [`crate::turns::collapse`]): a resolved
/// copy before the first block written after its answer, the open ones at the end.
fn place(mut blocks: Vec<Block>, t: &Transcript, copies: &[RequestCopy]) -> Vec<Block> {
    if copies.is_empty() {
        return blocks;
    }
    let block = |c: &RequestCopy| Block::Copy { worker: c.worker.clone(), item: c.item.clone() };
    if copies.iter().all(|c| c.open) {
        blocks.extend(copies.iter().map(block));
        return blocks;
    }
    let times: Vec<Option<i64>> = blocks.iter().map(|b| block_time(b, t)).collect();
    // Resolved copies are sorted by answer time, so each goes at or after the one before.
    let mut slots: Vec<Vec<Block>> = vec![Vec::new(); blocks.len() + 1];
    for c in copies.iter().filter(|c| !c.open) {
        let at = match c.answered {
            Some(when) => times.iter().position(|bt| bt.is_some_and(|bt| bt > when)).unwrap_or(blocks.len()),
            None => blocks.len(),
        };
        slots[at].push(block(c));
    }
    let mut out = Vec::with_capacity(blocks.len() + copies.len());
    for (i, b) in blocks.into_iter().enumerate() {
        out.append(&mut slots[i]);
        out.push(b);
    }
    out.append(&mut slots.pop().unwrap_or_default());
    out.extend(copies.iter().filter(|c| c.open).map(block));
    out
}

/// An ancestor's blocks with its copies placed among them: what both renderers draw.
pub fn with_copies(state: &AppState, ancestor: &ThreadKey, blocks: Vec<Block>) -> Vec<Block> {
    let Some(t) = state.transcripts.get(ancestor) else { return blocks };
    place(blocks, t, &copies(state, ancestor))
}
