//! Agent browser calls (docs/11-preview.md#agents), carried out on the calling thread's tabs.
//!
//! `yaacd` stamps the thread; the hub adds the target; this module only ever looks the tab up
//! inside that `(target, thread)` area, so an agent cannot reach another thread's tabs. Calls
//! never switch the user's view or take focus, and every call is answered once: a result, an
//! error, or — after [`CALL_LIMIT`] — a timeout error. Nothing here blocks the UI thread: each
//! call is a small state machine advanced by [`Agent::step`] on every frame or pipe tick.
// Without the browser host only the decline path is built.
#![cfg_attr(not(feature = "preview"), allow(dead_code, unused_imports))]
use std::collections::HashMap;
use std::time::{Duration, Instant};

use yaac_core::{ThreadKey, client::Client};
use yaac_protocol::rpc::{ErrorCode, ErrorObject, RpcId};
use yaac_protocol::{BrowserAction, BrowserCallResult};

#[cfg(feature = "preview")]
pub(crate) use engine::{Driver, Engine};

/// The hub answers the daemon by itself at 30 s; answer before that.
#[cfg(feature = "preview")]
const CALL_LIMIT: Duration = Duration::from_secs(25);
/// Bound on waiting for a page load (`open`, `navigate`).
#[cfg(feature = "preview")]
const LOAD_LIMIT: Duration = Duration::from_secs(20);
/// A load that started and finished between two ticks is never seen busy; after this long
/// idle, the load is taken as finished.
#[cfg(feature = "preview")]
const LOAD_GRACE: Duration = Duration::from_millis(1500);
/// How long the agent mark stays after the last action, so the user can see it.
const MARK_LINGER: Duration = Duration::from_secs(3);
/// Poll interval while a call waits on the browser host (hidden tabs do not wake the UI).
#[cfg(feature = "preview")]
const POLL: Duration = Duration::from_millis(50);

/// Pending agent calls and the tabs marked as agent-driven.
#[derive(Default)]
pub(crate) struct Agent {
    #[cfg(feature = "preview")]
    jobs: Vec<engine::Job>,
    /// Last action time per marked tab.
    marks: HashMap<(ThreadKey, u64), Instant>,
}

fn error(code: ErrorCode, message: impl Into<String>) -> ErrorObject {
    ErrorObject::new(code, message)
}

/// The agent-visible id of a tab, scoped to its thread.
fn tab_id(text: &str) -> Option<u64> {
    text.parse().ok()
}

fn wire_tab(tab: &crate::preview_controller::BrowserTab) -> yaac_protocol::BrowserTab {
    yaac_protocol::BrowserTab {
        tab: tab.id.to_string(),
        url: tab.url.clone(),
        title: tab.title.clone(),
        loading: tab.loading,
    }
}

impl Agent {
    fn mark(
        &mut self,
        client: &Client,
        browsers: &mut crate::preview_controller::Browsers,
        key: &ThreadKey,
        id: u64,
        now: Instant,
    ) {
        if let Some(tab) = browsers.tab_mut(key, id) {
            tab.agent = Some(
                client
                    .state
                    .provider_of(key)
                    .map_or_else(String::new, |p| p.kind.to_string()),
            );
            self.marks.insert((key.clone(), id), now);
        }
    }

    /// Clear marks whose agent stopped acting; the delay until the next one expires.
    fn expire_marks(
        &mut self,
        browsers: &mut crate::preview_controller::Browsers,
        now: Instant,
        acting: &[(ThreadKey, u64)],
    ) -> Option<Duration> {
        let mut next = None::<Duration>;
        self.marks.retain(|(key, id), last| {
            if acting.iter().any(|(k, i)| k == key && i == id) {
                *last = now;
            }
            let left = (*last + MARK_LINGER).saturating_duration_since(now);
            if left.is_zero() {
                if let Some(tab) = browsers.tab_mut(key, *id) {
                    tab.agent = None;
                }
                return false;
            }
            next = Some(next.map_or(left, |n| n.min(left)));
            true
        });
        next
    }

    /// Build without the browser host: no window here can serve the call, so the hub offers
    /// it to another window (or answers `no_window`).
    #[cfg(not(feature = "preview"))]
    pub(crate) fn step(
        &mut self,
        client: &mut Client,
        browsers: &mut crate::preview_controller::Browsers,
        now: Instant,
    ) -> Option<Duration> {
        for (rpc, _) in client.take_browser_calls() {
            client.answer_browser_call(
                rpc,
                Err(error(
                    ErrorCode::MethodNotFound,
                    "this window has no browser host",
                )),
            );
        }
        self.expire_marks(browsers, now, &[])
    }
}

#[cfg(feature = "preview")]
mod engine {
    use super::*;
    use crate::preview_controller::Browsers;
    use serde_json::{Value, json};
    use yaac_preview::wire::Action;
    use yaac_protocol::{
        BrowserConsole, BrowserConsoleEntry, BrowserEvalResult, BrowserHistory, BrowserNetwork,
        BrowserNetworkEntry, BrowserScreenshot, BrowserSnapshot, BrowserTabs, Empty,
    };

    /// The renderer's engine handles. The egui renderer and the WPF surfaces both hold the
    /// started tabs; the controller holds only their state.
    pub(crate) trait Driver {
        /// The started engine tab, if this renderer holds it.
        fn engine(&mut self, id: u64) -> Option<&mut dyn Engine>;
        /// DevTools replies (messages with an `id`) received since the last call, by tab.
        fn take_replies(&mut self) -> Vec<(u64, Value)>;
    }

    /// What the calls need of one engine tab ([`yaac_preview::Tab`]; a fake in tests).
    pub(crate) trait Engine {
        fn send(&mut self, action: Action);
        /// Send a DevTools command; the reply arrives through [`Driver::take_replies`].
        fn devtools_command(&mut self, method: &str, params: Value) -> i32;
        fn diagnostics_since(&self, cursor: u64) -> Vec<(u64, Value)>;
    }

    impl Engine for yaac_preview::Tab {
        fn send(&mut self, action: Action) {
            yaac_preview::Tab::send(self, action)
        }
        fn devtools_command(&mut self, method: &str, params: Value) -> i32 {
            yaac_preview::Tab::devtools_command(self, method, params)
        }
        fn diagnostics_since(&self, cursor: u64) -> Vec<(u64, Value)> {
            yaac_preview::Tab::diagnostics_since(self, cursor)
        }
    }

    pub(crate) type Answers = Vec<(RpcId, Outcome)>;

    pub(crate) struct Job {
        rpc: RpcId,
        key: ThreadKey,
        tab: Option<u64>,
        started: Instant,
        state: State,
    }

    enum State {
        /// Waiting for a renderer to start the tab's engine.
        Engine(BrowserAction),
        Load {
            tab: u64,
            open: bool,
            since: Option<Instant>,
            busy: bool,
        },
        Cdp {
            tab: u64,
            ids: Vec<i32>,
            got: Vec<Option<Value>>,
            next: Next,
        },
    }

    enum Next {
        Snapshot,
        Screenshot,
        /// The element's page rectangle is known; capture it.
        Clip,
        /// The element's centre is known; click it.
        ClickAt,
        /// The input events were sent.
        Done(&'static str),
        Type(String),
        Press(Vec<Value>),
        Eval,
    }

    pub(crate) type Outcome = Result<BrowserCallResult, ErrorObject>;

