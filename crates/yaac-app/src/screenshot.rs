//! `yaac --demo --screenshots <dir>`: render the main-shell screens in both themes through
//! the real window (eframe's own viewport screenshot), one PNG each, then exit.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, ColorImage};
use yaac_core::agents::PanelMode;
use yaac_core::demo::{DemoHub, Scenario, Scene, threads};

use crate::app::{Menu, YaacApp};
use crate::theme::ThemeChoice;

/// The screens, by brief number and file name.
pub(crate) const SCREENS: &[(&str, &str)] = &[
    ("01", "approval"),
    ("02", "orchestrator"),
    ("03", "worker-side"),
    ("04", "blocked-worker"),
    ("05", "request-states"),
    ("09", "first-message"),
    ("10", "in-use"),
    ("12", "forked-thread"),
    ("19", "notices"),
    ("20", "budget"),
    ("23", "queued-arrives"),
    ("24", "destination"),
    ("25", "permission"),
    ("26", "mute-archive"),
    ("27", "agents-pill"),
    ("28", "agents-card"),
    ("29", "create-project"),
];

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
            setup(app, id);
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
    let scenario = if id == "09" { Scenario::Fresh } else { Scenario::Fleet };
    app.reset_demo(scenario);
    let hub = app.demo.clone();
    let play = |app: &mut YaacApp, s: Scene| {
        if let Some(h) = &hub {
            h.play(s);
            h.drain();
        }
        app.pump_demo();
    };
    let docked = matches!(id, "02" | "03" | "04" | "20" | "23" | "24");
    app.ui.panel = if docked { PanelMode::Docked } else { PanelMode::Pill };
    match id {
        "01" | "10" => {
            let k = if id == "01" { threads::fix() } else { threads::handshake() };
            app.open(&k, None);
        }
        "02" | "27" | "28" => {
            let k = threads::orchestrator();
            app.open(&k, None);
            if id == "02" {
                app.ui.expansion.toggle_line(&k, &"o4".into());
            }
            if id == "28" {
                app.ui.panel = PanelMode::Card;
            }
        }
        "03" => app.open(&threads::billing(), None),
        "04" => app.open(&threads::index(), None),
        "05" => {
            play(app, Scene::RequestStates);
            app.open(&threads::fix(), None);
        }
        "09" => {
            if let Some(d) = app.client.state.new_threads.first_mut() {
                d.draft.text = "# Investigate frame parsing\nReproduce the **last-frame** drop in `decode()`.".into();
                let id = d.id;
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
        "12" => {
            let fix = threads::fix();
            app.open(&fix, None);
            // Fork to another instance on the same machine (a handoff).
            let own = app.client.state.summary(&fix).map(|s| s.instance.clone());
            let dest = app.client.state.target(&fix.target).and_then(|t| t.providers.iter().find(|p| Some(&p.instance) != own.as_ref() && p.store.key != app.client.state.provider_of(&fix).and_then(|o| o.store.key.clone())).map(|p| p.instance.clone()));
            if let Some(d) = dest {
                app.client.fork(&fix, &d);
            }
            app.pump_demo();
            if let Some(k) = app.client.state.selected_thread().cloned() {
                app.open(&k, None);
            }
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
            app.ui.menu = Some(Menu::Context(threads::billing()));
        }
        "29" => {
            app.open(&threads::fix(), None);
            let cp = crate::sidebar::create_project_for(app, &"devbox".into(), "~/scratch/spike-7");
            app.ui.create_project = Some(cp);
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
