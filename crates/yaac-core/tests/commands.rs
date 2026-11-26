//! The command table, key bindings and the Commander's search (D130, D131).

mod common;

use std::collections::BTreeSet;

use common::{NOW, boot};
use yaac_core::commands::{
    Avail, Bindings, COMMANDS, Cmd, Ctx, Scope, StepKind, StepValue, availability, by_id, default_chords, def_of, list, next_unseen, note_recent,
    step_in_list, step_items,
};
use yaac_core::demo::threads;
use yaac_core::keys::Chord;
use yaac_core::prefs::{UiPrefs, keys};
use yaac_core::sidebar::{Grouping, Ordering};
use yaac_core::state::AppState;
use yaac_core::ThreadKey;
use yaac_protocol::Capability;

fn ctx<'a>(state: &'a AppState, thread: Option<&'a ThreadKey>) -> Ctx<'a> {
    Ctx {
        state,
        now: NOW,
        thread,
        draft: None,
        can_back: false,
        can_forward: false,
        has_tray: false,
        grouping: Grouping::default(),
        ordering: Ordering::default(),
        archived_open: false,
        docked: false,
        theme: yaac_core::prefs::ThemePref::System,
        zoom: (13.5, 11.0, 18.0),
        host_windows: false,
        focused_card: None,
        list_selected: 0,
        input: yaac_core::input::InputMode::Pointer,
        terminal: Default::default(),
        fullscreen_tabs: false,
    }
}

#[test]
fn commander_omits_the_off_model_even_when_the_open_thread_uses_it() {
    let (mut client, _) = boot();
    let key = threads::fix();
    let id = client.state.summary(&key).unwrap().model.clone().unwrap();
    let mut provider = client.state.provider_of(&key).unwrap().clone();
    let index = provider.models.iter().position(|m| m.id == id).unwrap();
    provider.disabled_models.push(provider.models.remove(index));
    client.state.apply_providers(&key.target, vec![provider], false);
    let choices = step_items(StepKind::Model, &ctx(&client.state, Some(&key)), &[]);
    assert!(choices.iter().any(|choice| choice.reason.is_none()));
    assert!(!choices.iter().any(|choice| matches!(&choice.value, StepValue::Model(_, model) if *model == id)));
}

