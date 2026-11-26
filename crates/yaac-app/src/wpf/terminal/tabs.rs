use super::TerminalPipe;
use crate::app::YaacApp;
use crate::file_pane::TabKind;
use crate::preview_controller::Browsers;
use crate::terminal::{Session, SessionState, Shells};
use serde_json::{Value, json};
use std::time::Instant;
use yaac_core::{ThreadKey, attachments::DraftRef, client::Client};

pub(in crate::wpf) fn active(b: &Browsers, key: &ThreadKey, panel: bool) -> Option<(u64, u64)> {
    if panel {
        if !b.chat_visible(key) {
            return None;
        }
        let p = b.terms.panel(key).filter(|p| p.open)?;
        return Some((p.active, p.active));
    }
    let a = b.areas.get(key).filter(|a| !a.chat)?;
    let TabKind::Terminal(session) = b.tab(key, a.active)?.kind else {
        return None;
    };
    Some((a.active, session))
}

fn describe(id: u64, s: &Session) -> Value {
    json!({ "kind":"terminal", "id":id, "title":s.title(), "shell":s.shell_name(), "cwd":s.cwd,
        "failed":matches!(s.state,SessionState::Failed(_)), "unread":s.unseen, "bell":u8::from(s.bell(Instant::now())),
        "exitCode":match s.state { SessionState::Exited(code)=>Some(code), _=>None },
        "error":match &s.state {SessionState::Failed(msg)=>Some(msg.as_str()),
            SessionState::Reconnecting=>Some("Reconnecting…"), _=>None } })
}

pub(in crate::wpf) fn decorate(
    state: &mut Value,
    b: &Browsers,
    client: &Client,
    key: Option<&ThreadKey>,
    prefs: &yaac_core::prefs::TerminalPrefs,
) {
    let Some(key) = key else {
        state["terminalReason"] = json!("Open a thread to use Terminal.");
        return;
    };
    let reason = crate::terminal::unavailable(client, key);
    let shells = match b.terms.shells.get(&key.target) {
        Some(Shells::Ready(shells)) => json!(shells),
        _ => json!([]),
    };
    state["terminalReason"] = json!(reason);
    state["shells"] = shells.clone();
    if let Some(tabs) = state["tabs"].as_array_mut() {
        for tab in tabs {
            let Some(id) = tab["id"].as_u64() else {
                continue;
            };
            if let Some(TabKind::Terminal(sid)) = b.tab(key, id).map(|t| &t.kind)
                && let Some(session) = b.terms.get(*sid)
            {
                *tab = describe(id, session);
            }
        }
    }
    if let Some(p) = b.terms.panel(key) {
        let tabs: Vec<_> = p
            .tabs
            .iter()
            .filter_map(|&id| Some(describe(id, b.terms.get(id)?)))
            .collect();
        state["terminalPanel"] = json!({ "thread":key.address(),"tabs":tabs, "active":p.active,
            "open":p.open, "maximised":p.maximised, "share":prefs.panel_share,
            "terminalReason":reason, "shells":shells });
    }
}

