//! Saved hubs on the desktop (P412, docs/15-hub-transfer.md, docs/07-ui.md#several-hubs-on-desktop).
//!
//! The window keeps a list of hubs it can attach to: **This PC** (the local hub, over the local
//! socket) and any number of hubs on other machines by their Tailscale address. One is attached
//! at a time. The list is the host's own (`hubs.json` in the config dir, next to `display.json`),
//! never the hub cache: it says which hub to attach to, so it cannot live in a hub.
//!
//! A hub that moved (`hub/moved`, or the tombstone `moved.json` the old hub left in its data
//! dir) keeps its entry and `hubId`; only how it is reached changes. The window reaches a hub
//! that moved to one of its targets the way it reached that target, without Tailscale
//! (`via`: `wsl.exe` or `ssh` to `yaac hub-bridge` there); its Tailscale `address` is the
//! phones' way in. A hub that moved back to the PC's Windows is This PC again.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use yaac_protocol::attach::TargetTransport;

/// The port a hub's Tailscale attach listener uses when an address names none.
pub(crate) const DEFAULT_PORT: u16 = 47380;

/// The file, in the host's config dir.
pub(crate) const FILE: &str = "hubs.json";

/// The key of This PC in the file's `selected`.
const LOCAL_KEY: &str = "local";

/// One saved hub.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedHub {
    /// `host:port` of a hub on another machine; `None` is This PC (the local hub).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// What the user called it when adding it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The hub's stable id, from its last `hub/hello`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_id: Option<String>,
    /// The hub's machine name, from its last `hub/hello`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
    /// A hub that moved to one of the PC's targets: how this window reaches it without
    /// Tailscale (`hub/moved`'s `via`: `wsl` or `ssh`). Then `address` is only shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<TargetTransport>,
}

impl SavedHub {
    fn local() -> Self {
        Self::default()
    }

    /// This PC: the local hub over the local socket.
    pub(crate) fn is_local(&self) -> bool {
        self.address.is_none() && self.bridge().is_none()
    }

    /// The target a moved hub is reached through (`wsl`, `ssh`), when this window reaches it
    /// that way.
    pub(crate) fn bridge(&self) -> Option<&TargetTransport> {
        self.via.as_ref().filter(|v| matches!(v, TargetTransport::Wsl(_) | TargetTransport::Ssh(_)))
    }

    pub(crate) fn key(&self) -> String {
        match self.bridge() {
            Some(TargetTransport::Wsl(w)) => format!("wsl:{}", w.distro.to_lowercase()),
            Some(TargetTransport::Ssh(s)) => format!("ssh:{}", yaac_hub::ssh::Ident::from_transport(s).label()),
            _ => self.address.clone().unwrap_or_else(|| LOCAL_KEY.to_owned()),
        }
    }

    /// The name shown: This PC (also when its hub lives in one of its WSL distros), else the
    /// hub's machine name, the label, the address's machine.
    pub(crate) fn name(&self) -> String {
        let named = || self.host_name.clone().filter(|n| !n.is_empty()).or_else(|| self.label.clone().filter(|l| !l.is_empty()));
        match (self.bridge(), &self.address) {
            (Some(TargetTransport::Wsl(w)), _) => format!("This PC (in WSL: {})", w.distro),
            (Some(TargetTransport::Ssh(s)), _) => named().unwrap_or_else(|| s.host.clone()),
            (_, None) => "This PC".to_owned(),
            (_, Some(address)) => named().unwrap_or_else(|| crate::attach::machine_name(address).to_owned()),
        }
    }

    /// How it is reached, shown under the name (and the phones' Tailscale address).
    pub(crate) fn address_text(&self) -> String {
        let phones = |text: String| match &self.address {
            Some(a) => format!("{text} · phones: {a}"),
            None => format!("{text} · phones: no Tailscale there yet"),
        };
        match self.bridge() {
            Some(TargetTransport::Wsl(w)) => phones(format!("In WSL ({})", w.distro)),
            Some(TargetTransport::Ssh(s)) => phones(format!("Over SSH ({})", yaac_hub::ssh::Ident::from_transport(s).label())),
            _ => self.address.clone().unwrap_or_else(|| "This computer's hub".to_owned()),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    hubs: Vec<SavedHub>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<String>,
}

/// Writes go off the UI thread, newest wins.
#[derive(Default)]
struct Writer {
    lock: Arc<Mutex<()>>,
    generation: Arc<AtomicU64>,
}

/// The saved hubs and which one the window attaches to.
pub(crate) struct SavedHubs {
    pub hubs: Vec<SavedHub>,
    pub selected: usize,
    path: Option<PathBuf>,
    /// Tests write synchronously.
    sync_writes: bool,
    writer: Writer,
}

impl std::fmt::Debug for SavedHubs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedHubs").field("hubs", &self.hubs).field("selected", &self.selected).finish_non_exhaustive()
    }
}

