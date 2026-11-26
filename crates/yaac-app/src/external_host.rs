//! A browser host that lives outside the controller: the Android app's WebViews (P380,
//! docs/11-preview.md#android-proposed-p380). The controller keeps the tabs, as for CEF; the
//! host runs the pages. They talk through the view protocol:
//!
//! - **controller → host**, the frame `{"browserHost": {proxy, target, tabs, visible}}` whenever
//!   it changes: the WebViews that should exist (only the live target's), each with its address
//!   and a navigation serial `nav` (load `url` when `nav` grows; `history` says back, forward,
//!   reload or stop instead), the proxy every WebView uses, and the tab on screen;
//! - **controller → host**, `{"browserHostCall": {call, tab, action}}` for an agent's action
//!   (the `browser/call` action as the daemon sent it);
//! - **host → controller**, `browser.hostEvent {tab, url?, title?, loading?, back?, forward?,
//!   error?}` and `browser.hostResult {call, result? | error?}` (`result` is the
//!   `BrowserCallResult` JSON, `kind` included).
//!
//! The proxy is a [`TunnelProxy`] bound to one target, and Android's proxy setting is per
//! process, so tabs of one target run at a time: the visible thread's target when it has tabs,
//! else the one already live.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yaac_core::ThreadKey;
use yaac_core::client::Client;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, RpcId};
use yaac_protocol::{BrowserAction, BrowserCallResult, TargetId};

use crate::file_pane::TabKind;
use crate::preview_controller::Browsers;
use crate::tunnel_proxy::{TunnelProxy, Wake};

const CALL_LIMIT: Duration = Duration::from_secs(25);
pub(crate) const OTHER_MACHINE: &str = "The phone's browser is showing another machine; open this thread on the phone to use its tabs";

struct Call {
    rpc: RpcId,
    deadline: Instant,
    key: ThreadKey,
    tab: u64,
}

/// The external host's state in the controller.
#[derive(Default)]
pub(crate) struct Host {
    pub(crate) proxy: Option<TunnelProxy>,
    calls: HashMap<u64, Call>,
    next_call: u64,
    /// Agent actions to hand to the host with the next frames.
    pub(crate) outbox: Vec<Value>,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host").field("proxy", &self.proxy).field("calls", &self.calls.len()).finish_non_exhaustive()
    }
}

fn browser_tabs(b: &Browsers, target: &TargetId) -> impl Iterator<Item = (ThreadKey, u64)> {
    b.areas.iter().filter(move |(k, _)| &k.target == target).flat_map(|(k, a)| {
        a.tabs.iter().filter(|t| t.kind == TabKind::Browser && !t.url.is_empty()).map(move |t| (k.clone(), t.id))
    })
}

fn has_tabs(b: &Browsers, target: &TargetId) -> bool {
    browser_tabs(b, target).next().is_some()
}

impl Browsers {
    /// The embedder runs its own browser host (`shell.browserHost`).
    pub(crate) fn enable_external_host(&mut self) {
        if self.external.is_none() {
            self.external = Some(Host::default());
        }
    }

    pub(crate) fn live_target(&self) -> Option<&TargetId> {
        self.external.as_ref()?.proxy.as_ref().map(TunnelProxy::target)
    }

