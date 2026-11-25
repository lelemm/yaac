//! `yaac --demo --screenshots <dir>`: render the main-shell screens in both themes through
//! the real window (eframe's own viewport screenshot), one PNG each, then exit.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, ColorImage};
use yaac_core::agents::PanelMode;
use yaac_core::demo::{DemoHub, Scenario, Scene, threads};

use yaac_core::demo::ssh::{ScriptedSsh, SshScript};
use yaac_core::fork::ForkChoice;
use yaac_core::mode_confirm::ModeTarget;
use yaac_core::settings::Page;
use yaac_core::ssh_setup::{Revoke, SetupRecord, SshConn, SshSetup};

use crate::app::{ForkDialog, Menu, STORY_NOW, YaacApp};
use crate::settings::{RevokePanel, SshPanel};
use crate::theme::ThemeChoice;

/// The screens, by brief number and file name.
pub(crate) const SCREENS: &[(&str, &str)] = &[
    ("01", "approval"),
    ("02", "orchestrator"),
    ("03", "worker-side"),
    ("04", "blocked-worker"),
    ("05", "request-states"),
    ("07", "first-run"),
    ("08", "new-thread"),
    ("09", "first-message"),
    ("10", "in-use"),
    ("11", "fork"),
    ("11n", "fork-native"),
    ("12", "forked-thread"),
    ("13", "settings-targets"),
    ("13d", "settings-wsl-distros"),
    ("14", "ssh-confirm"),
    ("15", "ssh-ready"),
    ("16", "ssh-fail-revoke"),
    ("17", "settings-providers"),
    ("19", "notices"),
    ("20", "budget"),
    ("21", "narrow"),
    ("22", "ssh-sign-in"),
    ("23", "queued-arrives"),
    ("24", "destination"),
    ("25", "permission"),
    ("26", "mute-archive"),
    ("27", "agents-pill"),
    ("28", "agents-card"),
    ("29", "create-project"),
    ("30", "bypass-confirm"),
    ("31", "history-list"),
    ("32", "ssh-form"),
    ("33", "mute-dialog"),
    ("34", "wsl-stopped"),
    ("35", "sidebar-strip"),
    ("36", "sidebar-wide"),
    ("37", "attach-uploading"),
    ("38", "attach-unsupported"),
    ("39", "attach-unknown"),
    ("40", "attach-drop"),
    ("41", "context-menu"),
    ("42", "delete-confirm"),
    ("43", "archive-confirm"),
    ("44", "unseen"),
    ("45", "ordering-menu"),
    ("46", "pill-relations"),
    ("47", "pill-workers"),
    ("48", "docked-narrow"),
    ("60", "pr-badges"),
    ("61", "pr-more"),
    ("62", "pr-menu"),
    ("63", "pr-create"),
    ("49", "settings-window"),
    ("50", "fast-mode"),
    ("64", "commander"),
    ("65", "commander-filtered"),
    ("66", "commander-model"),
    ("67", "settings-keyboard"),
    ("68", "settings-about"),
    ("69", "rows-selected"),
    ("70", "selection-menu"),
    ("71", "bulk-archive-confirm"),
    ("72", "group-header-menu"),
    ("73", "bulk-archive-report"),
    ("74", "take-over-blocked"),
    ("75", "taken-over"),
    ("76", "message-markup"),
    ("77", "message-markup-open"),
    ("78", "move-dialog"),
    ("79", "move-dialog-placement"),
    ("80", "move-drag-valid"),
    ("81", "move-drag-other-machine"),
    ("82", "move-undo"),
    ("83", "markdown-table"),
    ("84", "tool-open"),
    ("85", "silent-reasoning"),
    ("86", "markdown-parity"),
    ("90", "agent-calls"),
    ("91", "agent-calls-open"),
    ("92", "subagents"),
    ("93", "subagents-open"),
    ("95", "harness-subagents"),
    ("96", "subagent-open"),
    ("100", "timeline"),
    ("101", "worked-collapsed"),
    ("102", "worked-expanded"),
    ("103", "message-time"),
    ("104", "worked-failed"),
    ("110", "rate-limits"),
    ("111", "rate-limits-tooltip"),
    ("112", "rate-limits-strip"),
    ("120", "running-queue"),
    // `!command` in the composer and its block in the transcript (P344, P346).
    ("140", "bang-composer"),
    ("141", "bang-composer-running"),
    ("142", "bang-left"),
    ("143", "bang-block-running"),
    ("144", "bang-block-done"),
    ("130", "many-agents-pill"),
    ("131", "many-agents-card"),
    ("132", "many-agents-docked"),
    ("133", "many-agents-alive-overflow"),
    // The side pane (docs/12-files.md#visual-design-of-the-side-pane), with the WPF
    // renderer's fixture: written as <theme>-<width>-<scene>.png.
    ("pane-wide-browser", "pane"),
    ("pane-wide-servers", "pane"),
    ("pane-wide-empty", "pane"),
    ("pane-wide-files", "pane"),
    ("pane-wide-diff", "pane"),
    ("pane-wide-many-tabs", "pane"),
    ("pane-wide-full-chat", "pane"),
    ("pane-wide-full-browser", "pane"),
    ("pane-narrow-browser", "pane"),
    ("pane-narrow-servers", "pane"),
    ("pane-narrow-empty", "pane"),
    ("pane-narrow-files", "pane"),
    ("pane-narrow-diff", "pane"),
    ("pane-narrow-many-tabs", "pane"),
    ("pane-narrow-full-chat", "pane"),
    ("pane-narrow-full-browser", "pane"),
    ("pane-docked-split", "pane"),
    ("pane-docked-full-chat", "pane"),
    // Terminals (docs/13-terminal.md), from the recorded fixtures in crates/yaac-term/fixtures
    // at 69 × 28 cells, in the side pane at the pane-wide ratio; written as <theme>-<scene>.png.
    ("term-bash-prompt", "term"),
    ("term-vim", "term"),
    ("term-htop", "term"),
    ("term-powershell-prompt", "term"),
    ("term-selection", "term"),
    ("term-search", "term"),
    ("term-exited", "term"),
    ("term-vim-full", "term"),
    ("term-htop-full", "term"),
    // The terminal panel under the composer (P338).
    ("term-panel-open", "term"),
    ("term-panel-maximised", "term"),
    ("term-panel-fullchat", "term"),
    ("term-panel-rename", "term"),
];

/// Screen 21 is drawn at the smallest supported window: 960 × 720 points.
const NARROW_SIZE: [f32; 2] = [960.0, 720.0];

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "small positive pixel sizes")]
fn crop(img: &ColorImage, w: f32, h: f32) -> ColorImage {
    let (w, h) = ((w as usize).min(img.size[0]), (h as usize).min(img.size[1]));
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        pixels.extend_from_slice(&img.pixels[y * img.size[0]..y * img.size[0] + w]);
    }
    ColorImage::new([w, h], pixels)
}

/// The side-pane scenes match the WPF renderer's captures: 960 × 587 points at 150 %.
const PANE_SIZE: [f32; 2] = [960.0, 587.0];
const PANE_ZOOM: f32 = 1.5;
/// The docked Related agents panel needs a window wider than [`crate::shell::NARROW`].
const DOCKED_SIZE: [f32; 2] = [1280.0, 800.0];

const SETTLE_FRAMES: u32 = 14;

