//! Pipe adapter for the shared browser controller. Only WPF presentation state lives here.
use std::io::{self, Write};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yaac_core::{ThreadKey, client::Client};

use super::{selected_thread, string, write_frame};
use super::files::{self, FilesPipe};
use crate::{app::YaacApp, preview_controller};

#[cfg(feature = "preview")]
mod native;

#[derive(Default)]
pub(super) struct BrowserPipe {
    previous: Value,
    files: FilesPipe,
    terminals: [super::terminal::TerminalPipe; 2],
    #[cfg(feature = "preview")]
    native: native::Surfaces,
    /// The last `browserHost` state sent to an external host (Android).
    host_state: Value,
}

impl BrowserPipe {
    pub(super) fn update(
        &mut self,
        app: &mut YaacApp,
        shown: bool,
        wake: impl Fn() + Clone + Send + Sync + 'static,
        stdout: &mut impl Write,
    ) -> io::Result<Option<Duration>> {
        let thread = (!app.ui.settings)
            .then(|| selected_thread(app).cloned())
            .flatten();
        let visible = thread.as_ref().filter(|_| shown);
        app.browsers.retain_tab_switch(visible);
        if let Some(key) = visible.filter(|key| crate::terminal::unavailable(&app.client,key).is_none()) {
            app.browsers.terms.shells(&mut app.client, &key.target);
        }
        app.browsers.check_runtime(wake.clone());
        if app.browsers.poll_runtime() {
            app.ctx.request_repaint();
        }
        app.browsers.set_file_wake(wake.clone());
        let next = app.browsers.tick(&mut app.client, visible, Instant::now());

        #[cfg(feature = "preview")]
        let updates = self.native.pump(&mut app.browsers, &mut app.client, visible, wake.clone());
        #[cfg(feature = "preview")]
        let next = match (next, app.browsers.browser_retry(Instant::now())) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        // Agent browser calls (docs/11-preview.md#agents), on the calling thread's tabs.
        #[cfg(feature = "preview")]
        let agent = app.browser_agent.step(&mut app.client, &mut app.browsers, &mut self.native, Instant::now());
        #[cfg(not(feature = "preview"))]
        let agent = app.browser_agent.step(&mut app.client, &mut app.browsers, Instant::now());
        let mut next = match (next, agent) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };

        self.files.prepare(
            thread
                .as_ref()
                .and_then(|key| app.browsers.areas.get(key))
                .filter(|area| !area.chat),
            app.browsers.side_by_side,
            wake.clone(),
        )?;
        let current = self.view(&app.browsers, &app.client, thread.as_ref(), &app.ui.terminal);
        if current != self.previous {
            write_frame(stdout, &json!({ "views": { "browser": current } }))?;
            self.previous = current;
        }
        if let Some(text) = app.browsers.terms.clipboard.take() {
            write_frame(stdout,&json!({"terminalCopy":text,"terminalNotice":"Copied by the terminal"}))?;
        }
        let terminal_theme = (app.ui.theme != crate::theme::ThemeChoice::System)
            .then(|| crate::terminal_ui::theme_of(&app.palette_for(&app.ctx)));
        for (index, pipe) in self.terminals.iter_mut().enumerate() {
            let panel = index == 1;
            let active = visible.and_then(|key| super::terminal::tabs::active(&app.browsers,key,panel));
            if let Some((_, id)) = active { app.browsers.terms.mark_shown(id); }
            let request_focus = active.is_some_and(|(_, id)| app.browsers.terms.focus == Some(id));
            let session = active.and_then(|(tab,id)| Some((tab,app.browsers.terms.get_mut(id)?)));
            pipe.surface(panel);
            if let Some(wait) = pipe.update(session, terminal_theme, stdout)? {
                next = Some(next.map_or(wait, |existing| existing.min(wait)));
            }
            if request_focus && let Some((tab,_))=active {
                app.browsers.terms.focus = None;
                write_frame(stdout,&json!({"terminalFocus":{"tab":tab,"panel":panel}}))?;
            }
        }
        if std::mem::take(&mut app.browsers.terms.focus_composer) {
            write_frame(stdout,&json!({"terminalFocus":{"composer":true}}))?;
        }
        if let Some(message) = app.browsers.notice.take() {
            write_frame(stdout, &json!({ "error": message }))?;
        }

