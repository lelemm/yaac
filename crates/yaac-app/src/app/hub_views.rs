//! What both desktop renderers (egui and the WPF view frames) show of the saved hubs (P412) and
//! of moving the hub (docs/15-hub-transfer.md): computed once here, drawn by each.

use yaac_core::state::HubTransferPhase;
use yaac_protocol::TargetId;

use super::{RECLAIM_AFTER, YaacApp};
use crate::attach::LinkStatus;

/// One saved hub in the drop-down and on Settings → Hubs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HubRow {
    pub index: usize,
    pub name: String,
    pub initial: String,
    pub address: String,
    /// Connected, Connecting…, Not reachable, Not attached.
    pub status: String,
    pub selected: bool,
    pub removable: bool,
    /// Attach again now (the selected hub, when it is not connected).
    pub retry: bool,
    /// **Start the hub here again** (the selected hub moved away from this PC and does not answer).
    pub reclaim: bool,
}

/// One destination in **Transfer hub to…**.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TransferChoice {
    pub target: TargetId,
    pub label: String,
    pub eligible: bool,
    pub reason: Option<String>,
    /// What phones will need there (Tailscale), from the hub.
    pub warning: Option<String>,
}

/// Settings → Advanced → Hub → Transfer hub to….
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TransferView {
    /// The destinations, or a line in their place (asking, or why they could not be listed).
    pub choices: Result<Vec<TransferChoice>, String>,
    pub picked: Option<TargetId>,
    /// An eligible destination is picked and no transfer runs.
    pub can_start: bool,
    /// The running or last transfer.
    pub progress: Option<TransferProgress>,
    /// The confirmation: title, text, button.
    pub confirm: Option<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TransferProgress {
    pub label: String,
    /// Percent, when known.
    pub percent: Option<f64>,
    pub message: Option<String>,
    pub failed: bool,
    pub running: bool,
}

/// Where a picked destination is, for the confirmation's wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MoveKind {
    /// Another machine (an SSH server).
    Server,
    /// One of this PC's WSL distros (P414).
    IntoWsl,
    /// This PC's own Windows.
    ToWindows,
}

/// What the confirmation says before the hub moves to `to`. `warning`: what the hub says about
/// phones there (no Tailscale, or another login), shown as is.
pub(crate) fn confirm_text(to: &str, kind: MoveKind, warning: Option<&str>) -> (String, String) {
    let body = match kind {
        MoveKind::Server => format!(
            "YAAC installs itself on {to}, copies the hub's threads, drafts and messages there, and starts the hub there; \
             the hub on this computer stops. Running turns keep going on their targets. This computer's window reaches the \
             hub over SSH, the way it reached {to}. WSL targets and this computer's own target can only be reached by a hub \
             on this computer: they stay listed, unavailable, until the hub comes back."
        ),
        MoveKind::IntoWsl => format!(
            "YAAC installs itself in {to}, copies the hub's threads, drafts and messages there, and sets the hub up there as \
             a system service that starts whenever {to} starts (as root, through wsl.exe; no password is asked). The hub \
             on this computer stops. Running turns keep going on their targets. This computer's other WSL distros and its \
             own Windows target stay reachable from there. WSL starts {to} when this window attaches to the hub, and stops \
             it when nothing on Windows uses it: while {to} is stopped, phones cannot reach the hub."
        ),
        MoveKind::ToWindows => format!(
            "The hub's threads, drafts and messages are copied to {to}, where YAAC is installed, and the hub starts there; \
             the hub here stops. Running turns keep going on their targets. The window on that PC attaches to it directly."
        ),
    };
    let body = match warning {
        Some(w) => format!("{body}\n\n{w}"),
        None => body,
    };
    (format!("Move the hub to {to}?"), body)
}

