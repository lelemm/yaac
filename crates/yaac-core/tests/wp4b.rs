//! Settings, SSH setup, fork, first run, mode confirmation, hello outcomes, and persisted
//! choices — tested without a window.

mod common;

use common::{NOW, boot, boot_with, settle};
use serde_json::{Value, json};
use yaac_core::client::Client;
use yaac_core::demo::ssh::{FINGERPRINT, ScriptedSsh, SshScript};
use yaac_core::demo::{DemoHub, Scenario, Scene, threads};
use yaac_core::fork::{ForkChoice, default_choice, view};
use yaac_core::link::HubLink;
use yaac_core::mode_confirm::ModeTarget;
use yaac_core::prefs::{SendKey, ThemePref, UiPrefs, keys};
use yaac_core::settings::{SshAction, providers_view, targets_view};
use yaac_core::ssh_setup::{Credential, Phase, Revoke, RevokePhase, Secret, SetupRecord, SshConn, SshSetup, StepMark, form_ready};
use yaac_core::state::HelloOutcome;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message};
use yaac_protocol::{ClientId, InstanceId, ModeId};

fn conn() -> SshConn {
    SshConn { name: "legacy".into(), host: "legacy.example.com".into(), port: 22, user: "deploy".into() }
}

fn password(text: &str) -> Credential {
    let mut s = Secret::new();
    for c in text.chars() {
        s.push(c);
    }
    Credential::Password(s)
}

// ── SSH setup ──────────────────────────────────────────────────────────────────────────

#[test]
fn setup_is_enabled_once_host_and_port_are_present() {
    assert!(!form_ready("", "22"));
    assert!(!form_ready("h", ""));
    assert!(!form_ready("h", "0"));
    assert!(form_ready("legacy.example.com", "22"));
}

#[test]
fn the_happy_path_confirms_signs_in_once_installs_and_verifies() {
    let mut b = ScriptedSsh::new(SshScript::Happy);
    let mut s = SshSetup::new(conn(), NOW);
    s.start(&mut b);
    s.pump(&mut b);
    let Phase::ConfirmHost(k) = &s.phase else { panic!("{:?}", s.phase) };
    assert_eq!(k.fingerprint, FINGERPRINT);
    assert_eq!(s.current().map(|l| l.text), Some("Confirm this is the right server".into()));
    s.confirm_host(&mut b);
    s.pump(&mut b);
    assert!(matches!(s.phase, Phase::SignIn { error: None }));
    assert!(s.steps.iter().any(|l| l.mark == StepMark::Hollow && l.text.contains("without a password")));
    s.sign_in(&mut b, password("hunter2"));
    s.pump(&mut b);
    assert_eq!(b.sign_ins, 1, "the credential is handed over exactly once");
    let Phase::Ready(SetupRecord::InstalledByYaac { key, date }) = &s.phase else { panic!("{:?}", s.phase) };
    assert_eq!(date, "2026-11-20");
    assert!(key.line.ends_with("yaac:laptop:legacy"));
    let texts: Vec<_> = s.steps.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Reached legacy.example.com",
            "Server confirmed",
            "Tried signing in without a password · not available",
            "Signed in with your password",
            "Installed YAAC's key for deploy",
            "Verified: signed in with the key alone"
        ]
    );
    // Nothing the machine keeps mentions the password.
    assert!(!format!("{s:?}").contains("hunter2"));
}

#[test]
fn access_already_in_place_installs_nothing() {
    let mut b = ScriptedSsh::new(SshScript::AccessInPlace);
    let mut s = SshSetup::new(conn(), NOW);
    s.start(&mut b);
    s.pump(&mut b);
    s.confirm_host(&mut b);
    s.pump(&mut b);
    assert!(matches!(s.phase, Phase::Ready(SetupRecord::AlreadyInPlace { .. })));
    assert_eq!(b.sign_ins, 0);
}