/// `host`, `host:port` or `[v6]:port` with the default port added when there is none; `None`
/// for an empty or malformed address.
pub(crate) fn normalize(address: &str) -> Option<String> {
    let a = address.trim();
    if a.is_empty() || a.contains(char::is_whitespace) || a.contains('/') {
        return None;
    }
    if let Some(rest) = a.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        return match tail {
            "" => Some(format!("[{host}]:{DEFAULT_PORT}")),
            t => t.strip_prefix(':').filter(|p| p.parse::<u16>().is_ok()).map(|p| format!("[{host}]:{p}")),
        };
    }
    // A bare IPv6 address has several colons and no port.
    if a.matches(':').count() > 1 {
        return a.parse::<std::net::Ipv6Addr>().ok().map(|ip| format!("[{ip}]:{DEFAULT_PORT}"));
    }
    match a.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok() => Some(format!("{host}:{port}")),
        Some(_) => None,
        None => Some(format!("{a}:{DEFAULT_PORT}")),
    }
}

impl SavedHubs {
    /// The list in `config_dir` (This PC alone when there is no file or it cannot be read).
    pub(crate) fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(FILE);
        let file: File = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self::from_file(file, Some(path))
    }

    /// A list kept in memory only (tests).
    #[cfg(test)]
    pub(crate) fn in_memory(hubs: Vec<SavedHub>, selected: usize) -> Self {
        let mut s = Self::from_file(File { hubs, selected: None }, None);
        s.selected = selected.min(s.hubs.len() - 1);
        s
    }

    fn from_file(file: File, path: Option<PathBuf>) -> Self {
        let mut hubs: Vec<SavedHub> = Vec::new();
        for h in file.hubs {
            let h = match &h.address {
                None => h,
                Some(a) => match normalize(a) {
                    Some(address) => SavedHub { address: Some(address), ..h },
                    None => continue,
                },
            };
            if !hubs.iter().any(|o| o.key() == h.key()) {
                hubs.push(h);
            }
        }
        if hubs.is_empty() {
            hubs.push(SavedHub::local());
        }
        let selected = file.selected.and_then(|k| hubs.iter().position(|h| h.key() == k)).unwrap_or(0);
        Self { hubs, selected, path, sync_writes: cfg!(test), writer: Writer::default() }
    }

    /// The hub the window attaches to.
    pub(crate) fn current(&self) -> &SavedHub {
        &self.hubs[self.selected]
    }

    /// Whether the badge and the drop-down are shown.
    pub(crate) fn several(&self) -> bool {
        self.hubs.len() > 1
    }

    fn save(&self) {
        let Some(path) = self.path.clone() else { return };
        let file = File { hubs: self.hubs.clone(), selected: Some(self.current().key()) };
        let Ok(bytes) = serde_json::to_vec_pretty(&file) else { return };
        let generation = self.writer.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let write = {
            let lock = self.writer.lock.clone();
            let latest = self.writer.generation.clone();
            move || {
                let _guard = lock.lock();
                if latest.load(Ordering::SeqCst) != generation {
                    return;
                }
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, &bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
        };
        if self.sync_writes {
            write();
        } else {
            let _ = std::thread::Builder::new().name("yaac-hubs-save".into()).spawn(write);
        }
    }

    /// Adds a hub on another machine. Returns its index, or why it cannot be added.
    pub(crate) fn add(&mut self, address: &str, label: &str) -> Result<usize, String> {
        let Some(address) = normalize(address) else {
            return Err("Enter the hub's Tailscale name or address, like desk or desk.tailnet.ts.net:47380.".to_owned());
        };
        if self.hubs.iter().any(|h| h.address.as_deref() == Some(address.as_str())) {
            return Err(format!("{address} is already saved."));
        }
        let label = label.trim();
        self.hubs.push(SavedHub { address: Some(address), label: (!label.is_empty()).then(|| label.to_owned()), ..Default::default() });
        self.save();
        Ok(self.hubs.len() - 1)
    }

    /// Whether `index` can be removed: not the attached hub (switch away first), never the last.
    pub(crate) fn removable(&self, index: usize) -> bool {
        index < self.hubs.len() && index != self.selected && self.hubs.len() > 1
    }

    /// Removes a saved hub (the hub itself is never touched).
    pub(crate) fn remove(&mut self, index: usize) -> bool {
        if !self.removable(index) {
            return false;
        }
        self.hubs.remove(index);
        if index < self.selected {
            self.selected -= 1;
        }
        self.save();
        true
    }

    /// Selects a hub; `true` when that changed which hub the window attaches to.
    pub(crate) fn select(&mut self, index: usize) -> bool {
        if index >= self.hubs.len() || index == self.selected {
            return false;
        }
        self.selected = index;
        self.save();
        true
    }

    /// The attached hub answered `hub/hello`: remember its id and machine name.
    pub(crate) fn hello(&mut self, hub_id: Option<&str>, host_name: Option<&str>) {
        let h = &mut self.hubs[self.selected];
        let id = hub_id.map(str::to_owned).or_else(|| h.hub_id.clone());
        let name = host_name.map(str::to_owned).or_else(|| h.host_name.clone());
        if h.hub_id != id || h.host_name != name {
            h.hub_id = id;
            h.host_name = name;
            self.save();
        }
    }

    /// A hub moved (`hub/moved`, or the tombstone): the entry with that `hubId` (else This
    /// PC, else the selected one) takes the new way in. `via`: how this window reaches it
    /// (`wsl`, `ssh`: through that target; `local`: it is This PC again); without one, the
    /// Tailscale `address` (older hubs). Any other entry for the same hub or way in is dropped.
    /// The moved hub is selected when its entry was. Returns whether the attached hub changed
    /// where it is reached.
    pub(crate) fn moved(&mut self, hub_id: &str, address: &str, host_name: &str, via: Option<&TargetTransport>) -> bool {
        let address = (!address.is_empty()).then(|| normalize(address).unwrap_or_else(|| address.to_owned()));
        let index = self.hubs.iter().position(|h| h.hub_id.as_deref() == Some(hub_id))
            .or_else(|| self.hubs.iter().position(|h| h.is_local() && h.hub_id.is_none()))
            .unwrap_or(self.selected);
        let was_selected = index == self.selected;
        let before = self.hubs[index].clone();
        let entry = &mut self.hubs[index];
        entry.hub_id = Some(hub_id.to_owned());
        if matches!(via, Some(TargetTransport::Local(_))) {
            // Back on this PC's Windows: the local socket again.
            (entry.address, entry.via, entry.label, entry.host_name) = (None, None, None, None);
        } else {
            entry.via = via.filter(|v| matches!(v, TargetTransport::Wsl(_) | TargetTransport::Ssh(_))).cloned();
            entry.address = address.clone();
            if !host_name.is_empty() {
                entry.host_name = Some(host_name.to_owned());
            }
        }
        let keep = self.hubs[index].clone();
        let keep_key = keep.key();
        let selected_key = self.current().key();
        let mut i = 0;
        self.hubs.retain(|h| {
            let this = i;
            i += 1;
            this == index || !(h.hub_id.as_deref() == Some(hub_id) || h.key() == keep_key || (address.is_some() && h.address == address))
        });
        self.selected = if was_selected {
            self.hubs.iter().position(|h| *h == keep).unwrap_or(0)
        } else {
            self.hubs.iter().position(|h| h.key() == selected_key).unwrap_or(0)
        };
        self.save();
        was_selected && (before.address != keep.address || before.via != keep.via)
    }

    /// Start the hub here again: the moved hub's entry becomes This PC again, selected.
    pub(crate) fn reclaim(&mut self, hub_id: &str) {
        let index = self.hubs.iter().position(|h| h.hub_id.as_deref() == Some(hub_id)).unwrap_or(self.selected);
        let entry = &mut self.hubs[index];
        entry.address = None;
        entry.via = None;
        entry.label = None;
        entry.host_name = None;
        let keep = entry.clone();
        let mut i = 0;
        self.hubs.retain(|h| {
            let this = i;
            i += 1;
            this == index || !h.is_local()
        });
        self.selected = self.hubs.iter().position(|h| *h == keep).unwrap_or(0);
        self.save();
    }
}