impl YaacApp {
    /// The saved hubs, for the drop-down and Settings → Hubs (empty outside a desktop window).
    pub(crate) fn hub_rows(&self) -> Vec<HubRow> {
        let Some(d) = &self.desk_hubs else { return Vec::new() };
        let status = self.hub.as_ref().and_then(|h| h.attach.lock().ok().map(|a| a.status()));
        let attached = self.client.state.hub.attached;
        let reclaim = self.reclaim_offered();
        d.saved.hubs.iter().enumerate().map(|(index, h)| {
            let selected = index == d.saved.selected;
            let (status, retry) = if !selected {
                ("Not attached".to_owned(), false)
            } else {
                match &status {
                    _ if attached => ("Connected".to_owned(), false),
                    Some(LinkStatus::Connecting) | Some(LinkStatus::Connected) => ("Connecting…".to_owned(), false),
                    _ => ("Not reachable".to_owned(), true),
                }
            };
            HubRow {
                index,
                name: h.name(),
                initial: super::hubs::initial(&d.saved.hubs, index),
                address: h.address_text(),
                status,
                selected,
                removable: d.saved.removable(index),
                retry,
                reclaim: selected && reclaim,
            }
        }).collect()
    }

    /// The badge left of the YAAC mark: the selected hub's initial, only when more than one
    /// hub is saved.
    pub(crate) fn hub_badge(&self) -> Option<String> {
        let d = self.desk_hubs.as_ref().filter(|d| d.saved.several())?;
        Some(super::hubs::initial(&d.saved.hubs, d.saved.selected))
    }

    /// Why the last Add a hub was refused.
    pub(crate) fn hub_add_error(&self) -> Option<&str> {
        self.desk_hubs.as_ref()?.add_error.as_deref()
    }

    /// The line shown when the selected hub moved away and does not answer.
    pub(crate) fn reclaim_line(&self) -> Option<String> {
        if !self.reclaim_offered() {
            return None;
        }
        let d = self.desk_hubs.as_ref()?;
        if let Some(distro) = d.moved_here.as_ref().and_then(|t| t.wsl_distro.as_deref())
            && d.reclaiming.is_some()
        {
            return Some(format!("Stopping the hub in WSL ({distro})…"));
        }
        let name = d.saved.current().name();
        Some(format!(
            "The hub moved to {name}, which has not answered for {} seconds. You can start the hub on this computer again, from the data it had when it moved.",
            RECLAIM_AFTER.as_secs()
        ))
    }

    /// What **Start the hub here again** asks before it does anything.
    pub(crate) fn reclaim_confirm_text(&self) -> String {
        let wsl = self.desk_hubs.as_ref().and_then(|d| d.moved_here.as_ref()).and_then(|t| t.wsl_distro.clone());
        match wsl {
            Some(distro) => format!(
                "The hub's system service in WSL ({distro}) is stopped and removed first. Then the hub starts here from the data it had when it moved: what changed on the hub in WSL since then stays there."
            ),
            None => "The hub starts here from the data it had when it moved. If the hub on the other machine is still running, the two will differ: stop it there first.".to_owned(),
        }
    }

    /// Settings → Advanced → Hub → Transfer hub to….
    pub(crate) fn transfer_view(&self) -> TransferView {
        let hub = &self.client.state.hub;
        let choices = match &hub.transfer_candidates {
            None if hub.attached => Err("Looking for machines that can take the hub…".to_owned()),
            None => Err("Attach to the hub first.".to_owned()),
            Some(Err(e)) => Err(format!("The machines could not be listed: {e}")),
            Some(Ok(list)) if list.is_empty() => Err("No targets. Add a WSL distro or an SSH target first.".to_owned()),
            Some(Ok(list)) => Ok(list.iter().map(|c| TransferChoice {
                target: c.target_id.clone(), label: c.label.clone(), eligible: c.eligible, reason: c.reason.clone(), warning: c.warning.clone(),
            }).collect()),
        };
        let running = hub.transfer.as_ref().is_some_and(|t| t.phase.active());
        let picked = self.ui.transfer_target.clone()
            .filter(|t| choices.as_ref().is_ok_and(|list: &Vec<TransferChoice>| list.iter().any(|c| &c.target == t)));
        let picked_choice = picked.as_ref().and_then(|t| choices.as_ref().ok()?.iter().find(|c| &c.target == t));
        let can_start = hub.attached && !running && picked_choice.is_some_and(|c| c.eligible);
        let progress = hub.transfer.as_ref().map(|t| {
            let to = choices.as_ref().ok().and_then(|l| l.iter().find(|c| c.target == t.target_id)).map_or_else(|| t.target_id.to_string(), |c| c.label.clone());
            let percent = match (t.phase.clone(), t.done, t.total) {
                (HubTransferPhase::Copying, Some(done), Some(total)) if total > 0 => Some((100.0 * done as f64 / total as f64).min(100.0)),
                _ => None,
            };
            TransferProgress {
                label: format!("{} · {to}", t.phase.label()),
                percent,
                message: t.message.clone(),
                failed: t.phase == HubTransferPhase::Failed,
                running: t.phase.active(),
            }
        });
        let kind = |t: &TargetId| match self.client.state.target(t).map(|t| &t.entry.transport) {
            Some(yaac_protocol::attach::TargetTransport::Wsl(_)) => MoveKind::IntoWsl,
            Some(yaac_protocol::attach::TargetTransport::WindowsInterop(_)) => MoveKind::ToWindows,
            _ => MoveKind::Server,
        };
        let confirm = picked_choice.map(|c| confirm_text(&c.label, kind(&c.target), c.warning.as_deref()));
        TransferView { choices, picked, can_start, progress, confirm }
    }

