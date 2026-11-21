//! Pull requests in the window (D119, D120): what each row's badge shows, its menus, and the
//! Create PR modal. Pure: the client fills [`PrStore`] from `forge/prs` and
//! `forge/prs/changed`; everything here is a function of the state.
//!
//! The forge kind is never branched on — only [`ForgeCapability`] values are.

use std::collections::BTreeMap;

use yaac_protocol::{
    ChecksState, ForgeCapability, ForgePrepareResult, ForgePrsResult, ItemBody, PrState, PrSummary, TargetId, TurnId,
};

use crate::key::ThreadKey;
use crate::state::AppState;

/// How long a visible row's pull requests are trusted before the window asks again (seconds).
pub const VISIBLE_REFRESH_SECS: i64 = 5 * 60;

/// One branch's pull requests on one target, by checkout root and branch.
pub type BranchKey = (TargetId, String, String);

/// What the window knows about pull requests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrStore {
    /// By (target, checkout root, branch).
    pub branches: BTreeMap<BranchKey, ForgePrsResult>,
    /// Which branch each thread was last answered for, and when it was asked (seconds).
    pub threads: BTreeMap<ThreadKey, ThreadPr>,
}

/// One thread's link to its branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadPr {
    /// The branch entry it maps to (from the last answer); `None` until answered, and for a
    /// thread outside a forge or on a detached head.
    pub branch: Option<BranchKey>,
    /// The branch the thread's summary named when it was asked (a change asks again).
    pub asked_branch: Option<String>,
    /// When it was last asked (seconds since the epoch).
    pub asked_at: i64,
    /// A request is in flight.
    pub loading: bool,
    /// The last answer said there is no supported forge.
    pub no_forge: bool,
}

impl PrStore {
    /// Records an answer to `forge/prs` for `key`.
    pub fn apply_answer(&mut self, key: &ThreadKey, r: ForgePrsResult) {
        let t = self.threads.entry(key.clone()).or_default();
        t.loading = false;
        t.no_forge = r.access.repo.is_none();
        match (&r.branch, t.no_forge) {
            (Some(b), false) => {
                let bk = (key.target.clone(), r.root.to_string(), b.clone());
                t.branch = Some(bk.clone());
                self.branches.insert(bk, r);
            }
            _ => t.branch = None,
        }
    }

    /// Records `forge/prs/changed` from `target`.
    pub fn apply_changed(&mut self, target: &TargetId, r: ForgePrsResult) {
        let Some(b) = r.branch.clone() else { return };
        self.branches.insert((target.clone(), r.root.to_string(), b), r);
    }

    /// The branch entry of a thread, when known.
    pub fn of(&self, key: &ThreadKey) -> Option<&ForgePrsResult> {
        self.threads.get(key).and_then(|t| t.branch.as_ref()).and_then(|b| self.branches.get(b))
    }

    /// Forget a target's answers (it was removed).
    pub fn drop_target(&mut self, target: &TargetId) {
        self.branches.retain(|k, _| &k.0 != target);
        self.threads.retain(|k, _| &k.target != target);
    }
}

/// Which threads of `visible` should be asked again now: never asked, asked for another
/// branch, or not asked for [`VISIBLE_REFRESH_SECS`]. Only on connected targets, only for
/// threads in git.
pub fn due(state: &AppState, visible: &[ThreadKey], now: i64) -> Vec<ThreadKey> {
    let mut out = Vec::new();
    for key in visible {
        let Some(s) = state.summary(key) else { continue };
        if s.git.is_none() && s.repo.is_none() {
            continue;
        }
        if !state.target(&key.target).is_some_and(crate::state::TargetRecord::connected) {
            continue;
        }
        let branch = s.git.as_ref().and_then(|g| g.branch.clone());
        let due = match state.prs.threads.get(key) {
            None => true,
            Some(t) if t.loading => false,
            Some(t) => t.asked_branch != branch || now - t.asked_at >= VISIBLE_REFRESH_SECS,
        };
        if due {
            out.push(key.clone());
        }
    }
    out
}

