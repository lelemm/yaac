//! Skills in the composer: when to open the picker, what it lists, what a pick inserts, and
//! which skills a sent message names. Pure functions shared by every renderer (egui and WPF);
//! nothing here branches on the provider's kind — only on the `skills` / `inlineSkills`
//! capabilities and the provider's display name for the tooltip (docs/04-providers.md#skills).
//!
//! * `/` at the very start of the message opens the picker when the provider has `skills`.
//! * `$` at the start of a word opens it only under `inlineSkills`. `$5`, `$HOME` mid-sentence
//!   in a provider without it, and paths such as `/home/me/x` never open anything.
//! * A trigger the provider does not support keeps the text as typed and shows a tooltip.
//! * A pick inserts `$name ` under `inlineSkills`, `/name ` otherwise; under `inlineSkills` the
//!   sent message also carries a structured `skill` part for each `$name` that is a listed skill.

use yaac_protocol::{Capability, InputPart, InstanceId, SkillInfo, SkillPart, SkillScope, TargetId, has_capability};

use crate::key::ThreadKey;
use crate::state::AppState;

/// What the instance can do with skills.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkillCaps {
    /// `skills`: the list exists and `/name` runs one.
    pub skills: bool,
    /// `inlineSkills`: a skill can be named anywhere, as `$name`.
    pub inline: bool,
}

impl SkillCaps {
    /// From a capability list (a thread's effective ones, or an instance's).
    pub fn of(caps: &[Capability]) -> Self {
        SkillCaps { skills: has_capability(caps, &Capability::Skills), inline: has_capability(caps, &Capability::InlineSkills) }
    }
}

/// Which character opened the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// `/` at the start of the message.
    Slash,
    /// `$` at the start of a word.
    Dollar,
}

/// An open trigger in the text: the range it covers (bytes, `$`/`/` included) and what follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Active {
    /// What opened it.
    pub trigger: Trigger,
    /// Byte offset of the `/` or `$`.
    pub start: usize,
    /// Byte offset just after the name typed so far (and after the rest of that word).
    pub end: usize,
    /// The name typed so far, up to the cursor.
    pub query: String,
}

/// What the text at the cursor asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detection {
    /// Nothing: an ordinary message.
    None,
    /// Open the picker.
    Open(Active),
    /// A skill trigger this provider does not support: leave the text, show this tooltip.
    Unsupported(String),
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.')
}

/// The end of the word that starts at `from`, in bytes.
fn word_end(text: &str, from: usize) -> usize {
    from + text[from..].find(|c: char| !is_name_char(c)).unwrap_or(text.len() - from)
}

/// What the cursor (a byte offset into `text`) is in. `provider` is the provider's name as the
/// user knows it ("Claude Code"), for the tooltip.
pub fn detect(text: &str, cursor: usize, caps: SkillCaps, provider: &str) -> Detection {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return Detection::None;
    }
    let before = &text[..cursor];
    // The whitespace-delimited word the cursor is in or at the end of.
    let word_start = before.rfind(char::is_whitespace).map_or(0, |i| i + before[i..].chars().next().map_or(1, char::len_utf8));
    let word = &before[word_start..];
    // `/name` as the first word of the message. A second `/` makes it a path.
    if let Some(name) = word.strip_prefix('/') {
        if word_start != 0 || name.contains(['/', '$']) {
            return Detection::None;
        }
        if caps.skills {
            return Detection::Open(Active { trigger: Trigger::Slash, start: 0, end: word_end(text, cursor), query: name.to_owned() });
        }
        return match name.chars().next() {
            Some(c) if c.is_alphabetic() => Detection::Unsupported(format!("{provider} does not list skills, so there is no skill picker here.")),
            _ => Detection::None,
        };
    }
    // `$name` at the start of a word, after an opening bracket or quote at most.
    let lead = word.len() - word.trim_start_matches(['(', '[', '{', '"', '\'']).len();
    let start = word_start + lead;
    if let Some(name) = before[start..].strip_prefix('$') {
        // `$5`, `$1,000`, `$$`: money and shell syntax, not skills.
        let starts_ok = name.chars().next().is_none_or(|c| c.is_alphabetic() || c == '_');
        if !starts_ok || name.contains(['/', '$']) {
            return Detection::None;
        }
        if caps.inline {
            return Detection::Open(Active { trigger: Trigger::Dollar, start, end: word_end(text, cursor), query: name.to_owned() });
        }
        // Not supported: say so only where a user would try it (the first word of the message,
        // typed as a name) — a `$HOME` in the middle of a sentence is just text.
        if start == 0 && name.chars().next().is_some_and(char::is_alphabetic) {
            let how = if caps.skills { " Start the message with /name to use a skill." } else { "" };
            return Detection::Unsupported(format!("{provider} cannot name a skill inside a message.{how}"));
        }
    }
    Detection::None
}

