//! The hub's own answers to agents' mesh queries (docs/10-agent-mesh.md#listing-is-scoped-and-bounded).
//!
//! Most `mesh/query` calls are routed to the destination `yaacd`. Two are answered here, from
//! what the hub already holds, so a question about the whole fleet is one question and never a
//! round of calls to every daemon:
//!
//! - `findThreads` — `find_thread` across every target, by id or fuzzy title, over the cached
//!   thread index ([`crate::find`]), ten per page;
//! - `listTargets` — `list_targets`: names, reachability, and how many threads are active on
//!   each (from the cached index). Names and counts only, never the threads.
//!
//! Everything here is pure: the core gathers the cached rows and calls these.

use yaac_protocol::{
    Address, FindThreadsQuery, FindThreadsReply, FoundThreadInfo, ListTargetsReply, MeshPeer, MeshThreadInfo, ProviderDescriptor, TargetCount,
    TargetId, TargetName, ThreadState, ThreadSummary,
};

use crate::find::{FindParams, FindResult, MatchKind};

/// The `hub/threads/find` parameters a `findThreads` query stands for.
pub fn find_params(q: &FindThreadsQuery) -> FindParams {
    FindParams { query: q.query.clone(), target: q.only_target.clone(), state: q.state.clone(), limit: None, cursor: q.cursor.clone() }
}

/// What a target's threads are labelled with.
#[derive(Clone, Debug, Default)]
pub struct TargetLabels {
    /// The target's name.
    pub name: Option<TargetName>,
    /// Whether it is connected.
    pub reachable: bool,
    /// Its provider instances (for "Codex · work" labels).
    pub providers: Vec<ProviderDescriptor>,
}

/// One summary as an agent sees it.
pub fn thread_info(target: &TargetId, summary: &ThreadSummary, labels: Option<&TargetLabels>) -> MeshThreadInfo {
    let provider = labels
        .and_then(|l| l.providers.iter().find(|p| p.instance == summary.instance))
        .map(|p| p.display_name.clone())
        .or_else(|| Some(summary.instance.as_str().to_owned()));
    MeshThreadInfo {
        peer: MeshPeer {
            address: Address::new(target, &summary.id),
            title: Some(summary.title.clone()),
            provider,
            model: summary.model.clone(),
            target_name: labels.and_then(|l| l.name.as_ref().map(|n| n.as_str().to_owned())),
            reachable: labels.map(|l| l.reachable),
        },
        state: summary.state.clone(),
        spawned_by: summary.spawned_by.clone(),
        cwd: summary.cwd.clone(),
        project: summary.project.clone(),
    }
}

fn matched(kind: MatchKind) -> &'static str {
    match kind {
        MatchKind::Id => "id",
        MatchKind::NativeId => "nativeId",
        MatchKind::Address => "address",
        MatchKind::Title => "title",
    }
}

/// A `hub/threads/find` page as a `findThreads` reply.
pub fn find_reply(page: &FindResult, labels: &dyn Fn(&TargetId) -> Option<TargetLabels>) -> FindThreadsReply {
    FindThreadsReply {
        results: page
            .results
            .iter()
            .map(|r| FoundThreadInfo { thread: thread_info(&r.target, &r.thread, labels(&r.target).as_ref()), matched: matched(r.matched).into(), exact: r.exact })
            .collect(),
        total: page.total,
        ambiguous: page.ambiguous,
        cursor: page.cursor.clone(),
    }
}

/// Whether a thread counts as active for `list_targets`.
pub fn is_active(state: &ThreadState) -> bool {
    matches!(state, ThreadState::Running | ThreadState::NeedsAttention | ThreadState::Waiting)
}

/// `listTargets`: every target with its active-thread count.
pub fn targets_reply(targets: &[(TargetId, TargetName, bool)], states: &[(TargetId, ThreadState)]) -> ListTargetsReply {
    ListTargetsReply {
        targets: targets
            .iter()
            .map(|(id, name, reachable)| TargetCount {
                id: id.clone(),
                name: name.clone(),
                reachable: *reachable,
                active_threads: u32::try_from(states.iter().filter(|(t, s)| t == id && is_active(s)).count()).unwrap_or(u32::MAX),
            })
            .collect(),
    }
}
