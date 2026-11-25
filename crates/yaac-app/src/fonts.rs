//! Window-local font discovery, import, and loading off the UI thread.
use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use fontdb::{Database, Source};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
};
use yaac_core::prefs::FontPrefs;

pub(crate) const SAMPLE: &str = "Aa Bb · Rotação · 0123";
pub(crate) const CODE_SAMPLE: &str = "let count = 42;  0 O  1 l";
pub(crate) const NERD_SAMPLE: &str = "\u{f09b}  \u{e0a0}  \u{f07b}  \u{f120}";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Normal,
    Code,
}

impl Role {
    pub(crate) fn get(self, prefs: &FontPrefs) -> &str {
        match self {
            Self::Normal => &prefs.normal,
            Self::Code => &prefs.code,
        }
    }

    pub(crate) fn set(self, prefs: &mut FontPrefs, value: String) {
        match self {
            Self::Normal => prefs.normal = value,
            Self::Code => prefs.code = value,
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct Choice {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) native: NativeFont,
}

#[derive(Clone, Default, Serialize)]
pub(crate) struct NativeFont {
    name: String,
    folder: Option<PathBuf>,
}

struct Face {
    native: NativeFont,
    regular: fontdb::ID,
    bold: fontdb::ID,
}

type Preview = (Role, String);

enum Request {
    Apply(FontPrefs, Option<Preview>),
    Import(Role),
}
enum Reply {
    Catalog(Vec<Choice>),
    Applied(FontPrefs, Option<Preview>, FontDefinitions, [NativeFont; 2]),
    Imported(Role, String),
    Error(String),
}

#[derive(Default)]
pub(crate) struct Fonts {
    worker: Option<(mpsc::Sender<Request>, mpsc::Receiver<Reply>)>,
    requested: FontPrefs,
    preview: Option<Preview>,
    requested_preview: Option<Preview>,
    loaded_preview: Option<Preview>,
    pub(crate) choices: Vec<Choice>,
    pub(crate) catalog_ready: bool,
    pub(crate) native: [NativeFont; 2],
    pub(crate) error: Option<String>,
}

impl Fonts {
    pub(crate) fn start(&mut self, ctx: &egui::Context) {
        if self.worker.is_some() {
            return;
        }
        let (send, commands) = mpsc::channel();
        let (events, recv) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            worker(commands, |reply| {
                let _ = events.send(reply);
                ctx.request_repaint();
            })
        });
        self.worker = Some((send, recv));
    }

    pub(crate) fn import(&mut self, role: Role, ctx: &egui::Context) {
        self.start(ctx);
        self.error = None;
        if let Some((send, _)) = &self.worker {
            let _ = send.send(Request::Import(role));
        }
    }

    pub(crate) fn preview_font(&self, role: Role, key: &str, size: f32) -> Option<egui::FontId> {
        self.loaded_preview
            .as_ref()
            .filter(|(loaded_role, loaded_key)| *loaded_role == role && loaded_key == key)
            .map(|_| egui::FontId::new(size, FontFamily::Name("picker-sample".into())))
    }

    pub(crate) fn preview(&mut self, role: Role, key: &str, ctx: &egui::Context) {
        let pick = (role, key.to_owned());
        if self.preview.as_ref() != Some(&pick) {
            self.preview = Some(pick);
            ctx.request_repaint();
        }
    }

    pub(crate) fn pump(&mut self, prefs: &mut FontPrefs, ctx: &egui::Context) -> bool {
        let mut changed = false;
        let Some((send, recv)) = &self.worker else {
            return false;
        };
        while let Ok(reply) = recv.try_recv() {
            match reply {
                Reply::Catalog(choices) => {
                    self.choices = choices;
                    self.catalog_ready = true;
                }
                Reply::Imported(role, key) => role.set(prefs, key),
                Reply::Error(error) => self.error = Some(error),
                Reply::Applied(selection, preview, definitions, native)
                    if selection == *prefs && preview == self.preview =>
                {
                    ctx.set_fonts(definitions);
                    self.native = native;
                    self.loaded_preview = preview;
                    changed = true;
                }
                Reply::Applied(..) => {}
            }
        }
        if *prefs != self.requested || self.preview != self.requested_preview {
            self.requested = prefs.clone();
            self.requested_preview = self.preview.clone();
            let _ = send.send(Request::Apply(prefs.clone(), self.preview.clone()));
        }
        changed
    }
}

