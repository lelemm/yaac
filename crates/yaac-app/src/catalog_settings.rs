//! Settings for the hub-owned skill catalog, shared by the native and egui views.
use std::{
    fs,
    io::Read,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use eframe::egui;
use serde_json::{Value, json};
use yaac_core::client::Client;
use yaac_protocol::{
    CatalogSkill, CatalogSkillFile, CatalogSkillInfo, CatalogSkillRemove, CatalogSkillSave,
    CatalogSkillsResult, MAX_CATALOG_SKILL_BYTES, rpc::RpcId,
};

use crate::app::YaacApp;

#[derive(Default)]
struct Draft {
    id: String,
    description: String,
    markdown: String,
    automatic: bool,
    revision: Option<String>,
    bundled: bool,
    files: Vec<CatalogSkillFile>,
}

impl Draft {
    fn opened(skill: CatalogSkill) -> Result<Self, String> {
        let CatalogSkill { info, mut files } = skill;
        let i = files
            .iter()
            .position(|f| f.path == "SKILL.md")
            .ok_or("Missing SKILL.md.")?;
        let bytes = STANDARD
            .decode(files.remove(i).data_base64)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            id: info.id,
            description: info.description,
            automatic: info.automatic,
            revision: Some(info.revision),
            bundled: info.bundled,
            files,
            markdown: String::from_utf8(bytes).map_err(|_| "SKILL.md must be UTF-8.")?,
        })
    }

    fn bundle(&self) -> Result<CatalogSkill, String> {
        let id = self.id.trim();
        let slug = id.strip_prefix("yaac.").unwrap_or(id);
        let description = self.description.trim();
        if slug.is_empty() || description.is_empty() || self.markdown.trim().is_empty() {
            return Err("Enter an ID, description, and instructions.".into());
        }
        let mut files = self.files.clone();
        let body = if self.markdown.starts_with("---\n") || self.markdown.starts_with("---\r\n") {
            self.markdown.clone()
        } else {
            // A JSON string is also a YAML double-quoted scalar.
            let description = serde_json::to_string(description).map_err(|e| e.to_string())?;
            format!(
                "---\nname: {slug}\ndescription: {description}\n---\n\n{}",
                self.markdown
            )
        };
        let bytes = files
            .iter()
            .map(|f| {
                let padding = f
                    .data_base64
                    .bytes()
                    .rev()
                    .take_while(|b| *b == b'=')
                    .count();
                f.data_base64.len() / 4 * 3 - padding
            })
            .sum::<usize>();
        if bytes + body.len() > MAX_CATALOG_SKILL_BYTES {
            return Err("Skill files exceed 512 KiB.".into());
        }
        files.push(CatalogSkillFile {
            path: "SKILL.md".into(),
            data_base64: STANDARD.encode(body),
            executable: false,
        });
        Ok(CatalogSkill {
            info: CatalogSkillInfo {
                id: format!("yaac.{slug}"),
                description: description.into(),
                automatic: self.automatic,
                revision: String::new(),
                bundled: false,
            },
            files,
        })
    }
}

enum Operation {
    Open,
    Save,
    Remove,
}

#[derive(Default)]
pub(crate) struct Settings {
    draft: Option<Draft>,
    imported: Option<mpsc::Receiver<Result<Option<Draft>, String>>>,
    pending: Option<(RpcId, Operation, Instant)>,
    confirmation: bool,
    notice: Option<(String, bool)>,
    reset: u64,
}

impl Settings {
    fn busy(&self) -> bool {
        self.imported.is_some() || self.pending.is_some()
    }

    fn edit(&mut self, draft: Option<Draft>) {
        self.draft = draft;
        self.notice = None;
        self.confirmation = false;
        self.reset = self.reset.wrapping_add(1);
    }

    fn start(&mut self, id: RpcId, operation: Operation) {
        self.pending = Some((id, operation, Instant::now() + Duration::from_secs(12)));
        self.notice = None;
    }

    fn open(&mut self, client: &mut Client, id: &str) {
        if !self.busy() {
            self.start(client.catalog_edit(id), Operation::Open);
        }
    }