    /// Keep the proxy on the right target (or none), pump its tunnels, and settle replies.
    pub(crate) fn external_tick(&mut self, client: &mut Client, visible: Option<&ThreadKey>, wake: Option<Wake>, now: Instant) {
        if self.external.is_none() {
            return;
        }
        let current = self.live_target().cloned();
        let wanted = visible
            .map(|k| k.target.clone())
            .filter(|t| has_tabs(self, t))
            .or_else(|| current.clone().filter(|t| has_tabs(self, t)))
            .or_else(|| self.areas.keys().map(|k| k.target.clone()).find(|t| has_tabs(self, t)));
        if wanted != current {
            self.switch_proxy(client, wanted, wake);
        }
        let Some(host) = self.external.as_mut() else { return };
        if let Some(proxy) = host.proxy.as_mut() {
            for (rpc, conn) in proxy.pump(client) {
                self.pending.insert(rpc, crate::preview_controller::Pending::Tunnel(conn));
            }
        }
        // Agent calls the host did not answer in time.
        let late: Vec<u64> = self.external.as_ref().map(|h| h.calls.iter().filter(|(_, c)| c.deadline <= now).map(|(id, _)| *id).collect()).unwrap_or_default();
        for id in late {
            if let Some(c) = self.external.as_mut().and_then(|h| h.calls.remove(&id)) {
                client.answer_browser_call(c.rpc, Err(ErrorObject::new(ErrorCode::InternalError, "The page did not answer in time")));
            }
        }
    }

    fn switch_proxy(&mut self, client: &mut Client, target: Option<TargetId>, wake: Option<Wake>) {
        let Some(host) = self.external.as_mut() else { return };
        if let Some(old) = host.proxy.take() {
            old.stop(client);
        }
        let (Some(target), Some(wake)) = (target, wake) else { return };
        match TunnelProxy::start(target, wake) {
            Ok(p) => host.proxy = Some(p),
            Err(e) => self.notice = Some(format!("The browser could not start its proxy: {e}")),
        }
    }

    /// The `browserHost` frame: the WebViews that should exist, and the one on screen.
    pub(crate) fn external_view(&self, visible: Option<&ThreadKey>) -> Option<Value> {
        let host = self.external.as_ref()?;
        let proxy = host.proxy.as_ref();
        let target = proxy.map(TunnelProxy::target);
        let tabs: Vec<Value> = target.map(|t| browser_tabs(self, t).collect::<Vec<_>>()).unwrap_or_default().into_iter().filter_map(|(k, id)| {
            let tab = self.tab(&k, id)?;
            Some(json!({ "tab": id, "thread": k.address(), "url": tab.url, "nav": tab.nav, "history": tab.history }))
        }).collect();
        let on_screen = visible.filter(|k| Some(&k.target) == target).and_then(|k| {
            let area = self.areas.get(k).filter(|a| !a.chat)?;
            self.tab(k, area.active).filter(|t| t.kind == TabKind::Browser && !t.url.is_empty()).map(|t| t.id)
        });
        Some(json!({ "proxy": proxy.map(TunnelProxy::url), "target": target, "tabs": tabs, "visible": on_screen }))
    }

    /// `browser.hostEvent`: the page's own state.
    pub(crate) fn external_event(&mut self, p: &Value) {
        let Some(id) = p["tab"].as_u64() else { return };
        let Some(tab) = self.areas.values_mut().flat_map(|a| a.tabs.iter_mut()).find(|t| t.id == id) else { return };
        if let Some(url) = p["url"].as_str() {
            tab.url = url.to_owned();
        }
        if let Some(title) = p["title"].as_str() {
            tab.title = title.to_owned();
        }
        if let Some(loading) = p["loading"].as_bool() {
            tab.loading = loading;
        }
        if let Some(back) = p["back"].as_bool() {
            tab.back = back;
        }
        if let Some(forward) = p["forward"].as_bool() {
            tab.forward = forward;
        }
        if p.get("error").is_some() {
            tab.error = p["error"].as_str().map(str::to_owned);
        }
    }

    /// The toolbar's back, forward, reload and stop.
    pub(crate) fn external_history(&mut self, key: &ThreadKey, id: u64, kind: &str) -> Result<(), String> {
        if !matches!(kind, "back" | "forward" | "reload" | "stop") {
            return Err(format!("unknown history action {kind}"));
        }
        let tab = self.tab_mut(key, id).ok_or("Tab closed")?;
        tab.nav += 1;
        tab.history = Some(kind.to_owned());
        Ok(())
    }