/// A small, monochrome hint inside the badge (P-entry in decisions.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeHint {
    /// A draft: the outline is dashed.
    Draft,
    /// Merged: the number is secondary, with a merge mark.
    Merged,
    /// Closed without merging: the number is secondary.
    Closed,
}

/// The badge at the right of a row's first line (D119, as changed: line 1, before the time).
#[derive(Clone, Debug, PartialEq)]
pub struct PrBadge {
    /// The pull request the badge stands for; `None`: `[ PR#      ]`, click to create.
    pub primary: Option<PrSummary>,
    /// The others from the same branch (the `+N`), open first.
    pub others: Vec<PrSummary>,
    /// State hint of the primary.
    pub hint: Option<BadgeHint>,
    /// Its checks: a small mark when pending or failing (passing draws nothing).
    pub checks: Option<ChecksState>,
    /// It can be used: the forge CLI is there and signed in.
    pub enabled: bool,
    /// Why it cannot (shown on hover): "gh is not logged in on devbox — run …".
    pub reason: Option<String>,
    /// Creating a pull request is possible.
    pub can_create: bool,
}

impl PrBadge {
    /// The number text, `""` for the create badge.
    pub fn number_text(&self) -> String {
        self.primary.as_ref().map(|p| p.number.to_string()).unwrap_or_default()
    }

    /// `+N`, or `None`.
    pub fn more_text(&self) -> Option<String> {
        (!self.others.is_empty()).then(|| format!("+{}", self.others.len()))
    }

    /// The hover text of the badge itself.
    pub fn tooltip(&self) -> String {
        if let Some(r) = &self.reason {
            return r.clone();
        }
        match &self.primary {
            None => "Create a pull request for this branch".to_owned(),
            Some(p) => format!("{} — {}\nOpen in the browser", describe(p), p.title),
        }
    }
}

/// "#33342 · draft · checks failing".
pub fn describe(p: &PrSummary) -> String {
    let mut parts = vec![format!("#{}", p.number)];
    match p.state {
        PrState::Open if p.draft => parts.push("draft".into()),
        PrState::Open => parts.push("open".into()),
        PrState::Merged => parts.push("merged".into()),
        PrState::Closed => parts.push("closed".into()),
        PrState::Unknown(ref s) => parts.push(s.clone()),
    }
    match p.checks {
        Some(ChecksState::Failing) => parts.push("checks failing".into()),
        Some(ChecksState::Pending) => parts.push("checks running".into()),
        Some(ChecksState::Passing) => parts.push("checks passed".into()),
        _ => {}
    }
    parts.join(" · ")
}

fn hint_of(p: &PrSummary) -> Option<BadgeHint> {
    match p.state {
        PrState::Open if p.draft => Some(BadgeHint::Draft),
        PrState::Merged => Some(BadgeHint::Merged),
        PrState::Closed => Some(BadgeHint::Closed),
        _ => None,
    }
}

/// The badge of a thread's row; `None` (no badge) outside git, when the remote is not on a
/// supported forge, and until the target has answered.
pub fn badge_of(state: &AppState, key: &ThreadKey) -> Option<PrBadge> {
    let s = state.summary(key)?;
    if s.git.is_none() && s.repo.is_none() {
        return None;
    }
    let r = state.prs.of(key)?;
    r.access.repo.as_ref()?;
    let caps = &r.access.capabilities;
    let can_list = caps.contains(&ForgeCapability::PrList);
    let enabled = !caps.is_empty();
    let reason = (!enabled).then(|| r.access.reason.clone().unwrap_or_else(|| "Pull requests are not available on this target".into()));
    // The badge stands for the open pull request when there is one, else the most recent.
    let mut prs = if can_list { r.prs.clone() } else { Vec::new() };
    // The default branch itself: no create badge, only a pull request from it (P-entry).
    if prs.is_empty() && enabled && r.branch.is_some() && r.branch == r.default_branch {
        return None;
    }
    let first = prs.iter().position(|p| p.state == PrState::Open).unwrap_or(0);
    let primary = (!prs.is_empty()).then(|| prs.remove(first));
    Some(PrBadge {
        hint: primary.as_ref().and_then(hint_of),
        checks: primary.as_ref().and_then(|p| p.checks.clone()).filter(|c| matches!(c, ChecksState::Pending | ChecksState::Failing)),
        primary,
        others: prs,
        enabled,
        reason,
        can_create: caps.contains(&ForgeCapability::PrCreate),
    })
}