/// The run.
pub(crate) struct Shots {
    dir: PathBuf,
    jobs: Vec<(&'static str, &'static str, bool)>,
    cur: usize,
    frame: u32,
    started: f64,
    waiting: bool,
    image: Arc<Mutex<Option<Arc<ColorImage>>>>,
    /// Files written.
    pub written: Vec<PathBuf>,
}

impl std::fmt::Debug for Shots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shots").field("cur", &self.cur).finish_non_exhaustive()
    }
}

impl Shots {
    pub(crate) fn new(dir: PathBuf, only: Option<&[String]>) -> Self {
        let mut jobs = Vec::new();
        for (id, name) in SCREENS {
            if only.as_ref().is_some_and(|o| !o.iter().any(|x| x == id)) {
                continue;
            }
            jobs.push((*id, *name, true));
            jobs.push((*id, *name, false));
        }
        Self { dir, jobs, cur: 0, frame: 0, started: 0.0, waiting: false, image: Arc::default(), written: Vec::new() }
    }

    /// Advance; returns true when every screen is written.
    pub(crate) fn step(&mut self, app: &mut YaacApp, ctx: &egui::Context) -> bool {
        if self.cur >= self.jobs.len() {
            return true;
        }
        let (id, name, dark) = self.jobs[self.cur];
        if self.waiting {
            let img = self.image.lock().ok().and_then(|mut g| g.take());
            if let Some(img) = img {
                let theme = if dark { "dark" } else { "light" };
                let file = match id.strip_prefix("pane-").or_else(|| id.strip_prefix("term-")) {
                    Some(scene) => self.dir.join(format!("{theme}-{scene}.png")),
                    None => self.dir.join(format!("{id}-{name}-{theme}.png")),
                };
                let img = if id == "21" {
                    crop(&img, NARROW_SIZE[0] * 1.25, NARROW_SIZE[1] * 1.25)
                } else if id.starts_with("pane-docked") {
                    crop(&img, DOCKED_SIZE[0] * 1.25, DOCKED_SIZE[1] * 1.25)
                } else if id.starts_with("pane-") || id.starts_with("term-") {
                    crop(&img, PANE_SIZE[0] * PANE_ZOOM, PANE_SIZE[1] * PANE_ZOOM)
                } else {
                    (*img).clone()
                };
                match write_png(&file, &img) {
                    Ok(()) => {
                        eprintln!("wrote {} ({}x{})", file.display(), img.size[0], img.size[1]);
                        self.written.push(file);
                    }
                    Err(e) => eprintln!("could not write {}: {e}", file.display()),
                }
                self.cur += 1;
                self.frame = 0;
                self.waiting = false;
            }
            ctx.request_repaint();
            return self.cur >= self.jobs.len();
        }
        if self.frame == 0 {
            if self.cur == 0 {
                eprintln!(
                    "native pixels per point {:?}, screen {:?}, monitor {:?}",
                    ctx.input(|i| i.viewport().native_pixels_per_point),
                    ctx.content_rect(),
                    ctx.input(|i| i.viewport().monitor_size)
                );
            }
            ctx.set_zoom_factor(if (id.starts_with("pane-") && !id.starts_with("pane-docked")) || id.starts_with("term-") { PANE_ZOOM } else { 1.25 });
            // Each screen starts from fresh widget memory (scroll positions from the screen
            // before must not carry over).
            ctx.data_mut(egui::util::IdTypeMap::clear);
            // Code blocks show their copy button as on hover.
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique(crate::markdown::FORCE_COPY_BUTTONS), true));
            // Scenes may depend on the theme (a terminal replays vim's light or dark colours).
            app.ui.theme = if dark { ThemeChoice::Dark } else { ThemeChoice::Light };
            setup(app, id);
            // This display cannot resize the window reliably, so screen 21 lays the shell out in
            // 960 × 720 points and the picture is cropped to it.
            app.ui.layout_size = if id == "21" {
                Some(egui::vec2(NARROW_SIZE[0], NARROW_SIZE[1]))
            } else if id.starts_with("pane-docked") {
                Some(egui::vec2(DOCKED_SIZE[0], DOCKED_SIZE[1]))
            } else if id.starts_with("pane-") || id.starts_with("term-") {
                Some(egui::vec2(PANE_SIZE[0], PANE_SIZE[1]))
            } else {
                None
            };
            app.ui.theme = if dark { ThemeChoice::Dark } else { ThemeChoice::Light };
            app.applied = None;
            self.started = ctx.input(|i| i.time);
        }
        self.frame += 1;
        // Wait for layout to settle and for popups and dialogs to finish fading in.
        if self.frame >= SETTLE_FRAMES && ctx.input(|i| i.time) - self.started > 0.6 {
            let slot = self.image.clone();
            ctx.request_screenshot(move |img| {
                if let Ok(mut g) = slot.lock() {
                    *g = Some(img);
                }
            });
            self.waiting = true;
        }
        ctx.request_repaint();
        false
    }
}