/// What a picker row stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A skill of the provider.
    Skill {
        /// Where its `SKILL.md` is on the target.
        path: String,
        /// Where it comes from.
        scope: Option<SkillScope>,
    },
    /// A command of YAAC's own, offered by the caller (for example `/compact` under the
    /// `compact` capability). Choosing it inserts `/name ` and leaves the rest to the caller.
    Builtin,
}

/// One row of the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The name, without `/` or `$`.
    pub name: String,
    /// One line about it.
    pub description: String,
    /// "[file]", when the skill gives an argument hint.
    pub hint: Option<String>,
    /// What it is.
    pub kind: EntryKind,
}

impl Entry {
    /// A built-in slash command row.
    pub fn builtin(name: &str, description: &str) -> Self {
        Entry { name: name.to_owned(), description: description.to_owned(), hint: None, kind: EntryKind::Builtin }
    }

    /// The `/compact` row, to hand to [`entries`] when the thread has the `compact` capability.
    pub fn compact() -> Self {
        Entry::builtin("compact", "Summarise the conversation to free context")
    }

    /// Where it came from, for the row's small print ("project", "user").
    pub fn scope_word(&self) -> Option<&'static str> {
        match &self.kind {
            EntryKind::Builtin => Some("built in"),
            EntryKind::Skill { scope: Some(SkillScope::Project), .. } => Some("project"),
            EntryKind::Skill { scope: Some(SkillScope::User), .. } => Some("user"),
            EntryKind::Skill { scope: Some(SkillScope::System), .. } => Some("system"),
            _ => None,
        }
    }
}

/// The most rows a picker shows.
pub const PICKER_MAX: usize = 50;

/// The rows for `query`: built-ins first, then skills; names that start with the query before
/// names that contain it before descriptions that do. `builtins` is whatever the caller offers
/// (only `/` lists them; pass none for `$`).
pub fn entries(skills: &[SkillInfo], builtins: &[Entry], query: &str) -> Vec<Entry> {
    let q = query.to_lowercase();
    let all = builtins.iter().cloned().chain(skills.iter().map(|s| Entry {
        name: s.name.clone(),
        description: s.description.clone(),
        hint: s.argument_hint.clone(),
        kind: EntryKind::Skill { path: s.path.clone(), scope: s.scope.clone() },
    }));
    let mut ranked: Vec<(u8, Entry)> = all
        .filter_map(|e| {
            let name = e.name.to_lowercase();
            let rank = if q.is_empty() || name.starts_with(&q) {
                0
            } else if name.contains(&q) {
                1
            } else if e.description.to_lowercase().contains(&q) {
                2
            } else {
                return None;
            };
            Some((rank, e))
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank); // stable: built-ins, then skills by their listed order
    ranked.into_iter().take(PICKER_MAX).map(|(_, e)| e).collect()
}

/// The text after a pick, and where the cursor goes (a byte offset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// The new text.
    pub text: String,
    /// The cursor, just after the inserted name and its space.
    pub cursor: usize,
}

/// What replaces the trigger and the name typed so far (`active.start..active.end`) when `entry`
/// is picked: `$name ` when the provider names skills inline, `/name ` otherwise (and always for
/// a built-in). No space is added when one already follows.
pub fn insertion(text: &str, active: &Active, entry: &Entry, caps: SkillCaps) -> String {
    let sigil = if caps.inline && entry.kind != EntryKind::Builtin { '$' } else { '/' };
    let rest = &text[active.end.min(text.len())..];
    let space = if rest.starts_with(char::is_whitespace) { "" } else { " " };
    format!("{sigil}{}{space}", entry.name)
}

/// Replaces the trigger and the name typed so far with the chosen entry (see [`insertion`]).
pub fn apply(text: &str, active: &Active, entry: &Entry, caps: SkillCaps) -> Edit {
    let end = active.end.min(text.len());
    let head = format!("{}{}", &text[..active.start], insertion(text, active, entry, caps));
    Edit { cursor: head.len(), text: format!("{head}{}", &text[end..]) }
}

