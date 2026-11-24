//! Thread-local window state and asynchronous hub operations shared by the renderers.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::json;
use url::Url;
use yaac_core::{ThreadKey, client::Client};
use yaac_protocol::{PreviewPort, PreviewPortsResult, attach, rpc::RpcId};

const PORT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct BrowserTab {
    pub(crate) id: u64,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) loading: bool,
    pub(crate) back: bool,
    pub(crate) forward: bool,
    pub(crate) error: Option<String>,
    /// Provider kind of the agent acting on this tab, only while it acts; renderers draw its
    /// harness logo beside the title (docs/11-preview.md#the-browser-area).
    pub(crate) agent: Option<String>,
    /// Set once per renderer handle, only after the hub supplies a target proxy.
    socks: Option<u16>,
    started: bool,
}

impl BrowserTab {
    /// Whether a renderer has taken the engine handle for this tab.
    #[cfg(feature = "preview")]
    pub(crate) fn started(&self) -> bool {
        self.started
    }
}

pub(crate) struct BrowserArea {
    pub(crate) tabs: Vec<BrowserTab>,
    pub(crate) active: u64,
    pub(crate) ratio: f32,
    pub(crate) full_screen: bool,
    pub(crate) ports: Vec<PreviewPort>,
    pub(crate) ports_error: Option<String>,
    epoch: u64,
    next_poll: Instant,
    polling: bool,
}

impl Default for BrowserArea {
    fn default() -> Self {
        Self {
            tabs: vec![],
            active: 0,
            ratio: 0.5,
            full_screen: false,
            ports: vec![],
            ports_error: None,
            epoch: 0,
            next_poll: Instant::now(),
            polling: false,
        }
    }
}

enum Pending {
    Proxy(ThreadKey, u64),
    External(ThreadKey, u64, Url),
    Ports(ThreadKey, u64),
}

#[derive(Default)]
pub(crate) struct Browsers {
    pub(crate) areas: HashMap<ThreadKey, BrowserArea>,
    pub(crate) notice: Option<String>,
    #[cfg(feature = "preview")]
    probe: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    #[cfg(feature = "preview")]
    checked: bool,
    pub(crate) host_error: Option<String>,
    pending: HashMap<RpcId, Pending>,
    next: u64,
    #[cfg(feature = "preview")]
    host: yaac_preview::BrowserHost,
}

pub(crate) fn address(input: &str) -> Result<Url, String> {
    let s = input.trim();
    let number = s.strip_prefix(':').unwrap_or(s);
    let s = if !number.is_empty() && number.bytes().all(|c| c.is_ascii_digit()) {
        let port = number
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or("Invalid port")?;
        format!("http://localhost:{port}/")
    } else if s.contains("://") {
        s.to_owned()
    } else {
        format!("http://{s}")
    };
    let url = Url::parse(&s).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Enter an HTTP or HTTPS address".into());
    }
    Ok(url)
}

/// URL of the listener as reached from the target.
pub(crate) fn listener_url(port: &PreviewPort) -> String {
    let host = match port.host.as_str() {
        "::" | "::1" => "[::1]",
        "0.0.0.0" | "127.0.0.1" => "localhost",
        host => host,
    };
    format!("http://{host}:{}/", port.port)
}

pub(crate) fn supported(client: &Client, key: &ThreadKey) -> bool {
    client.state.target(&key.target).is_some_and(|t| {
        t.connected()
            && t.entry
                .state
                .daemon
                .as_ref()
                .is_some_and(|d| d.port_forward)
    })
}

