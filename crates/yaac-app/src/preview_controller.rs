//! Thread-local window state and asynchronous hub operations shared by the renderers.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::json;
use url::Url;
use yaac_core::{ThreadKey, client::Client};
use yaac_protocol::{PreviewPort, PreviewPortsResult, attach, rpc::RpcId};

use crate::file_pane::{Document, FileRequest, FileTree, TabKind, Work};
use std::sync::Arc;

const PORT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct BrowserTab {
    pub(crate) id: u64,
    pub(crate) kind: TabKind,
    pub(crate) document: Option<Arc<Document>>,
    pub(crate) work: Option<Work>,
    pub(crate) generation: u64,
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
    snapshot: Option<std::path::PathBuf>,
    socks: Option<u16>,
    started: bool,
    #[cfg(feature = "preview")]
    retry_at: Option<Instant>,
    #[cfg(feature = "preview")]
    pub(crate) retry_delay: Option<Duration>,
    #[cfg(feature = "preview")]
    started_at: Option<Instant>,
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
    /// In full screen, the pinned **Chat** tab is shown instead of `active`, which stays the
    /// last non-chat tab for Restore (docs/12-files.md#tab-strip). False outside full screen.
    pub(crate) chat: bool,
    /// The transcript (item count, last item revision) as last seen while the chat was on
    /// screen, for the Chat tab's attention mark.
    chat_seen: Option<(usize, u64)>,
    pub(crate) tree: FileTree,
    pub(crate) ports: Vec<PreviewPort>,
    pub(crate) ports_error: Option<String>,
    /// Set by the first `preview/ports` reply (a result or an error): until then the New tab
    /// page shows a spinner instead of "No servers".
    pub(crate) ports_discovered: bool,
    pub(crate) epoch: u64,
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
            chat: false,
            chat_seen: None,
            tree: FileTree::default(),
            ports: vec![],
            ports_error: None,
            ports_discovered: false,
            epoch: 0,
            next_poll: Instant::now(),
            polling: false,
        }
    }
}

