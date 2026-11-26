//! The same miniature chat is drawn in both Appearance grids and in a tool preview.
use super::{
    app::YaacApp,
    theme::{Palette, ThemeChoice},
};
use crate::theme_capture::{Capture, ThemeInput};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use egui::{self, Color32};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::Read,
    sync::{Arc, mpsc},
};
use yaac_core::{
    display::Renderer,
    settings::Page,
    themes::{Base, MAX_BYTES, Theme},
};
use yaac_protocol::{
    ThemeAction, ThemeCallResult,
    rpc::{ErrorCode, ErrorObject, RpcId},
};

impl Palette {
    pub(crate) fn custom(theme: &Theme) -> Self {
        let mut p = if theme.base == Base::Light {
            Self::LIGHT
        } else {
            Self::DARK
        };
        macro_rules! overrides { ($($field:ident),*) => {$(
            if let Some(rgb) = theme.colors.get(stringify!($field)).and_then(|s| s.strip_prefix('#')).and_then(|s| u32::from_str_radix(s,16).ok()) {
                let [_,r,g,b] = rgb.to_be_bytes(); p.$field = Color32::from_rgb(r,g,b);
            }
        )*}; }
        overrides!(
            bg, panel, raised, faint, border, text, text2, primary, on_primary, selected, needs,
            done, failed, user_bg, fast, ultrafast, shell
        );
        p.running = p.text;
        p.code_bg = p.faint;
        p
    }
    pub(crate) fn wpf_colors(self) -> Value {
        let rgb = |c: Color32| u32::from_be_bytes([0, c.r(), c.g(), c.b()]);
        json!({"dark":self.dark,"YaacCanvas":rgb(self.bg),"YaacPanel":rgb(self.panel),"YaacCard":rgb(self.raised),
        "YaacRaised":rgb(self.raised),"YaacInput":rgb(self.faint),"YaacRule":rgb(self.border),"YaacText":rgb(self.text),
        "YaacMuted":rgb(self.text2),"YaacAccent":rgb(self.primary),"YaacOnAccent":rgb(self.on_primary),"YaacLink":rgb(self.text),
        "YaacSelected":rgb(self.selected),"YaacHover":rgb(self.faint),"YaacWarning":rgb(self.needs),"YaacFailure":rgb(self.failed),
        "YaacSuccess":rgb(self.done),"YaacUser":rgb(self.user_bg),"YaacFast":rgb(self.fast),"YaacUltrafast":rgb(self.ultrafast),"YaacShell":rgb(self.shell)})
    }
}

struct Cached {
    capture: Capture,
    texture: Option<egui::TextureHandle>,
}

type Completed = Result<Cached, String>;
type CaptureJob = (String, mpsc::Receiver<Result<Capture, String>>);

#[derive(Default)]
pub(crate) struct Import {
    file: Option<mpsc::Receiver<Result<Option<Theme>, String>>>,
    previews: Vec<(RpcId, String, ThemeCallResult)>,
    queued: VecDeque<(String, ThemeInput, bool)>,
    job: Option<CaptureJob>,
    cache: VecDeque<(String, Completed)>,
    pub(crate) error: Option<String>,
}

fn key(input: &ThemeInput, wpf: bool) -> String {
    let theme = match input {
        ThemeInput::BuiltIn(choice) => choice.id().to_owned(),
        ThemeInput::Custom(theme) => serde_json::to_string(theme).expect("serializable theme"),
    };
    format!("{}:{theme}", if wpf { "wpf" } else { "egui" })
}

impl Import {
    fn cached(&self, key: &str) -> Option<&Completed> {
        self.cache
            .iter()
            .find_map(|(name, preview)| (name == key).then_some(preview))
    }

