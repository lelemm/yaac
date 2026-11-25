//! The control row, derived only from effective capabilities.
mod common;

use common::{boot, boot_with};
use yaac_core::controls::{ControlRowView, StartedInputs, draft_row, reconcile_effort, started_row};
use yaac_core::demo::{Scenario, threads};
use yaac_protocol::{Capability, EffortId, InstanceId};

fn row(c: &yaac_core::client::Client, key: &yaac_core::ThreadKey, edit: impl FnOnce(&mut yaac_protocol::ThreadEffective)) -> ControlRowView {
    let t = c.state.transcripts.get(key).unwrap();
    let mut eff = t.effective.clone().unwrap();
    edit(&mut eff);
    let s = c.state.summary(key).unwrap();
    started_row(StartedInputs {
        providers: &c.state.target(&key.target).unwrap().providers,
        instance: &s.instance,
        target_name: key.target.as_str(),
        effective: Some(&eff),
        summary_model: s.model.as_ref(),
        summary_model_off: s.model_off == Some(true),
        summary_fast: s.fast,
        summary_ultrafast: s.ultrafast == Some(true),
        next: None,
        context: t.context.as_ref(),
        may_act: c.state.may_act(key),
        agents: s.spawned_by.is_none().then(|| c.state.agents_policy(key)),
    })
}

#[test]
fn started_thread_shows_what_can_be_changed() {
    let (c, _) = boot();
    let ControlRowView::Row(r) = row(&c, &threads::fix(), |_| {}) else { panic!("hidden") };
    let model = r.model.unwrap();
    assert_eq!((model.instance_label.as_str(), model.changeable), ("Codex · work", true));
    // Only its own instance is selectable; the others say why not.
    for g in &model.groups {
        if g.instance == InstanceId::new("codex-work") {
            assert!(g.enabled && g.models.iter().any(|m| m.selected));
        } else {
            assert!(!g.enabled);
            assert_eq!(g.reason.as_deref(), Some("fork to switch provider"));
            assert!(g.models.is_empty());
        }
    }
    assert_eq!(r.effort.unwrap().label, "effort: high");
    assert_eq!(r.mode.unwrap().label, "mode: ask");
    assert_eq!(r.context_percent, Some(38));
    assert_eq!(r.agents.unwrap().label, "agents: ask", "a chat a person started; ask by default");
    assert!(r.fork);
    assert!(!r.target_and_folder);
}

#[test]
fn a_missing_capability_removes_its_control() {
    let (c, _) = boot();
    let fix = threads::fix();
    let without = |cap: Capability| {
        let ControlRowView::Row(r) = row(&c, &fix, |e| e.capabilities.retain(|x| x != &cap)) else { panic!() };
        r
    };
    let r = without(Capability::ModelSwitch);
    assert!(!r.model.as_ref().unwrap().changeable);
    assert!(r.model.unwrap().groups.iter().all(|g| !g.enabled));
    // Effort only when it can actually be changed: efforts reported *and* effortSwitch.
    assert!(without(Capability::EffortSwitch).effort.is_none());
    let ControlRowView::Row(r) = row(&c, &fix, |e| e.efforts.clear()) else { panic!() };
    assert!(r.effort.is_none());
    assert!(without(Capability::Modes).mode.is_none());
    assert!(without(Capability::ContextUsage).context_percent.is_none());
}

#[test]
fn a_read_only_thread_shows_no_selectors_at_all() {
    let (c, _) = boot();
    assert_eq!(row(&c, &threads::handshake(), |_| {}), ControlRowView::Hidden);
}