    fn no_tab(tab: &str) -> ErrorObject {
        error(
            ErrorCode::InvalidParams,
            format!("There is no tab {tab} in this thread; see browser_list."),
        )
    }

    fn page_error(message: impl Into<String>) -> ErrorObject {
        error(ErrorCode::InvalidParams, message)
    }

    impl Agent {
        /// Take new calls from the hub and advance the pending ones. Returns when to run
        /// again (while a call waits, or until an agent mark expires).
        pub(crate) fn step(
            &mut self,
            client: &mut Client,
            browsers: &mut Browsers,
            driver: &mut dyn Driver,
            now: Instant,
        ) -> Option<Duration> {
            let calls = client.take_browser_calls();
            let (answers, next) = self.run(client, browsers, driver, calls, now);
            for (rpc, outcome) in answers {
                client.answer_browser_call(rpc, outcome);
            }
            next
        }

        /// [`Agent::step`] without the hub link: new calls in, finished answers out.
        pub(crate) fn run(
            &mut self,
            client: &mut Client,
            browsers: &mut Browsers,
            driver: &mut dyn Driver,
            calls: Vec<(RpcId, yaac_protocol::attach::BrowserCallParams)>,
            now: Instant,
        ) -> (Answers, Option<Duration>) {
            let mut answers = Vec::new();
            for (rpc, call) in calls {
                let key = ThreadKey {
                    target: call.target,
                    thread: call.call.thread_id,
                };
                let mut job = Job {
                    rpc,
                    key,
                    tab: None,
                    started: now,
                    state: State::Engine(BrowserAction::List(Empty {})),
                };
                match self.begin(client, browsers, driver, &mut job, call.call.action, now) {
                    Some(outcome) => {
                        if let Some(id) = job.tab {
                            self.mark(client, browsers, &job.key, id, now);
                        }
                        answers.push((job.rpc, outcome));
                    }
                    None => {
                        if let Some(id) = job.tab {
                            self.mark(client, browsers, &job.key, id, now);
                        }
                        self.jobs.push(job);
                    }
                }
            }
            let mut replies = driver.take_replies();
            let jobs = std::mem::take(&mut self.jobs);
            for mut job in jobs {
                let outcome = if now.duration_since(job.started) >= CALL_LIMIT {
                    Some(Err(error(
                        ErrorCode::InternalError,
                        "The browser did not finish in time.",
                    )))
                } else {
                    self.advance(browsers, driver, &mut job, &mut replies, now)
                };
                match outcome {
                    Some(outcome) => answers.push((job.rpc, outcome)),
                    None => self.jobs.push(job),
                }
            }
            let acting: Vec<_> = self
                .jobs
                .iter()
                .filter_map(|j| Some((j.key.clone(), j.tab?)))
                .collect();
            let marks = self.expire_marks(browsers, now, &acting);
            let next = if self.jobs.is_empty() {
                marks
            } else {
                Some(POLL)
            };
            (answers, next)
        }

        /// Start a call; `Some` when it finished at once.
        fn begin(
            &mut self,
            client: &mut Client,
            browsers: &mut Browsers,
            driver: &mut dyn Driver,
            job: &mut Job,
            action: BrowserAction,
            now: Instant,
        ) -> Option<Outcome> {
            let key = job.key.clone();
            match action {
                BrowserAction::List(_) => {
                    let tabs = browsers
                        .areas
                        .get(&key)
                        .map(|a| a.tabs.iter().filter(|t| t.kind == crate::file_pane::TabKind::Browser).map(wire_tab).collect())
                        .unwrap_or_default();
                    Some(Ok(BrowserCallResult::List(BrowserTabs { tabs })))
                }
                BrowserAction::Open(p) => match browsers.open_background(client, &key, &p.url) {
                    Ok(id) => {
                        job.tab = Some(id);
                        job.state = State::Load {
                            tab: id,
                            open: true,
                            since: None,
                            busy: false,
                        };
                        None
                    }
                    Err(e) => Some(Err(error(ErrorCode::CapabilityMissing, e))),
                },
                BrowserAction::Unknown(v) => Some(Err(error(
                    ErrorCode::MethodNotFound,
                    format!(
                        "this window does not know the browser action {}",
                        v.tag("kind")
                    ),
                ))),
                action => {
                    let name = action_tab(&action).unwrap_or_default().to_owned();
                    let Some(id) = tab_id(&name).filter(|id| browsers.tab(&key, *id).is_some_and(|t| t.kind == crate::file_pane::TabKind::Browser))
                    else {
                        return Some(Err(no_tab(&name)));
                    };
                    job.tab = Some(id);
                    match action {
                        BrowserAction::Close(_) => {
                            browsers.close(&key, id);
                            Some(Ok(BrowserCallResult::Close(Empty {})))
                        }
                        BrowserAction::Navigate(p) if p.url.is_some() => {
                            let url = p.url.unwrap_or_default();
                            if let Err(e) = browsers.navigate(client, &key, id, &url) {
                                return Some(Err(page_error(e)));
                            }
                            let url = browsers
                                .tab(&key, id)
                                .map(|t| t.url.clone())
                                .unwrap_or_default();
                            if let Some(engine) = driver.engine(id) {
                                engine.send(Action::Navigate(url));
                            }
                            job.state = State::Load {
                                tab: id,
                                open: false,
                                since: None,
                                busy: false,
                            };
                            None
                        }
                        action => {
                            job.state = State::Engine(action);
                            self.advance(browsers, driver, job, &mut Vec::new(), now)
                        }
                    }
                }
            }
        }