fn setup(app: &mut YaacApp, id: &str) {
    let scenario = if matches!(id, "07" | "08" | "09") { Scenario::Fresh } else { Scenario::Fleet };
    app.reset_demo(scenario);
    app.ui.timeline = crate::timeline::TimelineUi::default();
    app.ui.worked = yaac_core::turns::Opened::default();
    app.host_windows = false;
    let hub = app.demo.clone();
    let play = |app: &mut YaacApp, s: Scene| {
        if let Some(h) = &hub {
            h.play(s);
            h.drain();
        }
        app.pump_demo();
    };
    let docked = matches!(id, "02" | "03" | "04" | "20" | "21" | "23" | "24" | "44" | "48" | "50" | "60" | "95");
    app.ui.panel = if docked { PanelMode::Docked } else { PanelMode::Pill };
    if let Some(scene) = id.strip_prefix("pane-") {
        pane(app, scene);
        return;
    }
    if let Some(scene) = id.strip_prefix("term-") {
        terminal(app, scene);
        return;
    }
    match id {
        "01" | "10" => {
            let k = if id == "01" { threads::fix() } else { threads::handshake() };
            app.open(&k, None);
        }
        "02" | "27" | "28" => {
            let k = threads::orchestrator();
            // Came here from the frame decoder: Back is available, Forward is not.
            app.open(&threads::fix(), None);
            app.open(&k, None);
            if id == "02" {
                // A three-line message being written: the composer has grown to three lines.
                app.client.state.drafts.entry(k.clone()).or_default().text =
                    "When search-service finishes:\n- merge it behind the flag\n- then tell CI to run the full suite".into();
                app.ui.expansion.toggle_line(&k, &"o4".into());
            }
            if id == "28" {
                app.ui.panel = PanelMode::Card;
            }
        }
        "100" | "101" | "102" | "103" | "104" => {
            // Three weeks of prompts on Try the new parser (D153–D156).
            play(app, Scene::LongConversation);
            let k = threads::parser();
            app.open(&k, None);
            match id {
                // The timeline beside the thread, scrolled to the middle, a dash hovered.
                "100" => {
                    app.ui.timeline.go_to = Some("pl17-u".into());
                    app.ui.timeline.force_hover = Some(9);
                }
                // The newest turns, each folded into "Worked for …" and its answer.
                "101" => {}
                // One of them opened again.
                "102" => app.ui.worked.toggle(&k, &"p-long-30".into()),
                // A message hovered: its time under it.
                "103" => app.ui.timeline.force_stamp = Some("pl30-u".into()),
                // A failed turn: its error stays under the line.
                _ => app.ui.timeline.go_to = Some("pl11-u".into()),
            }
            if id != "101" && id != "102" && id != "103" {
                app.ui.scroll_end = false;
            }
        }
        "140" | "141" | "142" | "143" | "144" => bang(app, id, &play),
        "120" => {
            // Queue or send now (D166): billing-service (Claude Code) is working; two messages
            // wait in the strip above the composer, and a third is being typed. Queue is the
            // primary; Send now says it interrupts, since this harness cannot steer.
            play(app, Scene::QueueWhileRunning);
            let k = threads::billing();
            app.open(&k, None);
            app.client.state.drafts.entry(k).or_default().text = "And bump the crate's patch version.".into();
        }
        "84" => {
            // An open tool call (D158): the down chevron and faint fill on its line, the guide
            // down its body that closes it.
            let k = threads::users();
            app.open(&k, None);
            app.ui.expansion.toggle_line(&k, &"u4".into());
            // Its finished turn opened again (D156), so the line shows.
            app.ui.worked.toggle(&k, &"u-t1".into());
            app.ui.scroll_to = Some("u4".into());
            app.ui.scroll_end = false;
        }
        "83" => {
            // A markdown table in an agent's message: a grid with a bold header, not the cells
            // run together (the bug the owner saw).
            let k = threads::users();
            app.open(&k, None);
            app.ui.scroll_to = Some("u5".into());
            app.ui.scroll_end = false;
        }
        "86" => {
            // Keep a single reference transcript for comparing Markdown across renderers.
            let key = threads::users();
            app.open(&key, None);
            let id = yaac_protocol::ItemId::from("u5");
            if let Some(transcript) = app.client.state.transcripts.get_mut(&key)
                && let Some(mut item) = transcript.item(&id).cloned()
                && let yaac_protocol::ItemBody::AgentMessage(message) = &mut item.body
            {
                message.text = r#"## Session checks

**Claude history loads** from the configured directory. The *fallback* uses `~/.claude`.
Read the [provider notes](https://example.com/providers) for background.

- Read the project's sessions.
- Expand a command to see its result.
  - Click the image to inspect it.

> The preview opens fitted to the available space.

```sh
git status --short
git diff --check
```

| Check | Result |
| --- | --- |
| Build | **Passed** |
| Warnings | `0` |"#.to_owned();
                transcript.upsert(item, transcript.revision.saturating_add(1));
            }
            app.ui.scroll_to = Some(id);
            app.ui.scroll_end = false;
        }
        "85" => {
            // A Codex turn whose hidden reasoning (empty summaries) sat between the commands:
            // no "Thought for 0s" lines, only the long pause and the reasoning with text (D148).
            let k = threads::parser();
            app.open(&k, None);
            app.ui.expansion.toggle_run(&k, &"parser-3".into());
            // The finished turn opened again (D156), so its run shows.
            app.ui.worked.toggle(&k, &"p-t1".into());
        }
        "03" => {
            // A file change: one diff expanded in place, the others collapsed, and the turn's
            // summary line at the end.
            let k = threads::billing();
            app.open(&k, None);
            app.ui.expansion.toggle_file(&k, &"b3".into(), 0);
            // The finished turn opened again (D156), so its work shows.
            app.ui.worked.toggle(&k, &"b-t1".into());
            app.ui.scroll_to = Some("b3".into());
            app.ui.scroll_end = false;
        }
        "04" => app.open(&threads::index(), None),
        "05" => {
            play(app, Scene::RequestStates);
            app.open(&threads::fix(), None);
        }
        "07" => {}
        "08" | "09" => {
            // The user made a project for their first thread ("New project…" in the picker).
            app.client.create_project(&"local".into(), "/home/dev/api", "api", None);
            app.pump_demo();
            app.start_first_thread();
            if let Some(d) = app.client.state.new_threads.first_mut() {
                d.draft.text = "# Investigate frame parsing\nReproduce the **last-frame** drop in `decode()`.".into();
            }
            app.ui.focus_composer = false;
            if id == "08" {
                app.ui.menu = Some(Menu::Model);
            } else {
                if let Some(id) = app.client.state.new_threads.first().map(|d| d.id) {
                    app.client.start_thread(id);
                }
                app.pump_demo();
                if let Some(h) = &hub {
                    for _ in 0..62 {
                        h.tick();
                        app.pump_demo();
                    }
                }
                if let Some(k) = app.client.state.selected_thread().cloned() {
                    app.open(&k, None);
                }
            }
        }
        "11" => {
            let fix = threads::fix();
            app.open(&fix, None);
            let dest = ForkChoice { instance: "claude".into(), model: None };
            app.client.fork_preview(&fix, &dest.instance);
            app.pump_demo();
            app.ui.fork = Some(ForkDialog { key: fix, choice: dest, preview_open: true, keep_children: false, message: "Carry on from here: add the regression test for the split frame.".into() });
        }
        "11n" => {
            let fix = threads::fix();
            app.open(&fix, None);
            let Some(dest) = yaac_core::fork::default_choice(&app.client.state, &fix) else { return };
            app.client.fork_preview(&fix, &dest.instance);
            app.pump_demo();
            app.ui.fork = Some(ForkDialog { key: fix, choice: dest, preview_open: true, keep_children: false, message: String::new() });
        }
        "12" => {
            let fix = threads::fix();
            app.open(&fix, None);
            app.client.fork(&fix, &ForkChoice { instance: "claude".into(), model: None }, Some("Carry on from here: add the regression test for the split frame."), false);
            app.pump_demo();
            if let Some(k) = app.client.state.selected_thread().cloned() {
                app.open(&k, None);
            }
        }
        "13" | "13d" => {
            play(app, Scene::WslConnections);
            app.host_windows = true;
            app.client.state.wsl_on = true;
            app.ui.settings = true;
            app.ui.settings_page = Page::Targets;
            app.client.load_wsl_distros();
            app.pump_demo();
            if id == "13d" {
                // The distro dropdown open: Ubuntu, the default, already has a connection, so
                // nothing is chosen.
                app.ui.menu = Some(Menu::WslDistro);
            }
        }
        "14" | "22" | "15" => {
            app.ui.settings = true;
            app.ui.settings_page = Page::Targets;
            let mut panel = SshPanel::new(STORY_NOW, None);
            panel.name = "legacy".into();
            panel.host = "legacy.example.com".into();
            panel.user = "deploy".into();
            panel.existing = Some("legacy".into());
            panel.setup = SshSetup::new(SshConn { name: "legacy".into(), host: "legacy.example.com".into(), port: 22, user: "deploy".into() }, STORY_NOW);
            let b = app.ssh.as_mut();
            panel.setup.start(b);
            panel.setup.pump(b);
            if id != "14" {
                panel.setup.confirm_host(b);
                panel.setup.pump(b);
            }
            if id == "22" {
                for c in "correct horse".chars() {
                    panel.password.push(c);
                }
            }
            if id == "15" {
                let mut secret = yaac_core::ssh_setup::Secret::new();
                for c in "correct horse".chars() {
                    secret.push(c);
                }
                panel.setup.sign_in(b, yaac_core::ssh_setup::Credential::Password(secret));
                panel.setup.pump(b);
            }
            app.ui.set.ssh = Some(panel);
            // Keep the legacy row (it is the server being set up) off the list's "Not set up".
        }
        "16" => {
            app.ui.settings = true;
            app.ui.settings_page = Page::Targets;
            // "Could not finish": the server accepts passwords, not keys.
            app.ssh = Box::new(ScriptedSsh::new(SshScript::KeysRefused));
            let mut panel = SshPanel::new(STORY_NOW, None);
            panel.name = "legacy".into();
            panel.host = "legacy.example.com".into();
            panel.user = "deploy".into();
            panel.setup = SshSetup::new(SshConn { name: "legacy".into(), host: "legacy.example.com".into(), port: 22, user: "deploy".into() }, STORY_NOW);
            let b = app.ssh.as_mut();
            panel.setup.start(b);
            panel.setup.pump(b);
            panel.setup.confirm_host(b);
            panel.setup.pump(b);
            let mut secret = yaac_core::ssh_setup::Secret::new();
            secret.push('x');
            panel.setup.sign_in(b, yaac_core::ssh_setup::Credential::Password(secret));
            panel.setup.pump(b);
            app.ui.set.ssh = Some(panel);
            // "Revoked": devbox's key was removed today.
            app.client.state.ssh_records.insert("devbox".into(), SetupRecord::Revoked { date: "2026-11-20".into() });
            // "Could not revoke": staging cannot be reached.
            app.ssh = Box::new(ScriptedSsh::new(SshScript::Unreachable));
            let key = yaac_core::demo::ssh::demo_line("staging");
            let conn = SshConn { name: "staging".into(), host: "staging.example.com".into(), port: 22, user: "dev".into() };
            let mut revoke = Revoke::start(conn, key, STORY_NOW, app.ssh.as_mut());
            revoke.pump(app.ssh.as_mut());
            app.ui.set.revoke = Some(RevokePanel { target: "staging".into(), revoke });
        }
        "17" => {
            play(app, Scene::LocalSignedOut);
            app.client.list_instances(&"devbox".into());
            app.pump_demo();
            app.ui.set.providers_target = Some("devbox".into());
            app.ui.settings = true;
            app.ui.settings_page = Page::Providers;
        }
        "19" => {
            play(app, Scene::Notices);
            let fix = threads::fix();
            app.open(&fix, None);
            app.client.state.drafts.entry(fix).or_default().attachments = DemoHub::notice_attachments();
        }
        "20" => {
            play(app, Scene::BudgetReached);
            app.open(&threads::orchestrator(), None);
        }
        "21" => {
            app.open(&threads::orchestrator(), None);
        }
        "23" => {
            play(app, Scene::StagingReconnects);
            let k = threads::orchestrator();
            app.open(&k, None);
            app.ui.expansion.toggle_line(&k, &"o9".into());
        }
        "24" => {
            play(app, Scene::StagingReconnects);
            app.open(&threads::notes(), Some("n1".into()));
        }
        "25" => {
            play(app, Scene::Permission);
            app.open(&threads::orchestrator(), None);
        }
        "26" => {
            app.open(&threads::fix(), None);
            let (mute, unmute) = DemoHub::mute_scene();
            for k in mute {
                app.client.set_muted(&k, true);
            }
            for k in unmute {
                app.client.set_muted(&k, false);
            }
            app.ui.muted_open = true;
            // The end of the list: Muted open (a timed mute shows when it ends), Archived
            // collapsed to its header beneath it.
            app.ui.sidebar_to_end = true;
            app.ui.menu = Some(Menu::Context(threads::billing()));
        }
        "29" => {
            app.open(&threads::fix(), None);
            let cp = crate::sidebar::create_project_for(app, &"devbox".into(), "~/scratch/spike-7");
            app.ui.create_project = Some(cp);
        }
        "30" => {
            let fix = threads::fix();
            app.open(&fix, None);
            app.ui.focus_card = None;
            app.ui.menu = Some(Menu::Mode);
            app.client.choose_mode(ModeTarget::Thread(fix), "bypass".into());
        }
        "35" => {
            // The sidebar collapsed to the strip, the card of one thread open as on hover.
            app.open(&threads::orchestrator(), None);
            app.ui.sidebar.collapsed = true;
            app.ui.strip_hover_key = Some((threads::billing(), egui::Rect::NOTHING));
            // users-service finished unseen: the ring around its logo (D114).
            app.client.set_view(&threads::users(), yaac_core::state::ViewPatch { unseen: Some(true), ..Default::default() });
            app.pump_demo();
        }
        "37" | "38" | "39" | "40" => {
            // Attachments in the composer of billing-service (devbox).
            let k = threads::billing();
            app.open(&k, None);
            let support = match id {
                "38" => yaac_protocol::Support::Unsupported,
                "39" => yaac_protocol::Support::Unknown,
                _ => yaac_protocol::Support::Supported,
            };
            if let Some(e) = app.client.state.transcripts.get_mut(&k).and_then(|t| t.effective.as_mut()) {
                e.image_input = support;
            }
            let draft = app.client.state.drafts.entry(k.clone()).or_default();
            match id {
                "37" => {
                    draft.text = "The trace from the failing run, and the latency plot.".into();
                    draft.attachments = vec![DemoHub::uploading(1, "trace.log", 0.62), DemoHub::chip(2, "plot.png", Some("image/png"), true)];
                }
                "38" => {
                    draft.text = "This is the flow I mean.".into();
                    draft.attachments = vec![
                        DemoHub::chip(2, "diagram.png", Some("image/png"), true),
                        DemoHub::failed(3, "build-cache.tar", "build-cache.tar is larger than 512 MiB."),
                    ];
                }
                "39" => {
                    draft.text = "Make the settings page look like this sketch.".into();
                    draft.attachments = vec![DemoHub::chip(4, "sketch.png", Some("image/png"), true), DemoHub::chip(5, "notes.md", Some("text/markdown"), true)];
                }
                _ => {
                    draft.text = "Compare these two logs.".into();
                    draft.attachments = vec![DemoHub::chip(5, "run-1.log", None, true)];
                    app.ui.drop_preview = true;
                }
            }
            for name in ["plot.png", "diagram.png", "sketch.png"] {
                app.files.set_thumb(crate::files::ThumbKey::Path(format!("/home/dev/Pictures/{name}")), Some(crate::files::demo_thumb()));
            }
            app.ui.focus_card = None;
        }
        "41" => {
            // A sidebar row's right-click menu (D109); the row keeps its hover colour (D108).
            app.open(&threads::fix(), None);
            app.ui.focus_card = None;
            app.ui.menu = Some(Menu::Context(threads::billing()));
        }
        "110" | "111" | "112" => {
            // The rate-limit panel (D161): one Claude account used on local and devbox (5h
            // used up, weekly at 55%) and one Codex account at 55% of its week. 111: the
            // Claude account's tooltip. 112: the collapsed strip's tiny form.
            app.open(&threads::orchestrator(), None);
            match id {
                "111" => app.ui.limits_tip_force = Some("claude:demo0001".into()),
                "112" => app.ui.sidebar.collapsed = true,
                _ => {}
            }
        }
        "36" => {
            // The sidebar dragged wider.
            app.open(&threads::orchestrator(), None);
            app.ui.sidebar.width = 440.0;
        }
        "34" => {
            // A thread on a stopped WSL distro (D96): readable from the cache; the composer and
            // control row give way to one line with Start it.
            play(app, Scene::Notices);
            let wsl = yaac_protocol::TargetId::new("wsl-ubuntu");
            let key = yaac_core::ThreadKey::new(wsl, yaac_protocol::ThreadId::new("t-wsl-build"));
            let hs = threads::notes();
            app.open(&hs, None);
            if let Some(mut rec) = app.client.state.threads.get(&hs).cloned() {
                rec.summary.id = key.thread.clone();
                rec.summary.title = "Port the build script".into();
                app.client.state.threads.insert(key.clone(), rec);
            }
            if let Some(t) = app.client.state.transcripts.get(&hs).cloned() {
                app.client.state.transcripts.insert(key.clone(), t);
            }
            app.client.state.selection = yaac_core::state::Selection::Thread(key);
        }
        "33" => {
            // Mute, with "Until" chosen and its calendar dropdown open (D112).
            app.open(&threads::fix(), None);
            app.ui.focus_card = None;
            app.apply(vec![crate::app::Action::OpenMute(threads::billing(), false)]);
            if let Some(d) = &mut app.ui.mute {
                d.choice = yaac_core::mute::MuteChoice::Until;
                d.calendar_open = true;
            }
        }
        "42" => {
            app.open(&threads::fix(), None);
            app.ui.focus_card = None;
            app.apply(vec![crate::app::Action::OpenDelete(threads::billing())]);
        }
        "43" => {
            app.open(&threads::fix(), None);
            app.ui.focus_card = None;
            app.apply(vec![crate::app::Action::OpenArchive(threads::billing())]);
        }
        "44" | "46" => {
            // Turns that ended while nobody looked (D114): billing-service and users-service
            // carry the cornflower outline in the sidebar, the docked panel, and the pill.
            app.open(&threads::orchestrator(), None);
            for k in [threads::billing(), threads::users()] {
                app.client.set_view(&k, yaac_core::state::ViewPatch { unseen: Some(true), ..Default::default() });
            }
            app.pump_demo();
            if id == "46" {
                // A parent too, so the pill shows all three relations (D118).
                if let Some(r) = app.client.state.threads.get_mut(&threads::orchestrator()) {
                    r.summary.spawned_by = Some(threads::notes().address());
                }
            }
        }
        "45" => {
            // The ordering menu (D116) over a list whose groups float what needs the user
            // and what finished unseen to the top.
            app.open(&threads::orchestrator(), None);
            app.client.set_view(&threads::billing(), yaac_core::state::ViewPatch { unseen: Some(true), ..Default::default() });
            app.pump_demo();
            app.ui.ordering.order = yaac_core::sidebar::Order::AToZ;
            app.ui.menu = Some(Menu::Ordering);
        }
        "130" | "131" | "132" | "133" => {
            // An orchestrator with 30 related agents (D168), 7 of them alive. 130: the pill —
            // the alive ones, then "more (23)". 131: the card opened from "more", scrolled to
            // the hidden ones. 132: docked, the finished folded under "17 inactive". 133: two
            // more running than the pill has room for: "more (23 · 2 alive)".
            play(app, Scene::ManyAgents);
            app.open(&threads::orchestrator(), None);
            match id {
                "131" => {
                    app.ui.panel = PanelMode::Card;
                    app.ui.card_to_hidden = true;
                }
                "132" => {
                    app.ui.panel = PanelMode::Docked;
                    app.ui.dock_to_fold = true;
                }
                "133" => {
                    for t in ["t-many-cx01", "t-many-cx02"] {
                        let key = yaac_core::key::ThreadKey::new("local", t);
                        if let Some(r) = app.client.state.threads.get_mut(&key) {
                            r.summary.state = yaac_protocol::ThreadState::Running;
                            r.summary.preview = "running cargo test -p auth".into();
                        }
                    }
                }
                _ => {}
            }
        }
        "95" | "96" => {
            // Claude Code's own subagents (D151): search-service, started by the user here,
            // has one YAAC worker (index-rebuild) and two subagents of its own. 95: its docked
            // panel lists all three as Workers; 96: one subagent opened, read-only.
            play(app, Scene::HarnessSubagents);
            if let Some(r) = app.client.state.threads.get_mut(&threads::search()) {
                r.summary.spawned_by = None;
            }
            if id == "95" {
                app.open(&threads::search(), None);
            } else {
                app.open(&threads::search(), None);
                app.open(&threads::subagent_explore(), None);
            }
        }
        "47" => {
            // A thread started by the user with one worker: the pill has one cluster.
            app.open(&threads::search(), None);
            if let Some(r) = app.client.state.threads.get_mut(&threads::search()) {
                r.summary.spawned_by = None;
            }
        }
        "60" | "61" | "62" | "63" => {
            // Pull requests (D119, D120): every badge state in the sidebar and the docked
            // panel — open with failing checks, a draft with +1, merged, create, and one whose
            // target's az is signed out — then the +1 list, the badge menu, and the modal.
            app.open(&threads::orchestrator(), None);
            let keys: Vec<_> = app.client.state.threads.keys().cloned().collect();
            app.client.forge_visible(&keys, STORY_NOW);
            if let Some(h) = &hub {
                h.drain();
            }
            app.pump_demo();
            match id {
                "61" => app.ui.pr_more_force = Some(threads::billing()),
                "62" => app.ui.menu = Some(Menu::PrBadge(threads::billing())),
                "63" => {
                    app.client.open_create_pr(&threads::fix());
                    if let Some(h) = &hub {
                        h.drain();
                    }
                    app.pump_demo();
                }
                _ => {}
            }
        }
        "69" | "70" => {
            // Several rows selected (D138): users-service, billing-service and search-service
            // (Ctrl+click, across three groups) with Fix frame decoder open; the bar under the
            // list. 70: the selection's menu, right-clicked on billing-service.
            use yaac_core::bulk::Click;
            app.open(&threads::fix(), None);
            let order = crate::commander::sidebar_order(app);
            for k in [threads::users(), threads::billing(), threads::search()] {
                app.ui.list_sel.click(&order, &k, Click::Toggle, None);
            }
            app.ui.list_order = order;
            if id == "70" {
                app.ui.menu = Some(Menu::Selection(threads::billing()));
            }
        }
        "90" | "91" | "92" | "93" => {
            // Agents talking to agents (D150). 90: the orchestrator starts a worker, messages
            // billing-service, hears back, and waits — each a line with the other agent as a
            // pill. 91: the message and the reply opened. 92: users-service (Claude Code) and
            // its own subagents. 93: the SendMessage that resumed a subagent opened, with its
            // raw call (the JSON the owner saw) behind "Raw call".
            play(app, Scene::AgentCalls);
            let (k, first) = if matches!(id, "90" | "91") { (threads::orchestrator(), "ac1") } else { (threads::users(), "ua1") };
            app.open(&k, None);
            // users-service's turn is finished: opened again (D156) so its calls show.
            if matches!(id, "92" | "93") {
                app.ui.worked.toggle(&k, &"u-t4".into());
            }
            match id {
                "91" => {
                    app.ui.expansion.toggle_line(&k, &"ac4".into());
                    app.ui.expansion.toggle_line(&k, &"ac5".into());
                }
                "93" => {
                    app.ui.expansion.toggle_line(&k, &"ua3".into());
                    app.ui.expansion.toggle_raw(&k, &"ua3".into());
                }
                _ => {}
            }
            app.ui.scroll_to = Some(if id == "93" { "ua3" } else { first }.into());
            app.ui.scroll_end = false;
        }
        "76" | "77" => {
            // Claude Code's markup made readable (D141): a pasted log (collapsed), a slash
            // command chip, a reminder behind "Context", notices for a subagent's report and
            // background tasks. 75: the paste and the context opened, one notice expanded.
            play(app, Scene::HarnessMarkup);
            let k = threads::users();
            app.open(&k, None);
            // The notices are in a finished turn: opened again (D156) so they show.
            app.ui.worked.toggle(&k, &"u-t3".into());
            if id == "77" {
                app.ui.expansion.toggle_pasted(&k, &"um1".into(), 1);
                app.ui.expansion.toggle_context(&k, &"um1".into());
                app.ui.expansion.toggle_line(&k, &"um6".into());
            }
            app.ui.scroll_to = Some("um1".into());
            app.ui.scroll_end = false;
        }
        "71" => {
            // The bulk Archive confirmation (D111, D138): five chosen, two can be archived;
            // two are running and one is in use in a terminal.
            app.open(&threads::fix(), None);
            let keys = [threads::users(), threads::billing(), threads::search(), threads::orchestrator(), threads::handshake()];
            app.ui.bulk_archive = Some(yaac_core::bulk::BulkArchiveDialog::new(&app.client.state, &keys, STORY_NOW));
        }
        "72" => {
            // A group header's menu (D138): "api · devbox", opened from its dots.
            app.open(&threads::fix(), None);
            if let Some(id) = yaac_core::sidebar::section_of(&app.client.state, app.ui.grouping, &threads::billing()) {
                app.ui.menu = Some(Menu::Header(id));
            }
        }
        "73" => {
            // After a bulk Archive: one report of what did not simply succeed (D138).
            use yaac_core::bulk::Outcome;
            app.open(&threads::fix(), None);
            app.client.bulk_report = yaac_core::bulk::archive_report(&[
                ("users-service".into(), Outcome::Ok),
                ("Release notes".into(), Outcome::Failed("staging is offline".into())),
                ("billing-service".into(), Outcome::Uncertain),
            ]);
        }
        "74" | "75" => {
            // D140: the holder will not let go (74), or the thread was just taken over (75).
            let k = threads::handshake();
            app.open(&k, None);
            app.pump_demo();
            if let Some(h) = &hub {
                if id == "74" {
                    h.block_take_over(&k, Some("Claude Code runs it as a background session; it cannot be continued elsewhere until that session is stopped there (claude stop 7f3a2c91)"));
                    h.drain();
                } else {
                    app.client.take_over(&k);
                }
            }
            app.pump_demo();
        }
        "78" | "79" => {
            // The Move dialog (D144). 78: Fix frame decoder, whose harness (Codex) moves its
            // folder too. 79: billing-service, whose harness (Claude Code) cannot: only where
            // YAAC lists it changes.
            app.open(&threads::fix(), None);
            let key = if id == "78" { threads::fix() } else { threads::billing() };
            app.ui.move_dialog = Some(yaac_core::moving::MoveDialog::new(&app.client.state, &[key]));
        }
        "80" | "81" => {
            // A row dragged to another group (D144). 76: billing-service over "yaac" on its own
            // machine (the group is outlined; "Move to yaac"). 77: search-service (gpu-box) over
            // the same group: refused, "A thread cannot move to another machine".
            app.open(&threads::fix(), None);
            let key = if id == "80" { threads::billing() } else { threads::search() };
            let over = yaac_core::sidebar::section_of(&app.client.state, app.ui.grouping, &threads::fix());
            app.ui.drag = Some(crate::app::DragRows { keys: vec![key], force_over: over });
        }
        "82" => {
            // After a move that needed no confirmation: the bar's notice with Undo (D144).
            app.open(&threads::fix(), None);
            let keys = [threads::billing(), threads::users()];
            app.client.move_notice = Some(yaac_core::moving::notice(&app.client.state, &keys, "yaac"));
        }
        "68" => {
            // Settings → About: version, build, links, components (D137).
            app.ui.settings = true;
            app.ui.settings_page = Page::About;
        }
        "49" => {
            // Settings → Window: the tray option and the renderer, with the line about the
            // renderer this very window runs on (D127, D128).
            app.ui.settings = true;
            app.ui.settings_page = Page::Window;
        }
        "64" | "65" | "66" => {
            // The Commander (D130): over the orchestrator, every group (64); typed "agent" (65);
            // Change model… continuing inside it as the model list (66).
            use yaac_core::commands::{Cmd, StepKind};
            app.open(&threads::orchestrator(), None);
            let mut c = crate::commander::CommanderUi::at(if id == "66" { Some((Cmd::Model, StepKind::Model)) } else { None }, None);
            if id == "65" {
                c.query = "agent".into();
            }
            if id == "64" {
                // Used a moment ago, so they lead under Recent.
                app.ui.recent = vec!["running-only".into(), "fork".into()];
            }
            app.ui.commander = Some(c);
        }
        "67" => {
            // Settings → Keyboard (D131): Ctrl+K was pressed for the Commander, which Search
            // threads has; the row asks to replace or cancel. Rename was moved to F6.
            use yaac_core::commands::Cmd;
            app.ui.settings = true;
            app.ui.settings_page = Page::Keyboard;
            app.ui.bindings.assign(Cmd::Rename, yaac_core::keys::Chord::parse("F6").unwrap_or_else(|| yaac_core::keys::Chord::key(false, false, false, "F6")), false);
            app.ui.set.keys.conflict = Some((Cmd::Commander, yaac_core::keys::Chord::key(true, false, false, "K"), Cmd::SearchThreads));
        }
        "50" => {
            // Fast mode (D125): a worker running in it. The lightning toggle under the
            // composer is on; its spinner is yellow in the header, the sidebar and the panel.
            app.open(&threads::search(), None);
        }
        "48" => {
            // The docked panel at its narrowest: titles ellipsised, every mark kept (D115).
            app.open(&threads::orchestrator(), None);
            app.ui.agents_width = yaac_core::prefs::AGENTS_MIN_WIDTH;
        }
        "32" => {
            // The Add SSH server form with realistic long values: every field usable.
            app.ui.settings = true;
            app.ui.settings_page = Page::Targets;
            let mut panel = SshPanel::new(STORY_NOW, None);
            panel.name = "build-cluster-west".into();
            panel.host = "build-runner-07.eu-west.example.org".into();
            panel.user = "deploy".into();
            panel.port = "2222".into();
            app.ui.set.ssh = Some(panel);
        }
        "31" => {
            // Several places visited; the back button's list is open.
            for k in [threads::billing(), threads::index(), threads::ci(), threads::fix()] {
                app.open(&k, None);
            }
            app.ui.settings_page = Page::Providers;
            app.ui.history.visit(yaac_core::history::Place::Settings(Page::Providers), None);
            app.ui.history.visit(yaac_core::history::Place::Thread(threads::orchestrator()), None);
            app.open(&threads::orchestrator(), None);
            app.ui.nav_menu = Some(false);
            app.ui.nav_sel = 0;
        }
        _ => {}
    }
}

/// `!command` (P346): the composer in `!` mode idle (140) and while a turn runs (141, with a
/// queued command in the strip), the normal composer after leaving it (142), and the command
/// block running (143) and finished with folded output, then the answer (144).
fn bang(app: &mut YaacApp, id: &str, play: &dyn Fn(&mut YaacApp, Scene)) {
    use yaac_protocol::{Capability, InputPart, Item, ItemBody, ItemId, ItemStatus, TextPart, TextRole, ToolCallItem, ToolCallSource, ToolKind, TurnId, UserMessageItem};
    let running = id == "141";
    if running {
        play(app, Scene::QueueWhileRunning);
    }
    let k = if running { threads::billing() } else { threads::users() };
    app.open(&k, None);
    let Some(t) = app.client.state.transcripts.get_mut(&k) else { return };
    if let Some(e) = t.effective.as_mut()
        && !e.capabilities.contains(&Capability::UserShellCommand)
    {
        e.capabilities.push(Capability::UserShellCommand);
    }
    let at = |s: i64| Some(yaac_core::time::timestamp(STORY_NOW - s));
    let mk = |id: &str, turn: &str, status: ItemStatus, body: ItemBody, ago: i64| Item { id: ItemId::new(id), turn_id: TurnId::new(turn), status, started_at: at(ago), completed_at: at(ago - 1), body };
    let bang_msg = |text: &str| ItemBody::UserMessage(UserMessageItem { parts: vec![InputPart::Text(TextPart { text: text.into(), role: Some(TextRole::Command), label: None })], op_id: None });
    let call = |title: &str, output: &str, exit: Option<i32>, ms: Option<u64>| {
        ItemBody::ToolCall(ToolCallItem {
            tool: ToolKind::Command,
            title: title.into(),
            input: None,
            output: output.into(),
            output_truncated: false,
            output_bytes: None,
            exit_code: exit,
            cwd: Some(yaac_protocol::TargetPath::new("/home/dev/users-service")),
            agent_call: None,
            skill: None,
            images: Vec::new(),
            source: Some(ToolCallSource::UserShell),
            duration_ms: ms,
        })
    };
    let mut rev = 1_000_000;
    let mut add = |t: &mut yaac_core::transcript::Transcript, item: Item| {
        rev += 1;
        t.upsert(item, rev);
    };
    match id {
        "143" => {
            let out = "   Compiling users v0.4.2 (/home/dev/users-service)\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.81s\n     Running unittests src/lib.rs\n\nrunning 38 tests\ntest auth::tests::rejects_expired_token ... ok\ntest auth::tests::refreshes_before_expiry ... ok\n";
            add(t, mk("bang-u", "bang-t", ItemStatus::Completed, bang_msg("!cargo test -p users"), 8));
            add(t, mk("bang-c", "bang-t", ItemStatus::InProgress, call("cargo test -p users", out, None, None), 7));
            t.active_turn = Some(TurnId::new("bang-t"));
        }
        "144" => {
            let mut out = String::from("   Compiling users v0.4.2 (/home/dev/users-service)\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.81s\n     Running unittests src/lib.rs\n\nrunning 38 tests\n");
            for name in ["rejects_expired_token", "refreshes_before_expiry", "hashes_with_argon2", "locks_after_five_failures", "unlocks_after_cooldown", "keeps_session_on_refresh", "drops_session_on_logout", "normalises_email_case", "rejects_duplicate_email", "lists_users_paginated", "filters_by_role", "sorts_by_created_at", "exports_csv", "imports_csv_with_header", "rejects_csv_without_header", "audits_role_change", "audits_password_reset"] {
                out.push_str(&format!("test users::tests::{name} ... ok\n"));
            }
            out.push_str("test users::tests::sends_welcome_email ... FAILED\n\nfailures:\n    users::tests::sends_welcome_email\n\ntest result: FAILED. 37 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.42s\n");
            add(t, mk("bang-u", "bang-t", ItemStatus::Completed, bang_msg("!cargo test -p users"), 60));
            add(t, mk("bang-c", "bang-t", ItemStatus::Completed, call("cargo test -p users", &out, Some(101), Some(6_240)), 59));
            add(t, mk("bang-a", "bang-t", ItemStatus::Completed, ItemBody::AgentMessage(yaac_protocol::AgentMessageItem { text: "One test fails: `sends_welcome_email` expects the mailer stub to be called once, but the signup path now queues the email instead of sending it. I can update the test to assert on the queue, or make signup send synchronously again — which do you prefer?".into() }), 50));
        }
        _ => {}
    }
    if running {
        let entry = yaac_protocol::QueuedMessage { queue_id: "bang-q".into(), input: vec![InputPart::text("!cargo clippy -p billing")], queued_at: yaac_core::time::timestamp(STORY_NOW - 5), model: None, effort: None, mode: None, fast: None, ultrafast: None, shell_command: Some("cargo clippy -p billing".into()) };
        t.queued.push(entry);
    }
    let draft = match id {
        "140" => "!cargo test -p users -- --nocapture",
        "141" => "!git status --short",
        "142" => "cargo test -p users -- --nocapture",
        _ => "",
    };
    app.client.state.drafts.entry(k).or_default().text = draft.into();
    if matches!(id, "140" | "141" | "142") {
        app.ui.focus_composer = true;
    }
}

/// The side pane on Migrate auth module · orchestrator: New tab, Files, the app.rs diff and
/// a page; `scene` is `<wide|narrow>-<browser|servers|empty|files|diff|many-tabs|full-chat|full-browser>`.
fn pane(app: &mut YaacApp, scene: &str) {
    use crate::file_pane::TabKind;
    use yaac_protocol::{FsTreeEntry, FsTreeEntryKind, FsTreeResult, GitFileStatus, PreviewPort};
    let (width, name) = scene.split_once('-').unwrap_or((scene, ""));
    let key = threads::orchestrator();
    app.open(&key, None);
    if let Some(t) = app.client.state.targets.iter_mut().find(|t| t.entry.id == key.target) {
        t.entry.state.daemon = Some(yaac_protocol::attach::DaemonSummary {
            build: "demo".into(),
            older: false,
            schema_hash_matches: true,
            can_update: false,
            port_forward: true,
            file_view: true,
            terminal: false,
            terminal_unavailable_reason: None,
        });
    }
    app.preview_ui.demo_page = true;
    let b = &mut app.browsers;
    let many = name == "many-tabs";
    let new_tab = if many { None } else { Some(b.insert(&key)) };
    let files = b.insert(&key);
    let diff = b.insert(&key);
    let page = b.insert(&key);
    {
        let tab = b.tab_mut(&key, files).unwrap();
        tab.kind = TabKind::Files;
        tab.title = "Files".into();
        let tab = b.tab_mut(&key, diff).unwrap();
        tab.kind = TabKind::Diff("src/app.rs".into());
        tab.title = "app.rs".into();
        let old = "use crate::Session;\n\npub fn expires_in(session: &Session) -> u64 {\n    let timeout = 60;\n    session.created_at + timeout\n}\n";
        let input = yaac_code::DiffInput {
            path: "src/app.rs".into(),
            old_path: None,
            old: Some(old.into()),
            new: Some(old.replace("= 60;", "= 300;")),
            notice: None,
        };
        tab.work = yaac_code::spawn_diff(input, || {}).ok().map(crate::file_pane::Work::Diff);
        tab.loading = true;
        let tab = b.tab_mut(&key, page).unwrap();
        tab.url = "http://localhost:5173/".into();
        tab.title = "Project documentation".into();
        tab.back = true;
    }
    let mut active = match name {
        "files" => files,
        "diff" => diff,
        "browser" | "full-browser" | "full-chat" => page,
        _ => new_tab.unwrap_or(page),
    };
    if many {
        for n in ["lib.rs", "client.rs", "session.rs", "auth.rs", "config.rs", "main.rs", "schema.rs"] {
            let id = b.insert(&key);
            let tab = b.tab_mut(&key, id).unwrap();
            tab.kind = TabKind::File(format!("src/{n}"));
            tab.title = n.into();
            tab.document = Some(std::sync::Arc::new(crate::file_pane::Document::Notice("Loading…".into())));
        }
        active = b.insert(&key);
    }
    let entry = |name: &str, dir: bool, git: Option<GitFileStatus>, changed: bool| FsTreeEntry {
        name: name.into(),
        kind: if dir { FsTreeEntryKind::Dir } else { FsTreeEntryKind::File },
        size: None,
        mtime: None,
        git,
        ignored: false,
        changed,
    };
    let area = b.areas.get_mut(&key).unwrap();
    area.tree.folders.insert(String::new(), FsTreeResult {
        entries: vec![
            entry("assets", true, None, false),
            entry("docs", true, None, false),
            entry("src", true, None, true),
            entry("tests", true, None, true),
            entry(".gitignore", false, None, false),
            entry("Cargo.toml", false, None, false),
            entry("README.md", false, None, false),
        ],
        repo: true,
        truncated: false,
    });
    area.tree.folders.insert("src".into(), FsTreeResult {
        entries: vec![
            entry("app.rs", false, Some(GitFileStatus::Modified), false),
            entry("auth.rs", false, Some(GitFileStatus::Added), false),
            entry("lib.rs", false, None, false),
            entry("main.rs", false, None, false),
        ],
        repo: true,
        truncated: false,
    });
    area.tree.expanded.insert("src".into());
    if name != "empty" {
        let folder = app.client.state.project_of(&key).map(|p| p.folder.as_str().to_owned()).unwrap_or_else(|| "/work/auth".into());
        area.ports = [(5173, "vite", format!("{folder}/web")), (8000, "python", folder), (9000, "node", "/opt/services/admin".into())]
            .into_iter()
            .map(|(port, command, cwd)| PreviewPort { port, host: "127.0.0.1".into(), pid: None, command: Some(command.into()), cwd: Some(cwd) })
            .collect();
    }
    area.ratio = match width {
        "wide" => 0.32,
        "docked" => 0.72,
        _ => 0.67,
    };
    if width == "docked" {
        app.ui.panel = PanelMode::Docked;
        if name == "split" {
            active = page;
        }
    }
    b.select(&key, active);
    if name.starts_with("full") {
        b.set_full_screen(&key, true);
        if name == "full-chat" {
            b.select_chat(&key);
        }
    }
    b.hold_requests(&app.client, &key);
}

/// A terminal scene: a session fed one of yaac-term's recorded fixtures (the same bytes the WPF
/// renderer replays), pinned to the fixture's 69 × 28 grid.
fn terminal(app: &mut YaacApp, scene: &str) {
    use crate::terminal::SessionState;
    let key = threads::orchestrator();
    app.open(&key, None);
    if let Some(t) = app.client.state.targets.iter_mut().find(|t| t.entry.id == key.target) {
        t.entry.state.daemon = Some(yaac_protocol::attach::DaemonSummary {
            build: "demo".into(),
            older: false,
            schema_hash_matches: true,
            can_update: false,
            port_forward: true,
            file_view: true,
            terminal: true,
            terminal_unavailable_reason: None,
        });
    }
    let fixture = |name: &str| -> &'static [u8] {
        match name {
            "vim" => include_bytes!("../../yaac-term/fixtures/vim.vt"),
            "vim-light" => include_bytes!("../../yaac-term/fixtures/vim-light.vt"),
            "htop" => include_bytes!("../../yaac-term/fixtures/htop.vt"),
            "powershell-prompt" => include_bytes!("../../yaac-term/fixtures/powershell-prompt.vt"),
            "search" => include_bytes!("../../yaac-term/fixtures/search.vt"),
            "exited" => include_bytes!("../../yaac-term/fixtures/exited.vt"),
            _ => include_bytes!("../../yaac-term/fixtures/bash-prompt.vt"),
        }
    };
    let base = scene.trim_end_matches("-full");
    let (stream, shell) = match base {
        "powershell-prompt" => ("powershell-prompt", ("pwsh", "PowerShell")),
        "selection" | "panel-open" | "panel-maximised" | "panel-fullchat" | "panel-rename" => ("bash-prompt", ("/bin/bash", "bash")),
        // vim chooses its colours for the background it is told (it asks the terminal, OSC 11).
        "vim" if app.ui.theme == crate::theme::ThemeChoice::Light => ("vim-light", ("/bin/bash", "bash")),
        other => (other, ("/bin/bash", "bash")),
    };
    let cwd = "/home/dev/work/auth".to_owned();
    let shell = Some(yaac_protocol::TerminalShell { path: shell.0.into(), name: shell.1.into() });
    let terms = &mut app.browsers.terms;
    let id = terms.insert(&key, shell, cwd);
    terms.detach_demo(id);
    let panel = scene.starts_with("panel-");
    let s = terms.get_mut(id).expect("just inserted");
    if !panel {
        s.pinned = Some((69, 28));
        s.term.resize(69, 28);
    }
    s.term.feed(fixture(stream));
    while !s.term.idle() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    match scene {
        "selection" => {
            s.term.select_start(8, 0, yaac_term::CellSide::Left, yaac_term::SelectKind::Simple);
            s.term.select_to(14, 8, yaac_term::CellSide::Right);
        }
        "search" => {
            s.term.search("denied");
            s.search = Some("denied".into());
        }
        "exited" => s.state = SessionState::Exited(3),
        _ => {}
    }
    if scene == "panel-rename" {
        // A second terminal, named; the first being renamed in place (P343).
        let other = terms.insert(&key, Some(yaac_protocol::TerminalShell { path: "/usr/bin/zsh".into(), name: "zsh".into() }), "/home/dev/work/auth".into());
        terms.detach_demo(other);
        if let Some(o) = terms.get_mut(other) {
            o.name = Some("dev server".into());
        }
        terms.panels.entry(key.clone()).or_default().tabs.push(other);
    }
    if panel {
        let p = terms.panels.entry(key.clone()).or_default();
        p.tabs.push(id);
        p.active = id;
        p.open = true;
        p.maximised = scene == "panel-maximised";
        if scene == "panel-fullchat" {
            let b = &mut app.browsers;
            let tab = b.insert(&key);
            if let Some(t) = b.tab_mut(&key, tab) {
                t.url = "http://localhost:5173/".into();
                t.title = "Project documentation".into();
            }
            b.set_full_screen(&key, true);
            b.select_chat(&key);
            app.preview_ui.demo_page = true;
            b.hold_requests(&app.client, &key);
        }
    } else {
        let b = &mut app.browsers;
        let tab = b.terminal_tab(&key, id);
        if let Some(area) = b.areas.get_mut(&key) {
            area.ratio = 0.32;
        }
        if scene.ends_with("-full") {
            b.set_full_screen(&key, true);
        }
        b.select(&key, tab);
        b.hold_requests(&app.client, &key);
    }
    app.browsers.terms.focus = Some(id);
    if let Some(s) = app.browsers.terms.get_mut(id) {
        s.shown_focused = true;
    }
    if scene == "panel-rename" {
        app.browsers.terms.begin_rename(id);
        if let Some(r) = app.browsers.terms.renaming.as_mut() {
            r.text = "build watch".into();
        }
    }
    // Opening the thread put the caret in its composer; the terminal has it here.
    app.ui.focus_composer = false;
}

fn write_png(path: &std::path::Path, img: &ColorImage) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let w = std::io::BufWriter::new(file);
    let [width, height] = img.size;
    let mut enc = png::Encoder::new(w, u32::try_from(width).map_err(|e| e.to_string())?, u32::try_from(height).map_err(|e| e.to_string())?);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    writer.write_image_data(&bytes).map_err(|e| e.to_string())
}
