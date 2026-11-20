//! The control row under the composer, derived only from capabilities: the thread's effective
//! capabilities for a started thread, the target's descriptors for a draft. Nothing here looks
//! at provider kind.

use yaac_protocol::{
    Capability, ContextUsage, EffortId, EffortInfo, InstanceId, ModeId, ModeInfo, ModelId, ModelInfo,
    ProviderDescriptor, ProviderStatus, Support, ThreadEffective, has_capability,
};

use crate::images::model_image_support;
use crate::state::{AgentsPolicy, NewThreadDraft, NextSettings};

/// One model in the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerModel {
    /// Its id.
    pub id: ModelId,
    /// Its label.
    pub name: String,
    /// The instance's default model.
    pub is_default: bool,
    /// The current selection (ticked).
    pub selected: bool,
    /// Whether it can see images.
    pub image: Support,
    /// "effort: low – high", when it reports effort levels.
    pub effort_range: Option<String>,
    /// Selectable.
    pub enabled: bool,
}

/// One provider instance in the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerGroup {
    /// The instance.
    pub instance: InstanceId,
    /// "Codex", "Codex · work".
    pub name: String,
    /// The group is selectable.
    pub enabled: bool,
    /// Why not ("not signed in on local", "fork to switch provider").
    pub reason: Option<String>,
    /// Its models (empty when the whole group is disabled).
    pub models: Vec<PickerModel>,
}

/// The model control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelControl {
    /// The instance label on the trigger.
    pub instance_label: String,
    /// The model label on the trigger.
    pub model_label: String,
    /// Whether the picker opens (otherwise the trigger is a plain label).
    pub changeable: bool,
    /// Picker contents, grouped by instance.
    pub groups: Vec<PickerGroup>,
}

/// A selector with a current value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice<T> {
    /// The current value.
    pub current: Option<T>,
    /// Its label.
    pub label: String,
    /// Options: id, label, is the default.
    pub options: Vec<(T, String, bool)>,
}

/// The row.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ControlRow {
    /// Model picker.
    pub model: Option<ModelControl>,
    /// Effort selector.
    pub effort: Option<Choice<EffortId>>,
    /// Mode selector.
    pub mode: Option<Choice<ModeId>>,
    /// "agents: ask / allow" — only on a thread a person started.
    pub agents: Option<Choice<AgentsPolicy>>,
    /// Context meter, percent full.
    pub context_percent: Option<u8>,
    /// Fork button.
    pub fork: bool,
    /// Target and working directory pickers (drafts).
    pub target_and_folder: bool,
}

/// The row, or nothing at all (a read-only thread shows no selectors).
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant, reason = "built once per frame, never stored in bulk")]
pub enum ControlRowView {
    /// No selectors.
    Hidden,
    /// The row.
    Row(ControlRow),
}

/// A short reason an instance cannot be used, in plain words.
pub fn unavailable_reason(p: &ProviderDescriptor, target_name: &str) -> Option<String> {
    let word = match &p.status {
        ProviderStatus::Ready => return None,
        ProviderStatus::NeedsAuth => format!("not signed in on {target_name}"),
        ProviderStatus::NotInstalled => format!("not installed on {target_name}"),
        ProviderStatus::TooOld => format!("too old on {target_name}"),
        ProviderStatus::Unreachable => format!("unreachable on {target_name}"),
        ProviderStatus::StartFailed => format!("could not start on {target_name}"),
        ProviderStatus::AccessDenied => format!("access denied on {target_name}"),
        ProviderStatus::Disabled => "disabled".to_owned(),
        ProviderStatus::Probing => "checking…".to_owned(),
        ProviderStatus::Unknown(s) => s.clone(),
    };
    Some(word)
}

fn effort_range(efforts: &[EffortInfo]) -> Option<String> {
    let label = |e: &EffortInfo| e.display_name.clone().unwrap_or_else(|| e.id.to_string());
    match efforts {
        [] => None,
        [one] => Some(format!("effort: {}", label(one))),
        [first, .., last] => Some(format!("effort: {} – {}", label(first), label(last))),
    }
}

fn picker_model(m: &ModelInfo, selected: Option<&ModelId>, enabled: bool) -> PickerModel {
    PickerModel {
        id: m.id.clone(),
        name: m.display_name.clone(),
        is_default: m.is_default,
        selected: selected == Some(&m.id),
        image: model_image_support(m),
        effort_range: effort_range(&m.efforts),
        enabled,
    }
}