        fn advance(
            &mut self,
            browsers: &mut Browsers,
            driver: &mut dyn Driver,
            job: &mut Job,
            replies: &mut Vec<(u64, Value)>,
            now: Instant,
        ) -> Option<Outcome> {
            let key = job.key.clone();
            loop {
                let state =
                    std::mem::replace(&mut job.state, State::Engine(BrowserAction::List(Empty {})));
                match state {
                    State::Engine(action) => {
                        let id = job.tab?;
                        let Some(tab) = browsers.tab(&key, id) else {
                            return Some(Err(no_tab(&id.to_string())));
                        };
                        if let Some(e) = &tab.error {
                            return Some(Err(error(ErrorCode::InternalError, e.clone())));
                        }
                        if tab.url.is_empty() {
                            return Some(Err(page_error(
                                "The tab has no page; use browser_navigate first.",
                            )));
                        }
                        let Some(engine) = driver.engine(id) else {
                            job.state = State::Engine(action);
                            return None;
                        };
                        match start(engine, id, action) {
                            Ok(Started::Now(outcome)) => return Some(Ok(outcome)),
                            Ok(Started::Load) => {
                                job.state = State::Load {
                                    tab: id,
                                    open: false,
                                    since: None,
                                    busy: false,
                                }
                            }
                            Ok(Started::Cdp(ids, next)) => {
                                job.state = State::Cdp {
                                    tab: id,
                                    got: vec![None; ids.len()],
                                    ids,
                                    next,
                                }
                            }
                            Err(e) => return Some(Err(e)),
                        }
                    }
                    State::Load {
                        tab: id,
                        open,
                        since,
                        mut busy,
                    } => {
                        let Some(tab) = browsers.tab(&key, id) else {
                            return Some(Err(error(
                                ErrorCode::InternalError,
                                "The tab was closed.",
                            )));
                        };
                        if let Some(e) = &tab.error {
                            return Some(Err(error(ErrorCode::InternalError, e.clone())));
                        }
                        let since = if tab.started() {
                            Some(since.unwrap_or(now))
                        } else {
                            None
                        };
                        busy |= tab.loading;
                        let page = !tab.url.is_empty() && tab.url != "about:blank";
                        let idle = since.is_some_and(|s| now.duration_since(s) >= LOAD_GRACE);
                        let done = page && !tab.loading && (busy || idle);
                        if done || now.duration_since(job.started) >= LOAD_LIMIT {
                            let state = wire_tab(tab);
                            return Some(Ok(if open {
                                BrowserCallResult::Open(state)
                            } else {
                                BrowserCallResult::Navigate(state)
                            }));
                        }
                        job.state = State::Load {
                            tab: id,
                            open,
                            since,
                            busy,
                        };
                        return None;
                    }
                    State::Cdp {
                        tab: id,
                        ids,
                        mut got,
                        next,
                    } => {
                        replies.retain(|(t, v)| {
                            let Some(i) = (*t == id)
                                .then(|| {
                                    ids.iter()
                                        .position(|n| v["id"].as_i64() == Some(i64::from(*n)))
                                })
                                .flatten()
                            else {
                                return true;
                            };
                            got[i] = Some(v.clone());
                            false
                        });
                        if got.iter().any(Option::is_none) {
                            job.state = State::Cdp {
                                tab: id,
                                ids,
                                got,
                                next,
                            };
                            return None;
                        }
                        let got: Vec<Value> = got.into_iter().flatten().collect();
                        if let Some(e) = got.iter().find_map(|v| v.get("error")) {
                            return Some(Err(error(
                                ErrorCode::InternalError,
                                format!(
                                    "The browser refused: {}",
                                    e["message"].as_str().unwrap_or("unknown error")
                                ),
                            )));
                        }
                        let last = got.last().cloned().unwrap_or(Value::Null);
                        let Some(engine) = driver.engine(id) else {
                            return Some(Err(error(
                                ErrorCode::InternalError,
                                "The tab's browser stopped.",
                            )));
                        };
                        match finish(engine, next, &last) {
                            Ok(Started::Now(outcome)) => return Some(Ok(outcome)),
                            Ok(Started::Cdp(ids, next)) => {
                                job.state = State::Cdp {
                                    tab: id,
                                    got: vec![None; ids.len()],
                                    ids,
                                    next,
                                }
                            }
                            Ok(Started::Load) => {
                                job.state = State::Load {
                                    tab: id,
                                    open: false,
                                    since: None,
                                    busy: false,
                                }
                            }
                            Err(e) => return Some(Err(e)),
                        }
                    }
                }
            }
        }
    }

    fn action_tab(action: &BrowserAction) -> Option<&str> {
        Some(match action {
            BrowserAction::Screenshot(p) => &p.tab,
            BrowserAction::Snapshot(p) | BrowserAction::Close(p) => &p.tab,
            BrowserAction::Console(p) | BrowserAction::Network(p) => &p.tab,
            BrowserAction::Navigate(p) => &p.tab,
            BrowserAction::Click(p) => &p.tab,
            BrowserAction::Type(p) => &p.tab,
            BrowserAction::Press(p) => &p.tab,
            BrowserAction::Eval(p) => &p.tab,
            BrowserAction::List(_) | BrowserAction::Open(_) | BrowserAction::Unknown(_) => {
                return None;
            }
        })
    }

    enum Started {
        Now(BrowserCallResult),
        Load,
        Cdp(Vec<i32>, Next),
    }

    fn send(engine: &mut dyn Engine, commands: Vec<(&str, Value)>, next: Next) -> Started {
        let ids = commands
            .into_iter()
            .map(|(m, p)| engine.devtools_command(m, p))
            .collect();
        Started::Cdp(ids, next)
    }

    /// `Runtime.evaluate` of a page function applied to JSON arguments.
    fn call(function: &str, args: Value) -> (&'static str, Value) {
        let expression = format!("({function})({args})");
        (
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
        )
    }

    fn target(reference: &Option<String>, selector: &Option<String>) -> Value {
        json!({ "ref": reference, "selector": selector })
    }

    fn start(
        engine: &mut dyn Engine,
        _id: u64,
        action: BrowserAction,
    ) -> Result<Started, ErrorObject> {
        Ok(match action {
            BrowserAction::Snapshot(_) => {
                send(engine, vec![call(SNAPSHOT_JS, json!({}))], Next::Snapshot)
            }
            BrowserAction::Screenshot(p) => match p.selector {
                Some(selector) => send(
                    engine,
                    vec![call(&RECT_JS, target(&None, &Some(selector)))],
                    Next::Clip,
                ),
                None => send(
                    engine,
                    vec![("Page.captureScreenshot", json!({ "format": "png" }))],
                    Next::Screenshot,
                ),
            },
            BrowserAction::Console(p) => Started::Now(BrowserCallResult::Console(console(
                engine,
                p.cursor.unwrap_or(0),
            ))),
            BrowserAction::Network(p) => Started::Now(BrowserCallResult::Network(network(
                engine,
                p.cursor.unwrap_or(0),
            ))),
            BrowserAction::Navigate(p) => {
                engine.send(match p.history {
                    Some(BrowserHistory::Back) => Action::Back,
                    Some(BrowserHistory::Forward) => Action::Forward,
                    Some(BrowserHistory::Reload) => Action::Reload,
                    _ => return Err(page_error("Give url, or history back, forward or reload.")),
                });
                Started::Load
            }
            BrowserAction::Click(p) => match (p.x, p.y) {
                (Some(x), Some(y)) => send(
                    engine,
                    click_events(f64::from(x), f64::from(y)),
                    Next::Done("click"),
                ),
                _ => send(
                    engine,
                    vec![call(&CENTER_JS, target(&p.reference, &p.selector))],
                    Next::ClickAt,
                ),
            },
            BrowserAction::Type(p) => {
                let args = json!({ "ref": p.reference, "selector": p.selector, "clear": p.clear.unwrap_or(false), "empty": p.text.is_empty() });
                send(engine, vec![call(&FOCUS_JS, args)], Next::Type(p.text))
            }
            BrowserAction::Press(p) => {
                let events = key_events(&p.key).map_err(page_error)?;
                if p.reference.is_some() || p.selector.is_some() {
                    let args = json!({ "ref": p.reference, "selector": p.selector, "clear": false, "empty": true });
                    send(engine, vec![call(&FOCUS_JS, args)], Next::Press(events))
                } else {
                    send(
                        engine,
                        events
                            .into_iter()
                            .map(|e| ("Input.dispatchKeyEvent", e))
                            .collect(),
                        Next::Done("press"),
                    )
                }
            }
            BrowserAction::Eval(p) => send(
                engine,
                vec![(
                    "Runtime.evaluate",
                    json!({ "expression": p.expression, "returnByValue": true, "awaitPromise": true, "userGesture": true }),
                )],
                Next::Eval,
            ),
            _ => return Err(error(ErrorCode::InternalError, "unexpected browser action")),
        })
    }