/// What clicking the badge does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BadgeClick {
    /// Open this URL in the host's default browser.
    Open(String),
    /// Open the Create PR modal.
    Create,
    /// Nothing (disabled).
    Nothing,
}

/// The badge's click (D119): the PR in the browser, or the create modal.
pub fn click(b: &PrBadge) -> BadgeClick {
    if !b.enabled {
        return BadgeClick::Nothing;
    }
    match &b.primary {
        Some(p) if !p.url.is_empty() => BadgeClick::Open(p.url.clone()),
        Some(_) => BadgeClick::Nothing,
        None if b.can_create => BadgeClick::Create,
        None => BadgeClick::Nothing,
    }
}

/// The badge's right-click menu (D119).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeAction {
    /// Create a new PR.
    Create,
    /// Copy the PR's link.
    CopyLink,
}

/// One row of the badge's menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadgeMenuItem {
    /// What it does.
    pub action: BadgeAction,
    /// Label.
    pub label: &'static str,
    /// Usable now.
    pub enabled: bool,
    /// Why not.
    pub reason: Option<String>,
}

/// *Create a new PR*, *Copy link*.
pub fn badge_menu(b: &PrBadge) -> Vec<BadgeMenuItem> {
    vec![
        BadgeMenuItem {
            action: BadgeAction::Create,
            label: "Create a new PR",
            enabled: b.enabled && b.can_create,
            reason: if b.enabled { (!b.can_create).then(|| "Pull requests cannot be created from this target".into()) } else { b.reason.clone() },
        },
        BadgeMenuItem {
            action: BadgeAction::CopyLink,
            label: "Copy link",
            enabled: b.primary.as_ref().is_some_and(|p| !p.url.is_empty()),
            reason: b.primary.is_none().then(|| "This branch has no pull request yet".into()),
        },
    ]
}

// ── The Create PR modal (D120) ─────────────────────────────────────────────────────────

/// Where the modal's data is.
#[derive(Clone, Debug, PartialEq)]
pub enum Prepare {
    /// `forge/prepare` in flight.
    Loading,
    /// Answered.
    Ready(Box<ForgePrepareResult>),
    /// It failed.
    Failed(String),
}

/// "Ask the agent to write it": the turn it was asked in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentDraft {
    /// `turn/start` sent; no turn id yet.
    Sending,
    /// The agent is writing, in this turn.
    Writing(TurnId),
    /// The reply could not be used.
    Failed(String),
}

/// The Create PR modal.
#[derive(Clone, Debug, PartialEq)]
pub struct CreatePr {
    /// The thread whose branch it is.
    pub key: ThreadKey,
    /// `forge/prepare`.
    pub prepare: Prepare,
    /// Target branch (default: the repository's default branch).
    pub base: String,
    /// Title (prefilled from the commits; the user's edits are kept).
    pub title: String,
    /// Description.
    pub body: String,
    /// Open as a draft.
    pub draft: bool,
    /// The user edited title or description (a later prefill must not overwrite them).
    pub edited: bool,
    /// The agent is drafting.
    pub agent: Option<AgentDraft>,
    /// `forge/pr/create` in flight.
    pub creating: bool,
    /// The last error.
    pub error: Option<String>,
    /// The target-branch dropdown is open.
    pub base_open: bool,
}