    fn import(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.imported = Some(rx);
        self.notice = None;
        std::thread::spawn(move || {
            let result = rfd::FileDialog::new()
                .set_title("Choose a folder containing SKILL.md")
                .pick_folder()
                .map(|folder| import_folder(&folder))
                .transpose();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn save(&mut self, client: &mut Client) {
        if self.busy() {
            return;
        }
        let Some(draft) = &self.draft else { return };
        match draft.bundle() {
            Ok(skill) => {
                let request = CatalogSkillSave {
                    op_id: client.op_id(),
                    previous_revision: draft.revision.clone(),
                    skill,
                };
                self.start(client.catalog_save(request), Operation::Save);
            }
            Err(error) => self.notice = Some((error, true)),
        }
    }

    fn remove(&mut self, client: &mut Client) {
        if !self.confirmation || self.busy() {
            return;
        }
        let Some(draft) = &self.draft else { return };
        let Some(previous_revision) = draft.revision.clone().filter(|_| !draft.bundled) else {
            return;
        };
        let request = CatalogSkillRemove {
            op_id: client.op_id(),
            id: draft.id.clone(),
            previous_revision,
        };
        self.confirmation = false;
        self.start(client.catalog_remove(request), Operation::Remove);
    }

    pub(crate) fn upkeep(&mut self, client: &mut Client, ctx: &egui::Context) {
        if let Some(rx) = &self.imported {
            match rx.try_recv() {
                Ok(result) => {
                    self.imported = None;
                    match result {
                        Ok(Some(draft)) => self.edit(Some(draft)),
                        Ok(None) => {}
                        Err(error) => self.notice = Some((error, true)),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.imported = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((id, result)) = client.take_catalog_reply()
            && self
                .pending
                .as_ref()
                .is_some_and(|(expected, _, _)| *expected == id)
            && let Some((_, operation, _)) = self.pending.take()
        {
            let result = result.and_then(|value| match operation {
                Operation::Open => serde_json::from_value::<CatalogSkillsResult>(value)
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.skill.ok_or_else(|| "No skill returned.".into()))
                    .and_then(Draft::opened)
                    .map(Some),
                Operation::Save => serde_json::from_value(value)
                    .map_err(|e| e.to_string())
                    .and_then(Draft::opened)
                    .map(Some),
                Operation::Remove => Ok(None),
            });
            match result {
                Ok(draft) => {
                    self.edit(draft);
                    match operation {
                        Operation::Save => self.notice = Some(("Skill saved.".into(), false)),
                        Operation::Remove => self.notice = Some(("Skill removed.".into(), false)),
                        Operation::Open => {}
                    }
                }
                Err(error) => self.notice = Some((error, true)),
            }
        }
        if let Some((_, _, deadline)) = self.pending {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.pending = None;
                client.catalog_list();
                self.notice = Some((
                    "The hub didn’t confirm the request. Refresh the catalog before retrying."
                        .into(),
                    true,
                ));
            } else {
                ctx.request_repaint_after(left);
            }
        }
    }
}

fn import_folder(folder: &Path) -> Result<Draft, String> {
    let mut dirs = vec![folder.to_path_buf()];
    let mut visited = 0;
    let mut size = 0;
    let mut markdown = None;
    let mut files = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_name() == ".git" {
                continue;
            }
            visited += 1;
            if visited > 256 {
                return Err("Choose a skill folder with up to 64 files.".into());
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let path = entry.path();
            if kind.is_dir() {
                dirs.push(path);
                continue;
            }
            if !kind.is_file() || files.len() + usize::from(markdown.is_some()) >= 64 {
                return Err("A skill may contain up to 64 regular files.".into());
            }
            let relative = path
                .strip_prefix(folder)
                .ok()
                .and_then(Path::to_str)
                .ok_or("Skill filenames must be UTF-8.")?
                .replace(std::path::MAIN_SEPARATOR, "/");
            let file = fs::File::open(&path).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                file.metadata()
                    .map_err(|e| e.to_string())?
                    .permissions()
                    .mode()
                    & 0o100
                    != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            let mut bytes = Vec::new();
            file.take((MAX_CATALOG_SKILL_BYTES - size + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            size += bytes.len();
            if size > MAX_CATALOG_SKILL_BYTES {
                return Err("Skill files exceed 512 KiB.".into());
            }
            if relative == "SKILL.md" {
                markdown = Some(String::from_utf8(bytes).map_err(|_| "SKILL.md must be UTF-8.")?);
            } else {
                files.push(CatalogSkillFile {
                    path: relative,
                    data_base64: STANDARD.encode(bytes),
                    executable,
                });
            }
        }
    }
    let markdown = markdown.ok_or("Choose the folder containing SKILL.md.")?;
    let frontmatter = markdown
        .strip_prefix("---")
        .and_then(|s| s.split_once("\n---"))
        .map_or("", |(front, _)| front);
    let value = |key: &str| -> Option<&str> {
        frontmatter
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(k, _)| *k == key)
                    .map(|(_, v)| v.trim().trim_matches(['"', '\'']))
            })
            .filter(|v| !matches!(*v, "" | "|" | ">" | ">-" | "|-"))
    };
    let id = value("name")
        .or_else(|| folder.file_name().and_then(|s| s.to_str()))
        .unwrap_or_default()
        .to_owned();
    let description = value("description").unwrap_or_default().to_owned();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Draft {
        id,
        description,
        markdown,
        files,
        ..Default::default()
    })
}

const GUIDANCE: &str = "Use a yaac. skill ID in chat to load it on the agent’s target. Enable automatic use if the model should also consider it when relevant.";

pub(crate) fn show(ui: &mut egui::Ui, app: &mut YaacApp) {
    ui.heading("Skills");
    ui.label(GUIDANCE);
    let settings = &mut app.catalog_settings;
    let client = &mut app.client;
    let enabled = client.state.hub.attached && !settings.busy();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(enabled, egui::Button::new("New skill"))
            .clicked()
        {
            settings.edit(Some(Draft::default()));
        }
        if ui
            .add_enabled(enabled, egui::Button::new("Import folder…"))
            .clicked()
        {
            settings.import(&app.ctx);
        }
        if ui
            .add_enabled(enabled, egui::Button::new("Refresh"))
            .clicked()
        {
            client.catalog_list();
        }
    });
    if let Some(error) = &client.state.catalog_error {
        ui.colored_label(app.p.failed, error);
    }
    if let Some((message, error)) = &settings.notice {
        ui.colored_label(if *error { app.p.failed } else { app.p.text }, message);
    }
    if settings.busy() {
        ui.spinner();
    }
    let mut open = None;
    for skill in &client.state.catalog_skills {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(enabled, egui::Button::new(&skill.id))
                .clicked()
            {
                open = Some(skill.id.clone());
            }
            ui.label(mode(skill));
        });
        ui.label(&skill.description);
    }
    if let Some(id) = open {
        settings.open(client, &id);
    }
    if let Some(draft) = settings.draft.as_mut() {
        ui.separator();
        ui.add_enabled_ui(enabled, |ui| {
            ui.label("Skill ID");
            ui.add_enabled(
                draft.revision.is_none(),
                egui::TextEdit::singleline(&mut draft.id).hint_text("yaac.my-skill"),
            );
            ui.label("Description");
            ui.text_edit_singleline(&mut draft.description);
            ui.horizontal(|ui| {
                ui.radio_value(&mut draft.automatic, false, "Manual");
                ui.radio_value(&mut draft.automatic, true, "Auto when relevant");
            });
            ui.label("SKILL.md");
            ui.add(
                egui::TextEdit::multiline(&mut draft.markdown)
                    .desired_rows(14)
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_CATALOG_SKILL_BYTES),
            );
            ui.label(format!("{} supporting files", draft.files.len()));
        });
        let removable = !draft.bundled && draft.revision.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(enabled, egui::Button::new("Save skill"))
                .clicked()
            {
                settings.save(client);
            }
            if ui
                .add_enabled(enabled, egui::Button::new("Close editor"))
                .clicked()
            {
                settings.edit(None);
            }
            if removable
                && ui
                    .add_enabled(enabled, egui::Button::new("Remove…"))
                    .clicked()
            {
                settings.confirmation = true;
            }
        });
        if settings.confirmation {
            ui.horizontal(|ui| {
                if ui.button("Confirm removal").clicked() {
                    settings.remove(client);
                }
                if ui.button("Cancel").clicked() {
                    settings.confirmation = false;
                }
            });
        }
    }
}