    /// The value of a `Runtime.evaluate` reply, or the page's exception as an error.
    fn evaluated(reply: &Value) -> Result<Value, ErrorObject> {
        let result = &reply["result"];
        if let Some(details) = result.get("exceptionDetails") {
            let text = details["exception"]["description"]
                .as_str()
                .or(details["text"].as_str())
                .unwrap_or("The page threw an exception.");
            return Err(page_error(text.lines().next().unwrap_or(text).to_owned()));
        }
        Ok(result["result"]
            .get("value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn finish(engine: &mut dyn Engine, next: Next, reply: &Value) -> Result<Started, ErrorObject> {
        Ok(match next {
            Next::Snapshot => {
                let tree = evaluated(reply)?.as_str().unwrap_or_default().to_owned();
                Started::Now(BrowserCallResult::Snapshot(BrowserSnapshot { tree }))
            }
            Next::Screenshot => {
                let data = reply["result"]["data"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                Started::Now(BrowserCallResult::Screenshot(BrowserScreenshot { data }))
            }
            Next::Clip => {
                let r = evaluated(reply)?;
                let clip = json!({ "x": r["x"], "y": r["y"], "width": r["width"], "height": r["height"], "scale": 1 });
                send(
                    engine,
                    vec![(
                        "Page.captureScreenshot",
                        json!({ "format": "png", "clip": clip }),
                    )],
                    Next::Screenshot,
                )
            }
            Next::ClickAt => {
                let r = evaluated(reply)?;
                let (x, y) = (r["x"].as_f64().unwrap_or(0.), r["y"].as_f64().unwrap_or(0.));
                send(engine, click_events(x, y), Next::Done("click"))
            }
            Next::Type(text) => {
                evaluated(reply)?;
                if text.is_empty() {
                    return Ok(Started::Now(BrowserCallResult::Type(Empty {})));
                }
                send(
                    engine,
                    vec![("Input.insertText", json!({ "text": text }))],
                    Next::Done("type"),
                )
            }
            Next::Press(events) => {
                evaluated(reply)?;
                send(
                    engine,
                    events
                        .into_iter()
                        .map(|e| ("Input.dispatchKeyEvent", e))
                        .collect(),
                    Next::Done("press"),
                )
            }
            Next::Eval => Started::Now(BrowserCallResult::Eval(BrowserEvalResult {
                value: evaluated(reply)?,
            })),
            Next::Done(kind) => Started::Now(match kind {
                "click" => BrowserCallResult::Click(Empty {}),
                "type" => BrowserCallResult::Type(Empty {}),
                _ => BrowserCallResult::Press(Empty {}),
            }),
        })
    }

    fn click_events(x: f64, y: f64) -> Vec<(&'static str, Value)> {
        ["mouseMoved", "mousePressed", "mouseReleased"]
            .into_iter()
            .map(|kind| {
                let button = if kind == "mouseMoved" { "none" } else { "left" };
                (
                    "Input.dispatchMouseEvent",
                    json!({ "type": kind, "x": x, "y": y, "button": button, "clickCount": 1 }),
                )
            })
            .collect()
    }

    /// `Input.dispatchKeyEvent` down and up for `Control+Shift+a`-style names.
    pub(super) fn key_events(name: &str) -> Result<Vec<Value>, String> {
        let mut parts: Vec<&str> = name.split('+').collect();
        // "Control++" presses the plus key.
        if name.ends_with("++") {
            parts.pop();
            *parts.last_mut().unwrap() = "+";
        }
        let key = parts
            .pop()
            .filter(|k| !k.is_empty())
            .ok_or("No key named.")?;
        let mut modifiers = 0;
        for m in parts {
            modifiers |= match m.to_ascii_lowercase().as_str() {
                "alt" | "option" => 1,
                "control" | "ctrl" => 2,
                "meta" | "cmd" | "command" => 4,
                "shift" => 8,
                _ => {
                    return Err(format!(
                        "Unknown modifier {m}; use Alt, Control, Meta or Shift."
                    ));
                }
            };
        }
        let named: &[(&str, &str, i64, &str)] = &[
            ("Enter", "Enter", 13, "\r"),
            ("Tab", "Tab", 9, ""),
            ("Escape", "Escape", 27, ""),
            ("Backspace", "Backspace", 8, ""),
            ("Delete", "Delete", 46, ""),
            ("Insert", "Insert", 45, ""),
            ("ArrowLeft", "ArrowLeft", 37, ""),
            ("ArrowUp", "ArrowUp", 38, ""),
            ("ArrowRight", "ArrowRight", 39, ""),
            ("ArrowDown", "ArrowDown", 40, ""),
            ("Home", "Home", 36, ""),
            ("End", "End", 35, ""),
            ("PageUp", "PageUp", 33, ""),
            ("PageDown", "PageDown", 34, ""),
            ("Space", "Space", 32, " "),
        ];
        let (key, code, vk, text) = if let Some(&(k, c, v, t)) = named
            .iter()
            .find(|(k, ..)| k.eq_ignore_ascii_case(key) || (key == "Esc" && *k == "Escape"))
        {
            (k.to_owned(), c.to_owned(), v, t.to_owned())
        } else if let Some(n) = key
            .strip_prefix(['F', 'f'])
            .and_then(|n| n.parse::<i64>().ok())
            .filter(|n| (1..=12).contains(n))
        {
            (format!("F{n}"), format!("F{n}"), 111 + n, String::new())
        } else {
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return Err(format!(
                    "Unknown key {key}; use a single character or a name like Enter, Tab, Escape, ArrowDown, F5."
                ));
            };
            let code = if c.is_ascii_alphabetic() {
                format!("Key{}", c.to_ascii_uppercase())
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            };
            let vk = if c.is_ascii_alphanumeric() {
                i64::from(c.to_ascii_uppercase() as u8)
            } else {
                0
            };
            (c.to_string(), code, vk, c.to_string())
        };
        // Text is produced only without command modifiers, as on a keyboard.
        let text = if modifiers & 7 == 0 {
            text
        } else {
            String::new()
        };
        let mut down = json!({ "type": if text.is_empty() { "rawKeyDown" } else { "keyDown" }, "key": key, "code": code,
            "windowsVirtualKeyCode": vk, "modifiers": modifiers });
        if !text.is_empty() {
            down["text"] = json!(text);
            down["unmodifiedText"] = json!(text);
        }
        let up = json!({ "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers });
        Ok(vec![down, up])
    }

    const MAX_ENTRIES: usize = 500;
    const MAX_TEXT: usize = 2000;

    fn clip(mut s: String) -> String {
        if s.len() > MAX_TEXT {
            let mut end = MAX_TEXT;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
            s.push('…');
        }
        s
    }

    /// Console messages and uncaught exceptions after `cursor`.
    pub(super) fn console_entries(events: &[(u64, Value)], cursor: u64) -> BrowserConsole {
        let next = events.last().map_or(cursor, |(n, _)| (*n).max(cursor));
        let mut entries: Vec<_> = events
            .iter()
            .filter(|(n, _)| *n > cursor)
            .filter_map(|(_, v)| {
                let p = &v["params"];
                match v["method"].as_str()? {
                    "Runtime.consoleAPICalled" => {
                        let text = p["args"].as_array().map(|args| {
                            args.iter()
                                .map(|a| match &a["value"] {
                                    Value::String(s) => s.clone(),
                                    Value::Null => a["description"]
                                        .as_str()
                                        .or(a["type"].as_str())
                                        .unwrap_or("")
                                        .to_owned(),
                                    other => other.to_string(),
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        });
                        Some(BrowserConsoleEntry {
                            level: p["type"].as_str().unwrap_or("log").to_owned(),
                            text: clip(text.unwrap_or_default()),
                        })
                    }
                    "Runtime.exceptionThrown" => {
                        let d = &p["exceptionDetails"];
                        let text = d["exception"]["description"]
                            .as_str()
                            .or(d["text"].as_str())
                            .unwrap_or("Uncaught exception");
                        Some(BrowserConsoleEntry {
                            level: "exception".into(),
                            text: clip(text.to_owned()),
                        })
                    }
                    _ => None,
                }
            })
            .collect();
        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }
        BrowserConsole {
            entries,
            cursor: next,
        }
    }

    /// Requests with an event after `cursor`; earlier events of the same request fill in
    /// its URL and method.
    pub(super) fn network_entries(events: &[(u64, Value)], cursor: u64) -> BrowserNetwork {
        let next = events.last().map_or(cursor, |(n, _)| (*n).max(cursor));
        #[derive(Default)]
        struct Request {
            last: u64,
            entry: Option<BrowserNetworkEntry>,
            start: Option<f64>,
        }
        let mut order: Vec<String> = Vec::new();
        let mut requests: HashMap<String, Request> = HashMap::new();
        for (n, v) in events {
            let Some(method) = v["method"].as_str().filter(|m| m.starts_with("Network.")) else {
                continue;
            };
            let p = &v["params"];
            let Some(id) = p["requestId"].as_str() else {
                continue;
            };
            let r = requests.entry(id.to_owned()).or_insert_with(|| {
                order.push(id.to_owned());
                Request::default()
            });
            r.last = *n;
            let e = r.entry.get_or_insert_with(|| BrowserNetworkEntry {
                url: String::new(),
                method: String::new(),
                status: None,
                duration_ms: None,
                error: None,
            });
            let ms = |end: &Value, start: Option<f64>| {
                end.as_f64()
                    .zip(start)
                    .map(|(e, s)| ((e - s).max(0.) * 1000.) as u64)
            };
            match method {
                "Network.requestWillBeSent" => {
                    e.url = clip(p["request"]["url"].as_str().unwrap_or_default().to_owned());
                    e.method = p["request"]["method"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    r.start = p["timestamp"].as_f64();
                }
                "Network.responseReceived" => {
                    e.status = p["response"]["status"]
                        .as_u64()
                        .and_then(|s| u16::try_from(s).ok());
                    if e.url.is_empty() {
                        e.url = clip(p["response"]["url"].as_str().unwrap_or_default().to_owned());
                    }
                }
                "Network.loadingFinished" => e.duration_ms = ms(&p["timestamp"], r.start),
                "Network.loadingFailed" => {
                    e.duration_ms = ms(&p["timestamp"], r.start);
                    e.error = Some(p["errorText"].as_str().unwrap_or("failed").to_owned());
                }
                _ => {}
            }
        }
        let mut entries: Vec<_> = order
            .iter()
            .filter_map(|id| requests.remove(id))
            .filter(|r| r.last > cursor)
            .filter_map(|r| r.entry)
            .collect();
        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }
        BrowserNetwork {
            entries,
            cursor: next,
        }
    }

    fn console(engine: &dyn Engine, cursor: u64) -> BrowserConsole {
        console_entries(&engine.diagnostics_since(cursor), cursor)
    }

    fn network(engine: &dyn Engine, cursor: u64) -> BrowserNetwork {
        network_entries(&engine.diagnostics_since(0), cursor)
    }

    /// Resolves `{ref, selector}` to an element, from the refs the last snapshot handed out.
    const RESOLVE_JS: &str = r#"function resolve(a) {
  const st = globalThis[Symbol.for("yaac.refs")];
  if (a.ref) {
    const el = st && st.els.get(a.ref) && st.els.get(a.ref).deref();
    if (!el || !el.isConnected) throw new Error("Reference " + a.ref + " is not on the page; take a new browser_snapshot.");
    return el;
  }
  const el = document.querySelector(a.selector);
  if (!el) throw new Error("No element matches " + a.selector);
  return el;
}"#;

    /// Viewport centre of an element, scrolled into view.
    static CENTER_JS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            r#"(a) => {{ {RESOLVE_JS}
  const el = resolve(a);
  el.scrollIntoView({{ block: "center", inline: "center", behavior: "instant" }});
  const r = el.getBoundingClientRect();
  if (!r.width && !r.height) throw new Error("The element has no size on the page.");
  return {{ x: r.left + r.width / 2, y: r.top + r.height / 2 }};
}}"#
        )
    });