/// The catalogue's rows (docs/plan/commands.md) and the commands each stands for.
const CATALOGUE: &[(&str, &[&str])] = &[
    ("Commander", &["commander"]),
    ("Search threads", &["search-threads"]),
    ("Find a thread…", &["find-thread"]),
    ("Next thread that needs you", &["needs-next"]),
    ("Jump to the bottom", &["jump-to-bottom"]),
    ("Go to previous / next message", &["prev-message", "next-message"]),
    ("Next unseen finished thread", &["next-unseen"]),
    ("Next / previous thread in the list", &["next-thread", "prev-thread"]),
    ("Back / forward", &["back", "forward"]),
    ("History…", &["history"]),
    ("Go to the parent thread", &["parent"]),
    ("Go to a related agent…", &["related"]),
    ("Settings", &["settings"]),
    ("Settings: a page…", &["settings-page"]),
    ("New thread", &["new-thread"]),
    ("New thread in this project", &["new-in-project"]),
    ("Rename", &["rename"]),
    ("Copy thread ID", &["copy-id"]),
    ("Pin / Unpin", &["pin"]),
    ("Mute… / Unmute", &["mute"]),
    ("Archive… / Unarchive", &["archive"]),
    ("Delete…", &["delete"]),
    ("Dismiss / Show again", &["dismiss"]),
    ("Dismiss all in this temporary project", &["dismiss-all-temp"]),
    ("Fork…", &["fork"]),
    ("Take over", &["take-over"]),
    ("Move to project… / Move selected…", &["move", "move-selected"]),
    ("Check for the latest", &["check-latest"]),
    ("Continue (budget)", &["continue-budget"]),
    ("Archive selected… / Mute selected… / Clear selection", &["archive-selected", "mute-selected", "clear-selection"]),
    ("Focus the composer", &["focus-composer"]),
    ("Send / Queue message", &["send"]),
    ("Send now", &["send-now"]),
    ("New line", &["new-line"]),
    ("Stop the turn", &["stop"]),
    ("Attach files…", &["attach"]),
    ("Paste", &["paste"]),
    ("Change model…", &["model"]),
    ("Change effort…", &["effort"]),
    ("Change mode…", &["mode"]),
    ("Agents: ask / allow…", &["agents"]),
    ("Fast mode on / off", &["fast"]),
    ("Allow once / Allow for this thread / Deny", &["allow-once", "allow-session", "deny"]),
    ("Answer the question", &["answer-question"]),
    ("Open this branch's PR", &["open-pr"]),
    ("Create a pull request…", &["create-pr"]),
    ("Copy the PR link", &["copy-pr-link"]),
    ("Toggle the sidebar", &["toggle-sidebar"]),
    ("Cycle full-screen tabs", &["cycle-fullscreen-tabs"]),
    ("Dock / undock the agents panel", &["dock"]),
    ("Grouping: machine / project / worktree", &["grouping"]),
    ("Order: last message / A → Z / Z → A", &["order"]),
    ("Keep unseen at the top on / off", &["unseen-first"]),
    ("Keep needs-you at the top on / off", &["needs-first"]),
    ("Show only running on / off", &["running-only"]),
    ("Show dismissed / Show archived", &["show-dismissed", "show-archived"]),
    ("Theme: system / light / dark", &["theme"]),
    ("Zoom in / out / reset", &["zoom-in", "zoom-out", "zoom-reset"]),
    ("Expand / collapse all tool lines", &["expand-all", "collapse-all"]),
    ("Copy the selection / Copy message", &["copy-selection", "copy-message"]),
    ("Retry connecting…", &["retry"]),
    ("Start a WSL distribution…", &["start-wsl"]),
    ("Enable / disable a target…", &["target-enable"]),
    ("Add an SSH target… / Add WSL…", &["add-ssh", "add-wsl"]),
    ("Refresh providers and models…", &["refresh-providers"]),
    ("Toggle terminal panel", &["toggle-terminal-panel"]),
    ("New terminal", &["new-terminal"]),
    ("New terminal with shell…", &["new-terminal-with-shell"]),
    ("Next / Previous terminal", &["next-terminal", "prev-terminal"]),
    ("Maximise terminal panel", &["maximise-terminal-panel"]),
    ("Hide to the tray", &["hide-to-tray"]),
    ("Quit YAAC", &["quit"]),
    ("Renderer…", &["renderer"]),
];

#[test]
fn ids_are_unique_and_every_catalogue_row_is_in_the_table() {
    let ids: BTreeSet<&str> = COMMANDS.iter().map(|d| d.id).collect();
    assert_eq!(ids.len(), COMMANDS.len(), "unique ids");
    let cmds: BTreeSet<Cmd> = COMMANDS.iter().map(|d| d.cmd).collect();
    assert_eq!(cmds.len(), COMMANDS.len(), "one row per command");
    // The catalogue in the doc, row by row.
    let doc = include_str!("../../../docs/plan/commands.md");
    let catalogue = &doc[doc.find("## Catalogue").expect("catalogue")..doc.find("## Default key conflicts").expect("end")];
    let rows: Vec<&str> = catalogue
        .lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| Command |") && !l.starts_with("| ---"))
        .map(|l| l.split('|').nth(1).unwrap_or("").trim())
        .collect();
    assert_eq!(rows.len(), CATALOGUE.len(), "{rows:?}");
    let mut covered = BTreeSet::new();
    for row in rows {
        let (_, want) = CATALOGUE.iter().find(|(n, _)| *n == row).unwrap_or_else(|| panic!("catalogue row {row:?} is mapped"));
        for id in *want {
            assert!(by_id(id).is_some(), "{id} is in the table");
            covered.insert(*id);
        }
    }
    assert_eq!(covered, ids, "every command is in the catalogue");
}