fn model_label(p: Option<&ProviderDescriptor>, id: Option<&ModelId>) -> String {
    match (p, id) {
        (Some(p), Some(id)) => {
            p.models.iter().find(|m| &m.id == id).map_or_else(|| id.to_string(), |m| m.display_name.clone())
        }
        (None, Some(id)) => id.to_string(),
        (Some(p), None) => p.models.iter().find(|m| m.is_default).map_or_else(|| "default".to_owned(), |m| m.display_name.clone()),
        (None, None) => "default".to_owned(),
    }
}

fn effort_choice(efforts: &[EffortInfo], current: Option<&EffortId>, default: Option<&EffortId>) -> Choice<EffortId> {
    let cur = current.or(default).cloned();
    let label_of = |id: &EffortId| {
        efforts.iter().find(|e| &e.id == id).and_then(|e| e.display_name.clone()).unwrap_or_else(|| id.to_string())
    };
    Choice {
        label: format!("effort: {}", cur.as_ref().map_or_else(|| "default".to_owned(), label_of)),
        options: efforts
            .iter()
            .map(|e| (e.id.clone(), e.display_name.clone().unwrap_or_else(|| e.id.to_string()), Some(&e.id) == default))
            .collect(),
        current: cur,
    }
}

fn mode_choice(modes: &[ModeInfo], current: Option<&ModeId>) -> Choice<ModeId> {
    let cur = current.cloned().or_else(|| modes.iter().find(|m| m.is_default).map(|m| m.id.clone()));
    let label = cur
        .as_ref()
        .map(|id| modes.iter().find(|m| &m.id == id).map_or_else(|| id.to_string(), |m| m.display_name.clone()))
        .unwrap_or_else(|| "default".to_owned());
    Choice {
        label: format!("mode: {label}"),
        options: modes.iter().map(|m| (m.id.clone(), m.display_name.clone(), m.is_default)).collect(),
        current: cur,
    }
}

/// The control row of a draft: every instance on the target is selectable (unavailable ones
/// are listed with their reason); no Fork, no context meter.
pub fn draft_row(providers: &[ProviderDescriptor], target_name: &str, draft: &NewThreadDraft) -> ControlRow {
    let instance = draft
        .instance
        .clone()
        .or_else(|| providers.iter().find(|p| p.status == ProviderStatus::Ready).map(|p| p.instance.clone()));
    let selected_p = instance.as_ref().and_then(|i| providers.iter().find(|p| &p.instance == i));
    let selected_model = draft
        .model
        .clone()
        .or_else(|| selected_p.and_then(|p| p.models.iter().find(|m| m.is_default)).map(|m| m.id.clone()));
    let groups = providers
        .iter()
        .map(|p| {
            let reason = unavailable_reason(p, target_name);
            let enabled = reason.is_none();
            let sel = if Some(&p.instance) == instance.as_ref() { selected_model.as_ref() } else { None };
            PickerGroup {
                instance: p.instance.clone(),
                name: p.display_name.clone(),
                enabled,
                reason,
                models: if enabled {
                    p.models.iter().filter(|m| !m.hidden).map(|m| picker_model(m, sel, true)).collect()
                } else {
                    Vec::new()
                },
            }
        })
        .collect();
    let model_info = selected_p.and_then(|p| p.models.iter().find(|m| Some(&m.id) == selected_model.as_ref()));
    let effort = model_info
        .filter(|m| !m.efforts.is_empty())
        .map(|m| effort_choice(&m.efforts, draft.effort.as_ref(), m.default_effort.as_ref()));
    let mode = selected_p
        .filter(|p| has_capability(&p.capabilities, &Capability::Modes) && !p.modes.is_empty())
        .map(|p| mode_choice(&p.modes, draft.mode.as_ref()));
    ControlRow {
        model: Some(ModelControl {
            instance_label: selected_p.map_or_else(|| "No provider".to_owned(), |p| p.display_name.clone()),
            model_label: model_label(selected_p, selected_model.as_ref()),
            changeable: true,
            groups,
        }),
        effort,
        mode,
        agents: None,
        context_percent: None,
        fork: false,
        target_and_folder: true,
    }
}

