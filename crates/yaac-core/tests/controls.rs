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
