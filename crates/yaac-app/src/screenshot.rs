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
    ("12", "forked-thread"),
    ("13", "settings-targets"),
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
                let file = self.dir.join(format!("{id}-{name}-{}.png", if dark { "dark" } else { "light" }));
                let img = if id == "21" { crop(&img, NARROW_SIZE[0] * 1.25, NARROW_SIZE[1] * 1.25) } else { (*img).clone() };
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
            ctx.set_zoom_factor(1.25);
            // Code blocks show their copy button as on hover.
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique(crate::markdown::FORCE_COPY_BUTTONS), true));
            setup(app, id);
            // This display cannot resize the window reliably, so screen 21 lays the shell out in
            // 960 × 720 points and the picture is cropped to it.
            app.ui.layout_size = (id == "21").then(|| egui::vec2(NARROW_SIZE[0], NARROW_SIZE[1]));
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
    app.host_windows = false;
    let hub = app.demo.clone();
    let play = |app: &mut YaacApp, s: Scene| {
        if let Some(h) = &hub {
            h.play(s);
            h.drain();
        }
        app.pump_demo();
    };
    let docked = matches!(id, "02" | "03" | "04" | "20" | "21" | "23" | "24");
    app.ui.panel = if docked { PanelMode::Docked } else { PanelMode::Pill };
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
        "03" => {
            // A file change: one diff expanded in place, the others collapsed, and the turn's
            // summary line at the end.
            let k = threads::billing();
            app.open(&k, None);
            app.ui.expansion.toggle_file(&k, &"b3".into(), 0);
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
            app.new_thread();
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
            app.ui.fork = Some(ForkDialog { key: fix, choice: dest, preview_open: true });
        }
        "12" => {
            let fix = threads::fix();
            app.open(&fix, None);
            app.client.fork(&fix, &ForkChoice { instance: "claude".into(), model: None });
            app.pump_demo();
            if let Some(k) = app.client.state.selected_thread().cloned() {
                app.open(&k, None);
            }
        }
        "13" => {
            play(app, Scene::WslConnections);
            app.host_windows = true;
            app.client.state.wsl_on = true;
            app.ui.settings = true;
            app.ui.settings_page = Page::Targets;
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
            app.ui.needs_popup = true;
            app.ui.needs_sel = 0;
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
        "33" => {
            // Mute, with "For 2 hours" chosen: the moment it ends shows beneath, in local time.
            app.open(&threads::fix(), None);
            app.ui.focus_card = None;
            app.apply(vec![crate::app::Action::OpenMute(threads::billing(), false)]);
            if let Some(d) = &mut app.ui.mute {
                d.choice = yaac_core::mute::MuteChoice::For;
            }
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
