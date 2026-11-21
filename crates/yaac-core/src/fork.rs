//! The fork dialog (screen 11; docs/05-threads.md#fork): the destination, the result line that
//! follows from it (native or handoff, with the reason), what is and is not carried over, where
//! the conversation goes, and the transcript preview from `thread/forkPreview`.
//!
//! Native or handoff is decided from capabilities and stores, never from provider kind: a
//! native fork needs the source instance to declare `threadFork` and the destination to share
//! its conversation store. The target's preview answer, once it arrives, wins.

use yaac_protocol::{Capability, ForkKind, InputPart, InstanceId, ItemBody, ModelId, ProviderDescriptor, has_capability};

use crate::key::ThreadKey;
use crate::state::{AppState, ForkPreviewState};

/// The destination the user picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkChoice {
    /// The instance on the same target.
    pub instance: InstanceId,
    /// The model (the instance's default when absent).
    pub model: Option<ModelId>,
}

/// One destination in the "Continue with" picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Destination {
    /// The instance.
    pub instance: InstanceId,
    /// "Claude", "Codex · work".
    pub name: String,
    /// Its models: id, label, is the default.
    pub models: Vec<(ModelId, String, bool)>,
    /// Why it cannot be chosen, when it cannot.
    pub reason: Option<String>,
    /// Native from this source.
    pub native: bool,
}

/// The dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkView {
    /// "Fix frame decoder · Codex · work · <model>".
    pub from: String,
    /// "at the last completed turn (turn 12)".
    pub at: String,
    /// Every instance on the target.
    pub destinations: Vec<Destination>,
    /// The trigger of the selector: instance and model labels.
    pub chosen: Option<(String, String)>,
    /// "Instance: Claude on devbox · account: work@example.com".
    pub instance_line: Option<String>,
    /// "Handoff" or "Native fork".
    pub result_word: String,
    /// "— the destination is a different provider, so the conversation is passed as a
    /// transcript file."
    pub result_text: String,
    /// The reason, in secondary text ("Native fork is only available within the same provider
    /// and store.").
    pub reason: Option<String>,
    /// "12 turns · 9,400 words · 3 files still on the target".
    pub carried: String,
    /// "hidden reasoning · 2 images no longer on the target"; `None` when nothing is left out.
    pub not_carried: Option<String>,
    /// "This sends the conversation to Claude (work@example.com)." — for a handoff only.
    pub notice: Option<String>,
    /// The first lines of the transcript, once the preview arrived.
    pub preview: Option<String>,
    /// Still asking the target.
    pub preview_loading: bool,
    /// The fork is not possible, and why (the Fork button is then unavailable).
    pub unavailable: Option<String>,
    /// A handoff: the conversation crosses as a transcript. The dialog then asks for the new
    /// thread's first message (the preamble goes in front of it) and an explicit confirmation.
    pub handoff: bool,
    /// "Transcript: 9.4 KB" — what a handoff sends, once the preview arrived.
    pub size: Option<String>,
    /// The confirm button: "Fork", or "Send and fork" for a handoff.
    pub confirm: String,
    /// The confirm button waits for the preview: a handoff is confirmed only after it is shown.
    pub confirm_waits: bool,
}

/// "812 bytes", "9.4 KB", "1.2 MB".
fn size_text(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    if bytes < 1024 {
        plural(usize::try_from(bytes).unwrap_or(usize::MAX), "byte", "bytes")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    }
}