    fn schedule(&mut self, input: ThemeInput, wpf: bool, priority: bool) -> String {
        let key = key(&input, wpf);
        if self.cached(&key).is_some()
            || self
                .job
                .as_ref()
                .is_some_and(|(running, _)| *running == key)
        {
            return key;
        }
        if priority {
            self.queued.retain(|(queued, _, _)| *queued != key);
            self.queued.push_front((key.clone(), input, wpf));
        } else if !self.queued.iter().any(|(queued, _, _)| *queued == key) {
            self.queued.push_back((key.clone(), input, wpf));
        }
        key
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if let Some((name, rx)) = self.job.take() {
            match rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => self.job = Some((name, rx)),
                result => {
                    let completed = result
                        .unwrap_or_else(|_| Err("Preview renderer stopped.".into()))
                        .map(|capture| {
                            let texture = name.starts_with("egui:").then(|| {
                                ctx.load_texture(
                                    "theme screenshot",
                                    capture.image.clone(),
                                    egui::TextureOptions::LINEAR,
                                )
                            });
                            Cached { capture, texture }
                        });
                    self.cache.push_back((name, completed));
                }
            }
        }
        if self.job.is_none()
            && let Some((name, input, wpf)) = self.queued.pop_front()
        {
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            self.job = Some((name, rx));
            std::thread::spawn(move || {
                let _ = tx.send(crate::theme_capture::generate(input, wpf));
                ctx.request_repaint();
            });
        }
    }

    fn prune(&mut self, catalog: &yaac_core::themes::Catalog, wpf: bool) {
        let valid: Vec<_> = catalog
            .entries
            .values()
            .map(|theme| key(&ThemeInput::Custom(theme.clone()), wpf))
            .collect();
        let keep = |name: &String| {
            name.ends_with(":dark")
                || name.ends_with(":light")
                || valid.contains(name)
                || self.previews.iter().any(|(_, request, _)| request == name)
        };
        self.cache.retain(|(name, _)| keep(name));
        self.queued.retain(|(name, _, _)| keep(name));
    }

    pub(crate) fn card(&self, ui: &mut egui::Ui, theme: &Theme) -> egui::Response {
        let key = key(&ThemeInput::Custom(theme.clone()), false);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(264.0, 144.0), egui::Sense::click());
        if let Some(Ok(cached)) = self.cached(&key)
            && let Some(texture) = &cached.texture
        {
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            let pending = self.cached(&key).is_none();
            ui.painter()
                .rect_filled(rect, 4.0, ui.visuals().faint_bg_color);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if pending {
                    "Rendering preview…"
                } else {
                    "Preview unavailable"
                },
                egui::FontId::proportional(13.0),
                ui.visuals().weak_text_color(),
            );
            if let Some(Err(error)) = self.cached(&key) {
                return response.on_hover_text(error);
            }
        }
        response
    }

    pub(crate) fn wpf_card(&self, theme: &Theme, selected: bool) -> Value {
        let key = key(&ThemeInput::Custom(theme.clone()), true);
        let preview = self.cached(&key);
        json!({
            "name": theme.name, "selected": selected,
            "imagePath": preview.and_then(|p| p.as_ref().ok()).map(|p| p.capture.tile.to_string_lossy()),
            "status": match preview { Some(Ok(_)) => "", Some(Err(_)) => "Preview unavailable", None => "Rendering preview…" },
            "error": preview.and_then(|p| p.as_ref().err()),
        })
    }
}