#[test]
fn default_keys_parse_and_global_ones_are_unique() {
    let mut seen = std::collections::BTreeMap::new();
    for d in COMMANDS {
        assert_eq!(default_chords(d).len(), d.keys.len(), "{} keys parse", d.id);
        assert!(!d.name.is_empty() && d.explain.ends_with('.'), "{}", d.id);
        // Local keys (Enter, Esc, Ctrl+Enter, Ctrl+V, Ctrl+C) only work where they are, so the
        // same key may stand for a different local command elsewhere (Esc: stop, deny).
        if d.scope != Scope::Global {
            continue;
        }
        for c in default_chords(d) {
            if let Some(other) = seen.insert(c.clone(), d.id) {
                panic!("{c} is the default of both {other} and {}", d.id);
            }
        }
    }
    assert_eq!(Bindings::default().label(Cmd::Commander), "Ctrl+Space");
    assert_eq!(Bindings::default().label(Cmd::Back), "Alt+← / Mouse back");
    assert_eq!(Bindings::default().resolve(&Chord::parse("Ctrl+Shift+R").unwrap()), Some(Cmd::RunningOnly));
    // A local key never resolves globally: Esc stays with the composer and the card.
    assert_eq!(Bindings::default().resolve(&Chord::parse("Escape").unwrap()), None);
}

#[test]
fn thread_commands_are_hidden_with_no_thread_open() {
    let (c, _) = boot();
    let none = ctx(&c.state, None);
    for cmd in [Cmd::Rename, Cmd::Pin, Cmd::Fork, Cmd::Delete, Cmd::Parent, Cmd::Model, Cmd::Stop, Cmd::OpenPr] {
        assert_eq!(availability(cmd, &none), Avail::Hidden, "{cmd:?}");
    }
    assert_eq!(availability(Cmd::NewThread, &none), Avail::Enabled);
    let rows = list(&none, "", &[]);
    assert!(rows.iter().all(|r| r.cmd != Cmd::Rename));
    assert_eq!(rows[0].heading, Some("Go to"));
    let k = threads::fix();
    let open = ctx(&c.state, Some(&k));
    assert_eq!(availability(Cmd::Pin, &open), Avail::Enabled);
    assert!(list(&open, "", &[]).iter().any(|r| r.cmd == Cmd::Rename));
}

#[test]
fn a_missing_capability_disables_with_its_reason() {
    let (mut c, hub) = boot();
    let k = threads::fix();
    c.open(&k);
    common::settle(&mut c, &hub);
    // The harness cannot rename: listed, disabled, with why.
    let strip = |caps: &mut Vec<Capability>| caps.retain(|x| *x != Capability::ThreadRename && *x != Capability::FastMode);
    if let Some(e) = c.state.transcripts.get_mut(&k).and_then(|t| t.effective.as_mut()) {
        strip(&mut e.capabilities);
    }
    for t in &mut c.state.targets {
        for p in &mut t.providers {
            strip(&mut p.capabilities);
            for m in &mut p.models {
                m.capabilities.retain(|x| x.capability != Capability::FastMode);
            }
        }
    }
    let x = ctx(&c.state, Some(&k));
    assert_eq!(availability(Cmd::Rename, &x), Avail::Disabled("This harness cannot rename threads".into()));
    assert_eq!(availability(Cmd::Fast, &x), Avail::Disabled("This model has no fast mode".into()));
    let row = list(&x, "rename", &[]).into_iter().find(|r| r.cmd == Cmd::Rename).expect("still listed");
    assert!(!row.avail.enabled());
}