/// Inputs for a started thread's row.
#[derive(Clone, Copy, Debug)]
pub struct StartedInputs<'a> {
    /// Every instance on the thread's target.
    pub providers: &'a [ProviderDescriptor],
    /// The thread's own instance (never changes).
    pub instance: &'a InstanceId,
    /// The target's name.
    pub target_name: &'a str,
    /// Effective capabilities, when the target has sent them.
    pub effective: Option<&'a ThreadEffective>,
    /// The model the summary last reported, used until `applied` is known.
    pub summary_model: Option<&'a ModelId>,
    /// Selections for the next message.
    pub next: Option<&'a NextSettings>,
    /// Context usage.
    pub context: Option<&'a ContextUsage>,
    /// Whether this window may act on the thread.
    pub may_act: bool,
    /// The chat's agents policy when a person started it; `None` for a worker, which acts
    /// under its root's answer.
    pub agents: Option<AgentsPolicy>,
}

/// The control row of a started thread. Read-only: hidden entirely. The model picker lists
/// only the thread's own instance as selectable (under `modelSwitch`); other instances are
/// listed disabled with "fork to switch provider". Effort appears only when it can actually be
/// changed (efforts reported *and* `effortSwitch`); mode under `modes`; the context meter
/// under `contextUsage`.
pub fn started_row(input: StartedInputs<'_>) -> ControlRowView {
    if !input.may_act {
        return ControlRowView::Hidden;
    }
    let caps: &[Capability] = input.effective.map_or(&[], |e| e.capabilities.as_slice());
    let own = input.providers.iter().find(|p| &p.instance == input.instance);
    let applied_model = input
        .next
        .and_then(|n| n.model.as_ref())
        .or_else(|| input.effective.and_then(|e| e.applied.model.as_ref()))
        .or(input.summary_model);
    let can_switch = has_capability(caps, &Capability::ModelSwitch);
    let groups = input
        .providers
        .iter()
        .map(|p| {
            if &p.instance == input.instance {
                let reason = unavailable_reason(p, input.target_name);
                let enabled = reason.is_none() && can_switch;
                PickerGroup {
                    instance: p.instance.clone(),
                    name: p.display_name.clone(),
                    enabled,
                    reason,
                    models: p.models.iter().filter(|m| !m.hidden).map(|m| picker_model(m, applied_model, enabled)).collect(),
                }
            } else {
                PickerGroup {
                    instance: p.instance.clone(),
                    name: p.display_name.clone(),
                    enabled: false,
                    reason: Some("fork to switch provider".to_owned()),
                    models: Vec::new(),
                }
            }
        })
        .collect();
    let effort = input.effective.and_then(|e| {
        (!e.efforts.is_empty() && has_capability(caps, &Capability::EffortSwitch)).then(|| {
            effort_choice(
                &e.efforts,
                input.next.and_then(|n| n.effort.as_ref()).or(e.applied.effort.as_ref()),
                None,
            )
        })
    });
    let mode = input.effective.and_then(|e| {
        (has_capability(caps, &Capability::Modes) && !e.modes.is_empty())
            .then(|| mode_choice(&e.modes, input.next.and_then(|n| n.mode.as_ref()).or(e.applied.mode.as_ref())))
    });
    let context_percent = if has_capability(caps, &Capability::ContextUsage) {
        input.context.and_then(|c| match (c.used, c.window) {
            (Some(u), Some(w)) if w > 0 => Some(u8::try_from((u.saturating_mul(100) / w).min(100)).unwrap_or(100)),
            _ => None,
        })
    } else {
        None
    };
    ControlRowView::Row(ControlRow {
        model: Some(ModelControl {
            instance_label: own.map_or_else(|| input.instance.to_string(), |p| p.display_name.clone()),
            model_label: model_label(own, applied_model),
            changeable: can_switch,
            groups,
        }),
        effort,
        mode,
        agents: input
            .agents
            .filter(|_| has_capability(caps, &Capability::MeshTools))
            .map(agents_choice),
        context_percent,
        fork: true,
        target_and_folder: false,
    })
}

/// The "agents: ask ▾" selector.
pub fn agents_choice(current: AgentsPolicy) -> Choice<AgentsPolicy> {
    Choice {
        label: match current {
            AgentsPolicy::Ask => "agents: ask".to_owned(),
            AgentsPolicy::Allow => "agents: allow".to_owned(),
        },
        options: vec![
            (AgentsPolicy::Ask, "Ask for permission".to_owned(), true),
            (AgentsPolicy::Allow, "Allow without asking".to_owned(), false),
        ],
        current: Some(current),
    }
}

/// The effort to keep after the model changes: the current one if the new model offers it,
/// else the new model's default.
pub fn reconcile_effort(model: &ModelInfo, current: Option<&EffortId>) -> Option<EffortId> {
    match current {
        Some(c) if model.efforts.iter().any(|e| &e.id == c) => Some(c.clone()),
        _ => model.default_effort.clone(),
    }
}