    /// Page rectangle of an element, scrolled into view, for a clipped screenshot.
    static RECT_JS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            r#"(a) => {{ {RESOLVE_JS}
  const el = resolve(a);
  el.scrollIntoView({{ block: "center", inline: "center", behavior: "instant" }});
  const r = el.getBoundingClientRect();
  if (!r.width || !r.height) throw new Error("The element has no size on the page.");
  return {{ x: r.left + scrollX, y: r.top + scrollY, width: r.width, height: r.height }};
}}"#
        )
    });

    /// Focus the named element (or keep the focused one) and place the caret at the end, or
    /// select everything for `clear`, so the following `Input.insertText` replaces it.
    static FOCUS_JS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            r#"(a) => {{ {RESOLVE_JS}
  const el = (a.ref || a.selector) ? resolve(a) : document.activeElement;
  if (!el || el === document.body || el === document.documentElement) throw new Error("No element has focus; give ref or selector.");
  el.focus();
  const field = typeof el.setSelectionRange === "function" && "value" in el;
  if (a.clear) {{
    if (field) el.select(); else document.execCommand("selectAll");
    if (a.empty) document.execCommand("delete");
  }} else if (field) {{
    try {{ const n = el.value.length; el.setSelectionRange(n, n); }} catch (e) {{}}
  }}
  return true;
}}"#
        )
    });

    /// The page as an indented accessibility-style outline; interactive elements get a
    /// `[ref=eN]` that stays the same for the element's life.
    const SNAPSHOT_JS: &str = r#"() => {
  const S = Symbol.for("yaac.refs");
  const st = globalThis[S] || (globalThis[S] = { next: 0, ids: new WeakMap(), els: new Map() });
  const refOf = (el) => { let r = st.ids.get(el); if (!r) { r = "e" + (++st.next); st.ids.set(el, r); st.els.set(r, new WeakRef(el)); } return r; };
  const LIMIT = 200000; let size = 0, cut = false;
  const clean = (s) => (s || "").replace(/\s+/g, " ").trim();
  const short = (s, n) => s.length > n ? s.slice(0, n) + "…" : s;
  const q = (s) => JSON.stringify(short(clean(s), 120));
  const out = ["page " + q(document.title) + " " + location.href];
  const push = (depth, s) => { if (cut) return; size += s.length + depth * 2 + 3; if (size > LIMIT) { cut = true; out.push("… (truncated)"); return; } out.push("  ".repeat(depth) + "- " + s); };
  const SKIP = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "HEAD", "META", "LINK"]);
  const ACT = new Set(["link", "button", "textbox", "searchbox", "checkbox", "radio", "slider", "spinbutton", "combobox", "option", "menuitem", "menuitemcheckbox", "menuitemradio", "tab", "switch", "treeitem"]);
  const LEAF = new Set(["link", "button", "heading", "img", "textbox", "searchbox", "checkbox", "radio", "slider", "spinbutton", "combobox", "option", "switch", "iframe"]);
  const roleOf = (el) => {
    const explicit = el.getAttribute("role"); if (explicit) return explicit.split(" ")[0];
    switch (el.tagName) {
      case "A": return el.hasAttribute("href") ? "link" : null;
      case "BUTTON": case "SUMMARY": return "button";
      case "INPUT": { const t = (el.type || "text").toLowerCase();
        if (t === "hidden") return "hidden";
        if (["button", "submit", "reset", "image"].includes(t)) return "button";
        if (t === "checkbox" || t === "radio") return t;
        if (t === "range") return "slider";
        return "textbox"; }
      case "TEXTAREA": return "textbox";
      case "SELECT": return "combobox";
      case "OPTION": return "option";
      case "H1": case "H2": case "H3": case "H4": case "H5": case "H6": return "heading";
      case "IMG": return "img";
      case "UL": case "OL": return "list";
      case "LI": return "listitem";
      case "NAV": return "navigation";
      case "MAIN": return "main";
      case "FORM": return "form";
      case "DIALOG": return "dialog";
      case "TABLE": return "table";
      case "TR": return "row";
      case "TD": return "cell";
      case "TH": return "columnheader";
      case "IFRAME": return "iframe";
    }
    return el.isContentEditable && el.getAttribute("contenteditable") !== null ? "textbox" : null;
  };
  const nameOf = (el, role) => clean(
    el.getAttribute("aria-label") ||
    (el.getAttribute("aria-labelledby") || "").split(" ").map((id) => (document.getElementById(id) || {}).innerText || "").join(" ") ||
    (el.labels && el.labels[0] && el.labels[0].innerText) || el.getAttribute("alt") || el.getAttribute("title") ||
    el.getAttribute("placeholder") || (LEAF.has(role) && role !== "textbox" && role !== "combobox" ? el.innerText || el.textContent : ""));
  const hidden = (el) => { if (el.hidden || el.getAttribute("aria-hidden") === "true") return true; const cs = getComputedStyle(el); return cs.display === "none" || cs.visibility === "hidden"; };
  const walk = (node, depth) => {
    const kids = node.shadowRoot ? [...node.shadowRoot.childNodes, ...node.childNodes] : node.childNodes;
    for (const child of kids) {
      if (cut) return;
      if (child.nodeType === 3) { const t = clean(child.nodeValue); if (t) push(depth, "text: " + short(t, 300)); continue; }
      if (child.nodeType !== 1 || SKIP.has(child.tagName) || hidden(child)) continue;
      const role = roleOf(child);
      if (role === "hidden" || role === "presentation" || role === "none") { if (role !== "hidden") walk(child, depth); continue; }
      if (!role) { walk(child, depth); continue; }
      let line = role;
      const name = nameOf(child, role);
      if (name) line += " " + q(name);
      if (role === "heading" && /^H[1-6]$/.test(child.tagName)) line += " level=" + child.tagName[1];
      if ("value" in child && (role === "textbox" || role === "searchbox" || role === "combobox" || role === "slider" || role === "spinbutton")) line += " value=" + q(String(child.value));
      if (role === "checkbox" || role === "radio" || role === "switch") line += child.checked || child.getAttribute("aria-checked") === "true" ? " checked" : " unchecked";
      if (child.disabled || child.getAttribute("aria-disabled") === "true") line += " disabled";
      if (role === "link") line += " -> " + child.getAttribute("href");
      if (role === "iframe") line += " " + (child.getAttribute("src") || "");
      if (ACT.has(role) || child.onclick) line += " [ref=" + refOf(child) + "]";
      push(depth, line);
      if (!LEAF.has(role)) walk(child, depth + 1);
    }
  };
  if (document.body) walk(document.body, 0);
  return out.join("\n");
}"#;
}