impl Browsers {
    pub(crate) fn check_runtime(&mut self, wake: impl FnOnce() + Send + 'static) {
        #[cfg(feature = "preview")]
        if !self.checked && self.probe.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            self.probe = Some(rx);
            std::thread::spawn(move || {
                let _ = tx.send(yaac_preview::runtime_available());
                wake();
            });
        }
        #[cfg(not(feature = "preview"))]
        let _ = wake;
    }

    pub(crate) fn reason(&self, client: &Client, key: Option<&ThreadKey>) -> Option<String> {
        if !cfg!(feature = "preview") {
            return Some("This build has no browser host".into());
        }
        if let Some(e) = &self.host_error {
            return Some(e.clone());
        }
        #[cfg(feature = "preview")]
        if !self.checked {
            return Some("Checking browser host…".into());
        }
        let Some(key) = key else {
            return Some("Open a thread first".into());
        };
        (!supported(client, key)).then(|| "The target is offline or lacks port forwarding".into())
    }

    pub(crate) fn new_tab(&mut self, client: &Client, key: &ThreadKey) -> Result<u64, String> {
        if let Some(reason) = self.reason(client, Some(key)) {
            return Err(reason);
        }
        Ok(self.insert(key))
    }

    fn insert(&mut self, key: &ThreadKey) -> u64 {
        self.next += 1;
        let area = self.areas.entry(key.clone()).or_default();
        if area.tabs.is_empty() {
            area.epoch = self.next;
        }
        area.tabs.push(BrowserTab {
            id: self.next,
            ..Default::default()
        });
        area.active = self.next;
        self.next
    }

    /// An agent's tab: added after the thread's tabs without changing the active tab (the
    /// user's view), except that a thread's first tab opens its browser area.
    #[cfg(feature = "preview")]
    pub(crate) fn open_background(&mut self, client: &mut Client, key: &ThreadKey, input: &str) -> Result<u64, String> {
        if let Some(reason) = self.reason(client, Some(key)) {
            return Err(reason);
        }
        address(input)?;
        let active = self.areas.get(key).filter(|a| !a.tabs.is_empty()).map(|a| a.active);
        let id = self.insert(key);
        if let Some(active) = active {
            self.areas.get_mut(key).expect("area just used").active = active;
        }
        if let Err(e) = self.navigate(client, key, id, input) {
            self.close(key, id);
            return Err(e);
        }
        Ok(id)
    }

    #[cfg(feature = "preview")]
    pub(crate) fn tab(&self, key: &ThreadKey, id: u64) -> Option<&BrowserTab> {
        self.areas.get(key)?.tabs.iter().find(|t| t.id == id)
    }

    pub(crate) fn tab_mut(&mut self, key: &ThreadKey, id: u64) -> Option<&mut BrowserTab> {
        self.areas
            .get_mut(key)?
            .tabs
            .iter_mut()
            .find(|t| t.id == id)
    }

    pub(crate) fn close(&mut self, key: &ThreadKey, id: u64) {
        let Some(area) = self.areas.get_mut(key) else {
            return;
        };
        let Some(index) = area.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        area.tabs.remove(index);
        if area.tabs.is_empty() {
            self.areas.remove(key);
        } else if area.active == id {
            area.active = area.tabs[index.min(area.tabs.len() - 1)].id;
        }
    }

    pub(crate) fn navigate(
        &mut self,
        client: &mut Client,
        key: &ThreadKey,
        id: u64,
        input: &str,
    ) -> Result<(), String> {
        if !supported(client, key) {
            return Err("Target port forwarding unavailable".into());
        }
        let url = address(input)?;
        let tab = self.tab_mut(key, id).ok_or("Tab closed")?;
        tab.url = url.to_string();
        tab.error = None;
        if !tab.started
            && tab.socks.is_none()
            && !self
                .pending
                .values()
                .any(|v| matches!(v, Pending::Proxy(k,i) if k==key && *i==id))
        {
            let rpc =
                client.preview_request(attach::names::PREVIEW_PROXY, json!({"target":key.target}));
            self.pending.insert(rpc, Pending::Proxy(key.clone(), id));
        }
        Ok(())
    }

    #[cfg(feature = "preview")]
    pub(crate) fn open_ready(
        &mut self,
        key: &ThreadKey,
        id: u64,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Option<yaac_preview::Tab> {
        let tab = self.tab_mut(key, id)?;
        let socks = tab.socks.take()?;
        tab.started = true;
        let url = tab.url.clone();
        Some(self.host.open(&url, Some(socks), wake))
    }

    pub(crate) fn external(
        &mut self,
        client: &mut Client,
        key: &ThreadKey,
        id: u64,
    ) -> Result<(), String> {
        let input = self.tab_mut(key, id).ok_or("Tab closed")?.url.clone();
        self.forward(client, key, id, &input)
    }

    pub(crate) fn external_address(
        &mut self,
        client: &mut Client,
        key: &ThreadKey,
        input: &str,
    ) -> Result<(), String> {
        self.forward(client, key, 0, input)
    }

    fn forward(
        &mut self,
        client: &mut Client,
        key: &ThreadKey,
        id: u64,
        input: &str,
    ) -> Result<(), String> {
        if !supported(client, key) {
            return Err("Target port forwarding unavailable".into());
        }
        let url = address(input)?;
        let host = url
            .host_str()
            .ok_or("No server address")?
            .trim_matches(['[', ']']);
        self.notice = None;
        let rpc = client.preview_request(
            attach::names::PREVIEW_OPEN,
            json!({"target":key.target,"host":host,"port":url.port_or_known_default()}),
        );
        self.pending
            .insert(rpc, Pending::External(key.clone(), id, url));
        Ok(())
    }

    pub(crate) fn tick(
        &mut self,
        client: &mut Client,
        visible: Option<&ThreadKey>,
        now: Instant,
    ) -> Option<Duration> {
        #[cfg(feature = "preview")]
        if let Some(result) = self.probe.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.host_error = result.err();
            self.checked = true;
            self.probe = None;
        }
        for (id, result) in client.take_preview_replies() {
            match self.pending.remove(&id) {
                Some(Pending::Proxy(k, id)) => {
                    if let Some(tab) = self.tab_mut(&k, id) {
                        match result.and_then(|v| {
                            serde_json::from_value::<attach::PreviewProxyResult>(v)
                                .map_err(|e| e.to_string())
                        }) {
                            Ok(proxy) => tab.socks = Some(proxy.socks_port),
                            Err(e) => tab.error = Some(e),
                        }
                    }
                }
                Some(Pending::External(k, id, mut url)) => {
                    match result.and_then(|v| {
                        serde_json::from_value::<attach::PreviewOpenResult>(v)
                            .map_err(|e| e.to_string())
                    }) {
                        Ok(forward) => {
                            let _ = url.set_host(Some("127.0.0.1"));
                            let _ = url.set_port(Some(forward.local_port));
                            crate::browser::open(url.as_str());
                        }
                        Err(e) => {
                            if let Some(tab) = self.tab_mut(&k, id) {
                                tab.error = Some(e)
                            } else {
                                self.notice = Some(e)
                            }
                        }
                    }
                }
                Some(Pending::Ports(k, epoch)) => {
                    if let Some(area) = self
                        .areas
                        .get_mut(&k)
                        .filter(|a| a.polling && a.epoch == epoch)
                    {
                        area.polling = false;
                        match result.and_then(|v| {
                            serde_json::from_value::<PreviewPortsResult>(v)
                                .map_err(|e| e.to_string())
                        }) {
                            Ok(mut r) => {
                                let folder = client
                                    .state
                                    .project_of(&k)
                                    .map(|p| p.folder.as_str())
                                    .or_else(|| client.state.summary(&k).map(|s| s.cwd.as_str()));
                                r.ports.sort_by_key(|p| {
                                    (
                                        !folder.zip(p.cwd.as_deref()).is_some_and(|(root, cwd)| {
                                            yaac_core::projects::contains(root, cwd)
                                        }),
                                        p.port,
                                    )
                                });
                                area.ports = r.ports;
                                area.ports_error = None;
                            }
                            Err(e) => area.ports_error = Some(e),
                        }
                    }
                }
                _ => {}
            }
        }
        let key = visible.filter(|k| supported(client, k))?;
        let area = self.areas.get_mut(key)?;
        if area.polling {
            return None;
        }
        if now >= area.next_poll {
            let rpc = client.preview_request(
                attach::names::REQUEST,
                json!({"target":key.target,"method":yaac_protocol::methods::names::PREVIEW_PORTS,"params":{}}),
            );
            self.pending
                .insert(rpc, Pending::Ports(key.clone(), area.epoch));
            area.polling = true;
            area.next_poll = now + PORT_INTERVAL;
            return None;
        }
        Some(area.next_poll.saturating_duration_since(now))
    }
}