#[test]
fn a_failed_verification_removes_the_key_and_says_so() {
    let mut b = ScriptedSsh::new(SshScript::KeysRefused);
    let mut s = SshSetup::new(conn(), NOW);
    s.start(&mut b);
    s.pump(&mut b);
    s.confirm_host(&mut b);
    s.pump(&mut b);
    s.sign_in(&mut b, password("pw"));
    s.pump(&mut b);
    assert_eq!(s.phase, Phase::Failed { cleaned_up: true });
    let failed = s.steps.iter().find(|l| l.mark == StepMark::Failed).expect("a failed step");
    assert_eq!(failed.detail.as_deref(), Some("This server does not accept key logins."));
    assert!(s.steps.last().is_some_and(|l| l.text.starts_with("Removed the key again · confirmed")));
    assert!(s.leftover().is_none());
    s.try_again();
    assert!(matches!(s.phase, Phase::SignIn { .. }));
    assert!(!s.steps.iter().any(|l| l.text.starts_with("Signed in")));
}

#[test]
fn a_rejected_sign_in_changes_nothing_and_asks_again() {
    let mut b = ScriptedSsh::new(SshScript::Happy);
    let mut s = SshSetup::new(conn(), NOW);
    s.start(&mut b);
    s.pump(&mut b);
    s.confirm_host(&mut b);
    s.pump(&mut b);
    s.sign_in(&mut b, password(""));
    s.pump(&mut b);
    assert!(matches!(&s.phase, Phase::SignIn { error: Some(e) } if e.contains("Permission denied")));
    assert!(!s.steps.iter().any(|l| l.text.starts_with("Installed")));
}

#[test]
fn revoke_has_two_honest_endings() {
    let key = yaac_core::demo::ssh::demo_line("staging");
    let mut ok = ScriptedSsh::new(SshScript::Happy);
    let mut r = Revoke::start(conn(), key.clone(), NOW, &mut ok);
    assert_eq!(r.pump(&mut ok), Some(SetupRecord::Revoked { date: "2026-11-20".into() }));
    assert_eq!(r.phase, RevokePhase::Revoked);

    let mut down = ScriptedSsh::new(SshScript::Unreachable);
    let mut r = Revoke::start(conn(), key.clone(), NOW, &mut down);
    assert_eq!(r.pump(&mut down), None, "the record keeps the key until the user confirms");
    assert!(matches!(r.phase, RevokePhase::CouldNotRevoke { .. }));
    assert_eq!(r.key.line, key.line, "the complete line is kept for the user");
    assert_eq!(r.removed_by_hand(), SetupRecord::RemovedByHand { date: "2026-11-20".into() });
}

#[test]
fn a_secret_is_masked_and_moves_without_copies() {
    let mut s = Secret::new();
    for c in "correct horse".chars() {
        s.push(c);
    }
    assert_eq!(format!("{s:?}"), "Secret(••••••••)");
    let taken = s.take();
    assert!(s.is_empty());
    assert_eq!(taken.expose(), "correct horse");
    assert_eq!(taken.chars(), 13);
    let mut big = Secret::new();
    for _ in 0..300 {
        big.push('x');
    }
    assert_eq!(big.chars(), 300);
    big.clear();
    assert!(big.is_empty());
}

// ── settings ───────────────────────────────────────────────────────────────────────────

#[test]
fn targets_show_distinct_disable_remove_and_revoke() {
    let (c, _) = boot();
    let v = targets_view(&c.state);
    assert!(v.local.is_some());
    let row = |n: &str| v.ssh.iter().find(|r| r.name == n).unwrap_or_else(|| panic!("{n}"));
    assert_eq!(row("devbox").line, "Ready · set up by YAAC on 2026-11-20");
    assert_eq!(row("devbox").actions, [SshAction::Revoke, SshAction::Remove]);
    assert_eq!(row("gpu-box").line, "Ready · access was already in place");
    assert_eq!(row("gpu-box").actions, [SshAction::RevokeUnavailable("YAAC installed nothing on this server".into()), SshAction::Remove]);
    // Revoke stays available while offline, because YAAC installed a key there.
    assert_eq!(row("staging").line, "Offline · last seen 3h ago · set up by YAAC on 2026-11-02");
    assert_eq!(row("staging").actions, [SshAction::Retry, SshAction::Revoke, SshAction::Remove]);
    assert_eq!(row("legacy").line, "Not set up");
    assert_eq!(row("legacy").actions, [SshAction::SetUp, SshAction::Remove]);
    assert!(row("legacy").switch.is_none());
}