    /// `browser.hostResult`: the answer to an agent's action.
    pub(crate) fn external_result(&mut self, client: &mut Client, p: &Value) {
        let Some(call) = p["call"].as_u64().and_then(|id| self.external.as_mut()?.calls.remove(&id)) else { return };
        let outcome = match p.get("result") {
            Some(result) => serde_json::from_value::<BrowserCallResult>(result.clone())
                .map_err(|e| ErrorObject::new(ErrorCode::InternalError, format!("the browser answered badly: {e}"))),
            None => Err(ErrorObject::new(ErrorCode::InvalidParams, p["error"].as_str().unwrap_or("The browser could not do that"))),
        };
        if let (Ok(BrowserCallResult::Open(t) | BrowserCallResult::Navigate(t)), Some(tab)) = (&outcome, self.tab_mut(&call.key, call.tab)) {
            tab.url.clone_from(&t.url);
            tab.title.clone_from(&t.title);
            tab.loading = t.loading;
        }
        client.answer_browser_call(call.rpc, outcome);
    }

    /// Agents' `browser/call`s, carried out by the host. Returns the tabs acted on (for the
    /// agent mark).
    pub(crate) fn external_calls(&mut self, client: &mut Client, now: Instant) -> Vec<(ThreadKey, u64)> {
        let mut acting = Vec::new();
        for (rpc, call) in client.take_browser_calls() {
            let key = ThreadKey { target: call.target, thread: call.call.thread_id };
            match self.external_call(client, &key, &call.call.action, now) {
                Ok(Some((answer, tab))) => {
                    if let Some(tab) = tab {
                        acting.push((key.clone(), tab));
                    }
                    if let Some(answer) = answer {
                        client.answer_browser_call(rpc, Ok(answer));
                    } else if let Some(tab) = tab
                        && let Some(host) = self.external.as_mut()
                        && let Some(id) = host.outbox.last().and_then(|v| v["call"].as_u64())
                    {
                        host.calls.insert(id, Call { rpc, deadline: now + CALL_LIMIT, key, tab });
                    }
                }
                Ok(None) => {}
                Err(e) => client.answer_browser_call(rpc, Err(e)),
            }
        }
        acting
    }

