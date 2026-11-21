//! **Demo only.** A fake forge: the pull requests of the fleet's branches (D119), the Create
//! PR modal's data (D120), and a create that answers at once. The numbers, titles, and the
//! signed-out reason are the demo's own.

use serde_json::{Value, json};
use yaac_protocol::methods::names as m;
use yaac_protocol::rpc::{ErrorCode, RpcId};
use yaac_protocol::{
    ChecksState, ForgeAccess, ForgeCapability, ForgeKind, ForgePrepareResult, ForgePrsResult, ForgeRepo, PrState, PrSummary,
    PushState, TargetId, TargetPath, Timestamp,
};

use super::fleet::{BILLING, CI, FIX, HANDSHAKE, INDEX, SEARCH, USERS, k};
use super::{Inner, error, result, routed};
use crate::key::ThreadKey;

const ALL: [ForgeCapability; 7] = [
    ForgeCapability::PrList,
    ForgeCapability::PrGet,
    ForgeCapability::PrCreate,
    ForgeCapability::PrChecks,
    ForgeCapability::PrRerun,
    ForgeCapability::PrComments,
    ForgeCapability::PrReply,
];

/// The first words of the "Ask the agent to write it" prompt (see [`crate::pr::agent_prompt`]).
pub(super) const ASK_START: &str = "Write a title and a description for a pull request";

fn repo() -> ForgeRepo {
    ForgeRepo { kind: ForgeKind::Github, host: "github.com".into(), slug: "acme/platform".into(), web_url: "https://github.com/acme/platform".into(), from_fork: false }
}

pub(super) fn pr(number: u64, title: &str, state: PrState, draft: bool, head: &str, checks: Option<ChecksState>) -> PrSummary {
    PrSummary {
        number,
        title: title.into(),
        state,
        draft,
        url: format!("https://github.com/acme/platform/pull/{number}"),
        head: head.into(),
        base: "main".into(),
        checks,
        author: Some("lelemm".into()),
        updated_at: Some(Timestamp::new("2026-11-20T09:00:00Z")),
    }
}

/// The demo's pull requests by thread.
fn prs_of(key: &ThreadKey) -> Vec<PrSummary> {
    if *key == k(USERS) {
        vec![pr(33342, "Users service: move sessions to the new auth client", PrState::Open, false, "auth/users", Some(ChecksState::Failing))]
    } else if *key == k(BILLING) {
        vec![
            pr(33343, "Billing: token refresh on the new auth client", PrState::Open, true, "auth/billing", Some(ChecksState::Pending)),
            pr(33301, "Billing: first try at the token refresh", PrState::Closed, false, "auth/billing", None),
        ]
    } else if *key == k(SEARCH) || *key == k(INDEX) {
        vec![pr(33290, "Search: query the new auth client", PrState::Merged, false, "auth/search", Some(ChecksState::Passing))]
    } else {
        Vec::new()
    }
}

