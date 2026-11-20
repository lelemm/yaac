//! Finding a thread across every target, over the cached thread index
//! (docs/10-agent-mesh.md, `find_thread`): by id first, then by a fuzzy title match.
//!
//! - A query that **looks like an id** is tried as one: the YAAC thread id, the harness's own
//!   conversation id (`nativeId`), or an address (`target/thread`). A full match wins; otherwise
//!   every thread one of whose ids starts with the query (at least [`MIN_PREFIX`] characters) is
//!   a candidate. Several candidates are all returned, marked ambiguous — never a guess.
//! - Otherwise (or when no id matches) titles are matched word by word, tolerating partial
//!   words and typos ("ci integ" finds "CI integration"), ranked by match quality, then by
//!   recency.
//!
//! Results are **paged**: one page of at most [`PAGE_MAX`], the total number of matches, and a
//! cursor for the next page. A cursor is bound to the query and to a snapshot of the matching
//! ids taken on the first call, so paging through an index that changes meanwhile neither
//! repeats nor skips a thread (a thread removed meanwhile is left out; its place is not
//! filled). Cursors expire after [`CURSOR_TTL`].
//!
//! The matching ([`rank`]) is pure: no I/O, so the window, the hub, and the mesh can all use it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use yaac_protocol::{Address, TargetId, ThreadId, ThreadState, ThreadSummary};

/// The shortest id prefix that is matched.
pub const MIN_PREFIX: usize = 4;
/// The default and the largest page.
pub const PAGE_MAX: usize = 10;
/// How long a cursor stays valid.
pub const CURSOR_TTL: Duration = Duration::from_secs(5 * 60);
/// At most this many cursors are kept; the oldest go first.
const MAX_CURSORS: usize = 256;

/// `hub/threads/find` params.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindParams {
    /// An id, an id prefix, an address, or words of a title.
    pub query: String,
    /// Only threads of this target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetId>,
    /// Only threads in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ThreadState>,
    /// Page size (default and largest: [`PAGE_MAX`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The cursor of a previous page; then the other fields must be the same as on the first
    /// call (they are ignored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// How a result matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchKind {
    /// The YAAC thread id.
    Id,
    /// The harness's own conversation id.
    NativeId,
    /// The address.
    Address,
    /// The title.
    Title,
}

/// One result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundThread {
    /// Its target.
    pub target: TargetId,
    /// Its address.
    pub address: Address,
    /// How it matched.
    pub matched: MatchKind,
    /// Whether the id matched in full (rather than by prefix).
    pub exact: bool,
    /// Match quality, 0–1 (1 for an id).
    pub score: f64,
    /// The summary, as cached.
    pub thread: ThreadSummary,
}

/// Every match, best first (the pure part).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ranked {
    /// Best first.
    pub results: Vec<FoundThread>,
    /// The query was an id prefix that matched several threads.
    pub ambiguous: bool,
}

/// `hub/threads/find` result: one page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindResult {
    /// This page, best first.
    pub results: Vec<FoundThread>,
    /// How many threads matched in all (as of the first page).
    pub total: u32,
    /// The query was an id prefix that matched several threads: these are candidates, and
    /// none of them should be taken as the answer.
    pub ambiguous: bool,
    /// The next page, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Whether a query should be tried as an id first: one token, long enough, and shaped like an