/// Native from `source` into `dest`: the source can fork natively and both share one store.
pub fn is_native(source: &ProviderDescriptor, dest: &ProviderDescriptor) -> bool {
    has_capability(&source.capabilities, &Capability::ThreadFork) && source.store.same_store(&dest.store)
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

/// The default destination: the source's own instance and model (a native fork when possible).
pub fn default_choice(state: &AppState, key: &ThreadKey) -> Option<ForkChoice> {
    let s = state.summary(key)?;
    Some(ForkChoice { instance: s.instance.clone(), model: s.model.clone() })
}

/// The dialog for forking `key` into `choice`.
pub fn view(state: &AppState, key: &ThreadKey, choice: &ForkChoice) -> Option<ForkView> {
    let summary = state.summary(key)?;
    let target = state.target(&key.target)?;
    let target_name = target.entry.name.to_string();
    let source = target.provider(&summary.instance);
    let dest = target.provider(&choice.instance);
    let model_name = |p: Option<&ProviderDescriptor>, id: Option<&ModelId>| -> String {
        match (p, id) {
            (Some(p), Some(id)) => p.models.iter().find(|m| &m.id == id).map_or_else(|| id.to_string(), |m| m.display_name.clone()),
            (Some(p), None) => p.models.iter().find(|m| m.is_default).map_or_else(String::new, |m| m.display_name.clone()),
            (None, Some(id)) => id.to_string(),
            (None, None) => String::new(),
        }
    };
    let src_name = source.map_or_else(|| summary.instance.to_string(), |p| p.display_name.clone());
    let src_model = model_name(source, summary.model.as_ref());
    let from = [summary.title.clone(), src_name, src_model].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");

    // What the transcript held in memory says about the history.
    let transcript = state.transcripts.get(key);
    let items = transcript.map(|t| t.items().to_vec()).unwrap_or_default();
    let mut turns: Vec<&str> = Vec::new();
    let (mut words, mut files, mut reasoning) = (0usize, 0usize, false);
    for it in &items {
        if it.turn_id.as_str() != "" && !turns.contains(&it.turn_id.as_str()) {
            turns.push(it.turn_id.as_str());
        }
        match &it.body {
            ItemBody::UserMessage(u) => {
                for part in &u.parts {
                    match part {
                        InputPart::Text(t) => words += t.text.split_whitespace().count(),
                        InputPart::Image(i) if !transcript.is_some_and(|t| t.missing_uploads.contains(&i.upload_id)) => files += 1,
                        InputPart::File(f) if !transcript.is_some_and(|t| t.missing_uploads.contains(&f.upload_id)) => files += 1,
                        _ => {}
                    }
                }
            }
            ItemBody::AgentMessage(a) => words += a.text.split_whitespace().count(),
            ItemBody::Reasoning(_) => reasoning = true,
            _ => {}
        }
    }

    let preview_state = state.fork_previews.get(&(key.clone(), choice.instance.clone()));
    let preview = match preview_state {
        Some(ForkPreviewState::Ready(p)) => Some(p),
        _ => None,
    };
    let native = match preview {
        Some(p) => p.fork_kind == ForkKind::Native,
        None => matches!((source, dest), (Some(s), Some(d)) if is_native(s, d)),
    };
    let cutoff = preview.and_then(|p| p.cutoff_turn.as_ref()).map(ToString::to_string);
    // Turn ids are opaque: the dialog names the turn by its position in the thread.
    let at = match cutoff.as_ref().and_then(|t| turns.iter().position(|x| *x == t.as_str())) {
        Some(i) if turns.len() > i + 1 => format!("at the last completed turn (turn {} of {}; the last one is still running)", i + 1, turns.len()),
        Some(i) => format!("at the last completed turn (turn {})", i + 1),
        None => "at the last completed turn".to_owned(),
    };
    let account = preview.and_then(|p| p.destination_account.clone()).or_else(|| dest.and_then(|d| d.store.account.clone()));
    let dest_name = dest.map_or_else(|| choice.instance.to_string(), |d| d.display_name.clone());
    let instance_line = Some(match &account {
        Some(a) => format!("Instance: {dest_name} on {target_name} · account: {a}"),
        None => format!("Instance: {dest_name} on {target_name}"),
    });

    let (result_word, result_text, reason) = if native {
        ("Native fork".to_owned(), "— the harness copies the conversation with its full context.".to_owned(), None)
    } else {
        let why = if source.is_some_and(|s| !has_capability(&s.capabilities, &Capability::ThreadFork)) {
            "This provider cannot fork natively."
        } else {
            "Native fork is only available within the same provider and store."
        };
        let text = if source.zip(dest).is_some_and(|(s, d)| s.display_name == d.display_name) {
            "— the destination keeps its conversations elsewhere, so the conversation is passed as a transcript file."
        } else {
            "— the destination is a different provider, so the conversation is passed as a transcript file."
        };
        ("Handoff".to_owned(), text.to_owned(), Some(why.to_owned()))
    };

    let mut carried = vec![plural(turns.len(), "turn", "turns"), plural(words, "word", "words")];
    if files > 0 {
        carried.push(format!("{} still on the target", plural(files, "file", "files")));
    }
    let omitted = preview.map_or(0, |p| p.omitted_attachments.len());
    let mut not = Vec::new();
    if !native {
        if reasoning {
            not.push("hidden reasoning".to_owned());
        }
        if omitted > 0 {
            not.push(format!("{} no longer on the target", plural(omitted, "attachment", "attachments")));
        }
    }
    let notice = (!native).then(|| match &account {
        Some(a) => format!("This sends the conversation to {dest_name} ({a})."),
        None => format!("This sends the conversation to {dest_name}."),
    });

    let destinations = target
        .providers
        .iter()
        .map(|p| Destination {
            instance: p.instance.clone(),
            name: p.display_name.clone(),
            models: p.models.iter().filter(|m| !m.hidden).map(|m| (m.id.clone(), m.display_name.clone(), m.is_default)).collect(),
            reason: crate::controls::unavailable_reason(p, &target_name),
            native: source.is_some_and(|s| is_native(s, p)),
        })
        .collect();
    let unavailable = if source.is_some_and(|s| !has_capability(&s.capabilities, &Capability::ThreadRead)) {
        Some("This provider cannot read its history, so YAAC will not fork it.".to_owned())
    } else {
        match preview_state {
            Some(ForkPreviewState::Ready(p)) => p.unavailable_reason.clone(),
            Some(ForkPreviewState::Failed(e)) => Some(e.clone()),
            _ => None,
        }
        .or_else(|| dest.and_then(|d| crate::controls::unavailable_reason(d, &target_name)))
    };
    Some(ForkView {
        from,
        at,
        destinations,
        chosen: dest.map(|d| (d.display_name.clone(), model_name(Some(d), choice.model.as_ref()))),
        instance_line,
        result_word,
        result_text,
        reason,
        carried: carried.join(" · "),
        not_carried: (!not.is_empty()).then(|| not.join(" · ")),
        notice,
        preview: preview.and_then(|p| p.transcript_preview.clone()),
        preview_loading: matches!(preview_state, None | Some(ForkPreviewState::Loading)),
        unavailable,
        handoff: !native,
        size: preview.and_then(|p| p.transcript_bytes).filter(|_| !native).map(|b| format!("Transcript: {}", size_text(b))),
        confirm: if native { "Fork" } else { "Send and fork" }.to_owned(),
        confirm_waits: !native && preview.is_none(),
    })
}