#[test]
fn a_second_session_to_a_distro_is_noted() {
    let (mut c, hub) = boot();
    hub.play(Scene::WslConnections);
    settle(&mut c, &hub);
    let v = targets_view(&c.state);
    assert_eq!(v.wsl.len(), 3);
    assert_eq!(v.wsl[0].user, "(default)");
    assert_eq!(v.wsl[1].note.as_deref(), Some("second session to this distro"));
    assert!(!v.wsl[2].enabled);
}

#[test]
fn providers_page_lists_instances_with_their_facts() {
    let (mut c, hub) = boot();
    let devbox = "devbox".into();
    c.list_instances(&devbox);
    settle(&mut c, &hub);
    let v = providers_view(&c.state, &devbox).expect("devbox");
    let work = v.rows.iter().find(|r| r.name == "Codex · work").expect("work");
    assert_eq!(work.status, "Ready");
    assert!(work.editable);
    assert_eq!(work.socket.as_deref(), Some("/run/user/1000/codex-work/control.sock"));
    assert!(work.facts.iter().any(|f| f.starts_with("daemon ")));
    let claude = v.rows.iter().find(|r| r.facts.iter().any(|f| f.starts_with("account:"))).expect("an instance with an account");
    assert!(!claude.editable);
    assert!(!v.kinds.is_empty());
    hub.play(Scene::LocalSignedOut);
    settle(&mut c, &hub);
    let local = providers_view(&c.state, &"local".into()).expect("local");
    assert!(local.rows.iter().any(|r| r.status.starts_with("Not signed in · Run `claude`")));
}

#[test]
fn adding_and_removing_an_instance_goes_to_the_target() {
    let (mut c, hub) = boot();
    let devbox = "devbox".into();
    let kind = c.state.target(&devbox).unwrap().providers[0].kind.clone();
    let before = c.state.target(&devbox).unwrap().providers.len();
    c.add_instance(&devbox, &kind, Some("lab"), Some("/tmp/lab.sock"));
    settle(&mut c, &hub);
    assert_eq!(c.state.target(&devbox).unwrap().providers.len(), before + 1);
    let added = c.state.target(&devbox).unwrap().providers.last().unwrap().instance.clone();
    c.remove_instance(&devbox, &added);
    settle(&mut c, &hub);
    assert_eq!(c.state.target(&devbox).unwrap().providers.len(), before);
}

// ── fork ───────────────────────────────────────────────────────────────────────────────