#[test]
fn search_finds_by_name_alias_and_explanation_and_ranks_recent_first() {
    let (c, _) = boot();
    let k = threads::fix();
    let x = ctx(&c.state, Some(&k));
    let dock = list(&x, "dock", &[]);
    assert_eq!(dock.first().map(|r| r.cmd), Some(Cmd::Dock), "{dock:?}");
    // An alias ("snooze") and a fuzzy abbreviation ("shwrun").
    assert_eq!(list(&x, "snooze", &[]).first().map(|r| r.cmd), Some(Cmd::Mute));
    assert_eq!(list(&x, "shwrun", &[]).first().map(|r| r.cmd), Some(Cmd::RunningOnly));
    assert!(list(&x, "zzqqxx", &[]).is_empty());
    // Letters scattered through unrelated words do not match.
    assert!(list(&x, "agent", &[]).iter().all(|r| r.cmd != Cmd::Attach), "{:?}", list(&x, "agent", &[]));
    // Two equally good matches: the recently used one comes first.
    let plain: Vec<Cmd> = list(&x, "thread", &[]).into_iter().map(|r| r.cmd).collect();
    let last = *plain.last().unwrap();
    let mut recent = Vec::new();
    note_recent(&mut recent, def_of(last).id);
    assert_eq!(list(&x, "thread", &recent).first().map(|r| r.cmd), Some(last));
    // With no query, recent commands lead under their own heading.
    let rows = list(&x, "", &recent);
    assert_eq!((rows[0].cmd, rows[0].heading), (last, Some("Recent")));
    for _ in 0..20 {
        note_recent(&mut recent, "pin");
    }
    assert_eq!(recent.iter().filter(|r| *r == "pin").count(), 1);
}

#[test]
fn steps_offer_the_choices_of_the_open_thread() {
    let (c, _) = boot();
    let k = threads::fix();
    let x = ctx(&c.state, Some(&k));
    let models = step_items(StepKind::Model, &x, &[]);
    assert!(models.iter().any(|m| matches!(m.value, StepValue::Model(..)) && m.reason.is_none()), "{models:?}");
    assert!(models.iter().filter(|m| m.current).count() <= 1);
    let mute = step_items(StepKind::Mute, &x, &[]);
    assert_eq!(mute.last().map(|m| &m.value), Some(&StepValue::MuteDate));
    let order = step_items(StepKind::Order, &x, &[]);
    assert_eq!(order.iter().filter(|o| o.current).count(), 1);
}

#[test]
fn moving_through_the_list_and_to_the_next_unseen() {
    let (c, _) = boot();
    let order: Vec<ThreadKey> = c.state.threads.keys().cloned().collect();
    assert_eq!(step_in_list(&order, None, 1), order.first().cloned());
    assert_eq!(step_in_list(&order, Some(&order[0]), 1), order.get(1).cloned());
    assert_eq!(step_in_list(&order, Some(&order[0]), -1), None);
    if let Some(u) = next_unseen(&c.state, &order, None) {
        assert!(c.state.is_unseen(&u));
        if order.iter().filter(|k| c.state.is_unseen(k)).count() > 1 {
            assert_ne!(next_unseen(&c.state, &order, Some(&u)), Some(u.clone()), "moves on to another");
        }
    }
}

#[test]
fn rebinding_detects_conflicts_and_stores_only_differences() {
    let mut b = Bindings::default();
    let ctrl_k = Chord::parse("Ctrl+K").unwrap();
    // Ctrl+K belongs to Search threads.
    assert_eq!(b.conflict(&ctrl_k, Cmd::Commander), Some(Cmd::SearchThreads));
    assert!(!b.assign(Cmd::Commander, ctrl_k.clone(), false), "cancel keeps both");
    assert_eq!(b.resolve(&ctrl_k), Some(Cmd::SearchThreads));
    assert!(b.assign(Cmd::Commander, ctrl_k.clone(), true), "replace");
    assert_eq!(b.resolve(&ctrl_k), Some(Cmd::Commander));
    assert_eq!(b.resolve(&Chord::parse("Ctrl+Space").unwrap()), None, "the old key no longer works");
    assert!(b.chords(Cmd::SearchThreads).is_empty());
    // A local key is not taken over.
    assert!(!b.assign(Cmd::Pin, Chord::parse("Ctrl+V").unwrap(), true));
    // Stored as differences, through the window preferences.
    let p = UiPrefs { bindings: b.overrides.clone(), ..UiPrefs::default() };
    assert_eq!(p.changed(&UiPrefs::default()), [keys::KEY_BINDINGS]);
    let mut q = UiPrefs::default();
    q.load(keys::KEY_BINDINGS, &p.value(keys::KEY_BINDINGS));
    assert_eq!(q.bindings, b.overrides);
    assert_eq!(q.bindings.0.len(), 2, "{:?}", q.bindings);
    // Setting the defaults back stores nothing.
    b.reset(Cmd::SearchThreads);
    b.set(Cmd::Commander, default_chords(def_of(Cmd::Commander)));
    assert_eq!(b, Bindings::default());
    b.clear(Cmd::Rename);
    assert!(b.is_changed(Cmd::Rename));
    b.reset_all();
    assert_eq!(b, Bindings::default());
}