impl CreatePr {
    /// A modal for `key`, waiting for `forge/prepare`.
    pub fn new(key: ThreadKey) -> Self {
        Self {
            key,
            prepare: Prepare::Loading,
            base: String::new(),
            title: String::new(),
            body: String::new(),
            draft: false,
            edited: false,
            agent: None,
            creating: false,
            error: None,
            base_open: false,
        }
    }

    /// `forge/prepare` answered: fill what the user has not touched.
    pub fn prepared(&mut self, r: ForgePrepareResult) {
        if self.base.is_empty() {
            self.base = r.base.clone().or_else(|| r.default_branch.clone()).unwrap_or_default();
        }
        if !self.edited {
            self.title = r.title.clone();
            self.body = r.body.clone();
        }
        self.prepare = Prepare::Ready(Box::new(r));
    }

    /// The answer, when there is one.
    pub fn ready(&self) -> Option<&ForgePrepareResult> {
        match &self.prepare {
            Prepare::Ready(r) => Some(r),
            _ => None,
        }
    }

    /// The branches the target dropdown offers (the default first).
    pub fn branches(&self) -> Vec<String> {
        self.ready().map(|r| r.branches.clone()).unwrap_or_default()
    }

    /// The push-first notice (D120), when Create will push.
    pub fn push_notice(&self) -> Option<String> {
        let r = self.ready()?;
        let branch = r.branch.as_deref().unwrap_or("the branch");
        if !r.push.upstream {
            return Some(format!("Create will push {branch} first: it is not on the remote yet."));
        }
        (r.push.ahead > 0).then(|| {
            let n = r.push.ahead;
            format!("Create will push {branch} first: {n} commit{} not pushed yet.", if n == 1 { " is" } else { "s are" })
        })
    }

    /// The uncommitted-changes warning (D120): warned about, never pushed.
    pub fn uncommitted_warning(&self) -> Option<String> {
        let n = self.ready()?.push.uncommitted;
        (n > 0).then(|| format!("{n} file{} uncommitted changes — they are not part of the pull request.", if n == 1 { " has" } else { "s have" }))
    }

    /// Why Create cannot be pressed now, or `None` when it can.
    pub fn create_blocked(&self) -> Option<String> {
        let r = match &self.prepare {
            Prepare::Loading => return Some("Reading the branch…".into()),
            Prepare::Failed(e) => return Some(e.clone()),
            Prepare::Ready(r) => r,
        };
        if !r.access.capabilities.contains(&ForgeCapability::PrCreate) {
            return Some(r.access.reason.clone().unwrap_or_else(|| "Pull requests cannot be created from this target".into()));
        }
        if self.creating {
            return Some("Creating…".into());
        }
        if matches!(self.agent, Some(AgentDraft::Sending | AgentDraft::Writing(_))) {
            return Some("The agent is writing the description…".into());
        }
        let Some(branch) = r.branch.as_deref() else { return Some("The checkout is not on a branch".into()) };
        if self.base.trim().is_empty() {
            return Some("Choose a target branch".into());
        }
        if self.base == branch {
            return Some("The target branch is the branch itself".into());
        }
        if self.title.trim().is_empty() {
            return Some("A title is needed".into());
        }
        None
    }

    /// Why "Ask the agent to write it" cannot be used now.
    pub fn ask_blocked(&self, state: &AppState) -> Option<String> {
        if self.ready().is_none() {
            return Some("Reading the branch…".into());
        }
        if matches!(self.agent, Some(AgentDraft::Sending | AgentDraft::Writing(_))) {
            return Some("The agent is writing it…".into());
        }
        if !matches!(state.control(&self.key), yaac_protocol::Control::Free | yaac_protocol::Control::Mine) {
            return Some("This thread is in use elsewhere".into());
        }
        let busy = state.transcripts.get(&self.key).is_some_and(|t| t.active_turn.is_some())
            || state.summary(&self.key).is_some_and(|s| s.state == yaac_protocol::ThreadState::Running);
        busy.then(|| "The agent is busy with a turn".into())
    }