#[cfg(all(test, feature = "preview"))]
mod tests {
    use super::engine::*;
    use serde_json::json;

    #[test]
    fn key_names_become_down_and_up_events() {
        let enter = key_events("Enter").unwrap();
        assert_eq!(enter[0]["type"], "keyDown");
        assert_eq!(enter[0]["text"], "\r");
        assert_eq!(enter[0]["windowsVirtualKeyCode"], 13);
        assert_eq!(enter[1]["type"], "keyUp");
        let select_all = key_events("Control+a").unwrap();
        assert_eq!(select_all[0]["type"], "rawKeyDown");
        assert_eq!(select_all[0]["modifiers"], 2);
        assert_eq!(select_all[0]["code"], "KeyA");
        assert!(select_all[0].get("text").is_none());
        assert_eq!(key_events("Shift+Tab").unwrap()[0]["modifiers"], 8);
        assert_eq!(key_events("Control++").unwrap()[0]["key"], "+");
        assert_eq!(key_events("F5").unwrap()[0]["windowsVirtualKeyCode"], 116);
        assert!(key_events("Hyper+x").is_err());
        assert!(key_events("NotAKey").is_err());
    }

    #[test]
    fn console_and_network_read_after_the_cursor() {
        let events = vec![
            (
                1,
                json!({"method":"Network.requestWillBeSent","params":{"requestId":"1","request":{"url":"http://localhost:5173/","method":"GET"},"timestamp":1.0}}),
            ),
            (
                2,
                json!({"method":"Runtime.consoleAPICalled","params":{"type":"warning","args":[{"type":"string","value":"careful"},{"type":"number","value":3}]}}),
            ),
            (
                3,
                json!({"method":"Network.responseReceived","params":{"requestId":"1","response":{"status":200,"url":"http://localhost:5173/"}}}),
            ),
            (
                4,
                json!({"method":"Network.loadingFinished","params":{"requestId":"1","timestamp":1.25}}),
            ),
            (
                5,
                json!({"method":"Runtime.exceptionThrown","params":{"exceptionDetails":{"text":"Uncaught","exception":{"description":"TypeError: x is undefined"}}}}),
            ),
            (
                6,
                json!({"method":"Network.loadingFailed","params":{"requestId":"2","errorText":"net::ERR_CONNECTION_REFUSED"}}),
            ),
        ];
        let console = console_entries(&events, 0);
        assert_eq!(console.cursor, 6);
        assert_eq!(console.entries.len(), 2);
        assert_eq!(console.entries[0].level, "warning");
        assert_eq!(console.entries[0].text, "careful 3");
        assert_eq!(console.entries[1].level, "exception");
        assert_eq!(console_entries(&events[3..], 4).entries.len(), 1);
        let network = network_entries(&events, 0);
        assert_eq!(network.entries.len(), 2);
        assert_eq!(network.entries[0].url, "http://localhost:5173/");
        assert_eq!(network.entries[0].status, Some(200));
        assert_eq!(network.entries[0].duration_ms, Some(250));
        assert_eq!(
            network.entries[1].error.as_deref(),
            Some("net::ERR_CONNECTION_REFUSED")
        );
        // Only the request with an event after the cursor; its URL comes from earlier events.
        let later = network_entries(&events, 3);
        assert_eq!(later.entries.len(), 2);
        assert_eq!(network_entries(&events, 4).entries.len(), 1);
        assert_eq!(network_entries(&events, 6).entries.len(), 0);
    }

    use super::Agent;
    use crate::preview_controller::Browsers;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use yaac_core::{ThreadKey, client::Client, demo::threads};
    use yaac_preview::wire::Action;
    use yaac_protocol::rpc::{ErrorCode, RpcId};
    use yaac_protocol::{BrowserCallResult, attach};

    #[derive(Default)]
    struct FakeEngine {
        sent: Vec<String>,
        commands: Vec<(i32, String, Value)>,
        next: i32,
        diagnostics: Vec<(u64, Value)>,
    }