#[test]
fn the_recent_list_is_stored_and_unknown_ids_dropped() {
    let mut p = UiPrefs::default();
    p.load(keys::COMMANDER_RECENT, &serde_json::json!(["pin", "no-such-command", "dock"]));
    assert_eq!(p.recent, ["pin", "dock"]);
    assert_eq!(p.value(keys::COMMANDER_RECENT), serde_json::json!(["pin", "dock"]));
}

#[test]
fn expand_and_collapse_all_tool_lines() {
    let (c, _) = boot();
    let k = threads::orchestrator();
    let map = yaac_core::attention::AttentionMap::compute(&c.state);
    let mut e = yaac_core::activity::Expansion::default();
    yaac_core::activity::expand_all(&c.state, &map, &k, &mut e);
    let blocks = yaac_core::activity::blocks(&c.state, &map, &k, &e);
    assert!(!blocks.iter().any(|b| matches!(b, yaac_core::activity::Block::Folded { .. })));
    let lines: Vec<_> = blocks.iter().filter_map(|b| if let yaac_core::activity::Block::Run { lines } = b { Some(lines) } else { None }).flatten().filter(|l| l.diff.is_none()).collect();
    assert!(!lines.is_empty() && lines.iter().all(|l| l.expanded));
    yaac_core::activity::collapse_all(&c.state, &k, &mut e);
    assert_eq!(e, yaac_core::activity::Expansion::default());
}

#[test]
fn terminal_commands_and_full_screen_switcher_work_in_a_focused_terminal_by_default() {
    use yaac_core::commands::{Bindings, in_terminal_default};
    for d in COMMANDS {
        let terminal = matches!(d.cmd, Cmd::ToggleTerminalPanel | Cmd::NewTerminal | Cmd::MaximiseTerminalPanel | Cmd::NextTerminal | Cmd::PrevTerminal | Cmd::CycleFullscreenTabs);
        assert_eq!(in_terminal_default(d.cmd), terminal, "{}", d.id);
    }
    let mut b = Bindings::default();
    assert!(!b.works_in_terminal(Cmd::Commander), "Ctrl+Space goes to the shell (PSReadLine)");
    b.set_in_terminal(Cmd::Commander, true);
    assert!(b.works_in_terminal(Cmd::Commander));
    b.set_in_terminal(Cmd::ToggleTerminalPanel, true);
    assert_eq!(b.in_terminal.len(), 1, "only differences are kept");
    // Stored and read back as `terminalCommands`.
    let mut p = yaac_core::prefs::UiPrefs { terminal_commands: b.in_terminal.clone(), ..Default::default() };
    let v = p.value(yaac_core::prefs::keys::TERMINAL_COMMANDS);
    p.terminal_commands.clear();
    p.load(yaac_core::prefs::keys::TERMINAL_COMMANDS, &v);
    assert_eq!(p.terminal_commands, b.in_terminal);
    let t = p.value(yaac_core::prefs::keys::TERMINAL);
    assert_eq!(t["copyOnSelect"], false);
    assert_eq!(t["confirmMultilinePaste"], true);
}

#[test]
fn reasons_that_name_an_input_mechanic_follow_the_input_mode() {
    let state = AppState::default();
    let mut c = ctx(&state, None);
    let reason = |c: &Ctx<'_>| match availability(Cmd::ArchiveSelected, c) {
        Avail::Disabled(why) => why,
        other => panic!("{other:?}"),
    };
    assert_eq!(reason(&c), "Nothing is selected in the sidebar (Ctrl+click rows)");
    c.input = yaac_core::input::InputMode::Touch;
    assert_eq!(reason(&c), "Nothing is selected in the list (tap a row's state mark)");
}