impl Inner {
    pub(super) fn forge(&mut self, id: RpcId, target: &TargetId, method: &str, p: &Value) {
        let Some(key) = p.get("threadId").and_then(Value::as_str).map(|t| ThreadKey::new(target.clone(), t)) else {
            self.ready.push_back(error(id, ErrorCode::InvalidParams, "threadId is required"));
            return;
        };
        let Some(rec) = self.fleet.threads.get(&key) else {
            self.ready.push_back(error(id, ErrorCode::ThreadNotFound, "no such thread"));
            return;
        };
        let s = &rec.summary;
        let root = s.repo.as_ref().map_or_else(|| s.cwd.clone(), |r| r.root.clone());
        let branch = s.git.as_ref().and_then(|g| g.branch.clone());
        let ahead = s.git.as_ref().map_or(0, |g| g.ahead);
        let dirty = s.git.as_ref().map_or(0, |g| g.added + g.modified + g.deleted);
        let in_git = s.git.is_some();
        // The CI thread's checkout (and any thread sharing it) is on Azure DevOps, and az is
        // signed out on that machine.
        let ci_root = self.fleet.threads.get(&k(CI)).map(|r| r.summary.repo.as_ref().map_or_else(|| r.summary.cwd.clone(), |x| x.root.clone()));
        let access = if !in_git {
            ForgeAccess { repo: None, capabilities: Vec::new(), reason: None }
        } else if key.target == k(CI).target && ci_root.as_ref() == Some(&root) && branch.as_deref() == Some("main") {
            let ado = ForgeRepo { kind: ForgeKind::AzureDevOps, host: "dev.azure.com".into(), slug: "contoso/Platform/api".into(), web_url: "https://dev.azure.com/contoso/Platform/_git/api".into(), from_fork: false };
            ForgeAccess { repo: Some(ado), capabilities: Vec::new(), reason: Some(format!("az is not logged in on {} · run `az login` there", key.target)) }
        } else {
            ForgeAccess { repo: Some(repo()), capabilities: ALL.to_vec(), reason: None }
        };
        match method {
            m::FORGE_PRS => {
                let mut prs = prs_of(&key);
                prs.extend(self.created.iter().filter(|(t, _)| *t == key).map(|(_, p)| p.clone()));
                prs.sort_by_key(|p| (p.state != PrState::Open, std::cmp::Reverse(p.number)));
                let r = ForgePrsResult { root, branch, access, default_branch: Some("main".into()), prs, at: None, error: None };
                self.ready.push_back(result(id, serde_json::to_value(r).unwrap_or(Value::Null)));
            }
            m::FORGE_PREPARE => {
                let base = p.get("base").and_then(Value::as_str).unwrap_or("main").to_owned();
                let b = branch.clone().unwrap_or_default();
                let r = ForgePrepareResult {
                    access,
                    branch,
                    default_branch: Some("main".into()),
                    branches: vec!["main".into(), "release/1.4".into(), "auth/users".into(), "auth/billing".into()],
                    base: Some(base),
                    title: if key == k(FIX) || key == k(HANDSHAKE) { "Fix the frame decoder's off-by-one".into() } else { format!("Work on {b}") },
                    body: "- Count the kind byte in the frame length\n- Add a test with two frames back to back".into(),
                    push: PushState { upstream: key != k(FIX), ahead, uncommitted: dirty.min(3) },
                };
                self.ready.push_back(result(id, serde_json::to_value(r).unwrap_or(Value::Null)));
            }
            m::FORGE_PR_CREATE => {
                let title = p.get("title").and_then(Value::as_str).unwrap_or("").to_owned();
                let draft = p.get("draft").and_then(Value::as_bool).unwrap_or(false);
                let number = 33350 + self.created.len() as u64;
                let new = pr(number, &title, PrState::Open, draft, branch.as_deref().unwrap_or(""), Some(ChecksState::Pending));
                self.created.push((key.clone(), new.clone()));
                self.ready.push_back(result(id, json!({ "pr": new, "pushed": true })));
                let mut prs = prs_of(&key);
                prs.extend(self.created.iter().filter(|(t, _)| *t == key).map(|(_, p)| p.clone()));
                prs.sort_by_key(|p| (p.state != PrState::Open, std::cmp::Reverse(p.number)));
                let r = ForgePrsResult { root: TargetPath::new(root.as_str()), branch, access, default_branch: Some("main".into()), prs, at: None, error: None };
                self.ready.push_back(routed(target, m::FORGE_PRS_CHANGED, serde_json::to_value(r).unwrap_or(Value::Null)));
            }
            _ => self.ready.push_back(error(id, ErrorCode::MethodNotFound, method)),
        }
    }
}

/// The agent's reply to "Ask the agent to write it", in the shape the prompt asks for.
pub(super) fn agent_reply() -> String {
    "Title: Fix the frame decoder's off-by-one on the last frame\n\nDescription:\nThe length prefix already counts the kind byte, so the reader stopped one byte early on the last frame of a buffer.\n\n- Count the kind byte once, in `read_len`\n- Add a test that feeds two frames back to back\n\nNo protocol change: the frame format is untouched.".into()
}
