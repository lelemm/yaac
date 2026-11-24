//! Pipe adapter for the shared browser controller. Only WPF presentation state lives here.
use std::io::{self, Write};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yaac_core::{ThreadKey, client::Client};

use super::{selected_thread, string, write_frame};
use crate::{app::YaacApp, preview_controller};

#[cfg(feature = "preview")]
mod native;

#[derive(Default)]
pub(super) struct BrowserPipe {
    previous: Value,
    #[cfg(feature = "preview")]
    native: native::Surfaces,
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
        app.browsers.check_runtime(wake.clone());
        let next = app.browsers.tick(&mut app.client, visible, Instant::now());

        #[cfg(feature = "preview")]
        let updates = self.native.pump(&mut app.browsers, visible, wake);

        let current = self.view(&app.browsers, &app.client, thread.as_ref());
        if current != self.previous {
            write_frame(stdout, &json!({ "views": { "browser": current } }))?;
            self.previous = current;
        }
        if let Some(message) = app.browsers.notice.take() {
            write_frame(stdout, &json!({ "error": message }))?;
        }

        #[cfg(feature = "preview")]
        self.native.write(updates, stdout)?;
        Ok(next)
    }

    fn view(
        &self,
        browsers: &preview_controller::Browsers,
        client: &Client,
        thread: Option<&ThreadKey>,
    ) -> Value {
        let reason = browsers.reason(client, thread);
        let area = thread.and_then(|thread| browsers.areas.get(thread));
        json!({
            "thread": thread.map(ThreadKey::address),
            "reason": reason,
            "external": reason.is_some() && thread.is_some_and(|k| preview_controller::supported(client, k)),
            "active": area.map(|area| area.active),
            "ratio": area.map(|area| area.ratio),
            "fullScreen": area.is_some_and(|area| area.full_screen),
            "tabs": area.map(|area| area.tabs.iter().map(|tab| json!({
                "id": tab.id, "url": tab.url, "title": tab.title, "loading": tab.loading,
                "back": tab.back, "forward": tab.forward, "error": tab.error,
                "surface": self.has_surface(tab.id),
            })).collect::<Vec<_>>()).unwrap_or_default(),
            "ports": area.map(|area| area.ports.iter().map(|port| json!({
                "port": port.port, "command": port.command, "cwd": port.cwd,
                "url": preview_controller::listener_url(port),
            })).collect::<Vec<_>>()).unwrap_or_default(),
            "portsError": area.and_then(|area| area.ports_error.as_deref()),
        })
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

        let browsers = &mut app.browsers;
        match command {
            "browser.new" => {
                browsers.new_tab(&app.client, &thread)?;
            }
            "browser.openExternal" => {
                browsers.external_address(&mut app.client, &thread, string(payload, "url"))?;
            }
            "browser.fullScreen" => {
                if let Some(area) = browsers.areas.get_mut(&thread) {
                    area.full_screen = payload["fullScreen"] == true;
                }
            }
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
                    "browser.select" => {
                        browsers.areas.get_mut(&thread).expect("open area").active = id
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
                                    title: format!("DevTools — {name} · {target}"),
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
        let state = browser.view(&browsers, &client, Some(&key));

        let mut pipe = Vec::new();
        write_frame(&mut pipe, &json!({ "views": { "browser": state } })).unwrap();
        assert_eq!(pipe.last(), Some(&b'\n'));
        let received: Value = serde_json::from_slice(&pipe).unwrap();
        let state = &received["views"]["browser"];
        assert_eq!(state["thread"], json!(key.address()));
        assert_eq!(state["active"], 17);
        assert_eq!(state["tabs"][0]["title"], "Preview");
        assert_eq!(state["fullScreen"], true);
        assert!(browser.view(&browsers, &client, None)["active"].is_null());
    }
}