/// A UTF-16 offset (what WPF's text box counts in) as a byte offset into `text`; clamped.
pub fn byte_of_utf16(text: &str, units: usize) -> usize {
    let mut seen = 0;
    for (i, c) in text.char_indices() {
        if seen >= units {
            return i;
        }
        seen += c.len_utf16();
    }
    text.len()
}

/// A byte offset into `text` as a UTF-16 offset.
pub fn utf16_of_byte(text: &str, bytes: usize) -> usize {
    text[..bytes.min(text.len())].encode_utf16().count()
}

/// The skills a message names with `$name` (only meaningful under `inlineSkills`): one part
/// for each distinct listed skill, in order of first mention. A `$name` that is not a listed
/// skill is plain text.
pub fn references(text: &str, skills: &[SkillInfo]) -> Vec<SkillPart> {
    let mut out: Vec<SkillPart> = Vec::new();
    for (i, _) in text.match_indices('$') {
        let ok_before = i == 0 || text[..i].ends_with(|c: char| c.is_whitespace() || matches!(c, '(' | '[' | '{' | '"' | '\''));
        if !ok_before {
            continue;
        }
        let name = &text[i + 1..word_end(text, i + 1)];
        let name = name.trim_end_matches(['.', ':']);
        if name.is_empty() || out.iter().any(|p| p.name == name) {
            continue;
        }
        if let Some(s) = skills.iter().find(|s| s.name == name) {
            out.push(SkillPart { name: s.name.clone(), path: s.path.clone() });
        }
    }
    out
}

/// A message's parts with the skills its text names (`inlineSkills` only), after the text part.
pub fn with_references(mut parts: Vec<InputPart>, text: &str, skills: &[SkillInfo]) -> Vec<InputPart> {
    let refs = references(text, skills);
    let at = parts.iter().position(|p| !matches!(p, InputPart::Text(_))).unwrap_or(parts.len());
    for (n, r) in refs.into_iter().enumerate() {
        parts.insert(at + n, InputPart::Skill(r));
    }
    parts
}

/// Where a composer's skills come from: one instance, in one folder, on one target.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SkillScopeKey {
    /// The target.
    pub target: TargetId,
    /// The provider instance.
    pub instance: InstanceId,
    /// The working directory.
    pub cwd: String,
}

/// A composer's skills context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composer {
    /// Where the list comes from.
    pub scope: SkillScopeKey,
    /// What the provider can do.
    pub caps: SkillCaps,
    /// "Claude Code", for the tooltip.
    pub provider: String,
    /// Commands of YAAC's own the `/` picker lists beside the skills: `/compact` where the
    /// thread has the `compact` capability. Picking one only inserts `/name `; running it is
    /// the sender's business.
    pub builtins: Vec<Entry>,
}

/// The skills context of a started thread, from its effective capabilities (the instance's when
/// they are not loaded yet).
pub fn for_thread(state: &AppState, key: &ThreadKey) -> Option<Composer> {
    let summary = state.summary(key)?;
    let provider = state.provider_of(key)?;
    let all = match state.transcripts.get(key).and_then(|t| t.effective.as_ref()) {
        Some(e) => &e.capabilities,
        None => &provider.capabilities,
    };
    let caps = SkillCaps::of(all);
    let builtins = if has_capability(all, &Capability::Compact) { vec![Entry::compact()] } else { Vec::new() };
    Some(Composer {
        scope: SkillScopeKey { target: key.target.clone(), instance: summary.instance.clone(), cwd: summary.cwd.as_str().to_owned() },
        caps,
        provider: crate::header::short_provider(&provider.display_name, provider.name.as_deref()),
        builtins,
    })
}

/// The skills context of a new-thread draft: its chosen instance (or the one `thread/start` would
/// pick) in its folder.
pub fn for_new_thread(state: &AppState, draft: &crate::state::NewThreadDraft) -> Option<Composer> {
    let target = state.target(&draft.target)?;
    let provider = match &draft.instance {
        Some(i) => target.provider(i)?,
        None => target.providers.iter().find(|p| p.status == yaac_protocol::ProviderStatus::Ready)?,
    };
    let cwd = draft.cwd.as_ref()?;
    Some(Composer {
        scope: SkillScopeKey { target: draft.target.clone(), instance: provider.instance.clone(), cwd: cwd.as_str().to_owned() },
        caps: SkillCaps::of(&provider.capabilities),
        provider: crate::header::short_provider(&provider.display_name, provider.name.as_deref()),
        // Nothing to compact in a thread that has not started.
        builtins: Vec::new(),
    })
}

