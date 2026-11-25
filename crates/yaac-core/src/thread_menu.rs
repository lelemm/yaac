//! A thread's context menu (D109) and the two confirmations it opens: Delete (D110) and
//! Archive (D111). The menu's order is fixed by the owner:
//!
//! **Copy thread ID, Rename** — separator — **Fork, Move…, Pin/Unpin, Mute…, Archive** —
//! separator — **Delete** (in the destructive colour). Move… (D144) sits after Fork, the other
//! entry that takes the thread somewhere else. A thread another program holds has **Take over**
//! after Fork (D140), where its harness allows it. An archived thread has **Unarchive** where Archive
//! was and no pin or mute (it is only ever under *Archived*, D93). Whether an entry can be used
//! comes from capabilities, never from the provider's kind.

use yaac_protocol::{Capability, Control, ThreadState, has_capability};

use crate::key::ThreadKey;
use crate::state::AppState;
use crate::transcript::TakeOverRequest;

/// The ID this window shares for a conversation. Older threads keep their native copy value.
pub fn copy_id(state: &AppState, key: &ThreadKey) -> String {
    if key.thread.as_str().starts_with("yaac-") {
        return key.thread.to_string();
    }
    state.summary(key).and_then(|summary| summary.native_id.clone()).unwrap_or_else(|| key.address().to_string())
}

/// What an entry does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Copy the conversation's shareable thread id.
    CopyId,
    /// Rename.
    Rename,
    /// Fork.
    Fork,
    /// Take over a thread another program holds (D140).
    TakeOver,
    /// Open the Move dialog (D144): another project or worktree on the same machine.
    Move,
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

/// **Take over** (D140) for a thread: offered only while another program holds it and its
/// harness can hand it over (`threadTakeOver`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TakeOver {
    /// It can be clicked now.
    pub enabled: bool,
    /// The request is in flight.
    pub busy: bool,
    /// What clicking does, or why it cannot be used.
    pub tooltip: String,
    /// Why the last attempt failed (shown on the banner; never retried on its own).
    pub failure: Option<String>,
}

/// What a successful takeover means, shown on the button (the owner's words, D140).
pub const TAKE_OVER_TIP: &str = "Continue here. The other program is not stopped and will not see what you send from now on.";

/// Whether, and how, **Take over** is offered for a thread.
pub fn take_over(state: &AppState, key: &ThreadKey) -> Option<TakeOver> {
    if state.control(key) != Control::External || !can(state, key, &Capability::ThreadTakeOver) {
        return None;
    }
    // A harness's own sub-agent (D151) is held by its parent; there is nothing to take over.
    if state.summary(key).is_some_and(|s| s.subagent.as_ref().is_some_and(|sub| sub.read_only.is_some())) {
        return None;
    }
    let t = state.transcripts.get(key);
    if t.is_some_and(|t| t.take_over == Some(TakeOverRequest::InFlight)) {
        return Some(TakeOver { enabled: false, busy: true, tooltip: "Taking over…".to_owned(), failure: None });
    }
    let failure = t.and_then(|t| match &t.take_over {
        Some(TakeOverRequest::Failed(why)) => Some(why.clone()),
        _ => None,
    });
    if let Some(why) = t.and_then(|t| t.take_over_blocked.clone()) {
        return Some(TakeOver { enabled: false, busy: false, tooltip: capitalised(&why), failure });
    }
    Some(TakeOver { enabled: true, busy: false, tooltip: TAKE_OVER_TIP.to_owned(), failure })
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
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
    if let Some(t) = take_over(state, key) {
        let reason = if t.busy { "Taking over…" } else { "The other program will not hand this thread over now" };
        out.push(item(MenuAction::TakeOver, "Take over", t.enabled, reason));
    }
    if s.archived {
        out.push(item(MenuAction::Unarchive, "Unarchive", can(state, key, &Capability::ThreadArchive), "This harness cannot archive threads"));
    } else {
        let why = crate::moving::blocked(state, key);
        out.push(item(MenuAction::Move, "Move…", why.is_none(), why.unwrap_or("")));
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