    impl Engine for FakeEngine {
        fn send(&mut self, action: Action) {
            self.sent.push(format!("{action:?}"));
        }
        fn devtools_command(&mut self, method: &str, params: Value) -> i32 {
            self.next += 1;
            self.commands.push((self.next, method.into(), params));
            self.next
        }
        fn diagnostics_since(&self, cursor: u64) -> Vec<(u64, Value)> {
            self.diagnostics
                .iter()
                .filter(|(n, _)| *n > cursor)
                .cloned()
                .collect()
        }
    }

    #[derive(Default)]
    struct FakeDriver {
        tabs: HashMap<u64, FakeEngine>,
        replies: Vec<(u64, Value)>,
    }

    impl Driver for FakeDriver {
        fn engine(&mut self, id: u64) -> Option<&mut dyn Engine> {
            self.tabs.get_mut(&id).map(|e| e as &mut dyn Engine)
        }
        fn take_replies(&mut self) -> Vec<(u64, Value)> {
            std::mem::take(&mut self.replies)
        }
    }

    impl FakeDriver {
        /// Answer every DevTools command not answered yet with `result`.
        fn answer(&mut self, tab: u64, result: Value) {
            let engine = self.tabs.get_mut(&tab).unwrap();
            for (id, _, _) in engine.commands.drain(..) {
                self.replies
                    .push((tab, json!({ "id": id, "result": result })));
            }
        }
        fn methods(&self, tab: u64) -> Vec<String> {
            self.tabs[&tab]
                .commands
                .iter()
                .map(|(_, m, _)| m.clone())
                .collect()
        }
    }

    struct World {
        client: Client,
        browsers: Browsers,
        driver: FakeDriver,
        agent: Agent,
        key: ThreadKey,
        now: Instant,
        serial: u64,
    }

    impl World {
        fn new() -> Self {
            let (mut client, _) = crate::app::demo_client(yaac_core::demo::Scenario::Fleet);
            let key = threads::orchestrator();
            client
                .state
                .targets
                .iter_mut()
                .find(|t| t.entry.id == key.target)
                .unwrap()
                .entry
                .state
                .daemon = Some(attach::DaemonSummary {
                build: "test".into(),
                older: false,
                schema_hash_matches: true,
                can_update: false,
                port_forward: true,
                file_view: false,
            });
            let mut browsers = Browsers::default();
            browsers.assume_host();
            Self {
                client,
                browsers,
                driver: FakeDriver::default(),
                agent: Agent::default(),
                key,
                now: Instant::now(),
                serial: 0,
            }
        }
        fn tab(&mut self, key: &ThreadKey, url: &str) -> u64 {
            let id = self.browsers.started_tab(key, url);
            self.driver.tabs.insert(id, FakeEngine::default());
            id
        }
        /// Offer calls as `key`'s agent and advance once; the answers by call number.
        fn run(
            &mut self,
            key: &ThreadKey,
            actions: Vec<Value>,
        ) -> Vec<(
            u64,
            Result<BrowserCallResult, yaac_protocol::rpc::ErrorObject>,
        )> {
            let calls = actions
                .into_iter()
                .map(|action| {
                    self.serial += 1;
                    let params =
                        json!({ "target": key.target, "threadId": key.thread, "action": action });
                    (
                        RpcId::Number(self.serial as i64),
                        serde_json::from_value(params).unwrap(),
                    )
                })
                .collect();
            let (answers, _) = self.agent.run(
                &mut self.client,
                &mut self.browsers,
                &mut self.driver,
                calls,
                self.now,
            );
            answers
                .into_iter()
                .map(|(rpc, outcome)| match rpc {
                    RpcId::Number(n) => (n as u64, outcome),
                    other => panic!("unexpected id {other:?}"),
                })
                .collect()
        }
        fn one(
            &mut self,
            action: Value,
        ) -> Option<Result<BrowserCallResult, yaac_protocol::rpc::ErrorObject>> {
            let key = self.key.clone();
            self.run(&key, vec![action]).pop().map(|(_, o)| o)
        }
        fn tick(&mut self) -> Option<Result<BrowserCallResult, yaac_protocol::rpc::ErrorObject>> {
            let key = self.key.clone();
            self.run(&key, vec![]).pop().map(|(_, o)| o)
        }
    }

    #[test]
    fn an_agent_sees_only_its_own_threads_tabs() {
        let mut w = World::new();
        let mine = w.tab(&w.key.clone(), "http://localhost:5173/");
        let other_key = threads::users();
        let theirs = w.tab(&other_key, "http://localhost:3000/");
        let Some(Ok(BrowserCallResult::List(list))) = w.one(json!({ "kind": "list" })) else {
            panic!()
        };
        assert_eq!(
            list.tabs.iter().map(|t| t.tab.clone()).collect::<Vec<_>>(),
            vec![mine.to_string()]
        );
        // Another thread's tab id is not there for this agent, for reading or acting.
        for action in [
            json!({ "kind": "snapshot", "tab": theirs.to_string() }),
            json!({ "kind": "eval", "tab": theirs.to_string(), "expression": "1" }),
            json!({ "kind": "close", "tab": theirs.to_string() }),
            json!({ "kind": "click", "tab": "nonsense", "x": 1, "y": 1 }),
        ] {
            let Some(Err(e)) = w.one(action) else {
                panic!()
            };
            assert_eq!(e.code, ErrorCode::InvalidParams);
            assert!(e.message.contains("browser_list"));
        }
        assert!(w.driver.tabs[&theirs].commands.is_empty());
        assert!(w.browsers.tab(&other_key, theirs).is_some());
        let Some(Err(e)) = w.one(json!({ "kind": "zoom", "tab": mine.to_string() })) else {
            panic!()
        };
        assert_eq!(e.code, ErrorCode::MethodNotFound);
    }

    #[test]
    fn eval_marks_the_tab_while_acting_and_returns_the_json_value() {
        let mut w = World::new();
        let key = w.key.clone();
        let id = w.tab(&key, "http://localhost:5173/");
        assert!(
            w.one(json!({ "kind": "eval", "tab": id.to_string(), "expression": "1 + 1" }))
                .is_none()
        );
        assert!(
            w.browsers.tab(&key, id).unwrap().agent.is_some(),
            "marked while acting"
        );
        let (_, method, params) = &w.driver.tabs[&id].commands[0];
        assert_eq!(method, "Runtime.evaluate");
        assert_eq!(params["expression"], "1 + 1");
        assert_eq!(params["awaitPromise"], true);
        w.driver
            .answer(id, json!({ "result": { "type": "number", "value": 2 } }));
        let Some(Ok(BrowserCallResult::Eval(r))) = w.tick() else {
            panic!()
        };
        assert_eq!(r.value, json!(2));
        // An exception in the page is the agent's error, not a crash.
        w.one(json!({ "kind": "eval", "tab": id.to_string(), "expression": "x.y" }));
        w.driver.answer(id, json!({ "result": { "type": "object" }, "exceptionDetails": { "text": "Uncaught", "exception": { "description": "TypeError: x is undefined\n    at <anonymous>" } } }));
        let Some(Err(e)) = w.tick() else { panic!() };
        assert_eq!(e.message, "TypeError: x is undefined");
        // The mark lingers briefly, then goes.
        assert!(w.browsers.tab(&key, id).unwrap().agent.is_some());
        w.now += Duration::from_secs(4);
        w.tick();
        assert!(w.browsers.tab(&key, id).unwrap().agent.is_none());
    }