    /// One call: answered here (`Some(answer)`), or handed to the host (`None` answer, the call
    /// queued in the outbox).
    #[allow(clippy::type_complexity)]
    fn external_call(&mut self, client: &mut Client, key: &ThreadKey, action: &BrowserAction, _now: Instant) -> Result<Option<(Option<BrowserCallResult>, Option<u64>)>, ErrorObject> {
        let bad = |m: &str| ErrorObject::new(ErrorCode::InvalidParams, m);
        let tab_of = |text: &str| text.parse::<u64>().ok();
        match action {
            BrowserAction::List(_) => {
                let tabs = self.areas.get(key).map(|a| a.tabs.iter().filter(|t| t.kind == TabKind::Browser).map(|t| yaac_protocol::BrowserTab {
                    tab: t.id.to_string(), url: t.url.clone(), title: t.title.clone(), loading: t.loading,
                }).collect()).unwrap_or_default();
                return Ok(Some((Some(BrowserCallResult::List(yaac_protocol::BrowserTabs { tabs })), None)));
            }
            BrowserAction::Close(p) => {
                let id = tab_of(&p.tab).filter(|id| self.tab(key, *id).is_some()).ok_or_else(|| bad("No such tab"))?;
                self.close(key, id);
                return Ok(Some((Some(BrowserCallResult::Close(yaac_protocol::Empty {})), None)));
            }
            _ => {}
        }
        // Everything else runs in a page: only the live target's tabs run.
        if let Some(live) = self.live_target()
            && live != &key.target
            && has_tabs(self, live)
        {
            return Err(bad(OTHER_MACHINE));
        }
        let tab = match action {
            BrowserAction::Open(p) => {
                let id = self.open_background(client, key, &p.url).map_err(|e: String| bad(&e))?;
                Some(id)
            }
            BrowserAction::Screenshot(p) => tab_of(&p.tab),
            BrowserAction::Snapshot(p) => tab_of(&p.tab),
            BrowserAction::Console(p) | BrowserAction::Network(p) => tab_of(&p.tab),
            BrowserAction::Navigate(p) => tab_of(&p.tab),
            BrowserAction::Click(p) => tab_of(&p.tab),
            BrowserAction::Type(p) => tab_of(&p.tab),
            BrowserAction::Press(p) => tab_of(&p.tab),
            BrowserAction::Eval(p) => tab_of(&p.tab),
            _ => None,
        };
        let tab = tab.filter(|id| self.tab(key, *id).is_some()).ok_or_else(|| bad("No such tab; list the tabs again"))?;
        // A navigation's address as the toolbar reads it (`5173` → `http://localhost:5173/`).
        let mut action = action.clone();
        if let BrowserAction::Navigate(p) = &mut action
            && let Some(url) = p.url.as_mut()
        {
            *url = crate::preview_controller::address(url).map_err(|e| bad(&e))?.to_string();
        }
        let action = &action;
        let Some(host) = self.external.as_mut() else { return Err(bad("this window has no browser host")) };
        host.next_call += 1;
        let call = host.next_call;
        // The page script the action needs (the desktop's own, so refs and errors match):
        // the host evaluates `(script)(action)` and awaits it.
        use crate::preview_agent::scripts;
        let script: Option<&str> = match action {
            BrowserAction::Snapshot(_) => Some(scripts::SNAPSHOT_JS),
            BrowserAction::Click(p) if p.reference.is_some() || p.selector.is_some() => Some(scripts::CENTER_JS.as_str()),
            BrowserAction::Type(_) => Some(scripts::FOCUS_JS.as_str()),
            BrowserAction::Screenshot(p) if p.selector.is_some() => Some(scripts::RECT_JS.as_str()),
            _ => None,
        };
        host.outbox.push(json!({ "call": call, "tab": tab, "thread": key.address(), "action": action, "script": script }));
        Ok(Some((None, Some(tab))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole Android path on the demo: a browser tab on users-service starts the proxy,
    /// the host state names the tab and the proxy, and a SOCKS client (what the WebView is)
    /// fetches the demo dev server's page over `hub/preview/tunnel`.
    #[test]
    fn a_socks_client_fetches_the_target_page_through_the_controllers_proxy() {
        use std::io::{Read, Write};
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        app.browsers.enable_external_host();
        app.browsers.set_file_wake(|| {});
        app.apply(vec![crate::app::Action::Scene(yaac_core::demo::Scene::SidePanes)]);
        app.pump_demo();
        let key = yaac_core::demo::threads::users();
        app.open(&key, None);
        app.pump_demo();
        let id = app.browsers.new_tab(&app.client, &key).unwrap();
        app.browsers.navigate(&mut app.client, &key, id, "5173").unwrap();
        app.browsers.tick(&mut app.client, Some(&key), Instant::now());
        let state = app.browsers.external_view(Some(&key)).unwrap();
        assert_eq!(state["tabs"][0]["url"], "http://localhost:5173/");
        assert_eq!(state["tabs"][0]["nav"], 1);
        assert_eq!(state["visible"], id);
        let proxy = state["proxy"].as_str().unwrap().to_owned();
        let port: u16 = proxy.rsplit(':').next().unwrap().parse().unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            s.write_all(&[5, 1, 0]).unwrap();
            let mut b = [0u8; 2];
            s.read_exact(&mut b).unwrap();
            let host = b"localhost";
            let mut req = vec![5, 1, 0, 3, u8::try_from(host.len()).unwrap()];
            req.extend_from_slice(host);
            req.extend_from_slice(&5173u16.to_be_bytes());
            s.write_all(&req).unwrap();
            let mut reply = [0u8; 10];
            s.read_exact(&mut reply).unwrap();
            assert_eq!(reply[1], 0, "the tunnel opened");
            s.write_all(b"GET / HTTP/1.1\r\nHost: localhost:5173\r\n\r\n").unwrap();
            let mut page = String::new();
            let _ = s.read_to_string(&mut page);
            tx.send(page).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        let page = loop {
            app.pump_demo();
            app.browsers.tick(&mut app.client, Some(&key), Instant::now());
            if let Ok(page) = rx.try_recv() {
                break page;
            }
            assert!(Instant::now() < deadline, "no page through the proxy");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
        assert!(page.contains("Sign in"));

        // The last tab closes: the proxy goes away.
        app.browsers.close(&key, id);
        app.browsers.tick(&mut app.client, Some(&key), Instant::now());
        assert!(app.browsers.external_view(Some(&key)).unwrap()["proxy"].is_null());
    }

    #[test]
    fn agent_actions_go_to_the_host_with_the_desktops_page_scripts_and_list_is_answered_here() {
        let (mut client, _hub) = crate::app::demo_client(yaac_core::demo::Scenario::Fleet);
        let mut b = Browsers::default();
        b.enable_external_host();
        let key = ThreadKey::new("devbox", "t1");
        let id = b.insert(&key);
        let t = b.tab_mut(&key, id).unwrap();
        t.kind = TabKind::Browser;
        t.url = "http://localhost:5173/".into();
        let action = |v: Value| serde_json::from_value::<BrowserAction>(v).unwrap();
        let list = b.external_call(&mut client, &key, &action(json!({ "kind": "list" })), Instant::now()).unwrap().unwrap();
        assert!(matches!(list.0, Some(BrowserCallResult::List(ref l)) if l.tabs.len() == 1));
        let tab = id.to_string();
        let snap = b.external_call(&mut client, &key, &action(json!({ "kind": "snapshot", "tab": tab })), Instant::now()).unwrap().unwrap();
        assert_eq!(snap, (None, Some(id)));
        let eval = b.external_call(&mut client, &key, &action(json!({ "kind": "eval", "tab": tab, "expression": "1+1" })), Instant::now()).unwrap();
        assert!(eval.is_some());
        let out = &b.external.as_ref().unwrap().outbox;
        assert_eq!(out.len(), 2);
        assert!(out[0]["script"].as_str().unwrap().contains("[ref="), "the snapshot script");
        assert_eq!(out[0]["action"]["kind"], "snapshot");
        assert!(out[1]["script"].is_null());
        assert_eq!(out[1]["action"]["expression"], "1+1");
        assert!(b.external_call(&mut client, &key, &action(json!({ "kind": "eval", "tab": "999", "expression": "1" })), Instant::now()).is_err());
    }

    #[test]
    fn the_host_state_lists_only_the_live_targets_tabs_and_history_bumps_the_serial() {
        let mut b = Browsers::default();
        b.enable_external_host();
        let a = ThreadKey::new("devbox", "t1");
        let other = ThreadKey::new("staging", "t2");
        for (k, url) in [(&a, "http://localhost:5173/"), (&other, "http://localhost:3000/")] {
            let id = b.insert(k);
            let t = b.tab_mut(k, id).unwrap();
            t.kind = TabKind::Browser;
            t.url = url.into();
            t.nav = 1;
        }
        // No proxy yet: nothing runs.
        let view = b.external_view(Some(&a)).unwrap();
        assert!(view["tabs"].as_array().unwrap().is_empty());
        assert!(view["proxy"].is_null());
        let id = b.areas[&a].tabs[0].id;
        b.external_history(&a, id, "back").unwrap();
        assert_eq!((b.tab(&a, id).unwrap().nav, b.tab(&a, id).unwrap().history.as_deref()), (2, Some("back")));
        assert!(b.external_history(&a, id, "sideways").is_err());
        b.external_event(&json!({ "tab": id, "title": "Vite App", "loading": false, "back": true }));
        let t = b.tab(&a, id).unwrap();
        assert_eq!((t.title.as_str(), t.loading, t.back), ("Vite App", false, true));
    }
}