#[test]
fn the_fork_result_follows_from_the_destination() {
    let (mut c, hub) = boot();
    let fix = threads::fix();
    c.open(&fix);
    settle(&mut c, &hub);
    let own = default_choice(&c.state, &fix).expect("choice");
    let v = view(&c.state, &fix, &own).expect("view");
    assert_eq!(v.result_word, "Native fork");
    assert!(v.notice.is_none());
    // A native fork: plain "Fork", Enter works, no message asked, nothing to wait for.
    assert!(!v.handoff);
    assert_eq!(v.confirm, "Fork");
    assert!(!v.confirm_waits);
    assert!(v.size.is_none());
    assert!(v.destinations.iter().any(|d| d.instance == own.instance && d.native));
    assert!(v.destinations.iter().any(|d| d.instance.as_str() == "claude" && !d.native), "each choice is labelled native or handoff");
    let to_claude = ForkChoice { instance: InstanceId::new("claude"), model: None };
    let v = view(&c.state, &fix, &to_claude).expect("view");
    assert_eq!(v.result_word, "Handoff");
    assert_eq!(v.reason.as_deref(), Some("Native fork is only available within the same provider and store."));
    assert_eq!(v.notice.as_deref(), Some("This sends the conversation to Claude (work@example.com)."));
    assert!(v.preview_loading);
    // A handoff is confirmed only once what crosses has been shown.
    assert!(v.handoff);
    assert_eq!(v.confirm, "Send and fork");
    assert!(v.confirm_waits);
    c.fork_preview(&fix, &to_claude.instance);
    settle(&mut c, &hub);
    let v = view(&c.state, &fix, &to_claude).expect("view");
    assert!(!v.preview_loading);
    assert!(!v.confirm_waits);
    // Named by position, not by the opaque turn id.
    assert!(v.at.starts_with("at the last completed turn (turn "), "{}", v.at);
    let preview = v.preview.as_deref().expect("preview");
    assert!(preview.starts_with("# Prior conversation") && preview.contains("decode()"));
    assert!(v.size.as_deref().is_some_and(|s| s.starts_with("Transcript: ") && (s.ends_with(" KB") || s.ends_with(" bytes"))), "{:?}", v.size);
    assert!(v.not_carried.as_deref().is_some_and(|n| n.contains("2 attachments no longer on the target")));
    assert!(v.carried.contains("turn"));
    // The handoff goes with its first message (`thread/fork`), and the new thread opens with
    // the inherited transcript above its first turn.
    c.fork(&fix, &to_claude, Some("Carry on"));
    settle(&mut c, &hub);
    let opened = c.state.selected_thread().cloned().expect("the fork is open");
    assert!(c.state.summary(&opened).is_some_and(|s| s.title == "Fix frame decoder (fork)"));
    let items = c.state.transcripts.get(&opened).map(|t| t.items().to_vec()).unwrap_or_default();
    match items.first().map(|i| &i.body) {
        Some(yaac_protocol::ItemBody::Handoff(h)) => {
            assert_eq!(h.fork_kind, yaac_protocol::ForkKind::Handoff);
            assert!(h.transcript.as_deref().is_some_and(|t| t.contains("## Turn 1")));
        }
        other => panic!("the handoff item comes first, got {other:?}"),
    }
}

#[test]
fn a_native_fork_opens_idle_with_the_inherited_marker_and_no_transcript() {
    let (mut c, hub) = boot();
    let fix = threads::fix();
    c.open(&fix);
    settle(&mut c, &hub);
    let own = default_choice(&c.state, &fix).expect("choice");
    c.fork(&fix, &own, None);
    settle(&mut c, &hub);
    let opened = c.state.selected_thread().cloned().expect("the fork is open");
    assert_ne!(opened, fix);
    let items = c.state.transcripts.get(&opened).map(|t| t.items().to_vec()).unwrap_or_default();
    match items.first().map(|i| &i.body) {
        Some(yaac_protocol::ItemBody::Handoff(h)) => {
            assert_eq!(h.fork_kind, yaac_protocol::ForkKind::Native);
            assert!(h.transcript.is_none());
        }
        other => panic!("the handoff item comes first, got {other:?}"),
    }
}

// ── first run and the project picker ───────────────────────────────────────────────────

#[test]
fn first_run_shows_this_computer_with_the_targets_own_hint() {
    let (c, _) = boot_with_fresh();
    assert!(yaac_core::first_run::is_first_run(&c.state));
    let v = yaac_core::first_run::first_run(&c.state);
    assert!(v.can_start);
    assert_eq!(v.providers.len(), 2);
    let signed_out = v.providers.iter().find(|p| !p.ready).expect("one not ready");
    assert_eq!(signed_out.word, "not signed in");
    assert_eq!(signed_out.hint.as_deref(), Some("Run `claude` in a terminal to sign in"));
}