fn mode(info: &CatalogSkillInfo) -> &'static str {
    if info.automatic {
        "Auto when relevant"
    } else {
        "Manual"
    }
}

fn action(id: &str, label: &str, enabled: bool) -> Value {
    json!({ "id": id, "label": label, "enabled": enabled })
}

pub(crate) fn wpf_view(app: &YaacApp) -> Value {
    let settings = &app.catalog_settings;
    let enabled = app.client.state.hub.attached && !settings.busy();
    let mut rows: Vec<_> = app.client.state.catalog_skills.iter().map(|s| {
        json!({ "id": s.id, "label": s.id, "detail": s.description, "status": mode(s),
            "actions": [{ "id": "catalog.open", "label": "Edit", "enabled": enabled, "values": { "id": s.id } }] })
    }).collect();
    if let Some(error) = &app.client.state.catalog_error {
        rows.push(json!({ "id": "error", "status": error, "tone": "error" }));
    }
    if let Some((notice, is_error)) = &settings.notice {
        rows.push(json!({ "id": "notice", "status": notice, "tone": if *is_error { "error" } else { "" } }));
    }
    if settings.busy() {
        rows.push(json!({ "id": "busy", "status": "Working…" }));
    }
    let mut sections = vec![json!({
        "id": "catalog", "title": "Skill catalog", "detail": GUIDANCE, "rows": rows,
        "actions": [action("catalog.new", "New skill", enabled), action("catalog.import", "Import folder…", enabled),
            action("catalog.refresh", "Refresh", enabled)]
    })];
    if let Some(d) = &settings.draft {
        let mut actions = vec![
            json!({ "id": "catalog.save", "label": "Save skill", "enabled": enabled, "kind": "primary", "submit": true }),
            action("catalog.close", "Close editor", enabled),
        ];
        if !d.bundled && d.revision.is_some() {
            if settings.confirmation {
                actions.push(action("catalog.remove", "Confirm removal", enabled));
                actions.push(action("catalog.keep", "Cancel", enabled));
            } else {
                actions.push(action("catalog.confirm", "Remove…", enabled));
            }
        }
        sections.push(json!({ "id": "catalog-editor", "title": "Skill editor", "rows": [{
            "id": "form", "layout": "form", "resetKey": settings.reset.to_string(),
            "detail": format!("{} supporting files", d.files.len()),
            "fields": [
                { "id": "skillId", "label": "Skill ID", "value": d.id, "placeholder": "yaac.my-skill", "enabled": enabled && d.revision.is_none(), "required": true },
                { "id": "description", "label": "Description", "value": d.description, "enabled": enabled, "required": true, "type": "textarea", "height": 64, "maxLength": 240 },
                { "id": "activation", "label": "Activation", "type": "radio", "enabled": enabled,
                    "value": if d.automatic { "auto" } else { "manual" }, "options": [action("manual", "Manual", true), action("auto", "Auto when relevant", true)] },
                { "id": "markdown", "label": "SKILL.md", "type": "textarea", "value": d.markdown, "height": 300, "enabled": enabled, "required": true, "maxLength": MAX_CATALOG_SKILL_BYTES }
            ],
            "actions": actions
        }] }));
    }
    json!({ "sections": sections })
}

pub(crate) fn wpf_command(app: &mut YaacApp, id: &str, values: &Value) -> Result<(), String> {
    let settings = &mut app.catalog_settings;
    let client = &mut app.client;
    if settings.busy() || !client.state.hub.attached {
        return Ok(());
    }
    let field = |key: &str| values[key].as_str().unwrap_or_default().to_owned();
    match id {
        "catalog.new" => settings.edit(Some(Draft::default())),
        "catalog.open" => settings.open(client, &field("id")),
        "catalog.import" => settings.import(&app.ctx),
        "catalog.refresh" => client.catalog_list(),
        "catalog.close" => settings.edit(None),
        "catalog.confirm" => settings.confirmation = true,
        "catalog.keep" => settings.confirmation = false,
        "catalog.remove" => settings.remove(client),
        "catalog.save" => {
            let Some(draft) = &mut settings.draft else {
                return Ok(());
            };
            if draft.revision.is_none() {
                draft.id = field("skillId");
            }
            draft.description = field("description");
            draft.automatic = field("activation") == "auto";
            draft.markdown = field("markdown");
            settings.save(client);
        }
        _ => return Err("Unknown catalog command.".into()),
    }
    Ok(())
}