/// id (a digit, or one of `_ - / : .`).
pub fn looks_like_id(query: &str) -> bool {
    let q = query.trim();
    q.len() >= MIN_PREFIX
        && !q.chars().any(char::is_whitespace)
        && q.chars().any(|c| c.is_ascii_digit() || matches!(c, '_' | '-' | '/' | ':' | '.'))
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// Levenshtein distance, on characters.
fn distance(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// How well one query word matches one title word, 0–1.
fn word_score(q: &str, t: &str) -> f64 {
    if q == t {
        return 1.0;
    }
    if t.starts_with(q) {
        // A longer covered share of the word scores higher.
        return 0.8 + 0.15 * (q.chars().count() as f64 / t.chars().count() as f64);
    }
    if q.chars().count() >= 3 && t.contains(q) {
        return 0.7;
    }
    let qc: Vec<char> = q.chars().collect();
    let tc: Vec<char> = t.chars().collect();
    let allowed = match qc.len() {
        0..=3 => 0,
        4..=6 => 1,
        _ => 2,
    };
    if allowed == 0 {
        return 0.0;
    }
    // A typo in the whole word, or in the part of it the user has typed so far.
    let whole = distance(&qc, &tc);
    let prefix = if tc.len() > qc.len() { distance(&qc, &tc[..qc.len()]) } else { whole };
    let d = whole.min(prefix);
    if d > allowed {
        return 0.0;
    }
    let base = if d == whole { 0.65 } else { 0.55 };
    base - 0.1 * (d as f64 - 1.0)
}

/// How well a query matches a title, 0–1; 0 when some query word matches nothing.
pub fn title_score(query: &str, title: &str) -> f64 {
    let q = words(query);
    let t = words(title);
    if q.is_empty() || t.is_empty() {
        return 0.0;
    }
    let mut total = 0.0;
    let mut used = vec![false; t.len()];
    for qw in &q {
        let mut best = (0.0, None);
        for (i, tw) in t.iter().enumerate() {
            let s = word_score(qw, tw) - if used[i] { 0.2 } else { 0.0 };
            if s > best.0 {
                best = (s, Some(i));
            }
        }
        let (s, at) = best;
        if s <= 0.0 {
            return 0.0;
        }
        if let Some(i) = at {
            used[i] = true;
        }
        total += s;
    }
    let mut score = total / q.len() as f64;
    // The query's words in the title's order, from its start, are a slightly better match.
    if t.first().is_some_and(|w| word_score(&q[0], w) > 0.0) {
        score += 0.02;
    }
    score.min(1.0)
}

/// Every thread of `threads` (each with its target) that matches `params`, best first.
pub fn rank(threads: &[(TargetId, ThreadSummary)], params: &FindParams) -> Ranked {
    let pool: Vec<&(TargetId, ThreadSummary)> = threads
        .iter()
        .filter(|(target, _)| params.target.as_ref().is_none_or(|t| t == target))
        .filter(|(_, s)| params.state.as_ref().is_none_or(|st| &s.state == st))
        .collect();
    let query = params.query.trim();
    if query.is_empty() {
        return Ranked::default();
    }
    if looks_like_id(query) {
        let mut exact = Vec::new();
        let mut prefix = Vec::new();
        for (target, s) in &pool {
            let address = Address::new(target, &s.id);
            let keys = [
                (MatchKind::Id, s.id.as_str().to_owned()),
                (MatchKind::NativeId, s.native_id.clone().unwrap_or_default()),
                (MatchKind::Address, address.as_str().to_owned()),
            ];
            let found = |full: bool| {
                keys.iter()
                    .find(|(_, k)| !k.is_empty() && if full { k == query } else { k.starts_with(query) })
                    .map(|(kind, _)| *kind)
            };
            let hit = |matched, exact| FoundThread {
                target: target.clone(),
                address: address.clone(),
                matched,
                exact,
                score: 1.0,
                thread: s.clone(),
            };
            if let Some(kind) = found(true) {
                exact.push(hit(kind, true));
            } else if let Some(kind) = found(false) {
                prefix.push(hit(kind, false));
            }
        }
        let (mut hits, ambiguous) = if exact.is_empty() {
            let n = prefix.len();
            (prefix, n > 1)
        } else {
            let n = exact.len();
            (exact, n > 1)
        };
        if !hits.is_empty() {
            hits.sort_by(|a, b| b.thread.updated_at.cmp(&a.thread.updated_at));
            return Ranked { results: hits, ambiguous };
        }
    }
    let mut hits: Vec<FoundThread> = pool
        .iter()
        .filter_map(|(target, s)| {
            let score = title_score(query, &s.title);
            (score > 0.0).then(|| FoundThread {
                target: target.clone(),
                address: Address::new(target, &s.id),
                matched: MatchKind::Title,
                exact: false,
                score,
                thread: s.clone(),
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| b.thread.updated_at.cmp(&a.thread.updated_at))
    });
    Ranked { results: hits, ambiguous: false }
}

/// A cursor's error: it is unknown or has expired (answer `invalid_params`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("this search cursor has expired; search again")]
pub struct CursorExpired;

struct Snapshot {
    /// The matches as of the first page: key, how it matched, its score.
    hits: Vec<(TargetId, ThreadId, MatchKind, bool, f64)>,
    ambiguous: bool,
    page: usize,
    created: Instant,
}

/// Pages through search results with cursors (the hub keeps one).
#[derive(Default)]
pub struct Pager {
    snapshots: HashMap<String, Snapshot>,
    next_id: u64,
}

impl std::fmt::Debug for Pager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pager").field("cursors", &self.snapshots.len()).finish()
    }
}

impl Pager {
    /// One page. Without a cursor, searches `threads` and snapshots the matches; with one,
    /// continues that snapshot, taking each thread's summary as it is now in `threads`.
    pub fn page(&mut self, threads: &[(TargetId, ThreadSummary)], params: &FindParams, now: Instant) -> Result<FindResult, CursorExpired> {
        self.snapshots.retain(|_, s| now.duration_since(s.created) < CURSOR_TTL);
        let (snapshot, offset) = match &params.cursor {
            Some(cursor) => {
                let (id, offset) = cursor.split_once('.').ok_or(CursorExpired)?;
                let offset: usize = offset.parse().map_err(|_| CursorExpired)?;
                let snap = self.snapshots.remove(id).ok_or(CursorExpired)?;
                (snap, offset)
            }
            None => {
                let ranked = rank(threads, params);
                let page = params.limit.map_or(PAGE_MAX, |l| (l as usize).clamp(1, PAGE_MAX));
                let hits = ranked.results.into_iter().map(|f| (f.target, f.thread.id, f.matched, f.exact, f.score)).collect();
                (Snapshot { hits, ambiguous: ranked.ambiguous, page, created: now }, 0)
            }
        };
        let current: HashMap<(&TargetId, &ThreadId), &ThreadSummary> = threads.iter().map(|(t, s)| ((t, &s.id), s)).collect();
        let end = (offset + snapshot.page).min(snapshot.hits.len());
        let results = snapshot.hits[offset.min(end)..end]
            .iter()
            .filter_map(|(target, thread, matched, exact, score)| {
                let summary = current.get(&(target, thread))?;
                Some(FoundThread {
                    target: target.clone(),
                    address: Address::new(target, thread),
                    matched: *matched,
                    exact: *exact,
                    score: *score,
                    thread: (*summary).clone(),
                })
            })
            .collect();
        let total = u32::try_from(snapshot.hits.len()).unwrap_or(u32::MAX);
        let ambiguous = snapshot.ambiguous;
        let cursor = (end < snapshot.hits.len()).then(|| {
            let id = params.cursor.as_ref().and_then(|c| c.split_once('.')).map_or_else(
                || {
                    self.next_id += 1;
                    format!("f{}", self.next_id)
                },
                |(id, _)| id.to_owned(),
            );
            if self.snapshots.len() >= MAX_CURSORS
                && let Some(oldest) = self.snapshots.iter().min_by_key(|(_, s)| s.created).map(|(k, _)| k.clone())
            {
                self.snapshots.remove(&oldest);
            }
            self.snapshots.insert(id.clone(), snapshot);
            format!("{id}.{end}")
        });
        Ok(FindResult { results, total, ambiguous, cursor })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summary(id: &str, native: Option<&str>, title: &str, updated: &str) -> ThreadSummary {
        let mut v = json!({
            "id": id, "instance": "pi_1", "title": title, "cwd": "/r", "state": "idle", "control": "free",
            "originKind": "yaac", "createdAt": "2026-01-01T00:00:00Z", "updatedAt": updated
        });
        if let Some(n) = native {
            v["nativeId"] = json!(n);
        }
        serde_json::from_value(v).unwrap()
    }

    fn fleet() -> Vec<(TargetId, ThreadSummary)> {
        let a = TargetId::new("tg_local");
        let b = TargetId::new("tg_devbox");
        vec![
            (a.clone(), summary("t_aaaa1111", Some("019a-77aa-bb"), "CI integration", "2026-01-05T00:00:00Z")),
            (b.clone(), summary("t_aaaa2222", Some("5f3c9e1d-0000"), "Migrate auth module", "2026-01-04T00:00:00Z")),
            (b, summary("t_bbbb3333", None, "Fix frame decoder", "2026-01-03T00:00:00Z")),
            (a.clone(), summary("t_cccc4444", None, "Explain the handshake", "2026-01-02T00:00:00Z")),
            (a, summary("t_dddd5555", None, "Integration tests for CI", "2026-01-01T00:00:00Z")),
        ]
    }

    fn find(threads: &[(TargetId, ThreadSummary)], params: &FindParams) -> Ranked {
        rank(threads, params)
    }

    fn ids(r: &Ranked) -> Vec<&str> {
        r.results.iter().map(|f| f.thread.id.as_str()).collect()
    }

    #[test]
    fn scoring_table() {
        // (query, title, matches)
        let table: &[(&str, &str, bool)] = &[
            ("ci integ", "CI integration", true),
            ("integration", "CI integration", true),
            ("intgration", "CI integration", true), // one typo
            ("handshak", "Explain the handshake", true),
            ("frame decodr", "Fix frame decoder", true),
            ("auth", "Migrate auth module", true),
            ("xyz", "Migrate auth module", false),
            ("decoder zebra", "Fix frame decoder", false), // every word must match
            ("ci", "Fix frame decoder", false),
            ("", "anything", false),
        ];
        for (q, t, want) in table {
            assert_eq!(title_score(q, t) > 0.0, *want, "{q:?} vs {t:?}: {}", title_score(q, t));
        }
        // An exact word beats a prefix, a prefix beats a typo.
        assert!(title_score("integration", "CI integration") > title_score("integ", "CI integration"));
        assert!(title_score("integ", "CI integration") > title_score("intgration", "CI integration"));
    }

    #[test]
    fn id_table() {
        let threads = fleet();
        // (query, expected ids in order, ambiguous, matched kind of the first)
        let table: &[(&str, &[&str], bool, Option<MatchKind>)] = &[
            ("t_aaaa1111", &["t_aaaa1111"], false, Some(MatchKind::Id)),
            ("t_aaaa", &["t_aaaa1111", "t_aaaa2222"], true, Some(MatchKind::Id)), // never a guess
            ("t_bbbb", &["t_bbbb3333"], false, Some(MatchKind::Id)),
            ("019a-77aa-bb", &["t_aaaa1111"], false, Some(MatchKind::NativeId)),
            ("5f3c", &["t_aaaa2222"], false, Some(MatchKind::NativeId)),
            ("tg_devbox/t_bbbb3333", &["t_bbbb3333"], false, Some(MatchKind::Address)),
            ("tg_devbox/", &["t_aaaa2222", "t_bbbb3333"], true, Some(MatchKind::Address)),
            ("t_zz99", &[], false, None), // looks like an id, matches nothing, no title either
        ];
        for (q, want, ambiguous, kind) in table {
            let r = find(&threads, &FindParams { query: (*q).into(), ..FindParams::default() });
            assert_eq!(ids(&r), *want, "{q}");
            assert_eq!(r.ambiguous, *ambiguous, "{q}");
            assert_eq!(r.results.first().map(|f| f.matched), *kind, "{q}");
        }
        // Too short to be an id prefix: treated as words.
        assert!(!looks_like_id("t_a"));
        assert!(!looks_like_id("ci integ"));
    }

    #[test]
    fn titles_rank_by_quality_then_recency_and_respect_filters() {
        let threads = fleet();
        let r = find(&threads, &FindParams { query: "ci integ".into(), ..FindParams::default() });
        assert_eq!(ids(&r), ["t_aaaa1111", "t_dddd5555"]);
        assert_eq!(r.results[0].target.as_str(), "tg_local");
        let only_devbox = find(&threads, &FindParams { query: "frame".into(), target: Some(TargetId::new("tg_devbox")), ..FindParams::default() });
        assert_eq!(ids(&only_devbox), ["t_bbbb3333"]);
        let none_here = find(&threads, &FindParams { query: "frame".into(), target: Some(TargetId::new("tg_local")), ..FindParams::default() });
        assert!(none_here.results.is_empty());
        let running = find(&threads, &FindParams { query: "frame".into(), state: Some(ThreadState::Running), ..FindParams::default() });
        assert!(running.results.is_empty());
    }

    fn page_ids(r: &FindResult) -> Vec<String> {
        r.results.iter().map(|f| f.thread.id.as_str().to_owned()).collect()
    }

    #[test]
    fn three_pages_over_an_index_that_changes_neither_repeat_nor_skip() {
        let tg = TargetId::new("tg");
        let mk = |i: usize| (tg.clone(), summary(&format!("t_{i:04}"), None, "Same title", &format!("2026-01-{:02}T00:00:00Z", 28 - i)));
        let mut index: Vec<_> = (0..25).map(mk).collect();
        let mut pager = Pager::default();
        let now = Instant::now();
        // The page size is capped whatever the caller asks.
        let p1 = pager.page(&index, &FindParams { query: "same".into(), limit: Some(50), ..FindParams::default() }, now).unwrap();
        assert_eq!(p1.results.len(), PAGE_MAX);
        assert_eq!(p1.total, 25);
        // The index changes: a newer matching thread appears (it would rank first), one already
        // shown is renamed so it no longer matches, and one not yet shown is removed.
        index.push((tg.clone(), summary("t_new0", None, "Same title, newest", "2026-02-01T00:00:00Z")));
        index[0].1.title = "Something else".into();
        index.retain(|(_, s)| s.id.as_str() != "t_0012");
        let p2 = pager.page(&index, &FindParams { cursor: p1.cursor.clone(), ..FindParams::default() }, now).unwrap();
        let p3 = pager.page(&index, &FindParams { cursor: p2.cursor.clone(), ..FindParams::default() }, now).unwrap();
        assert!(p3.cursor.is_none());
        let mut seen: Vec<String> = [&p1, &p2, &p3].iter().flat_map(|p| page_ids(p)).collect();
        let n = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), n, "no thread repeated");
        let expected: Vec<String> = (0..25).filter(|i| *i != 12).map(|i| format!("t_{i:04}")).collect();
        assert_eq!(seen, expected, "none skipped; the removed one left out; the new one not slipped in");
        assert_eq!(p2.total, 25);
        // A used-up or unknown cursor is expired.
        assert_eq!(pager.page(&index, &FindParams { cursor: Some("f999.10".into()), ..FindParams::default() }, now), Err(CursorExpired));
        // Cursors expire.
        let p1 = pager.page(&index, &FindParams { query: "same".into(), ..FindParams::default() }, now).unwrap();
        let late = now + CURSOR_TTL + Duration::from_secs(1);
        assert_eq!(pager.page(&index, &FindParams { cursor: p1.cursor, ..FindParams::default() }, late), Err(CursorExpired));
    }
}