fn boot_with_fresh() -> (Client, DemoHub) {
    let hub = DemoHub::new(Scenario::Fresh, NOW);
    let mut c = Client::new(Box::new(hub.clone()), "test", ClientId::new("w"));
    settle(&mut c, &hub);
    (c, hub)
}

#[test]
fn projects_are_grouped_by_machine_most_recent_first() {
    let (c, _) = boot_with(Scenario::Fleet);
    let yaac = c.state.projects.iter().find(|p| p.name == "yaac").map(|p| p.id.clone()).expect("yaac");
    let groups = yaac_core::first_run::project_picker(&c.state, Some(&yaac));
    let names: Vec<_> = groups.iter().map(|g| g.name.as_str()).collect();
    // gpu-box has the most recent activity (search-service, 20 s ago), then local (30 s).
    assert_eq!(&names[..2], ["gpu-box", "local"]);
    let devbox = groups.iter().find(|g| g.name == "devbox").expect("devbox");
    // CI integration (api, 1 min ago) is newer than Fix frame decoder (yaac, 2 min ago).
    assert_eq!(devbox.projects[0].name, "api");
    assert!(devbox.projects.iter().any(|p| p.selected));
    let staging = groups.iter().find(|g| g.name == "staging").expect("staging");
    assert_eq!(staging.note.as_deref(), Some("offline"));
}

// ── a mode that bypasses approvals ─────────────────────────────────────────────────────

#[test]
fn a_mode_that_skips_approvals_waits_for_confirmation_every_time() {
    let (mut c, hub) = boot();
    let fix = threads::fix();
    c.open(&fix);
    settle(&mut c, &hub);
    let t = ModeTarget::Thread(fix.clone());
    c.choose_mode(t.clone(), ModeId::new("auto"));
    assert!(c.state.mode_confirm.is_none());
    assert_eq!(c.state.next_settings[&fix].mode, Some(ModeId::new("auto")));

    c.choose_mode(t.clone(), ModeId::new("bypass"));
    assert_eq!(c.state.next_settings[&fix].mode, Some(ModeId::new("auto")), "not applied before confirmation");
    let pending = c.state.mode_confirm.clone().expect("pending");
    let v = yaac_core::mode_confirm::view(&c.state, &pending);
    assert_eq!(v.title, "Let this agent act without asking?");
    assert_eq!(v.mode_name, "bypass permissions");
    assert_eq!(v.confirm_label, "Bypass permissions");
    assert_eq!(v.lines, ["Thread: Fix frame decoder", "Machine: devbox", "Folder: /home/dev/yaac"]);
    c.cancel_mode();
    assert!(c.state.mode_confirm.is_none());
    assert_eq!(c.state.next_settings[&fix].mode, Some(ModeId::new("auto")));

    c.choose_mode(t.clone(), ModeId::new("bypass"));
    c.confirm_mode();
    assert_eq!(c.state.next_settings[&fix].mode, Some(ModeId::new("bypass")));
    // Never remembered: choosing it again (after switching back) asks again.
    c.choose_mode(t.clone(), ModeId::new("ask"));
    c.choose_mode(t, ModeId::new("bypass"));
    assert!(c.state.mode_confirm.is_some());
}

#[test]
fn the_mode_selector_warns_while_a_bypass_mode_is_chosen() {
    let (mut c, hub) = boot();
    let fix = threads::fix();
    c.open(&fix);
    settle(&mut c, &hub);
    c.choose_mode(ModeTarget::Thread(fix.clone()), ModeId::new("bypass"));
    c.confirm_mode();
    let s = c.state.summary(&fix).cloned().unwrap();
    let providers = c.state.target(&fix.target).unwrap().providers.clone();
    let t = c.state.transcripts.get(&fix);
    let row = yaac_core::controls::started_row(yaac_core::controls::StartedInputs {
        providers: &providers,
        instance: &s.instance,
        target_name: "devbox",
        effective: t.and_then(|t| t.effective.as_ref()),
        summary_model: s.model.as_ref(),
        next: c.state.next_settings.get(&fix),
        context: None,
        may_act: true,
        agents: None,
    });
    let yaac_core::controls::ControlRowView::Row(row) = row else { panic!() };
    assert!(row.mode.expect("mode").warn);
}