#[cfg(all(test, feature = "preview"))]
impl Browsers {
    /// Pretend the browser host was found.
    pub(crate) fn assume_host(&mut self) {
        self.checked = true;
    }

    /// A tab whose engine a (fake) renderer already holds.
    pub(crate) fn started_tab(&mut self, key: &ThreadKey, url: &str) -> u64 {
        let id = self.insert(key);
        let tab = self.tab_mut(key, id).unwrap();
        tab.url = url.into();
        tab.started = true;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;
    use yaac_core::demo::{Scenario, threads};

    #[test]
    fn area_lifetime_and_ids_are_thread_scoped() {
        let mut browsers = Browsers::default();
        let a = ThreadKey::new("alpha", "same");
        let b = ThreadKey::new("beta", "same");
        let first = browsers.insert(&a);
        let second = browsers.insert(&a);
        let other = browsers.insert(&b);
        browsers.close(&a, other);
        assert!(browsers.tab_mut(&b, first).is_none());
        assert_eq!(browsers.areas[&b].active, other);
        browsers.close(&a, second);
        assert_eq!(browsers.areas[&a].active, first);
        browsers.close(&a, first);
        assert!(!browsers.areas.contains_key(&a));
        assert_eq!(browsers.areas[&b].tabs.len(), 1);
    }

    #[test]
    fn target_addresses() {
        for input in ["5173", ":5173", "localhost:5173", " 5173 "] {
            assert_eq!(address(input).unwrap().as_str(), "http://localhost:5173/");
        }
        assert_eq!(
            address("[::1]:5173/x").unwrap().as_str(),
            "http://[::1]:5173/x"
        );
        for input in ["0", "65536", "file:///etc", ""] {
            assert!(address(input).is_err());
        }
    }

    #[test]
    fn poll_only_for_visible_supported_area_and_wait_for_reply() {
        let (mut client, _) = crate::app::demo_client(Scenario::Fleet);
        let key = threads::orchestrator();
        let mut b = Browsers::default();
        let id = b.insert(&key);
        let now = Instant::now();
        let daemon = |c: &mut Client, supported| {
            c.state
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
                port_forward: supported,
            });
        };
        daemon(&mut client, false);
        assert!(b.navigate(&mut client, &key, id, "5173").is_err());
        b.tick(&mut client, Some(&key), now);
        assert!(b.pending.is_empty());
        daemon(&mut client, true);
        b.tick(&mut client, None, now);
        assert!(b.pending.is_empty());
        b.tick(&mut client, Some(&key), now);
        assert_eq!(b.pending.len(), 1);
        b.tick(&mut client, Some(&key), now + PORT_INTERVAL);
        assert_eq!(b.pending.len(), 1);
        client.pump();
        assert_eq!(b.tick(&mut client, Some(&key), now), Some(PORT_INTERVAL));
    }

    #[cfg(feature = "preview")]
    #[test]
    fn title_menu_opens_new_tab_in_the_selected_thread() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::orchestrator();
        app.browsers.checked = true;
        let target = app
            .client
            .state
            .targets
            .iter_mut()
            .find(|t| t.entry.id == key.target)
            .unwrap();
        target.entry.state.daemon = Some(attach::DaemonSummary {
            build: "test".into(),
            older: false,
            schema_hash_matches: true,
            can_update: false,
            port_forward: true,
        });
        let mut draw = |events| {
            let mut frame = eframe::Frame::_new_kittest();
            let mut out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1400., 900.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| eframe::App::ui(&mut app, ui, &mut frame),
            );
            out.textures_delta.clear();
            out.shapes
                .into_iter()
                .filter_map(|s| match s.shape {
                    egui::Shape::Text(t) => {
                        Some((t.galley.text().to_owned(), t.pos + t.galley.size() / 2.))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        fn click(
            draw: &mut impl FnMut(Vec<egui::Event>) -> Vec<(String, egui::Pos2)>,
            pos: egui::Pos2,
        ) {
            draw(vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                draw(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
        }
        draw(vec![]);
        click(&mut draw, egui::pos2(1246., 21.));
        let (_, browser) = draw(vec![])
            .into_iter()
            .find(|(text, _)| text == "Browser")
            .expect("Browser menu item");
        click(&mut draw, browser);
        let labels = draw(vec![]);
        assert!(
            labels
                .iter()
                .any(|(text, pos)| text == "Listening ports" && pos.x > 700.)
        );
        assert!(app.browsers.areas.contains_key(&key));
    }

    #[test]
    fn egui_full_screen_restores_split_and_last_close_restores_chat() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::orchestrator();
        let id = app.browsers.insert(&key);
        app.browsers.areas.get_mut(&key).unwrap().ratio = 0.37;
        let render = |app: &mut crate::app::YaacApp| {
            let mut chat = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000., 600.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    chat = crate::preview_ui::layout(ui, app, &key);
                },
            );
            output.textures_delta.clear();
            chat
        };
        let split = render(&mut app).unwrap();
        app.browsers.areas.get_mut(&key).unwrap().full_screen = true;
        assert!(render(&mut app).is_none());
        app.browsers.areas.get_mut(&key).unwrap().full_screen = false;
        assert_eq!(render(&mut app), Some(split));
        app.browsers.close(&key, id);
        assert!(render(&mut app).unwrap().width() > split.width() * 2.);
    }
}
