//! The client's side of pull requests (D119, D120): asking a target for the branches' pull
//! requests of the rows on screen, the Create PR modal's calls, and "Ask the agent to write
//! it". Every forge call runs on the target; the window only opens the URLs it is given.

use serde_json::{Value, json};
use yaac_protocol::methods::names as m;
use yaac_protocol::rpc::{ErrorCode, ErrorObject};
use yaac_protocol::{ForgePrCreateResult, ForgePrepareResult, ForgePrsResult, TargetId, TurnId};

use super::{Call, Client};
use crate::key::ThreadKey;
use crate::pr::{self, AgentDraft, CreatePr, Prepare};

impl Client {
    /// Asks for the pull requests of the rows on screen that are due (never asked, a new
    /// branch, or older than [`pr::VISIBLE_REFRESH_SECS`]). Cheap to call every frame.
    pub fn forge_visible(&mut self, visible: &[ThreadKey], now: i64) {
        for key in pr::due(&self.state, visible, now) {
            self.forge_ask(&key, now, false);
        }
    }

    /// Asks the target now (the row's PR menu was opened); `yaacd` still rate-limits it.
    pub fn forge_refresh(&mut self, key: &ThreadKey, now: i64) {
        if self.state.prs.threads.get(key).is_some_and(|t| t.loading) {
            return;
        }
        self.forge_ask(key, now, true);
    }

    fn forge_ask(&mut self, key: &ThreadKey, now: i64, refresh: bool) {
        let branch = self.state.summary(key).and_then(|s| s.git.as_ref()).and_then(|g| g.branch.clone());
        let t = self.state.prs.threads.entry(key.clone()).or_default();
        t.loading = true;
        t.asked_at = now;
        t.asked_branch = branch;
        let params = json!({ "threadId": key.thread, "refresh": refresh });
        self.routed(&key.target, m::FORGE_PRS, params, Call::ForgePrs(key.clone()));
    }

    /// Opens the Create PR modal (D120) for a thread and asks for what it needs.
    pub fn open_create_pr(&mut self, key: &ThreadKey) {
        self.state.create_pr = Some(CreatePr::new(key.clone()));
        self.forge_prepare(key, None);
        // "Ask the agent" reads the reply from the transcript: keep it subscribed.
        if !self.state.transcripts.contains_key(key) {
            self.state.transcripts.insert(key.clone(), crate::transcript::Transcript::new());
            self.state.recent.push_back(key.clone());
            self.subscribe(key);
        }
    }

    /// The target branch was changed: the prefill follows it unless the user has edited.
    pub fn set_create_pr_base(&mut self, base: &str) {
        let Some(d) = self.state.create_pr.as_mut() else { return };
        d.base = base.to_owned();
        d.base_open = false;
        let key = d.key.clone();
        if !d.edited {
            self.forge_prepare(&key, Some(base));
        }
    }

    fn forge_prepare(&mut self, key: &ThreadKey, base: Option<&str>) {
        let mut params = json!({ "threadId": key.thread });
        if let Some(b) = base {
            params["base"] = json!(b);
        }
        self.routed(&key.target, m::FORGE_PREPARE, params, Call::ForgePrepare(key.clone()));
    }

    /// Create (D120): pushes first on the target when needed. One operation id; an uncertain
    /// outcome is shown, never sent again.
    pub fn submit_create_pr(&mut self) -> bool {
        let Some(d) = self.state.create_pr.as_ref() else { return false };
        if d.create_blocked().is_some() {
            return false;
        }
        let key = d.key.clone();
        let params = json!({ "threadId": key.thread, "base": d.base.trim(), "title": d.title.trim(), "body": d.body, "draft": d.draft });
        let op = self.op_id();
        let mut params = params;
        params["opId"] = json!(op);
        if let Some(d) = self.state.create_pr.as_mut() {
            d.creating = true;
            d.error = None;
        }
        self.routed(&key.target.clone(), m::FORGE_PR_CREATE, params, Call::ForgeCreate(key));
        true
    }