// ── hello outcomes ─────────────────────────────────────────────────────────────────────

/// A link that answers `hub/hello` the way a hub of another build would.
struct OtherBuild {
    answer: Option<Result<Value, ErrorObject>>,
    out: Vec<Message>,
}

impl HubLink for OtherBuild {
    fn send(&mut self, message: Message) {
        if let Message::Request { id, method, .. } = message
            && method == "hub/hello"
        {
            match self.answer.take() {
                Some(Ok(v)) => self.out.push(Message::Response { id, result: v }),
                Some(Err(e)) => self.out.push(Message::Error { id: Some(id), error: e }),
                None => {}
            }
        }
    }
    fn poll(&mut self) -> Option<Message> {
        (!self.out.is_empty()).then(|| self.out.remove(0))
    }
}

#[test]
fn hello_has_three_outcomes() {
    let mut same = Client::new(Box::new(OtherBuild { answer: Some(Ok(json!({ "build": "1.0.0", "targets": [] }))), out: vec![] }), "1.0.0", ClientId::new("w"));
    same.pump();
    assert_eq!(same.state.hub.outcome, HelloOutcome::Attached);
    assert!(same.state.hub.attached);

    let mut newer = Client::new(Box::new(OtherBuild { answer: Some(Ok(json!({ "build": "0.9.0", "targets": [] }))), out: vec![] }), "1.0.0", ClientId::new("w"));
    newer.pump();
    assert_eq!(newer.state.hub.outcome, HelloOutcome::HubOutdated { hub_build: "0.9.0".into() });
    assert!(!newer.state.hub.attached);

    let err = ErrorObject::new(ErrorCode::from_wire("window_outdated"), "This window (1.0.0) is older than the running hub (2.0.0). Update YAAC.");
    let mut older = Client::new(Box::new(OtherBuild { answer: Some(Err(err)), out: vec![] }), "1.0.0", ClientId::new("w"));
    older.pump();
    assert!(matches!(&older.state.hub.outcome, HelloOutcome::WindowOutdated { message } if message.contains("Update YAAC")));
    assert!(!older.state.hub.attached);
}

// ── persisted choices ──────────────────────────────────────────────────────────────────

#[test]
fn window_choices_round_trip_through_ui_state_values() {
    let mut p = UiPrefs::default();
    p.grouping.worktree = true;
    p.docked = true;
    p.theme = ThemePref::Light;
    p.muted_open = true;
    p.font_size = 15.0;
    p.send_key = SendKey::CtrlEnter;
    let mut q = UiPrefs::default();
    for k in keys::ALL {
        q.load(k, &p.value(k));
    }
    assert_eq!(p, q);
    assert_eq!(p.changed(&UiPrefs::default()).len(), 6);
    // Garbage is ignored.
    let mut r = UiPrefs::default();
    r.load(keys::THEME, &json!(42));
    r.load(keys::FONT_SIZE, &json!(1000));
    assert_eq!(r, UiPrefs::default());
}