        #[cfg(feature = "preview")]
        self.native.write(updates, stdout)?;
        // The embedder's own browser host (Android, P380): what should run, and agent actions.
        if let Some(state) = app.browsers.external_view(visible)
            && state != self.host_state
        {
            write_frame(stdout, &json!({ "browserHost": state }))?;
            self.host_state = state;
        }
        for call in app.browsers.external.as_mut().map(|h| std::mem::take(&mut h.outbox)).unwrap_or_default() {
            write_frame(stdout, &json!({ "browserHostCall": call }))?;
        }
        Ok(if self.files.write(stdout)? { Some(Duration::ZERO) } else { next })
    }

    fn view(
        &self,
        browsers: &preview_controller::Browsers,
        client: &Client,
        thread: Option<&ThreadKey>,
        prefs: &yaac_core::prefs::TerminalPrefs,
    ) -> Value {
        let reason = browsers.reason(client, thread);
        let area = thread.and_then(|thread| browsers.areas.get(thread));
        let attention = thread
            .filter(|_| area.is_some_and(|area| area.full_screen && !area.chat))
            .map(|key| {
                let map = yaac_core::attention::AttentionMap::compute(&client.state);
                let mark = yaac_core::attention::display(&client.state, &map, key).mark;
                (browsers.chat_marked(client, &map, key), super::sidebar_view::tone(mark))
            });
        let chat_logo = thread
            .and_then(|key| client.state.provider_of(key))
            .and_then(|provider| crate::logos::for_kind(provider.kind.as_str()));
        let mut state = json!({
            "thread": thread.map(ThreadKey::address),
            "chat": area.is_some_and(|a| a.chat),
            "chatMarked": attention.is_some_and(|(marked, _)| marked),
            "chatTone": attention.map(|(_, tone)| tone),
            "chatTitle": thread.and_then(|key| client.state.summary(key)).map(|s| &s.title),
            "chatLogo": chat_logo.map(|(logo, _)| crate::logos::source(logo).0),
            "chatLogoColor": chat_logo.and_then(|(_, color)| color).map(|c| format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())),
            "machine": thread.map(|key| client.state.target_name(&key.target)),
            "project": thread.and_then(|key| client.state.project_of(key)).map(|p| &p.folder),
            "discovering": area.is_some_and(|a| !a.ports_discovered),
            "reason": reason,
            "filesReason": files::reason(client, thread),
            "files": self.files.view(area, browsers.side_by_side),
            "external": reason.is_some() && thread.is_some_and(|k| preview_controller::supported(client, k)),
            "active": area.map(|area| area.active),
            "ratio": area.map(|area| area.ratio),
            "fullScreen": area.is_some_and(|area| area.full_screen),
            "tabSwitcher": thread.and_then(|thread| browsers.switch_tab_index(thread)).zip(area).map(|(index, area)| json!({
                "index": index,
                "tabs": std::iter::once("Chat").chain(area.tabs.iter().map(|tab| {
                    if !tab.title.is_empty() { tab.title.as_str() } else if !tab.display_address().is_empty() { tab.display_address() } else { "New tab" }
                })).collect::<Vec<_>>(),
            })),
            "tabs": area.map(|area| area.tabs.iter().map(|tab| json!({
                "id": tab.id, "url": tab.display_address(), "title": tab.title, "loading": tab.loading,
                "artifact": tab.artifact_name().is_some(),
                "kind": files::kind(tab), "path": tab.kind.path(),
                "mark": tab.kind.path().and_then(|path| {
                    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
                    area.tree.folders.get(dir)?.entries.iter().find(|e| e.name == name)?.git.as_ref()
                }),
                "back": tab.back, "forward": tab.forward, "error": tab.error,
                "surface": self.has_surface(tab.id),
                "agentLogo": tab.agent.as_deref().and_then(crate::logos::for_kind).map(|(logo, _)| crate::logos::source(logo).0),
                "agentLogoColor": tab.agent.as_deref().and_then(crate::logos::for_kind).and_then(|(_, color)| color)
                    .map(|c| format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())),
            })).collect::<Vec<_>>()).unwrap_or_default(),
            "ports": area.map(|area| area.ports.iter().map(|port| json!({
                "port": port.port, "command": port.command, "cwd": port.cwd,
                "url": preview_controller::listener_url(port),
            })).collect::<Vec<_>>()).unwrap_or_default(),
            "portsError": area.and_then(|area| area.ports_error.as_deref()),
        });
        super::terminal::tabs::decorate(&mut state,browsers,client,thread,prefs);
        state
    }

    fn has_surface(&self, id: u64) -> bool {
        #[cfg(feature = "preview")]
        return self.native.contains(id);
        #[cfg(not(feature = "preview"))]
        {
            let _ = id;
            false
        }
    }

    // Return true when the command may have changed browser view state.
    pub(super) fn command(
        &mut self,
        app: &mut YaacApp,
        shown: bool,
        command: &str,
        payload: &Value,
    ) -> Result<bool, String> {
        let id = payload["tab"].as_u64();
        let data = &payload["data"];
        if command == "terminal.ack" {
            return Ok(self.terminals[usize::from(payload["panel"]==true)].ack(payload["frame"].as_u64()));
        }
        // The external host's reports apply whatever thread is shown.
        if command == "browser.hostEvent" {
            app.browsers.external_event(payload);
            return Ok(true);
        }
        if command == "browser.hostResult" {
            app.browsers.external_result(&mut app.client, payload);
            return Ok(true);
        }
        if command == "browser.frameAck" {
            #[cfg(feature = "preview")]
            if let (Some(tab), Some(frame)) = (id, data["id"].as_u64()) {
                self.native.ack(tab, frame, data["retry"] == true);
            }
            return Ok(false);
        }
        // Completion for a menu, dialog or composition can arrive after switching threads.
        if command == "browser.input" {
            #[cfg(feature = "preview")]
            if let (Some(tab), Some(key)) = (
                id,
                ThreadKey::from_address(&yaac_protocol::Address(
                    string(payload, "thread").to_owned(),
                )),
            ) {
                self.native.input(
                    &key,
                    tab,
                    data,
                    shown && !app.ui.settings && selected_thread(app) == Some(&key),
                )?;
            }
            return Ok(false);
        }
        let Some(thread) = selected_thread(app)
            .filter(|key| shown && !app.ui.settings && key.address().0 == string(payload, "thread"))
            .cloned()
        else {
            return Ok(false);
        };

        if command.starts_with("terminal.") {
            let pipe = &mut self.terminals[usize::from(payload["panel"]==true)];
            return super::terminal::tabs::command(pipe,app,&thread,id,command,payload);
        }
        if command.starts_with("files.") {
            return self.files.command(app, &thread, id, command, data, payload);
        }
        let browsers = &mut app.browsers;
        match command {
            "browser.new" => {
                browsers.new_tab(&app.client, &thread)?;
            }
            "browser.openExternal" => {
                browsers.external_address(&mut app.client, &thread, string(payload, "url"))?;
            }
            "browser.fullScreen" => {
                browsers.set_full_screen(&thread, payload["fullScreen"] == true);
            }
            "browser.chat" => browsers.select_chat(&thread),
            "browser.switchTab" => match string(payload, "mode") {
                "cycle" => { browsers.cycle_tab(&thread); }
                "commit" => browsers.finish_tab_switch(&thread, true),
                "cancel" => browsers.finish_tab_switch(&thread, false),
                _ => return Ok(false),
            },
            "browser.ratio" => {
                if let Some(area) = browsers.areas.get_mut(&thread)
                    && let Some(ratio) = payload["ratio"].as_f64()
                {
                    area.ratio = ratio.clamp(0.15, 0.85) as f32;
                }
            }
            _ => {
                let Some(id) = id.filter(|id| {
                    browsers
                        .areas
                        .get(&thread)
                        .is_some_and(|area| area.tabs.iter().any(|tab| tab.id == *id))
                }) else {
                    return Ok(false);
                };
                match command {
                    "browser.close" => browsers.close(&thread, id),
                    // Back, forward, reload and stop, for an external host's tab.
                    "browser.history" => browsers.external_history(&thread, id, string(payload, "history"))?,
                    "browser.select" => browsers.select(&thread, id),
                    "browser.navigate" | "browser.external" | "browser.inspect"
                        if browsers.tab(&thread, id).is_none_or(|tab| tab.kind != crate::file_pane::TabKind::Browser) =>
                    {
                        return Ok(false);
                    }
                    "browser.navigate" => {
                        browsers.navigate(&mut app.client, &thread, id, string(data, "url"))?;
                        #[cfg(feature = "preview")]
                        if let Some(tab) = browsers.tab_mut(&thread, id) {
                            self.native
                                .send(id, yaac_preview::wire::Action::Navigate(tab.url.clone()));
                        }
                    }
                    "browser.external" => browsers.external(&mut app.client, &thread, id)?,
                    "browser.inspect" => {
                        #[cfg(feature = "preview")]
                        if let Some(tab) = browsers.tab_mut(&thread, id) {
                            let name = if tab.title.is_empty() {
                                &tab.url
                            } else {
                                &tab.title
                            };
                            let target = app
                                .client
                                .state
                                .target(&thread.target)
                                .map_or(thread.target.as_str(), |target| {
                                    target.entry.name.as_str()
                                });
                            self.native.send(
                                id,
                                yaac_preview::wire::Action::Inspect {
                                    title: format!("DevTools · {name} · {target}"),
                                    at: serde_json::from_value(data["at"].clone()).ok(),
                                },
                            );
                        }
                        return Ok(false);
                    }
                    _ => return Ok(false),
                }
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_browser_view_as_an_independent_pipe_update() {
        use crate::preview_controller::{BrowserArea, BrowserTab, Browsers};
        use yaac_core::demo::{Scenario, threads};

        let (client, _hub) = crate::app::demo_client(Scenario::Fleet);
        let key = threads::orchestrator();
        let mut area = BrowserArea::default();
        let mut tab = BrowserTab::default();
        tab.id = 17;
        tab.title = "Preview".into();
        area.tabs.push(tab);
        area.active = 17;
        area.ratio = 0.42;
        area.full_screen = true;

        let browser = BrowserPipe::default();
        let mut browsers = Browsers::default();
        browsers.areas.insert(key.clone(), area);
        let state = browser.view(&browsers, &client, Some(&key), &Default::default());

        let mut pipe = Vec::new();
        write_frame(&mut pipe, &json!({ "views": { "browser": state } })).unwrap();
        assert_eq!(pipe.last(), Some(&b'\n'));
        let received: Value = serde_json::from_slice(&pipe).unwrap();
        let state = &received["views"]["browser"];
        assert_eq!(state["thread"], json!(key.address()));
        assert_eq!(state["active"], 17);
        assert_eq!(state["tabs"][0]["title"], "Preview");
        assert_eq!(state["fullScreen"], true);
        assert!(browser.view(&browsers, &client, None, &Default::default())["active"].is_null());

        browsers.select_chat(&key);
        let chat = browser.view(&browsers, &client, Some(&key), &Default::default());
        assert_eq!(chat["chat"], true);
        assert_eq!(chat["active"], 17);
        assert_eq!(chat["chatTitle"], client.state.summary(&key).unwrap().title);
        assert!(chat["chatLogo"].as_str().is_some_and(|logo| !logo.is_empty()));
        assert_eq!(chat["chatMarked"], false);
        browsers.set_full_screen(&key, false);
        let split = browser.view(&browsers, &client, Some(&key), &Default::default());
        assert_eq!(split["chat"], false);
        assert_eq!(split["active"], 17);
        assert_eq!(split["ratio"], state["ratio"]);
    }
}