    /// "Ask the agent to write it" (D120): a user turn in the thread asking for a title and a
    /// description in a fixed shape; the modal is filled from the reply when the turn ends.
    pub fn ask_agent_for_pr(&mut self) -> bool {
        let Some(d) = self.state.create_pr.as_ref() else { return false };
        if d.ask_blocked(&self.state).is_some() {
            return false;
        }
        let key = d.key.clone();
        let branch = d.ready().and_then(|r| r.branch.clone()).unwrap_or_default();
        let text = pr::agent_prompt(&branch, d.base.trim());
        let op = self.op_id();
        let params = json!({ "opId": op, "threadId": key.thread, "input": [{ "type": "text", "text": text }] });
        if let Some(d) = self.state.create_pr.as_mut() {
            d.agent = Some(AgentDraft::Sending);
        }
        self.routed(&key.target.clone(), m::TURN_START, params, Call::ForgeAsk(key));
        true
    }

    /// Fills the modal from the agent's reply once its turn has ended. Call each frame.
    pub fn poll_create_pr(&mut self) -> bool {
        let Some(mut d) = self.state.create_pr.take() else { return false };
        let changed = d.poll_agent(&self.state);
        self.state.create_pr = Some(d);
        changed
    }

    pub(super) fn on_forge_result(&mut self, call: Call, result: Value) {
        match call {
            Call::ForgePrs(key) => match serde_json::from_value::<ForgePrsResult>(result) {
                Ok(r) => self.state.prs.apply_answer(&key, r),
                Err(_) => {
                    if let Some(t) = self.state.prs.threads.get_mut(&key) {
                        t.loading = false;
                    }
                }
            },
            Call::ForgePrepare(key) => {
                let Some(d) = self.state.create_pr.as_mut().filter(|d| d.key == key) else { return };
                match serde_json::from_value::<ForgePrepareResult>(result) {
                    Ok(r) => d.prepared(r),
                    Err(e) => d.prepare = Prepare::Failed(format!("The target's answer could not be read: {e}")),
                }
            }
            Call::ForgeCreate(key) => {
                let r = serde_json::from_value::<ForgePrCreateResult>(result);
                if let Some(d) = self.state.create_pr.as_ref().filter(|d| d.key == key)
                    && d.creating
                {
                    self.state.create_pr = None;
                }
                if let Ok(r) = r {
                    // The new pull request is on the row at once (forge/prs/changed follows).
                    if let Some(bk) = self.state.prs.threads.get(&key).and_then(|t| t.branch.clone())
                        && let Some(e) = self.state.prs.branches.get_mut(&bk)
                    {
                        e.prs.retain(|p| p.number != r.pr.number);
                        e.prs.insert(0, r.pr.clone());
                    }
                }
            }
            Call::ForgeAsk(key) => {
                let turn = result.get("turnId").and_then(Value::as_str).map(TurnId::new);
                if let Some(d) = self.state.create_pr.as_mut().filter(|d| d.key == key) {
                    d.agent = Some(match turn {
                        Some(t) => AgentDraft::Writing(t),
                        None => AgentDraft::Failed("The agent did not start a turn.".into()),
                    });
                }
            }
            _ => {}
        }
    }

    pub(super) fn on_forge_error(&mut self, call: Call, error: &ErrorObject) {
        match call {
            Call::ForgePrs(key) => {
                if let Some(t) = self.state.prs.threads.get_mut(&key) {
                    t.loading = false;
                    // An older yaacd without the forge: no badge, and no asking again soon.
                    t.no_forge = error.code == ErrorCode::MethodNotFound;
                }
            }
            Call::ForgePrepare(key) => {
                if let Some(d) = self.state.create_pr.as_mut().filter(|d| d.key == key) {
                    d.prepare = Prepare::Failed(error.message.clone());
                }
            }
            Call::ForgeCreate(key) => {
                let text = if error.code == ErrorCode::OutcomeUnknown {
                    "YAAC lost contact while creating the pull request. It may or may not exist; it will not be created again. Check the branch's pull requests.".to_owned()
                } else {
                    format!("The pull request was not created: {}", error.message)
                };
                match self.state.create_pr.as_mut().filter(|d| d.key == key) {
                    Some(d) => {
                        d.creating = false;
                        d.error = Some(text);
                    }
                    None => self.error(Some(key), text),
                }
            }
            Call::ForgeAsk(key) => {
                if let Some(d) = self.state.create_pr.as_mut().filter(|d| d.key == key) {
                    d.agent = Some(AgentDraft::Failed(format!("The agent could not be asked: {}", error.message)));
                }
            }
            _ => {}
        }
    }

    pub(super) fn on_forge_changed(&mut self, target: &TargetId, params: Value) {
        if let Ok(r) = serde_json::from_value::<ForgePrsResult>(params) {
            self.state.prs.apply_changed(target, r);
        }
    }
}