    /// While Settings → Advanced is shown, the hub is asked once where it could move.
    pub(crate) fn transfer_upkeep(&mut self) {
        let shown = self.ui.settings && self.ui.settings_page == yaac_core::settings::Page::Advanced;
        if !shown {
            self.ui.transfer_asked = false;
        } else if !self.ui.transfer_asked && self.client.state.hub.attached && self.demo.is_none() {
            self.ui.transfer_asked = true;
            self.client.load_transfer_candidates();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};
    use yaac_protocol::frame::{Frame, FrameDecoder, encode_message_frame};
    use yaac_protocol::rpc::Message;

    use super::super::hubs::SavedHub;
    use super::super::{Action, RECLAIM_AFTER, YaacApp};
    use crate::attach::AttachConfig;

    /// A hub on `listener` for one connection: answers `hub/hello` with `hello`, then sends
    /// `after_hello`; every other request gets `{}`.
    fn fake_hub(listener: TcpListener, hello: Value, after_hello: Vec<Message>) {
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut decoder = FrameDecoder::new();
            let mut buf = [0; 8192];
            let send = |stream: &mut std::net::TcpStream, m: &Message| {
                let mut bytes = Vec::new();
                encode_message_frame(&serde_json::to_vec(m).unwrap(), &mut bytes).unwrap();
                stream.write_all(&bytes).is_ok()
            };
            loop {
                let Ok(size) = stream.read(&mut buf) else { return };
                if size == 0 { return }
                let Ok(frames) = decoder.decode(&buf[..size]) else { return };
                for frame in frames {
                    let Frame::Message(bytes) = frame else { continue };
                    let Ok(Message::Request { id, method, .. }) = serde_json::from_slice(&bytes) else { continue };
                    let hello_now = method == yaac_protocol::attach::names::HELLO;
                    let result = if hello_now { hello.clone() } else { json!({}) };
                    if !send(&mut stream, &Message::Response { id, result }) { return }
                    if hello_now {
                        for m in &after_hello {
                            if !send(&mut stream, m) { return }
                        }
                    }
                }
            }
        });
    }

    pub(crate) fn local(dir: &std::path::Path) -> AttachConfig {
        AttachConfig { paths: yaac_hub::HubPaths::under(dir), exe: PathBuf::new(), autostart: None, start_timeout: Duration::from_millis(200), remote: None, bridge: None }
    }

    pub(crate) fn window(config: AttachConfig) -> YaacApp {
        let launch = crate::Launch {
            demo: None,
            theme: crate::theme::ThemeChoice::Dark,
            shots: None,
            host_windows: false,
            attach: Some(config),
            display: crate::display::DisplayState::default(),
            shell: crate::tray::Shell::new(),
            tray: false,
            updater: crate::updater::Updater::off("tests"),
        };
        YaacApp::from_ctx(&egui::Context::default(), launch)
    }

    fn wait(app: &mut YaacApp, what: &str, ready: impl Fn(&YaacApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready(app) {
            assert!(Instant::now() < deadline, "{what}");
            app.client.pump();
            app.hub_upkeep();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn attached_to(app: &YaacApp) -> Option<String> {
        app.hub.as_ref().and_then(|h| h.config.remote.clone())
    }

    fn save_hubs(config_dir: &std::path::Path, hubs: Value) {
        std::fs::create_dir_all(config_dir).unwrap();
        std::fs::write(config_dir.join(super::super::hubs::FILE), serde_json::to_vec(&hubs).unwrap()).unwrap();
    }

    #[test]
    fn a_tombstone_attaches_to_the_new_address_instead_of_starting_the_local_hub() {
        let dir = tempfile::tempdir().unwrap();
        let config = local(dir.path());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        fake_hub(listener, json!({ "build": crate::BUILD, "targets": [], "hubId": "h-pc", "hostName": "box" }), vec![]);
        save_hubs(&config.paths.config_dir, json!({ "hubs": [{ "hubId": "h-pc", "hostName": "PC" }] }));
        std::fs::create_dir_all(&config.paths.data_dir).unwrap();
        let tombstone = yaac_hub::paths::MovedTombstone { hub_id: "h-pc".into(), address: address.clone(), host_name: "box".into(), at_ms: 1, wsl_distro: None, via: None };
        yaac_hub::paths::write_moved_tombstone(&config.paths, &tombstone).unwrap();

        let mut app = window(config.clone());
        assert_eq!(attached_to(&app).as_deref(), Some(address.as_str()), "never the local socket");
        let d = app.desk_hubs.as_ref().unwrap();
        assert_eq!(d.saved.hubs.len(), 1, "This PC became the moved hub");
        assert_eq!(d.saved.current().hub_id.as_deref(), Some("h-pc"));
        assert!(d.on_moved_hub());
        wait(&mut app, "the moved hub did not answer", |a| a.client.state.hub.attached);
        assert_eq!(app.desk_hubs.as_ref().unwrap().saved.current().name(), "box");
        assert!(!app.reclaim_offered(), "it answers");
        // Kept for the next start.
        let again = super::super::hubs::SavedHubs::load(&config.paths.config_dir);
        assert_eq!(again.current().address.as_deref(), Some(address.as_str()));
    }

    #[test]
    fn a_moved_hub_that_does_not_answer_can_be_started_here_again() {
        let dir = tempfile::tempdir().unwrap();
        let config = local(dir.path());
        // Nothing listens there.
        let address = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        std::fs::create_dir_all(&config.paths.data_dir).unwrap();
        let tombstone = yaac_hub::paths::MovedTombstone { hub_id: "h-pc".into(), address: address.clone(), host_name: "box".into(), at_ms: 1, wsl_distro: None, via: None };
        yaac_hub::paths::write_moved_tombstone(&config.paths, &tombstone).unwrap();
        let mut app = window(config.clone());
        assert_eq!(attached_to(&app).as_deref(), Some(address.as_str()));
        wait(&mut app, "the attempt did not fail", |a| a.hub.as_ref().unwrap().lost_since.is_some());
        assert!(!app.reclaim_offered(), "not at the first failed attempt");
        app.apply(vec![Action::ReclaimHub]);
        assert!(yaac_hub::paths::moved_tombstone(&config.paths).unwrap().is_some(), "nothing happens before it is offered");
        app.hub.as_mut().unwrap().lost_since = Some(Instant::now() - RECLAIM_AFTER);
        assert!(app.reclaim_offered());
        assert!(app.reclaim_line().unwrap().contains("box"));
        assert!(app.hub_rows()[0].reclaim);
        app.apply(vec![Action::ReclaimHub]);
        assert!(yaac_hub::paths::moved_tombstone(&config.paths).unwrap().is_none(), "the tombstone is gone");
        let d = app.desk_hubs.as_ref().unwrap();
        assert!(d.saved.current().is_local());
        assert_eq!(d.saved.current().hub_id.as_deref(), Some("h-pc"));
        assert!(d.moved_here.is_none());
        assert_eq!(attached_to(&app), None, "attached to the local hub again");
    }

    #[test]
    fn switching_hubs_attaches_to_the_chosen_one_and_hub_moved_follows_the_hub() {
        let dir = tempfile::tempdir().unwrap();
        let config = local(dir.path());
        let a = TcpListener::bind("127.0.0.1:0").unwrap();
        let b = TcpListener::bind("127.0.0.1:0").unwrap();
        let (a_addr, b_addr) = (a.local_addr().unwrap().to_string(), b.local_addr().unwrap().to_string());
        let moved = Message::notification(yaac_protocol::attach::names::MOVED, json!({ "hubId": "h-a", "address": b_addr, "hostName": "newbox" }));
        fake_hub(a, json!({ "build": crate::BUILD, "targets": [], "hubId": "h-a", "hostName": "oldbox" }), vec![moved]);
        fake_hub(b, json!({ "build": crate::BUILD, "targets": [], "hubId": "h-a", "hostName": "newbox" }), vec![]);
        save_hubs(&config.paths.config_dir, json!({ "hubs": [{}, { "address": a_addr, "label": "Lab" }], "selected": "local" }));

        let mut app = window(config.clone());
        assert_eq!(attached_to(&app), None, "This PC is selected");
        assert_eq!(app.hub_badge().as_deref(), Some("T"), "two hubs: the badge shows");
        let rows = app.hub_rows();
        assert_eq!((rows[1].name.as_str(), rows[1].status.as_str(), rows[1].selected), ("Lab", "Not attached", false));
        assert!(!rows[0].removable && rows[1].removable);

        app.apply(vec![Action::SelectHub(1)]);
        assert_eq!(attached_to(&app).as_deref(), Some(a_addr.as_str()));
        // Its hello names it; then it moves, and the window follows to the same entry.
        wait(&mut app, "the window did not follow the hub", |app| attached_to(app).as_deref() == Some(b_addr.as_str()));
        let d = app.desk_hubs.as_ref().unwrap();
        assert_eq!(d.saved.hubs.len(), 2);
        assert_eq!(d.saved.selected, 1);
        let entry: &SavedHub = d.saved.current();
        assert_eq!((entry.address.as_deref(), entry.hub_id.as_deref(), entry.label.as_deref()), (Some(b_addr.as_str()), Some("h-a"), Some("Lab")));
        assert!(d.moved_here.is_none(), "it was not this PC's hub");
        wait(&mut app, "the moved hub did not answer", |a| a.client.state.hub.attached);
        assert_eq!(app.hub_rows()[1].status, "Connected");

        // Add and remove through the actions.
        app.apply(vec![Action::AddHub("desk".into(), String::new())]);
        assert_eq!(app.desk_hubs.as_ref().unwrap().saved.hubs.len(), 3);
        app.apply(vec![Action::AddHub("desk:47380".into(), String::new())]);
        assert!(app.hub_add_error().unwrap().contains("already saved"));
        app.apply(vec![Action::RemoveHub(2)]);
        app.apply(vec![Action::RemoveHub(1)]);
        assert_eq!(app.desk_hubs.as_ref().unwrap().saved.hubs.len(), 2, "the attached hub is not removed");
    }

    #[test]
    fn the_phone_has_no_saved_hubs_and_shows_where_the_hub_moved() {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let moved = Message::notification(yaac_protocol::attach::names::MOVED, json!({ "hubId": "h", "address": "100.64.0.9:47380", "hostName": "box" }));
        fake_hub(listener, json!({ "build": crate::BUILD, "targets": [], "hubId": "h", "hostName": "PC" }), vec![moved]);
        let mut app = window(AttachConfig::remote(address.clone(), yaac_hub::HubPaths::under(dir.path())));
        assert!(app.desk_hubs.is_none());
        assert!(app.hub_badge().is_none() && app.hub_rows().is_empty());
        wait(&mut app, "hub/moved did not arrive", |a| a.client.state.hub.moved.is_some());
        assert_eq!(attached_to(&app).as_deref(), Some(address.as_str()), "the phone's controller does not move itself");
    }
}