    #[test]
    fn click_type_and_press_dispatch_input_after_locating_the_element() {
        let mut w = World::new();
        let key = w.key.clone();
        let id = w.tab(&key, "http://localhost:5173/");
        let tab = id.to_string();
        assert!(
            w.one(json!({ "kind": "click", "tab": tab, "ref": "e3" }))
                .is_none()
        );
        assert!(
            w.driver.tabs[&id].commands[0].2["expression"]
                .as_str()
                .unwrap()
                .contains(r#"{"ref":"e3","selector":null}"#)
        );
        w.driver
            .answer(id, json!({ "result": { "value": { "x": 10.5, "y": 20 } } }));
        assert!(w.tick().is_none());
        assert_eq!(w.driver.methods(id), vec!["Input.dispatchMouseEvent"; 3]);
        assert_eq!(w.driver.tabs[&id].commands[1].2["type"], "mousePressed");
        assert_eq!(w.driver.tabs[&id].commands[1].2["x"], 10.5);
        w.driver.answer(id, json!({}));
        assert!(matches!(w.tick(), Some(Ok(BrowserCallResult::Click(_)))));

        assert!(w.one(json!({ "kind": "type", "tab": tab, "selector": "#q", "text": "hello", "clear": true })).is_none());
        assert!(
            w.driver.tabs[&id].commands[0].2["expression"]
                .as_str()
                .unwrap()
                .contains(r##""selector":"#q""##)
        );
        w.driver.answer(id, json!({ "result": { "value": true } }));
        w.tick();
        assert_eq!(w.driver.methods(id), vec!["Input.insertText"]);
        assert_eq!(w.driver.tabs[&id].commands[0].2["text"], "hello");
        w.driver.answer(id, json!({}));
        assert!(matches!(w.tick(), Some(Ok(BrowserCallResult::Type(_)))));

        assert!(
            w.one(json!({ "kind": "press", "tab": tab, "key": "Enter" }))
                .is_none()
        );
        assert_eq!(w.driver.methods(id), vec!["Input.dispatchKeyEvent"; 2]);
        w.driver.answer(id, json!({}));
        assert!(matches!(w.tick(), Some(Ok(BrowserCallResult::Press(_)))));
        let Some(Err(e)) = w.one(json!({ "kind": "press", "tab": tab, "key": "Hyper+x" })) else {
            panic!()
        };
        assert_eq!(e.code, ErrorCode::InvalidParams);
        // A refusal from the browser is reported, not retried.
        w.one(json!({ "kind": "snapshot", "tab": tab }));
        let (n, _, _) = w.driver.tabs.get_mut(&id).unwrap().commands.remove(0);
        w.driver.replies.push((
            id,
            json!({ "id": n, "error": { "message": "Target closed" } }),
        ));
        let Some(Err(e)) = w.tick() else { panic!() };
        assert!(e.message.contains("Target closed"));
    }

    #[test]
    fn navigate_waits_for_the_load_within_its_bound_and_close_removes_the_tab() {
        let mut w = World::new();
        let key = w.key.clone();
        let id = w.tab(&key, "http://localhost:5173/");
        let tab = id.to_string();
        assert!(
            w.one(json!({ "kind": "navigate", "tab": tab, "history": "back" }))
                .is_none()
        );
        assert_eq!(w.driver.tabs[&id].sent, vec!["Back"]);
        w.browsers.tab_mut(&key, id).unwrap().loading = true;
        assert!(w.tick().is_none());
        let t = w.browsers.tab_mut(&key, id).unwrap();
        t.loading = false;
        t.url = "http://localhost:5173/previous".into();
        let Some(Ok(BrowserCallResult::Navigate(state))) = w.tick() else {
            panic!()
        };
        assert_eq!(state.url, "http://localhost:5173/previous");
        assert!(!state.loading);
        // A load that never ends is answered at the bound with loading still true.
        assert!(
            w.one(json!({ "kind": "navigate", "tab": tab, "url": "5173" }))
                .is_none()
        );
        assert_eq!(
            w.driver.tabs[&id].sent.last().unwrap(),
            r#"Navigate("http://localhost:5173/")"#
        );
        w.browsers.tab_mut(&key, id).unwrap().loading = true;
        w.now += Duration::from_secs(21);
        let Some(Ok(BrowserCallResult::Navigate(state))) = w.tick() else {
            panic!()
        };
        assert!(state.loading);
        assert!(matches!(
            w.one(json!({ "kind": "close", "tab": tab })),
            Some(Ok(BrowserCallResult::Close(_)))
        ));
        assert!(w.browsers.tab(&key, id).is_none());
    }

    #[test]
    fn open_adds_a_tab_without_switching_the_users_view() {
        let mut w = World::new();
        let key = w.key.clone();
        let users = w.tab(&key, "http://localhost:5173/");
        assert_eq!(w.browsers.areas[&key].active, users);
        assert!(
            w.one(json!({ "kind": "open", "url": "http://localhost:8080/" }))
                .is_none()
        );
        let area = &w.browsers.areas[&key];
        assert_eq!(area.tabs.len(), 2);
        assert_eq!(area.active, users, "the user's tab stays in front");
        let opened = area.tabs[1].id;
        assert_eq!(area.tabs[1].url, "http://localhost:8080/");
        assert!(area.tabs[1].agent.is_some());
        // No renderer starts the engine here, so the bounded wait answers with the tab.
        w.now += Duration::from_secs(21);
        let Some(Ok(BrowserCallResult::Open(state))) = w.tick() else {
            panic!()
        };
        assert_eq!(state.tab, opened.to_string());
        let Some(Err(e)) = w.one(json!({ "kind": "open", "url": "file:///etc/passwd" })) else {
            panic!()
        };
        assert!(e.message.contains("HTTP"));
    }

    #[test]
    fn a_call_the_browser_never_answers_times_out_once() {
        let mut w = World::new();
        let key = w.key.clone();
        let id = w.tab(&key, "http://localhost:5173/");
        assert!(
            w.one(json!({ "kind": "screenshot", "tab": id.to_string() }))
                .is_none()
        );
        assert_eq!(w.driver.methods(id), vec!["Page.captureScreenshot"]);
        w.now += Duration::from_secs(26);
        let Some(Err(e)) = w.tick() else { panic!() };
        assert_eq!(e.code, ErrorCode::InternalError);
        assert!(w.tick().is_none());
    }

    #[test]
    fn console_and_network_read_the_tabs_buffers() {
        let mut w = World::new();
        let key = w.key.clone();
        let id = w.tab(&key, "http://localhost:5173/");
        w.driver.tabs.get_mut(&id).unwrap().diagnostics = vec![
            (
                1,
                json!({"method":"Runtime.consoleAPICalled","params":{"type":"log","args":[{"type":"string","value":"hi"}]}}),
            ),
            (
                2,
                json!({"method":"Network.loadingFailed","params":{"requestId":"9","errorText":"net::ERR_FAILED"}}),
            ),
        ];
        let Some(Ok(BrowserCallResult::Console(c))) =
            w.one(json!({ "kind": "console", "tab": id.to_string() }))
        else {
            panic!()
        };
        assert_eq!((c.entries.len(), c.cursor), (1, 2));
        let Some(Ok(BrowserCallResult::Network(n))) =
            w.one(json!({ "kind": "network", "tab": id.to_string(), "cursor": 1 }))
        else {
            panic!()
        };
        assert_eq!(n.entries[0].error.as_deref(), Some("net::ERR_FAILED"));
    }
}