/// A `skills/list` in flight or answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillsState {
    /// Asked; no answer yet.
    Loading,
    /// The skills.
    Ready(Vec<SkillInfo>),
    /// The request failed; the message is shown as-is.
    Failed(String),
}

/// `input` with the skills `text` names, when the composer's provider has `inlineSkills` and its
/// skills are loaded. Otherwise `input` unchanged: the text alone is the message.
pub fn attach(state: &AppState, composer: Option<Composer>, input: Vec<InputPart>, text: &str) -> Vec<InputPart> {
    let Some(c) = composer.filter(|c| c.caps.inline) else { return input };
    match state.skills.get(&c.scope) {
        Some(SkillsState::Ready(list)) => with_references(input, text, list),
        _ => input,
    }
}

/// What the user did to the picker: kept by the window per composer, between frames.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickerState {
    /// The highlighted row.
    pub selected: usize,
    /// Esc closed the picker for the trigger that starts here; it stays closed until that
    /// trigger is gone.
    pub dismissed: Option<usize>,
}

impl PickerState {
    /// Down arrow, wrapping.
    pub fn next(&mut self, rows: usize) {
        if rows > 0 {
            self.selected = (self.selected + 1) % rows;
        }
    }

    /// Up arrow, wrapping.
    pub fn prev(&mut self, rows: usize) {
        if rows > 0 {
            self.selected = (self.selected + rows - 1) % rows;
        }
    }

    /// Esc.
    pub fn dismiss(&mut self, active: &Active) {
        self.dismissed = Some(active.start);
        self.selected = 0;
    }
}

/// The list's state under the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Rows to choose from.
    Rows,
    /// The list is loading.
    Loading,
    /// The list could not be read (the message); **Retry** asks again.
    Failed(String),
    /// Loaded, and nothing matches.
    Empty,
}

/// What a composer shows for its text and cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    /// Nothing.
    Closed,
    /// A tooltip for a trigger this provider does not support; the text is untouched.
    Tooltip(String),
    /// The picker.
    Open {
        /// The trigger it replaces on a pick.
        active: Active,
        /// The rows (empty unless `status` is `Rows`).
        rows: Vec<Entry>,
        /// The list's state.
        status: Status,
        /// The highlighted row, within `rows`.
        selected: usize,
        /// The list to ask for (`skills/list`), when it has not been asked yet.
        want: Option<SkillScopeKey>,
    },
}

/// The picker for `text` with the cursor at byte `cursor`. Pure: it asks for the list through
/// `want` (the window sends `skills/list`, then draws again).
pub fn view(state: &AppState, comp: &Composer, text: &str, cursor: usize, ps: &PickerState) -> View {
    match detect(text, cursor, comp.caps, &comp.provider) {
        Detection::None => View::Closed,
        Detection::Unsupported(tip) => View::Tooltip(tip),
        Detection::Open(active) => {
            if ps.dismissed == Some(active.start) {
                return View::Closed;
            }
            let (skills, status, want): (&[SkillInfo], Option<Status>, Option<SkillScopeKey>) = match state.skills.get(&comp.scope) {
                None => (&[], Some(Status::Loading), Some(comp.scope.clone())),
                Some(SkillsState::Loading) => (&[], Some(Status::Loading), None),
                Some(SkillsState::Failed(e)) => (&[], Some(Status::Failed(e.clone())), None),
                Some(SkillsState::Ready(list)) => (list, None, None),
            };
            // Only `/` lists YAAC's own commands; `$` names skills.
            let builtins: &[Entry] = if active.trigger == Trigger::Slash { &comp.builtins } else { &[] };
            let rows = if status.is_none() { entries(skills, builtins, &active.query) } else { Vec::new() };
            let status = status.unwrap_or(if rows.is_empty() { Status::Empty } else { Status::Rows });
            let selected = ps.selected.min(rows.len().saturating_sub(1));
            View::Open { active, rows, status, selected, want }
        }
    }
}