pub(crate) enum Pending {
    File(FileRequest),
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
    pub(crate) side_by_side: bool,
    pub(crate) file_wake: Option<Arc<dyn Fn() + Send + Sync>>,
    pub(crate) pending: HashMap<RpcId, Pending>,
    /// Terminal sessions: side-pane Terminal tabs and the panel under the composer.
    pub(crate) terms: crate::terminal::Terminals,
    tab_switch: Option<(ThreadKey, usize)>,
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
    if url.scheme() == "file" {
        return Err("Enter an HTTP or HTTPS address. For generated HTML, open its file card or create one with present_artifact.".into());
    }
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

fn chat_fingerprint(t: &yaac_core::transcript::Transcript) -> (usize, u64) {
    let n = t.items().len();
    (n, n.checked_sub(1).map_or(0, |i| t.item_rev(i)))
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

    pub(crate) fn insert(&mut self, key: &ThreadKey) -> u64 {
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
        area.chat = false;
        self.next
    }

    /// **+ → Terminal** in the side pane: a Terminal tab with a new session (the default shell
    /// unless one of `terminal/shells` is given).
    pub(crate) fn open_terminal(&mut self, client: &Client, key: &ThreadKey, shell: Option<yaac_protocol::TerminalShell>) -> Result<u64, String> {
        let session = self.terms.create(client, key, shell)?;
        Ok(self.terminal_tab(key, session))
    }

    /// **Toggle terminal panel** for a thread. The panel lives in the chat column, so when a
    /// non-Chat tab fills the window the Chat tab is shown with it.
    pub(crate) fn toggle_terminal_panel(&mut self, client: &Client, key: &ThreadKey) -> Result<bool, String> {
        let open = self.terms.toggle_panel(client, key)?;
        if open {
            self.show_chat_for_panel(key);
        }
        Ok(open)
    }

    /// **New terminal** in the panel (opening it), with a shell or the default.
    pub(crate) fn new_panel_terminal(&mut self, client: &Client, key: &ThreadKey, shell: Option<yaac_protocol::TerminalShell>) -> Result<u64, String> {
        let id = self.terms.panel_new(client, key, shell)?;
        self.show_chat_for_panel(key);
        Ok(id)
    }

    fn show_chat_for_panel(&mut self, key: &ThreadKey) {
        if self.areas.get(key).is_some_and(|a| a.full_screen && !a.chat) {
            self.select_chat(key);
        }
    }

    /// A side-pane tab showing an existing session.
    pub(crate) fn terminal_tab(&mut self, key: &ThreadKey, session: u64) -> u64 {
        let id = self.insert(key);
        let tab = self.tab_mut(key, id).expect("tab just inserted");
        tab.kind = TabKind::Terminal(session);
        id
    }

    /// An agent's tab: added after the thread's tabs without changing the active tab (the
    /// user's view), except that a thread's first tab opens its browser area.
    #[cfg(feature = "preview")]
    pub(crate) fn open_background(&mut self, client: &mut Client, key: &ThreadKey, input: &str) -> Result<u64, String> {
        if let Some(reason) = self.reason(client, Some(key)) {
            return Err(reason);
        }
        address(input)?;
        let active = self.areas.get(key).filter(|a| !a.tabs.is_empty()).map(|a| (a.active, a.chat));
        let id = self.insert(key);
        if let Some((active, chat)) = active {
            let area = self.areas.get_mut(key).expect("area just used");
            area.active = active;
            area.chat = chat;
        }
        if let Err(e) = self.navigate(client, key, id, input) {
            self.close(key, id);
            return Err(e);
        }
        Ok(id)
    }

    /// Demo screenshots: no port or tree requests for this area for an hour.
    pub(crate) fn hold_requests(&mut self, client: &Client, key: &ThreadKey) {
        let turn = client.state.transcripts.get(key).and_then(|t| t.last_completed_turn.clone());
        if let Some(area) = self.areas.get_mut(key) {
            area.next_poll = Instant::now() + Duration::from_secs(3600);
            area.ports_discovered = true;
            area.tree.hold(turn);
        }
    }

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

    /// Full screen on or off. Opening it keeps the tab that was active; Restore goes back to
    /// the split with the last non-chat tab on the right.
    pub(crate) fn set_full_screen(&mut self, key: &ThreadKey, on: bool) {
        self.finish_tab_switch(key, false);
        if let Some(area) = self.areas.get_mut(key) {
            area.full_screen = on;
            area.chat = false;
        }
    }

    /// Show a pane tab (leaving the Chat tab in full screen).
    pub(crate) fn select(&mut self, key: &ThreadKey, id: u64) {
        self.finish_tab_switch(key, false);
        if let Some(area) = self.areas.get_mut(key)
            && area.tabs.iter().any(|t| t.id == id)
        {
            area.active = id;
            area.chat = false;
        }
    }

    /// Show the pinned Chat tab; only in full screen (in the split the chat is always shown).
    pub(crate) fn select_chat(&mut self, key: &ThreadKey) {
        self.finish_tab_switch(key, false);
        if let Some(area) = self.areas.get_mut(key).filter(|a| a.full_screen) {
            area.chat = true;
        }
    }

    pub(crate) fn can_switch_tabs(&self, key: &ThreadKey) -> bool {
        self.areas.get(key).is_some_and(|a| a.full_screen && !a.tabs.is_empty())
    }

    pub(crate) fn cycle_tab(&mut self, key: &ThreadKey) -> bool {
        let Some(area) = self.areas.get(key).filter(|a| a.full_screen && !a.tabs.is_empty()) else { return false };
        let current = self.switch_tab_index(key).unwrap_or_else(|| {
            if area.chat { 0 } else { area.tabs.iter().position(|tab| tab.id == area.active).map_or(0, |i| i + 1) }
        });
        self.tab_switch = Some((key.clone(), (current + 1) % (area.tabs.len() + 1)));
        true
    }

    pub(crate) fn switch_tab_index(&self, key: &ThreadKey) -> Option<usize> {
        self.tab_switch.as_ref().filter(|(thread, _)| thread == key).map(|(_, index)| *index)
    }

    pub(crate) fn retain_tab_switch(&mut self, key: Option<&ThreadKey>) {
        if self.tab_switch.as_ref().is_some_and(|(thread, _)| Some(thread) != key || !self.can_switch_tabs(thread)) {
            self.tab_switch = None;
        }
    }

    pub(crate) fn finish_tab_switch(&mut self, key: &ThreadKey, activate: bool) {
        let Some(index) = self.switch_tab_index(key) else { return };
        self.tab_switch = None;
        if activate && self.can_switch_tabs(key) {
            if index == 0 {
                self.select_chat(key);
            } else if let Some(tab) = self.areas[key].tabs.get(index - 1) {
                self.select(key, tab.id);
            }
        }
    }

    /// Whether the thread's chat is on screen: no pane, the split, or the Chat tab.
    pub(crate) fn chat_visible(&self, key: &ThreadKey) -> bool {
        self.areas.get(key).is_none_or(|a| !a.full_screen || a.chat)
    }

    /// The Chat tab's attention mark: while another tab is shown in full screen, the thread
    /// runs (an agent acting), needs the user, or has new transcript items since the chat was
    /// last on screen.
    pub(crate) fn chat_marked(&self, client: &Client, map: &yaac_core::attention::AttentionMap, key: &ThreadKey) -> bool {
        if self.chat_visible(key) {
            return false;
        }
        let transcript = client.state.transcripts.get(key);
        map.of(key).is_some()
            || transcript.is_some_and(|t| t.active_turn.is_some())
            || self.areas.get(key).and_then(|a| a.chat_seen) != transcript.map(chat_fingerprint)
    }

    pub(crate) fn close(&mut self, key: &ThreadKey, id: u64) {
        self.finish_tab_switch(key, false);
        let Some(area) = self.areas.get_mut(key) else {
            return;
        };
        let Some(index) = area.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        if let TabKind::Terminal(session) = area.tabs[index].kind {
            self.terms.closing.push(session);
        }
        area.tabs.remove(index);
        if area.tabs.is_empty() {
            self.areas.remove(key);
        } else if area.active == id {
            area.active = area.tabs[index.min(area.tabs.len() - 1)].id;
        }
    }

    /// Open only a snapshot prepared by the host; normal navigation still validates user input.
    pub(crate) fn artifact(&mut self, client: &mut Client, key: &ThreadKey, file: &std::path::Path) -> Result<(), String> {
        let url = Url::parse("https://artifact.invalid/document").expect("static document URL");
        let id = self.new_tab(client, key)?;
        self.tab_mut(key, id).expect("new tab").snapshot = Some(file.to_owned());
        self.load_url(client, key, id, url)
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
        self.load_url(client, key, id, address(input)?)
    }

    fn load_url(&mut self, client: &mut Client, key: &ThreadKey, id: u64, url: Url) -> Result<(), String> {
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
        let now = Instant::now();
        if tab.started || tab.retry_at.is_some_and(|retry| retry > now) { return None; }
        let socks = tab.socks?;
        tab.started = true;
        tab.retry_at = None;
        tab.started_at = Some(now);
        tab.error = None;
        let url = tab.url.clone();
        let snapshot = tab.snapshot.clone();
        Some(self.host.open_document(&url, Some(socks), snapshot, wake))
    }

    #[cfg(feature = "preview")]
    pub(crate) fn restart_browser_tab(&mut self, key: &ThreadKey, id: u64, error: String, now: Instant) {
        let Some(tab) = self.tab_mut(key, id) else { return };
        let stable = tab.started_at.is_some_and(|opened| now.saturating_duration_since(opened) >= Duration::from_secs(30));
        let delay = tab.retry_delay.filter(|_| !stable).map_or(Duration::from_millis(250), |last| (last * 2).min(Duration::from_secs(10)));
        tab.started = false;
        tab.loading = true;
        tab.back = false;
        tab.forward = false;
        tab.retry_delay = Some(delay);
        tab.retry_at = Some(now + delay);
        tab.error = Some(format!("Restarting the browser · {error}"));
    }

    #[cfg(feature = "preview")]
    pub(crate) fn browser_retry(&self, now: Instant) -> Option<Duration> {
        self.areas.values().flat_map(|area| &area.tabs).filter_map(|tab| tab.retry_at)
            .min().map(|retry| retry.saturating_duration_since(now))
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

    #[cfg(feature = "preview")]
    pub(crate) fn external_link(&mut self, client: &mut Client, key: &ThreadKey, input: &str) {
        let Ok(url) = Url::parse(input) else { return };
        if !matches!(url.scheme(), "http" | "https") { return; }
        let target_local = match url.host() {
            Some(url::Host::Domain(name)) => name == "localhost" || name.ends_with(".localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_unspecified(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unspecified(),
            None => false,
        };
        if target_local {
            if let Err(error) = self.external_address(client, key, url.as_str()) { self.notice = Some(error); }
        } else {
            crate::browser::open(url.as_str());
        }
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

    pub(crate) fn tick(&mut self, client: &mut Client, visible: Option<&ThreadKey>, now: Instant) -> Option<Duration> {
        // Replies first (a terminal's stream becomes known), then the terminals' output, which
        // may have arrived in the same batch.
        let rest = self.tick_pane(client, visible, now);
        let terms = self.terms.tick(client, now);
        match (terms, rest) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn tick_pane(
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
            let Some(pending) = self.pending.remove(&id) else {
                self.terms.on_reply(client, &id, result);
                continue;
            };
            match Some(pending) {
                Some(Pending::File(request)) => self.file_reply(client, request, result),
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
                        area.ports_discovered = true;
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
        self.finish_file_jobs();
        if let Some(key) = visible
            && self.chat_visible(key)
            && let Some(area) = self.areas.get_mut(key)
        {
            area.chat_seen = client.state.transcripts.get(key).map(chat_fingerprint);
        }
        // The Chat tab hides the pane's tab: no tree or port polling while it is shown.
        let visible = visible.filter(|k| self.areas.get(k).is_none_or(|a| !(a.full_screen && a.chat)));
        if let Some(key) = visible && self.areas.get(key).and_then(|a| self.tab(key,a.active)).is_some_and(|t| t.kind != TabKind::Browser) {
            return self.tick_files(client,key,now);
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
    fn full_screen_switcher_keeps_the_current_tab_until_confirmation() {
        let mut browsers = Browsers::default();
        let key = ThreadKey::new("local", "chat");
        let first = browsers.insert(&key);
        let second = browsers.insert(&key);
        browsers.select(&key, first);
        assert!(!browsers.cycle_tab(&key));

        browsers.set_full_screen(&key, true);
        assert!(browsers.cycle_tab(&key));
        assert_eq!(browsers.switch_tab_index(&key), Some(2));
        assert_eq!(browsers.areas[&key].active, first);
        browsers.finish_tab_switch(&key, false);
        assert_eq!(browsers.areas[&key].active, first);

        browsers.cycle_tab(&key);
        browsers.finish_tab_switch(&key, true);
        assert_eq!(browsers.areas[&key].active, second);
        browsers.cycle_tab(&key);
        browsers.finish_tab_switch(&key, true);
        assert!(browsers.areas[&key].chat);
    }

    #[cfg(feature = "preview")]
    #[test]
    fn browser_host_exit_preserves_the_tab_and_backs_off() {
        let mut browsers = Browsers::default();
        let key = ThreadKey::new("local", "chat");
        let id = browsers.insert(&key);
        let now = Instant::now();
        let tab = browsers.tab_mut(&key, id).unwrap();
        tab.url = "http://localhost:5173/report".into();
        tab.socks = Some(12000);
        tab.started = true;
        tab.started_at = Some(now);

        browsers.restart_browser_tab(&key, id, "crash".into(), now);
        let tab = browsers.tab(&key, id).unwrap();
        assert_eq!(tab.url, "http://localhost:5173/report");
        assert_eq!(tab.socks, Some(12000));
        assert!(!tab.started);
        assert_eq!(browsers.browser_retry(now), Some(Duration::from_millis(250)));

        browsers.restart_browser_tab(&key, id, "crash again".into(), now);
        assert_eq!(browsers.browser_retry(now), Some(Duration::from_millis(500)));
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
                file_view: false,
                terminal: false,
                budget_time_limit: false,
                terminal_unavailable_reason: None,
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
            file_view: false,
            terminal: false,
            budget_time_limit: false,
            terminal_unavailable_reason: None,
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
                .any(|(text, pos)| text.starts_with("Servers on ") && pos.x > 700.)
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
                    let map = yaac_core::attention::AttentionMap::compute(&app.client.state);
                    chat = crate::preview_ui::layout(ui, app, &map, &key);
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
