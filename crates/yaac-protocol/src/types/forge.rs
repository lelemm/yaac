//! Pull requests (D119–D121): the forge a thread's repository is hosted on, and its pull
//! requests for the thread's branch. All forge work runs on the target, through `yaacd`, with
//! the target's own `gh` / `az` and their logins; the host only opens URLs.
//!
//! The forge kind is data for display; the window branches only on [`ForgeCapability`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{OpId, TargetPath, ThreadId, Timestamp};
use crate::macros::open_enum;

open_enum! {
    /// Which forge hosts a repository. Display only: branch on capabilities.
    pub enum ForgeKind fallback Unknown {
        /// GitHub or GitHub Enterprise, through `gh`.
        Github = "github",
        /// Azure DevOps Services, through `az`.
        AzureDevOps = "azureDevOps",
    }
}

open_enum! {
    /// What can be done with a repository's pull requests from this target, now.
    pub enum ForgeCapability fallback Unknown {
        /// List the pull requests of a branch.
        PrList = "prList",
        /// Read one pull request in full.
        PrGet = "prGet",
        /// Create a pull request.
        PrCreate = "prCreate",
        /// Read checks / policies, and a failed run's log.
        PrChecks = "prChecks",
        /// Rerun failed checks.
        PrRerun = "prRerun",
        /// Read review comments.
        PrComments = "prComments",
        /// Reply to and resolve review comments.
        PrReply = "prReply",
    }
}

open_enum! {
    /// Where a pull request is in its life.
    pub enum PrState fallback Unknown {
        /// Open (possibly a draft: see `draft`).
        Open = "open",
        /// Merged (Azure DevOps: completed).
        Merged = "merged",
        /// Closed without merging (Azure DevOps: abandoned).
        Closed = "closed",
    }
}

open_enum! {
    /// The combined state of a pull request's checks (GitHub) or policies (Azure DevOps).
    pub enum ChecksState fallback Unknown {
        /// No checks.
        None = "none",
        /// Some are still queued or running, none failed.
        Pending = "pending",
        /// All finished, none failed.
        Passing = "passing",
        /// At least one failed.
        Failing = "failing",
    }
}

open_enum! {
    /// One check's (or policy's) state.
    pub enum CheckState fallback Unknown {
        /// Queued or running.
        Pending = "pending",
        /// Succeeded.
        Success = "success",
        /// Failed (or was rejected / broken).
        Failure = "failure",
        /// Skipped, neutral, cancelled, or not applicable.
        Skipped = "skipped",
    }
}

/// The forge of a repository, as detected from its `origin` URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForgeRepo {
    /// Which forge (display only).
    pub kind: ForgeKind,
    /// Host name (`github.com`, a GitHub Enterprise host, `dev.azure.com`).
    pub host: String,
    /// The repository's name on the forge: `owner/repo` (GitHub) or `org/project/repo`
    /// (Azure DevOps). For a fork whose pull requests go upstream, the upstream repository.
    pub slug: String,
    /// The repository's web page.
    pub web_url: String,
    /// Pull requests are opened from a fork (`origin`) into this repository (`upstream`).
    #[serde(default)]
    pub from_fork: bool,
}

/// Whether a repository's pull requests can be worked with from this target, and if not, why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForgeAccess {
    /// The forge, when the remote is on a supported one. Absent: no badge at all (not in git,
    /// no remote, or a forge YAAC does not support).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<ForgeRepo>,
    /// What can be done now. Empty with a `reason` when the CLI is missing or signed out.
    pub capabilities: Vec<ForgeCapability>,
    /// Why nothing (or less) can be done, in words to show as they are ("gh is not logged in
    /// on devbox — run `gh auth login` there").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A pull request in a list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrSummary {
    /// The forge's number (GitHub) or id (Azure DevOps).
    pub number: u64,
    /// Title.
    pub title: String,
    /// Open, merged, or closed.
    pub state: PrState,
    /// A draft.
    #[serde(default)]
    pub draft: bool,
    /// Its web page (what the badge opens).
    pub url: String,
    /// The branch it merges from.
    pub head: String,
    /// The branch it merges into.
    pub base: String,
    /// The combined state of its checks, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<ChecksState>,
    /// Author's name on the forge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// When it was last updated (or created).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<Timestamp>,
}