#[test]
fn ui_state_is_sent_only_when_attached() {
    let (mut c, hub) = boot();
    c.set_ui_state(keys::THEME, &json!("dark"));
    settle(&mut c, &hub);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

// ── wire data instead of window-local seams ────────────────────────────────────────────

#[test]
fn mute_and_pin_are_hub_view_preferences_and_agents_go_through_thread_update() {
    let (mut c, hub) = boot();
    let fix = threads::fix();
    assert!(!c.state.is_muted(&fix, common::NOW));
    c.set_muted(&fix, true);
    c.set_pinned(&fix, true);
    c.set_agents_policy(&fix, yaac_core::state::AgentsPolicy::Allow);
    settle(&mut c, &hub);
    // Pin and mute are kept by the hub (`hub/view/set`), never read from the summary.
    assert!(c.state.is_muted(&fix, common::NOW) && c.state.is_pinned(&fix));
    let s = c.state.summary(&fix).unwrap();
    // A fresh window gets them back from the hub (`hub/view/get`).
    let mut c2 = yaac_core::client::Client::new(Box::new(hub.clone()), "test", yaac_protocol::ClientId::new(yaac_core::demo::DEMO_CLIENT));
    settle(&mut c2, &hub);
    assert!(c2.state.is_muted(&fix, common::NOW) && c2.state.is_pinned(&fix));
    assert_eq!(s.agents, yaac_protocol::AgentsPolicy::Allow);
    assert_eq!(c.state.agents_policy(&fix), yaac_core::state::AgentsPolicy::Allow);
}

#[test]
fn projects_come_from_the_target_and_threads_carry_their_own() {
    let (mut c, hub) = boot();
    assert_eq!(c.state.projects.len(), 6);
    let fix = threads::fix();
    assert_eq!(c.state.project_of(&fix).map(|p| p.name.as_str()), Some("yaac"));
    assert!(c.state.project_of(&threads::parser()).is_none(), "a temporary project");
    c.create_project(&"devbox".into(), "/home/dev", "Home", None);
    settle(&mut c, &hub);
    assert_eq!(c.state.projects.iter().filter(|p| p.target.as_str() == "devbox").count(), 3);
    // The deeper project still wins.
    assert_eq!(c.state.project_of(&fix).map(|p| p.name.as_str()), Some("yaac"));
}

#[test]
fn the_hub_search_pages_its_results() {
    let (mut c, hub) = boot();
    c.find("frame");
    settle(&mut c, &hub);
    let f = c.state.find.clone().expect("results");
    assert!(f.results.contains(&threads::fix()));
    assert!(!f.loading);
    c.find("");
    assert!(c.state.find.is_none());
}

#[test]
fn thought_has_a_duration_from_the_item_times() {
    let (mut c, hub) = boot();
    let fix = threads::fix();
    c.open(&fix);
    settle(&mut c, &hub);
    let map = yaac_core::attention::AttentionMap::compute(&c.state);
    let lines: Vec<String> = yaac_core::activity::blocks(&c.state, &map, &fix, &Default::default())
        .iter()
        .filter_map(|b| match b {
            yaac_core::activity::Block::Run { lines, .. } => Some(lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("|")),
            _ => None,
        })
        .collect();
    assert!(lines.iter().any(|l| l.contains("Thought for 4s")), "{lines:?}");
}

#[test]
fn a_stopped_wsl_distro_disables_its_threads_until_started() {
    let (mut c, hub) = boot();
    hub.play(yaac_core::demo::Scene::Notices);
    settle(&mut c, &hub);
    let wsl = yaac_protocol::TargetId::new("wsl-ubuntu");
    assert_eq!(c.state.stopped_distro(&wsl).as_deref(), Some("Ubuntu"));
    // A cached thread on the distro: listed and readable, but nothing that changes state.
    let key = yaac_core::ThreadKey::new(wsl.clone(), yaac_protocol::ThreadId::new("t-wsl"));
    let mut rec = c.state.threads[&threads::fix()].clone();
    rec.summary.id = key.thread.clone();
    c.state.threads.insert(key.clone(), rec);
    assert!(!c.state.may_act(&key));
    assert!(c.state.may_act(&threads::fix()), "other machines are not affected");
    // Start it: the hub connects (booting the distro); the threads come back by themselves.
    c.retry_target(&wsl);
    settle(&mut c, &hub);
    assert_eq!(c.state.stopped_distro(&wsl), None);
    assert!(c.state.may_act(&key));
}