pub(in crate::wpf) fn command(
    pipe: &mut TerminalPipe,
    app: &mut YaacApp,
    key: &ThreadKey,
    tab: Option<u64>,
    name: &str,
    p: &Value,
) -> Result<bool, String> {
    let b = &mut app.browsers;
    let terms = &mut b.terms;
    let client = &mut app.client;
    let data = &p["data"];
    let shell = || -> Result<_, String> {
        let Some(path) = p["shell"].as_str() else {
            return Ok(None);
        };
        let Some(Shells::Ready(shells)) = terms.shells.get(&key.target) else {
            return Err("The shell list is unavailable.".into());
        };
        shells
            .iter()
            .find(|s| s.path == path)
            .cloned()
            .map(Some)
            .ok_or_else(|| "That shell is unavailable.".into())
    };
    match name {
        "terminal.new" => {
            let shell = shell()?;
            b.open_terminal(client, key, shell)?;
        }
        "terminal.panelNew" => {
            let shell = shell()?;
            b.new_panel_terminal(client, key, shell)?;
        }
        "terminal.rename" => {
            let (Some(tab), Some(name)) = (tab, data["name"].as_str()) else {
                return Ok(false);
            };
            // A context menu or F2 can name a background tab. Side-pane tab IDs
            // and panel session IDs are independent, even within the same thread.
            let id = if p["panel"] == true {
                let visible = b.terms.panel(key).is_some_and(|p| p.open && p.tabs.contains(&tab));
                if !b.chat_visible(key) || !visible {
                    return Ok(false);
                }
                tab
            } else {
                let Some(TabKind::Terminal(id)) = b.tab(key, tab).map(|t| &t.kind) else {
                    return Ok(false);
                };
                *id
            };
            b.terms.rename(client, id, name);
        }
        "terminal.panelSelect" | "terminal.panelClose" => {
            let Some(id) = tab.filter(|id| {
                terms
                    .panel(key)
                    .is_some_and(|p| p.open && p.tabs.contains(id))
            }) else {
                return Ok(false);
            };
            if name == "terminal.panelClose" {
                terms.close(client, id)
            } else {
                terms.panel_select(key, id)
            }
        }
        "terminal.panelHide" => {
            if terms.panel_open(key) {
                b.toggle_terminal_panel(client, key)?;
            }
        }
        "terminal.panelMaximise" => terms.panel_maximise(key),
        "terminal.panelHeight" => {
            if let Some(share) = p["share"].as_f64() {
                app.ui.terminal.panel_share = share.clamp(0., 1.) as f32;
            }
        }
        _ => {
            let Some((_, id)) =
                active(b, key, p["panel"] == true).filter(|(visible, _)| Some(*visible) == tab)
            else {
                return Ok(false);
            };
            let terms = &mut b.terms;
            let Some(s) = terms.get(id) else {
                return Ok(false);
            };
            if name == "terminal.key"
                && let Some(key) = super::input::key(data["key"].as_str().unwrap_or(""))
            {
                let mods = super::input::mods(data);
                let selection = if mods.ctrl
                    && matches!(
                        key,
                        yaac_term::Key::Char('c' | 'C') | yaac_term::Key::Insert
                    ) {
                    s.term.selection_text()
                } else {
                    None
                };
                match yaac_term::shortcut(
                    key,
                    mods,
                    selection.is_some(),
                    app.ui.terminal.ctrl_v_to_shell,
                ) {
                    yaac_term::Shortcut::Copy => {
                        pipe.copied = selection;
                        if !mods.shift && matches!(key, yaac_term::Key::Char('c' | 'C')) {
                            s.term.clear_selection();
                        }
                        return Ok(true);
                    }
                    yaac_term::Shortcut::Paste => {
                        pipe.read_clipboard = true;
                        return Ok(true);
                    }
                    yaac_term::Shortcut::Send => {}
                }
            }
            if name == "terminal.mouse"
                && data["action"] == "up"
                && app.ui.terminal.copy_on_select
                && data["button"] == "Left"
            {
                pipe.copied = s.term.selection_text();
            }
            match name {
                "terminal.resize" => {
                    if let Some((c, r)) = pipe.viewport(data) {
                        terms.resize(client, id, c, r)
                    }
                }
                "terminal.restart" => {
                    pipe.force_full = true;
                    terms.restart(client, id);
                }
                "terminal.add" => {
                    let Some(excerpt) = s.excerpt() else {
                        return Ok(false);
                    };
                    let caret = p["caret"]
                        .as_u64()
                        .and_then(|n| usize::try_from(n).ok())
                        .unwrap_or(usize::MAX);
                    let caret = client
                        .draft(&DraftRef::Thread(key.clone()))
                        .map_or(0, |draft| {
                            crate::wpf::files::scalar_caret(&draft.text, caret)
                        });
                    if !crate::file_pane::insert_excerpt(
                        client,
                        key,
                        caret,
                        excerpt.label,
                        excerpt.text,
                    ) {
                        return Err("The composer is unavailable for this thread.".into());
                    }
                    if let Some(panel) = b.terms.panels.get_mut(key) {
                        panel.maximised = false;
                    }
                    if !b.chat_visible(key) {
                        b.select_chat(key);
                    }
                    app.ui.focus_composer = true;
                }
                "terminal.pasteDecision" if data["token"].as_u64() == Some(pipe.paste_token) => {
                    if let Some(text) = terms.get_mut(id).and_then(|s| s.paste_confirm.take())
                        && data["allow"] == true
                    {
                        if data["remember"] == true {
                            app.ui.terminal.confirm_multiline_paste = false;
                        }
                        terms.paste(client, id, &text, true);
                    }
                }
                "terminal.paste" => {
                    if let Some(text) = data["text"].as_str() {
                        terms.paste(client, id, text, !app.ui.terminal.confirm_multiline_paste);
                        if terms
                            .get(id)
                            .is_some_and(|session| session.paste_confirm.is_some())
                        {
                            pipe.paste_token += 1;
                            let lines = text.replace("\r\n", "\n").split(['\n', '\r']).count();
                            pipe.paste_prompt = Some(
                                json!({ "thread":key.address(), "tab":tab, "panel":pipe.panel,
                                "token":pipe.paste_token, "lines":lines }),
                            );
                        }
                    }
                }
                _ => {
                    if let Some(bytes) = pipe.input(s, name, data) {
                        if matches!(name, "terminal.key" | "terminal.text") {
                            terms.typed(client, id, &bytes)
                        } else {
                            terms.input(client, id, &bytes)
                        }
                    }
                }
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_resolves_background_tabs_in_their_own_strip() {
        let mut app = crate::app::tests::demo_app(&egui::Context::default());
        let key = yaac_core::demo::threads::orchestrator();
        let b = &mut app.browsers;
        let panel = b.terms.insert(&key, None, "/panel".into());
        let side = b.terms.insert(&key, None, "/side".into());
        let tab = b.terminal_tab(&key, side);
        assert_eq!(panel, tab);
        b.insert(&key);
        let active_panel = b.terms.insert(&key, None, "/active".into());
        b.terms.panels.insert(
            key.clone(),
            crate::terminal::Panel {
                open: true,
                active: active_panel,
                tabs: vec![panel, active_panel],
                maximised: false,
            },
        );
        let mut pipe = TerminalPipe::default();
        for (is_panel, name) in [(true, "jobs"), (false, "watch")] {
            assert!(command(&mut pipe, &mut app, &key, Some(tab), "terminal.rename",
                &json!({"panel":is_panel,"data":{"name":name}})).unwrap());
        }
        assert_eq!(app.browsers.terms.get(panel).unwrap().title(), "jobs");
        assert_eq!(app.browsers.terms.get(side).unwrap().title(), "watch");
        command(&mut pipe, &mut app, &key, Some(tab), "terminal.rename",
            &json!({"panel":true,"data":{"name":""}})).unwrap();
        assert!(app.browsers.terms.get(panel).unwrap().name.is_none());
        assert_eq!(app.browsers.terms.get(side).unwrap().title(), "watch");
    }

    #[test]
    fn side_and_panel_input_with_the_same_tab_number_reach_the_displayed_session() {
        let mut app = crate::app::tests::demo_app(&egui::Context::default());
        let key = yaac_core::demo::threads::orchestrator();
        let b = &mut app.browsers;
        // Side-pane tab numbers and terminal session numbers use independent counters.
        let panel_id = b.terms.insert(&key, None, "/panel".into());
        let side_session = b.terms.insert(&key, None, "/side".into());
        let side_tab = b.terminal_tab(&key, side_session);
        assert_eq!(side_tab, panel_id);
        b.terms.panels.insert(
            key.clone(),
            crate::terminal::Panel {
                open: true,
                active: panel_id,
                tabs: vec![panel_id],
                maximised: false,
            },
        );
        for (panel, expected) in [(true, panel_id), (false, side_session)] {
            let mut pipe = TerminalPipe::default();
            command(
                &mut pipe,
                &mut app,
                &key,
                Some(side_tab),
                "terminal.search",
                &json!({"panel":panel,"data":{"query":if panel {"first"} else {"second"}}}),
            )
            .unwrap();
            let s = app.browsers.terms.get_mut(expected).unwrap();
            let deadline = Instant::now() + std::time::Duration::from_secs(2);
            while !s.term.idle() {
                assert!(Instant::now() < deadline, "terminal search timed out");
                std::thread::yield_now();
            }
            s.term.snapshot(&mut s.frame);
            assert_eq!(
                s.frame.search.as_ref().unwrap().query,
                if panel { "first" } else { "second" }
            );
        }
        app.browsers.set_full_screen(&key, true);
        assert!(active(&app.browsers, &key, true).is_none());
        app.browsers.select_chat(&key);
        assert!(active(&app.browsers, &key, false).is_none());
        assert_eq!(
            active(&app.browsers, &key, true),
            Some((panel_id, panel_id))
        );
    }
}