/// How the thread's branch stands against its remote, for Create's "push first" notice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PushState {
    /// The branch has an upstream.
    pub upstream: bool,
    /// Commits not pushed.
    pub ahead: u32,
    /// Files with uncommitted changes (untracked included): warned about, never pushed.
    pub uncommitted: u32,
}

/// `forge/prs`: the pull requests whose head is the thread's branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForgePrsParams {
    /// The thread whose checkout and branch are meant.
    pub thread_id: ThreadId,
    /// Ask the forge now instead of answering from the target's cache (opening the row's
    /// menu). Still rate-limited by `yaacd`.
    #[serde(default)]
    pub refresh: bool,
}

/// `forge/prs` result, and the params of `forge/prs/changed`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForgePrsResult {
    /// The checkout's root (the cache is per repository root and branch).
    pub root: TargetPath,
    /// The branch asked about; absent on a detached head (then there are no pull requests).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The forge and what can be done.
    pub access: ForgeAccess,
    /// The repository's default branch as the checkout knows it (`origin/HEAD`): a row on it
    /// with no pull request shows no create badge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    /// Pull requests from that branch: open ones first, then the most recently updated.
    pub prs: Vec<PrSummary>,
    /// When the forge was last asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
    /// The last ask failed (the list is the last one known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `forge/prepare`: what the Create PR modal needs (D120).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForgePrepareParams {
    /// The thread whose branch the pull request is from.
    pub thread_id: ThreadId,
    /// The target branch to prefill against. Absent: the repository's default branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

/// `forge/prepare` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForgePrepareResult {
    /// The forge and what can be done.
    pub access: ForgeAccess,
    /// The branch the pull request would be from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The repository's default branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    /// Branches on the remote, for the target-branch dropdown (bounded).
    pub branches: Vec<String>,
    /// The target branch the title and description were prefilled against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Prefilled title (from the branch's commits).
    pub title: String,
    /// Prefilled description (from the branch's commits).
    pub body: String,
    /// Push state of the branch.
    pub push: PushState,
}

/// `forge/pr/create` (D120). Pushes the branch first when it has unpushed commits or no
/// upstream (`git push -u` on the target); uncommitted changes are left alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForgePrCreateParams {
    /// Operation id: an uncertain outcome is `outcome_unknown`, never re-sent.
    pub op_id: OpId,
    /// The thread whose branch the pull request is from.
    pub thread_id: ThreadId,
    /// Target branch.
    pub base: String,
    /// Title.
    pub title: String,
    /// Description (markdown).
    #[serde(default)]
    pub body: String,
    /// Open it as a draft.
    #[serde(default)]
    pub draft: bool,
}

/// `forge/pr/create` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForgePrCreateResult {
    /// The new pull request.
    pub pr: PrSummary,
    /// The branch was pushed first.
    #[serde(default)]
    pub pushed: bool,
}

/// `forge/pr/get`: one pull request in full.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForgePrGetParams {
    /// The thread whose repository is meant.
    pub thread_id: ThreadId,
    /// The pull request's number.
    pub number: u64,
}

/// A reviewer and where they stand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrReviewer {
    /// Name on the forge.
    pub name: String,
    /// Their verdict in words ("approved", "changes requested", "waiting for author",
    /// "requested", …).
    pub state: String,
    /// A required reviewer (Azure DevOps).
    #[serde(default)]
    pub required: bool,
}

/// One check or policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrCheck {
    /// Its name.
    pub name: String,
    /// Its state.
    pub state: CheckState,
    /// Where to see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// It blocks merging (Azure DevOps policies).
    #[serde(default)]
    pub blocking: bool,
}

/// `forge/pr/get` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForgePrGetResult {
    /// The summary.
    pub pr: PrSummary,
    /// The description.
    pub body: String,
    /// Reviewers.
    pub reviewers: Vec<PrReviewer>,
    /// Checks or policies.
    pub checks: Vec<PrCheck>,
}
