//! Fast mode (D125): the lightning toggle under the composer, offered only by capability; the
//! choice kept per thread and per draft; a model without it turns it off; the spinner of a
//! running turn in fast mode.
mod common;

use common::{boot, settle};
use yaac_core::attention::{AttentionMap, Mark, display};
use yaac_core::controls::{ControlRowView, FastToggle, StartedInputs, draft_row, model_has_fast, started_row};
use yaac_core::demo::threads;
use yaac_core::{ThreadKey, client::Client};
use yaac_protocol::{InstanceId, ModelId};

fn row(c: &Client, key: &ThreadKey) -> yaac_core::controls::ControlRow {
    let s = c.state.summary(key).unwrap();
    let t = c.state.transcripts.get(key);
    let view = started_row(StartedInputs {
        providers: &c.state.target(&key.target).unwrap().providers,
        instance: &s.instance,
        target_name: key.target.as_str(),
        effective: t.and_then(|t| t.effective.as_ref()),
        summary_model: s.model.as_ref(),
        summary_model_off: s.model_off == Some(true),
        summary_fast: s.fast,
        summary_ultrafast: s.ultrafast == Some(true),
        next: c.state.next_settings.get(key),
        context: None,
        may_act: c.state.may_act(key),
        agents: None,
    });
    let ControlRowView::Row(r) = view else { panic!("hidden") };
    r
}

#[test]
fn the_toggle_is_shown_only_for_a_model_that_has_fast_mode() {
    let (mut c, hub) = boot();
    // A Claude thread on a model that reports fast mode: the toggle, off.
    let users = threads::users();
    c.open(&users);
    settle(&mut c, &hub);
    assert_eq!(row(&c, &users).fast, Some(FastToggle { on: false, offers_fast: true, offers_ultrafast: false, ultrafast_on: false }));
    // A Codex thread whose model does not report it: no toggle.
    let fix = threads::fix();
    c.open(&fix);
    settle(&mut c, &hub);
    assert_eq!(row(&c, &fix).fast, None);
    // Unknown is not yes.
    let p = c.state.provider_of(&fix).unwrap();
    assert!(p.models.iter().all(|m| !model_has_fast(m)));
}

#[test]
fn the_choice_is_kept_per_thread_and_goes_with_the_next_turn() {
    let (mut c, hub) = boot();
    let users = threads::users();
    c.open(&users);
    settle(&mut c, &hub);
    c.set_fast(&users, true);
    assert_eq!(row(&c, &users).fast, Some(FastToggle { on: true, offers_fast: true, offers_ultrafast: false, ultrafast_on: false }));
    assert!(!c.state.thread_fast(&threads::billing()), "another thread is not touched");
    c.state.drafts.entry(users.clone()).or_default().text = "Quick one.".into();
    assert!(c.send(&users));
    settle(&mut c, &hub);
    // The target took it: the summary reports the thread in fast mode, and the choice stays.
    assert!(c.state.summary(&users).unwrap().fast);
    assert_eq!(row(&c, &users).fast, Some(FastToggle { on: true, offers_fast: true, offers_ultrafast: false, ultrafast_on: false }));
    // Off again.
    c.set_fast(&users, false);
    assert_eq!(row(&c, &users).fast, Some(FastToggle { on: false, offers_fast: true, offers_ultrafast: false, ultrafast_on: false }));
}

#[test]
fn switching_to_a_model_without_fast_mode_turns_it_off_with_a_note() {
    let (mut c, hub) = boot();
    let users = threads::users();
    c.open(&users);
    settle(&mut c, &hub);
    c.set_fast(&users, true);
    c.set_next_model(&users, ModelId::new("haiku-5"));
    let r = row(&c, &users);
    assert_eq!(r.fast, None, "hidden: the model has no fast mode");
    assert!(!c.state.thread_fast(&users));
    assert_eq!(r.note.as_deref(), Some("Fast mode is off: haiku-5 has no fast mode."));
    // Back to a model with fast mode: offered again, but off until turned on.
    c.set_next_model(&users, ModelId::new("opus-5.5"));
    let r = row(&c, &users);
    assert_eq!(r.fast, Some(FastToggle { on: false, offers_fast: true, offers_ultrafast: false, ultrafast_on: false }));
    assert_eq!(r.note, None);
    // A model switch while off says nothing.
    c.set_next_model(&users, ModelId::new("haiku-5"));
    assert_eq!(row(&c, &users).note, None);
}

#[test]
fn a_draft_keeps_its_own_choice_and_loses_it_on_a_model_without_it() {
    let (mut c, _) = boot();
    let id = c.new_thread("local".into(), Some("~/code/api".into()));
    let draft = |c: &Client| c.state.new_threads.iter().find(|d| d.id == id).unwrap().clone();
    let providers = c.state.target(&"local".into()).unwrap().providers.clone();
    // The default (Codex) model has no fast mode: no toggle.
    assert_eq!(draft_row(&providers, "local", &draft(&c)).fast, None);
    c.set_draft_model(id, InstanceId::new("claude"), ModelId::new("opus-5.5"));
    assert_eq!(draft_row(&providers, "local", &draft(&c)).fast, Some(FastToggle { on: false, offers_fast: true, offers_ultrafast: false, ultrafast_on: false }));
    c.set_draft_fast(id, true);
    assert_eq!(draft_row(&providers, "local", &draft(&c)).fast, Some(FastToggle { on: true, offers_fast: true, offers_ultrafast: false, ultrafast_on: false }));
    c.set_draft_model(id, InstanceId::new("claude"), ModelId::new("haiku-5"));
    let r = draft_row(&providers, "local", &draft(&c));
    assert_eq!(r.fast, None);
    assert!(!draft(&c).fast);
    assert_eq!(r.note.as_deref(), Some("Fast mode is off: haiku-5 has no fast mode."));
}

#[test]
fn a_running_turn_in_fast_mode_has_the_fast_spinner() {
    let (c, _) = boot();
    let map = AttentionMap::compute(&c.state);
    // The demo's search worker runs in fast mode, as its summary reports.
    let search = display(&c.state, &map, &threads::search());
    assert_eq!(search.mark, Mark::Running);
    assert!(search.fast);
    // Another running thread does not.
    let orch = display(&c.state, &map, &threads::orchestrator());
    assert_eq!(orch.mark, Mark::Running);
    assert!(!orch.fast);
    // Not running: never fast, whatever the setting.
    let users = display(&c.state, &map, &threads::users());
    assert!(!users.fast);
}

#[test]
fn a_started_draft_in_fast_mode_runs_in_it() {
    let (mut c, hub) = boot();
    let id = c.new_thread("local".into(), Some("/home/dev/api".into()));
    c.set_draft_model(id, InstanceId::new("claude"), ModelId::new("opus-5.5"));
    c.set_draft_fast(id, true);
    if let Some(d) = c.state.new_threads.iter_mut().find(|d| d.id == id) {
        d.draft.text = "Go fast.".into();
    }
    assert!(c.start_thread(id));
    settle(&mut c, &hub);
    let started = c.state.threads.values().find(|r| r.summary.title == "Go fast.").expect("the new thread");
    assert!(started.summary.fast);
}