fn font_root() -> Option<PathBuf> {
    #[cfg(feature = "hub")]
    if let Ok(paths) = yaac_hub::paths::HubPaths::from_env() {
        return Some(paths.data_dir.join("fonts"));
    }
    std::env::var_os("YAAC_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("yaac")))
        .map(|p| p.join("fonts"))
}

fn catalog(db: &Database, root: Option<&Path>) -> (Vec<Choice>, BTreeMap<String, Face>) {
    let mut families = BTreeMap::<String, (NativeFont, Vec<&fontdb::FaceInfo>)>::new();
    for face in db.faces() {
        let Some((name, _)) = face.families.first() else {
            continue;
        };
        let folder = match &face.source {
            Source::File(path) | Source::SharedFile(path, _)
                if root.is_some_and(|root| path.starts_with(root)) =>
            {
                path.parent().map(Path::to_owned)
            }
            _ => None,
        };
        let id = match folder
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
        {
            Some(hash) => format!("import:{hash}:{name}"),
            None => format!("system:{name}"),
        };
        families
            .entry(id)
            .or_insert_with(|| {
                (
                    NativeFont {
                        name: name.clone(),
                        folder,
                    },
                    Vec::new(),
                )
            })
            .1
            .push(face);
    }
    let mut choices = Vec::new();
    let mut faces = BTreeMap::new();
    for (id, (native, variants)) in families {
        let closest = |weight| {
            variants
                .iter()
                .min_by_key(|face| {
                    (
                        face.style != fontdb::Style::Normal,
                        face.weight.0.abs_diff(weight),
                    )
                })
                .expect("a font family has a face")
                .id
        };
        let label = if native.folder.is_some() {
            format!("{} (imported)", native.name)
        } else {
            native.name.clone()
        };
        choices.push(Choice {
            id: id.clone(),
            label,
            native: native.clone(),
        });
        faces.insert(
            id,
            Face {
                native,
                regular: closest(400),
                bold: closest(700),
            },
        );
    }
    choices.sort_by_cached_key(|choice| choice.label.to_lowercase());
    (choices, faces)
}

fn worker(commands: mpsc::Receiver<Request>, send: impl Fn(Reply)) {
    let root = font_root();
    let mut db = Database::new();
    db.load_system_fonts();
    if let Some(root) = &root {
        db.load_fonts_dir(root);
    }
    let (choices, mut faces) = catalog(&db, root.as_deref());
    send(Reply::Catalog(choices));
    let default = crate::theme::base_fonts();
    let mut cache = HashMap::<fontdb::ID, Arc<FontData>>::new();
    for command in commands {
        match command {
            Request::Apply(prefs, preview) => {
                let mut definitions = default.clone();
                let normal = faces.get(&prefs.normal);
                let code = faces.get(&prefs.code);
                let hover = preview.as_ref().and_then(|(_, key)| faces.get(key));
                let active = [
                    normal.map(|f| f.regular),
                    normal.map(|f| f.bold),
                    code.map(|f| f.regular),
                    hover.map(|f| f.regular),
                ];
                cache.retain(|id, _| active.contains(&Some(*id)));
                let mut load = |id| {
                    if let Some(data) = cache.get(&id) {
                        return Some(data.clone());
                    }
                    let data = db.with_face_data(id, |data, index| {
                        Arc::new(FontData::from_blob(Arc::new(data.to_vec()), index))
                    })?;
                    cache.insert(id, data.clone());
                    Some(data)
                };
                for (family, name, id) in [
                    (
                        FontFamily::Proportional,
                        "user-normal",
                        normal.map(|f| f.regular),
                    ),
                    (
                        FontFamily::Name("bold".into()),
                        "user-bold",
                        normal.map(|f| f.bold),
                    ),
                    (FontFamily::Monospace, "user-code", code.map(|f| f.regular)),
                ] {
                    if let Some(data) = id.and_then(&mut load) {
                        definitions.font_data.insert(name.into(), data);
                        definitions
                            .families
                            .entry(family)
                            .or_default()
                            .insert(0, name.into());
                    }
                }
                if let Some((role, _)) = &preview {
                    let base = match role {
                        Role::Normal => FontFamily::Proportional,
                        Role::Code => FontFamily::Monospace,
                    };
                    let mut family = default.families[&base].clone();
                    if let Some(data) = hover.and_then(|face| load(face.regular)) {
                        definitions.font_data.insert("picker-face".into(), data);
                        family.insert(0, "picker-face".into());
                    }
                    definitions
                        .families
                        .insert(FontFamily::Name("picker-sample".into()), family);
                }
                let native =
                    [normal, code].map(|face| face.map(|f| f.native.clone()).unwrap_or_default());
                send(Reply::Applied(prefs, preview, definitions, native));
            }
            Request::Import(role) => {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("Font files", &["ttf", "otf", "ttc", "otc"])
                    .pick_file()
                else {
                    continue;
                };
                match import_font(root.as_deref(), &path) {
                    Ok(folder) => {
                        db.load_fonts_dir(&folder);
                        let (choices, next) = catalog(&db, root.as_deref());
                        faces = next;
                        let id = choices
                            .iter()
                            .find(|choice| {
                                faces[&choice.id].native.folder.as_ref() == Some(&folder)
                            })
                            .map(|choice| choice.id.clone());
                        send(Reply::Catalog(choices));
                        if let Some(id) = id {
                            send(Reply::Imported(role, id));
                        }
                    }
                    Err(error) => send(Reply::Error(error)),
                }
            }
        }
    }
}

fn import_font(root: Option<&Path>, source: &Path) -> Result<PathBuf, String> {
    const MAX_BYTES: u64 = 64 * 1024 * 1024;
    use std::io::Read as _;
    let root = root.ok_or("YAAC's data directory is unavailable")?;
    let ext = source
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc") {
        return Err("Choose a TTF, OTF, TTC or OTC font file.".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(source)
        .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Font files can be up to 64 MiB.".into());
    }
    let folder = root.join(format!("{:x}", Sha256::digest(&bytes)));
    let mut validation = Database::new();
    let source = Arc::new(bytes);
    if validation
        .load_font_source(Source::Binary(source.clone()))
        .is_empty()
    {
        return Err("YAAC could not read a font in this file.".into());
    }
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    // WPF locates OpenType collections through the .ttc extension.
    let ext = if ext == "otc" { "ttc" } else { ext.as_str() };
    let dest = folder.join(format!("font.{ext}"));
    if !dest.is_file() {
        let mut temporary = tempfile::NamedTempFile::new_in(&folder).map_err(|e| e.to_string())?;
        temporary.write_all(&source).map_err(|e| e.to_string())?;
        temporary.persist(dest).map_err(|e| e.to_string())?;
    }
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_imported_font_loads_without_its_original_file() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("selected.ttf");
        let storage = temp.path().join("fonts");
        let data = include_bytes!("../assets/fonts/CaskaydiaCoveNerdFontMono-Regular.ttf");
        fs::write(&original, data).unwrap();
        let folder = import_font(Some(&storage), &original).unwrap();
        assert_eq!(import_font(Some(&storage), &original).unwrap(), folder);
        fs::remove_file(original).unwrap();
        let mut db = Database::new();
        db.load_fonts_dir(&storage);
        let (choices, faces) = catalog(&db, Some(&storage));
        let choice = choices
            .iter()
            .find(|font| font.label.contains("Caskaydia"))
            .unwrap();
        let face = &faces[&choice.id];
        assert_eq!(face.native.folder.as_deref(), Some(folder.as_path()));
        assert!(
            db.with_face_data(face.regular, |bytes, _| bytes == data)
                .unwrap()
        );
    }
}