#[test]
fn a_draft_offers_every_instance_and_no_fork_or_meter() {
    let (mut c, _) = boot_with(Scenario::Fresh);
    let id = c.new_thread("local".into(), Some("~/code/api".into()));
    let draft = c.state.new_threads.iter().find(|d| d.id == id).unwrap().clone();
    let providers = &c.state.target(&"local".into()).unwrap().providers;
    let r = draft_row(providers, "local", &draft);
    let model = r.model.unwrap();
    assert_eq!((model.instance_label.as_str(), model.model_label.as_str()), ("Codex", "gpt-6.2-sol"));
    let claude = model.groups.iter().find(|g| g.name == "Claude").unwrap();
    assert!(!claude.enabled);
    assert_eq!(claude.reason.as_deref(), Some("not signed in on local"));
    let codex = model.groups.iter().find(|g| g.name == "Codex").unwrap();
    assert!(codex.models.iter().find(|m| m.is_default).unwrap().selected);
    assert_eq!(codex.models[0].effort_range.as_deref(), Some("effort: low · high"));
    assert_eq!(r.effort.unwrap().label, "effort: high");
    assert_eq!(r.mode.unwrap().label, "mode: ask", "the harness's own default");
    assert!(!r.fork);
    assert_eq!(r.context_percent, None);
    assert!(r.target_and_folder);
}

#[test]
fn off_models_are_hidden_from_new_choices_and_only_the_current_one_is_marked_in_the_composer() {
    let (mut c, _) = boot();
    let key = threads::fix();
    let summary = c.state.summary(&key).unwrap();
    let current = summary.model.clone().unwrap();
    let instance = summary.instance.clone();
    let mut provider = c.state.provider_of(&key).unwrap().clone();
    let current_index = provider.models.iter().position(|m| m.id == current).unwrap();
    let off = provider.models.remove(current_index);
    let another_off = provider.models.pop().unwrap();
    let name = off.display_name.clone();
    let fallback = provider.models.first_mut().unwrap();
    fallback.is_default = true;
    let enabled = fallback.id.clone();
    provider.disabled_models = vec![off, another_off];
    c.state.apply_providers(&key.target, vec![provider], false);

    let ControlRowView::Row(started) = row(&c, &key, |_| {}) else { panic!("hidden") };
    let picker = started.model.unwrap();
    assert_eq!(picker.model_label, format!("{name} · off in Settings"));
    let own = picker.groups.iter().find(|g| g.instance == instance).unwrap();
    assert_eq!(own.models.len(), 2);
    let marked = &own.models[0];
    assert_eq!(marked.id, current);
    assert_eq!(marked.name, picker.model_label);
    assert!(marked.selected && marked.off && !marked.enabled);
    assert_eq!(own.models[1].id, enabled);

    let draft_id = c.new_thread(key.target.clone(), Some("/work".into()));
    let mut draft = c.state.new_threads.iter().find(|d| d.id == draft_id).unwrap().clone();
    draft.instance = Some(instance.clone());
    draft.model = Some(current);
    let providers = &c.state.target(&key.target).unwrap().providers;
    let picker = draft_row(providers, key.target.as_str(), &draft).model.unwrap();
    let own = picker.groups.iter().find(|g| g.instance == instance).unwrap();
    assert_eq!(own.models.len(), 1);
    assert_eq!(own.models[0].id, enabled);
    assert!(own.models[0].selected);

    let choice = yaac_core::fork::default_choice(&c.state, &key).unwrap();
    assert_eq!(choice.model, None);
    let fork = yaac_core::fork::view(&c.state, &key, &choice).unwrap();
    let dest = fork.destinations.iter().find(|d| d.instance == instance).unwrap();
    assert_eq!(dest.models.len(), 1);
    assert_eq!(dest.models[0].0, enabled);
}

#[test]
fn effort_resets_to_the_new_models_default_when_missing() {
    let (c, _) = boot();
    let p = &c.state.target(&"local".into()).unwrap().providers[0];
    let m = &p.models[0];
    assert_eq!(reconcile_effort(m, Some(&EffortId::new("low"))), Some(EffortId::new("low")));
    assert_eq!(reconcile_effort(m, Some(&EffortId::new("extreme"))), Some(EffortId::new("high")));
}

#[test]
fn a_worker_has_no_agents_selector() {
    let (c, _) = boot();
    let ControlRowView::Row(r) = row(&c, &threads::billing(), |_| {}) else { panic!() };
    assert!(r.agents.is_none());
}
