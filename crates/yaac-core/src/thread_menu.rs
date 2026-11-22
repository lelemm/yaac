//! A thread's context menu (D109) and the two confirmations it opens: Delete (D110) and
//! Archive (D111). The menu's order is fixed by the owner:
//!
//! **Copy thread ID, Rename** — separator — **Fork, Pin/Unpin, Mute…, Archive** — separator —
//! **Delete** (in the destructive colour). An archived thread has **Unarchive** where Archive
//! was and no pin or mute (it is only ever under *Archived*, D93). Whether an entry can be used
//! comes from capabilities, never from the provider's kind.

use yaac_protocol::{Capability, ThreadState, has_capability};

use crate::key::ThreadKey;
use crate::state::AppState;

/// What an entry does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Copy the harness's own id (else YAAC's address).
    CopyId,
    /// Rename.
    Rename,
    /// Fork.
    Fork,
    /// Pin.
    Pin,
    /// Unpin.
    Unpin,
    /// Open the Mute dialog.
    Mute,
    /// Unmute a muted thread.
    Unmute,
    /// Change a mute.
    ChangeMute,
    /// Hide a discovered thread outside any project.
    Dismiss,
    /// Show a dismissed thread again.
    ShowAgain,
    /// Ask to archive (D111).
    Archive,
    /// Unarchive.
    Unarchive,
    /// Ask to delete (D110).
    Delete,
    /// Open the Create PR modal for the thread's branch (D143).
    CreatePr,
}

/// One row of the menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// What it does.
    pub action: MenuAction,
    /// Its label.
    pub label: &'static str,
    /// It can be used now.
    pub enabled: bool,
    /// Drawn in the destructive colour.
    pub danger: bool,
    /// Why it cannot be used (a tooltip), when disabled.
    pub reason: Option<&'static str>,
}

/// A row or a separator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    /// A row.
    Item(MenuItem),
    /// A hairline between groups.
    Separator,
}

impl MenuEntry {
    /// The row, if this is one.
    pub fn item(&self) -> Option<&MenuItem> {
        match self {
            MenuEntry::Item(i) => Some(i),
            MenuEntry::Separator => None,
        }
    }
}

/// Whether the thread can do `cap`: its effective capabilities when known (it is open), else
/// its provider's.
pub fn can(state: &AppState, key: &ThreadKey, cap: &Capability) -> bool {
    if let Some(e) = state.transcripts.get(key).and_then(|t| t.effective.as_ref()) {
        return has_capability(&e.capabilities, cap);
    }
    state.provider_of(key).is_some_and(|p| has_capability(&p.capabilities, cap))
}

fn item(action: MenuAction, label: &'static str, enabled: bool, reason: &'static str) -> MenuEntry {
    MenuEntry::Item(MenuItem { action, label, enabled, danger: false, reason: (!enabled).then_some(reason) })
}

/// The menu of a thread row at `now`.
pub fn thread_menu(state: &AppState, key: &ThreadKey, now: i64) -> Vec<MenuEntry> {
    let Some(s) = state.summary(key) else { return Vec::new() };
    let may_act = state.control(key).may_act();
    let running = s.state == ThreadState::Running;
    let mut out = vec![
        item(MenuAction::CopyId, "Copy thread ID", true, ""),
        item(MenuAction::Rename, "Rename", can(state, key, &Capability::ThreadRename), "This harness cannot rename threads"),
        MenuEntry::Separator,
        item(MenuAction::Fork, "Fork", true, ""),
    ];
    if s.archived {
        out.push(item(MenuAction::Unarchive, "Unarchive", can(state, key, &Capability::ThreadArchive), "This harness cannot archive threads"));
    } else {
        if state.is_pinned(key) {
            out.push(item(MenuAction::Unpin, "Unpin", true, ""));
        } else {
            out.push(item(MenuAction::Pin, "Pin", true, ""));
        }
        if state.is_muted(key, now) {
            out.push(item(MenuAction::Unmute, "Unmute", true, ""));
            out.push(item(MenuAction::ChangeMute, "Change mute…", true, ""));
        } else {
            out.push(item(MenuAction::Mute, "Mute…", true, ""));
        }
        if state.is_dismissed(key) {
            out.push(item(MenuAction::ShowAgain, "Show again", true, ""));
        } else if state.project_of(key).is_none() {
            out.push(item(MenuAction::Dismiss, "Dismiss", true, ""));
        }
        out.push(item(MenuAction::Archive, "Archive…", can(state, key, &Capability::ThreadArchive), "This harness cannot archive threads"));
    }
    // Pull requests (D143): offered on a branch of a supported forge; the badge shows only
    // once a real one exists.
    if let Some(ok) = crate::pr::create_entry(state, key) {
        out.push(MenuEntry::Separator);
        out.push(MenuEntry::Item(MenuItem {
            action: MenuAction::CreatePr,
            label: "Create pull request…",
            enabled: ok.is_ok(),
            danger: false,
            reason: ok.err(),
        }));
    }
    out.push(MenuEntry::Separator);
    let (enabled, reason) = if running {
        (false, Some("Stop the running turn first"))
    } else if !may_act {
        (false, Some("This thread is in use elsewhere"))
    } else {
        (true, None)
    };
    out.push(MenuEntry::Item(MenuItem { action: MenuAction::Delete, label: "Delete…", enabled, danger: true, reason }));
    out
}

/// The Delete confirmation (D110).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteDialog {
    /// The thread.
    pub key: ThreadKey,
    /// "Delete <title>?"
    pub title: String,
    /// "Also delete the session from <harness>", shown only when the harness can delete its
    /// own sessions (`threadDelete`).
    pub harness_label: Option<String>,
    /// The checkbox (checked by default).
    pub also_in_harness: bool,
}

impl DeleteDialog {
    /// The dialog for a thread, or `None` if it is not known.
    pub fn new(state: &AppState, key: &ThreadKey) -> Option<Self> {
        let s = state.summary(key)?;
        let harness_label = can(state, key, &Capability::ThreadDelete).then(|| {
            let name = state.provider_of(key).map_or_else(|| "the harness".to_owned(), |p| p.display_name.clone());
            format!("Also delete the session from {name}")
        });
        let title = if s.title.trim().is_empty() { "this thread".to_owned() } else { s.title.clone() };
        Some(Self { key: key.clone(), title: format!("Delete {title}?"), also_in_harness: harness_label.is_some(), harness_label })
    }

    /// The explanation under the title, which follows the checkbox.
    pub fn body(&self) -> &'static str {
        if self.harness_label.is_some() && self.also_in_harness {
            "The thread and its session in the harness are deleted. This cannot be undone."
        } else if self.harness_label.is_some() {
            "YAAC forgets the thread and its history here. The harness keeps its session."
        } else {
            "YAAC forgets the thread and its history here. This harness cannot delete its own sessions, so it keeps it."
        }
    }

    /// What confirming asks the target for: delete the harness's session too.
    pub fn also_in_harness(&self) -> bool {
        self.harness_label.is_some() && self.also_in_harness
    }
}

/// The Archive confirmation (D111).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveDialog {
    /// The thread.
    pub key: ThreadKey,
    /// "Archive <title>?"
    pub title: String,
    /// What it does.
    pub body: &'static str,
}

impl ArchiveDialog {
    /// The dialog for a thread, or `None` if it is not known.
    pub fn new(state: &AppState, key: &ThreadKey) -> Option<Self> {
        let s = state.summary(key)?;
        Some(Self {
            key: key.clone(),
            title: format!("Archive {}?", s.title),
            body: "It moves to Archived, and loses its pin and its mute. Unarchive brings it back.",
        })
    }
}