impl YaacApp {
    pub(crate) fn import_theme(&mut self) {
        if self.theme_import.file.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.theme_import.file = Some(rx);
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let Some(path) = crate::platform::pick_file(
                    None,
                    Some(crate::platform::Filter { name: "YAAC theme", extensions: &["json"] }),
                ) else {
                    return Ok(None);
                };
                let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                    return Err("Choose a regular JSON file.".into());
                }
                let mut bytes = Vec::new();
                file.take(MAX_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                Theme::parse(&bytes).map(Some)
            })();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub(crate) fn select_theme(&mut self, name: &str) -> Result<(), String> {
        self.ui.theme = match name {
            "dark" => ThemeChoice::Dark,
            "light" => ThemeChoice::Light,
            "system" => ThemeChoice::System,
            _ if self.ui.themes.entries.contains_key(name) => {
                Arc::make_mut(&mut self.ui.themes).selected = name.to_owned();
                ThemeChoice::Custom
            }
            _ => return Err(format!("Unknown theme {name}.")),
        };
        self.ctx.request_repaint();
        Ok(())
    }

    fn theme_result(&self) -> ThemeCallResult {
        let builtins = ["system", "dark", "light"];
        let selected = match self.ui.theme {
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
            ThemeChoice::System => "system",
            ThemeChoice::Custom => &self.ui.themes.selected,
        };
        ThemeCallResult {
            selected: selected.into(),
            themes: builtins
                .into_iter()
                .map(String::from)
                .chain(self.ui.themes.entries.keys().cloned())
                .collect(),
            png: None,
        }
    }

    fn handle_theme(
        &mut self,
        id: &RpcId,
        action: ThemeAction,
    ) -> Result<Option<ThemeCallResult>, String> {
        match action {
            ThemeAction::List(_) => {}
            ThemeAction::Select(p) => self.select_theme(&p.name)?,
            ThemeAction::Import(p) => Arc::make_mut(&mut self.ui.themes)
                .import(Theme::parse(p.json.as_bytes())?, p.overwrite)?,
            ThemeAction::Preview(p) => {
                let input = match p.name.as_str() {
                    "dark" => ThemeInput::BuiltIn(ThemeChoice::Dark),
                    "light" => ThemeInput::BuiltIn(ThemeChoice::Light),
                    "system" => ThemeInput::BuiltIn(
                        if self.ctx.system_theme() == Some(egui::Theme::Light) {
                            ThemeChoice::Light
                        } else {
                            ThemeChoice::Dark
                        },
                    ),
                    name => ThemeInput::Custom(
                        self.ui
                            .themes
                            .entries
                            .get(name)
                            .cloned()
                            .ok_or("Unknown theme.")?,
                    ),
                };
                if self.theme_import.previews.len() >= 2 {
                    return Err("Theme preview is busy.".into());
                }
                let wpf = self.display.in_use.renderer == Renderer::WindowsWpf;
                let key = self.theme_import.schedule(input, wpf, true);
                self.theme_import
                    .previews
                    .push((id.clone(), key, self.theme_result()));
                return Ok(None);
            }
            ThemeAction::Unknown(_) => return Err("Unknown theme operation.".into()),
        }
        Ok(Some(self.theme_result()))
    }

    pub(crate) fn theme_upkeep(&mut self) {
        if let Some(result) = self
            .theme_import
            .file
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.theme_import.file = None;
            self.theme_import.error = result
                .and_then(|t| {
                    if let Some(t) = t {
                        let name = t.name.clone();
                        Arc::make_mut(&mut self.ui.themes).import(t, false)?;
                        self.select_theme(&name)?;
                    }
                    Ok(())
                })
                .err();
        }
        let reply = |result: Result<ThemeCallResult, String>| {
            result
                .map(|r| json!(r))
                .map_err(|e| ErrorObject::new(ErrorCode::InvalidParams, e))
        };
        for (id, call) in std::mem::take(&mut self.client.theme_calls) {
            match self.handle_theme(&id, call.action) {
                Ok(None) => {}
                Ok(Some(result)) => {
                    self.persist_prefs();
                    self.client.answer_window_call(id, Ok(json!(result)));
                }
                Err(message) => self.client.answer_window_call(id, reply(Err(message))),
            }
        }
        let wpf = self.display.in_use.renderer == Renderer::WindowsWpf;
        self.theme_import.prune(&self.ui.themes, wpf);
        if self.ui.settings && self.ui.settings_page == Page::Appearance {
            for theme in self.ui.themes.entries.values() {
                self.theme_import
                    .schedule(ThemeInput::Custom(theme.clone()), wpf, false);
            }
        }
        self.theme_import.pump(&self.ctx);
        for (id, key, mut result) in std::mem::take(&mut self.theme_import.previews) {
            if let Some(capture) = self.theme_import.cached(&key) {
                let outcome = match capture {
                    Ok(cached) => {
                        result.png = Some(STANDARD.encode(&cached.capture.full));
                        Ok(result)
                    }
                    Err(error) => Err(error.clone()),
                };
                self.client.answer_window_call(id, reply(outcome));
            } else {
                self.theme_import.previews.push((id, key, result));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_core::prefs::{UiPrefs, keys};
    #[test]
    fn imported_colors_apply_in_both_renderers_and_survive_restart() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let action = |v| serde_json::from_value::<ThemeAction>(v).unwrap();
        let id = RpcId::Number(1);
        let theme =
            r##"{"name":"Oasis","base":"light","colors":{"bg":"#cceedd","primary":"#334433"}}"##;
        let import = action(json!({"kind":"import","json":theme,"overwrite":false,"opId":"1"}));
        app.handle_theme(&id, import.clone()).unwrap();
        assert!(app.handle_theme(&id, import).is_err());
        app.handle_theme(
            &id,
            action(json!({"kind":"select","name":"Oasis","opId":"2"})),
        )
        .unwrap();
        crate::app::tests::window_frame(&ctx, &mut app, Default::default());
        let p = app.p;
        assert!(!p.dark);
        assert_eq!(p.bg, Color32::from_rgb(0xcc, 0xee, 0xdd));
        assert_eq!(p.wpf_colors()["YaacCanvas"], 0xcceedd);
        assert_eq!(crate::theme::visuals(&p).panel_fill, p.bg);
        let mut loaded = UiPrefs::default();
        let saved = app.ui.prefs().value(keys::THEME);
        #[cfg(feature = "hub")]
        let saved = {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("cache.sqlite");
            yaac_hub::cache::Cache::open(&path)
                .unwrap()
                .set_ui_state(keys::THEME, &saved)
                .unwrap();
            yaac_hub::cache::Cache::open_read_only(&path)
                .unwrap()
                .ui_state(keys::THEME)
                .unwrap()
                .unwrap()
        };
        loaded.load(keys::THEME, &saved);
        assert_eq!(loaded.theme, ThemeChoice::Custom);
        assert_eq!(loaded.themes, app.ui.themes);
        let updated = theme.replace("cceedd", "123456");
        app.handle_theme(
            &id,
            action(json!({"kind":"import","json":updated,"overwrite":true,"opId":"3"})),
        )
        .unwrap();
        crate::app::tests::window_frame(&ctx, &mut app, Default::default());
        assert_eq!(
            ctx.global_style().visuals.panel_fill,
            Color32::from_rgb(0x12, 0x34, 0x56)
        );
        app.handle_theme(&id, action(json!({"kind":"preview","name":"dark"})))
            .unwrap();
        assert_eq!(app.theme_result().selected, "Oasis");
        assert_eq!(app.theme_import.previews.len(), 1);
        assert_eq!(app.theme_import.queued.len(), 1);
    }
    #[test]
    fn validation_rejects_invalid_theme_files() {
        for s in [
            r##"{"name":" x","colors":{}}"##,
            r##"{"name":"light","colors":{}}"##,
            r##"{"name":"x","colors":{"typo":"#ffffff"}}"##,
            r#"{"name":"x","colors":{"text":"red"}}"#,
            r#"{"name":"x","base":"auto","colors":{}}"#,
        ] {
            assert!(Theme::parse(s.as_bytes()).is_err());
        }
        assert!(Theme::parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    }
}