    /// Looks for the agent's reply in the thread's transcript; fills the modal from it.
    /// Returns whether something changed.
    pub fn poll_agent(&mut self, state: &AppState) -> bool {
        let Some(AgentDraft::Writing(turn)) = &self.agent else { return false };
        let Some(t) = state.transcripts.get(&self.key) else { return false };
        // Wait for the turn to end, then take its last agent message.
        if t.active_turn.as_ref() == Some(turn) {
            return false;
        }
        let text = t.items().iter().rev().filter(|i| &i.turn_id == turn).find_map(|i| match &i.body {
            ItemBody::AgentMessage(m) => Some(m.text.clone()),
            _ => None,
        });
        let Some(text) = text else {
            // The turn may simply not be in the transcript yet.
            if t.items().iter().any(|i| &i.turn_id == turn) {
                self.agent = Some(AgentDraft::Failed("The agent's turn ended without a reply.".into()));
                return true;
            }
            return false;
        };
        match parse_agent_draft(&text) {
            Some((title, body)) => {
                self.title = title;
                self.body = body;
                self.edited = true;
                self.agent = None;
            }
            None => self.agent = Some(AgentDraft::Failed("The agent's reply did not have a title and a description.".into())),
        }
        true
    }
}

/// What the thread's agent is asked (a user message in the transcript; costs a turn).
pub fn agent_prompt(branch: &str, base: &str) -> String {
    format!(
        "Write a title and a description for a pull request of the branch `{branch}` into `{base}`, from the changes on this branch (look at `git log {base}..HEAD` and the diff). Do not create the pull request and do not change any file.\n\nReply with exactly this shape and nothing else:\n\nTitle: <one line>\n\nDescription:\n<the description, in markdown>"
    )
}

/// Reads `Title: …` / `Description: …` from the agent's reply (tolerant of code fences,
/// bold labels, and a heading instead of a label). `None` when there is no title.
pub fn parse_agent_draft(text: &str) -> Option<(String, String)> {
    let mut lines: Vec<&str> = text.lines().collect();
    // Drop surrounding code fences.
    while lines.first().is_some_and(|l| l.trim().is_empty() || l.trim().starts_with("```")) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty() || l.trim().starts_with("```")) {
        lines.pop();
    }
    let label = |l: &str, name: &str| -> Option<String> {
        let t = l.trim().trim_start_matches('#').trim().replace("**", "");
        let lower = t.to_lowercase();
        lower.starts_with(name).then(|| t[name.len()..].trim_start_matches(':').trim().to_owned())
    };
    let ti = lines.iter().position(|l| label(l, "title").is_some())?;
    let mut title = label(lines[ti], "title").unwrap_or_default();
    let mut rest = ti + 1;
    if title.is_empty() {
        // "Title:" on its own line, the title on the next.
        let next = lines.iter().skip(ti + 1).position(|l| !l.trim().is_empty())?;
        title = lines[ti + 1 + next].trim().to_owned();
        rest = ti + 2 + next;
    }
    let title = title.trim_matches('`').trim_matches('"').trim().to_owned();
    if title.is_empty() {
        return None;
    }
    let di = lines.iter().skip(rest).position(|l| label(l, "description").is_some()).map(|i| i + rest);
    let body = match di {
        Some(i) => {
            let first = label(lines[i], "description").unwrap_or_default();
            let mut b = Vec::new();
            if !first.is_empty() {
                b.push(first.as_str());
            }
            b.extend(lines[i + 1..].iter().copied());
            b.join("\n").trim().to_owned()
        }
        None => lines[rest.min(lines.len())..].join("\n").trim().to_owned(),
    };
    Some((title, body))
}