/// The initial on the badge: from the name, two letters when another hub's initial is the same
/// (docs/07-ui.md#several-hubs-on-android).
pub(crate) fn initial(hubs: &[SavedHub], index: usize) -> String {
    let first = |n: &str| n.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    let two = |n: &str| {
        let words: Vec<&str> = n.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        let s: String = match words.as_slice() {
            [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
            [a] => a.chars().take(2).collect(),
            [] => "?".into(),
        };
        s.to_uppercase()
    };
    let names: Vec<String> = hubs.iter().map(SavedHub::name).collect();
    let mine = first(&names[index]);
    if names.iter().enumerate().any(|(i, n)| i != index && first(n) == mine) {
        let mine2 = two(&names[index]);
        if names.iter().enumerate().any(|(i, n)| i != index && two(n) == mine2) {
            return format!("{}{}", mine, index + 1);
        }
        return mine2;
    }
    mine
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(address: &str, id: Option<&str>) -> SavedHub {
        SavedHub { address: Some(address.into()), hub_id: id.map(Into::into), ..Default::default() }
    }

    #[test]
    fn addresses_get_the_default_port() {
        assert_eq!(normalize("desk").as_deref(), Some("desk:47380"));
        assert_eq!(normalize(" desk.tailnet.ts.net:5000 ").as_deref(), Some("desk.tailnet.ts.net:5000"));
        assert_eq!(normalize("100.64.0.7").as_deref(), Some("100.64.0.7:47380"));
        assert_eq!(normalize("fd7a:115c::1").as_deref(), Some("[fd7a:115c::1]:47380"));
        assert_eq!(normalize("[fd7a:115c::1]:9").as_deref(), Some("[fd7a:115c::1]:9"));
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("desk:port"), None);
        assert_eq!(normalize("a b"), None);
    }

    #[test]
    fn a_missing_file_is_this_pc_alone_and_add_remove_select_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut hubs = SavedHubs::load(dir.path());
        assert_eq!(hubs.hubs, vec![SavedHub::local()]);
        assert!(!hubs.several());
        assert_eq!(hubs.add("desk", "Work").unwrap(), 1);
        assert!(hubs.add("desk:47380", "").is_err(), "the same address twice");
        assert!(hubs.add("", "").is_err());
        assert!(hubs.several());
        assert!(hubs.select(1));
        assert!(!hubs.select(1), "already selected");
        assert!(!hubs.removable(1), "the attached hub is not removed");
        let again = SavedHubs::load(dir.path());
        assert_eq!(again.selected, 1);
        assert_eq!(again.current().address.as_deref(), Some("desk:47380"));
        assert_eq!(again.current().name(), "Work");
        assert!(hubs.remove(0));
        assert_eq!(hubs.selected, 0);
        assert!(!hubs.remove(0), "never the last one");
        assert_eq!(SavedHubs::load(dir.path()).hubs.len(), 1);
    }

    #[test]
    fn hello_names_the_attached_hub() {
        let mut hubs = SavedHubs::in_memory(vec![SavedHub::local(), remote("desk:47380", None)], 1);
        hubs.hello(Some("h-desk"), Some("DESK"));
        assert_eq!(hubs.current().hub_id.as_deref(), Some("h-desk"));
        assert_eq!(hubs.current().name(), "DESK");
        hubs.hello(None, None);
        assert_eq!(hubs.current().hub_id.as_deref(), Some("h-desk"), "an older hub's hello keeps what is known");
    }

    #[test]
    fn hub_moved_replaces_this_pc_by_the_remote_entry() {
        let mut hubs = SavedHubs::in_memory(vec![
            SavedHub { hub_id: Some("h-pc".into()), host_name: Some("PC".into()), ..Default::default() },
            remote("lab:47380", Some("h-lab")),
        ], 0);
        assert!(hubs.moved("h-pc", "100.64.0.9:47380", "box", None));
        assert_eq!(hubs.hubs.len(), 2);
        assert_eq!(hubs.selected, 0);
        let c = hubs.current();
        assert_eq!((c.address.as_deref(), c.hub_id.as_deref(), c.name().as_str()), (Some("100.64.0.9:47380"), Some("h-pc"), "box"));
        assert!(!hubs.hubs.iter().any(SavedHub::is_local), "the old This PC entry is gone");
        assert!(!hubs.moved("h-pc", "100.64.0.9:47380", "box", None), "the same move again changes nothing");
    }

    #[test]
    fn hub_moved_drops_another_entry_for_the_same_address() {
        let mut hubs = SavedHubs::in_memory(vec![remote("old:47380", Some("h")), remote("new:47380", None), SavedHub::local()], 2);
        assert!(!hubs.moved("h", "new", "new", None), "the attached hub (This PC) did not move");
        assert_eq!(hubs.hubs.len(), 2);
        assert_eq!(hubs.hubs[0].address.as_deref(), Some("new:47380"));
        assert!(hubs.current().is_local(), "the selection stays on This PC");
    }

    #[test]
    fn a_hub_in_this_pcs_wsl_stays_this_pc_reached_through_wsl() {
        let dir = tempfile::tempdir().unwrap();
        let mut hubs = SavedHubs::load(dir.path());
        hubs.hello(Some("h-pc"), Some("PC"));
        let wsl: TargetTransport = serde_json::from_value(serde_json::json!({ "kind": "wsl", "distro": "Ubuntu" })).unwrap();
        // No Tailscale in the distro: the window still follows it.
        assert!(hubs.moved("h-pc", "", "ubuntu", Some(&wsl)));
        let c = hubs.current();
        assert!(!c.is_local() && c.address.is_none() && c.bridge() == Some(&wsl));
        assert_eq!(c.name(), "This PC (in WSL: Ubuntu)");
        assert_eq!(c.address_text(), "In WSL (Ubuntu) · phones: no Tailscale there yet");
        // Saved and read back, still selected.
        let again = SavedHubs::load(dir.path());
        assert_eq!(again.current().bridge(), Some(&wsl));
        // Tailscale came up there later and the hub moved on to a server: SSH, with its address.
        let ssh: TargetTransport = serde_json::from_value(serde_json::json!({ "kind": "ssh", "host": "box", "user": "me" })).unwrap();
        assert!(hubs.moved("h-pc", "100.64.0.9", "box", Some(&ssh)));
        assert_eq!(hubs.current().name(), "box");
        assert_eq!(hubs.current().address_text(), "Over SSH (me@box:22) · phones: 100.64.0.9:47380");
        assert_eq!(hubs.hubs.len(), 1);
        // And back to this PC's Windows: This PC on the local socket.
        assert!(hubs.moved("h-pc", "100.91.112.95:47380", "PC", Some(&TargetTransport::Local(yaac_protocol::attach::LocalTransport {}))));
        assert!(hubs.current().is_local());
        assert_eq!(hubs.current().name(), "This PC");
        assert_eq!(hubs.current().hub_id.as_deref(), Some("h-pc"));
    }

    #[test]
    fn reclaim_turns_the_moved_entry_back_into_this_pc() {
        let mut hubs = SavedHubs::in_memory(vec![remote("box:47380", Some("h-pc")), remote("lab:47380", Some("h-lab"))], 1);
        hubs.reclaim("h-pc");
        assert_eq!(hubs.selected, 0);
        assert!(hubs.current().is_local());
        assert_eq!(hubs.current().hub_id.as_deref(), Some("h-pc"));
        assert_eq!(hubs.current().name(), "This PC");
    }

    #[test]
    fn initials_take_two_letters_when_they_collide() {
        let hubs = vec![SavedHub::local(), remote("tower:47380", None), SavedHub { host_name: Some("work box".into()), ..remote("w:1", None) }];
        assert_eq!(initial(&hubs, 0), "TP");
        assert_eq!(initial(&hubs, 1), "TO");
        assert_eq!(initial(&hubs, 2), "W");
    }
}
