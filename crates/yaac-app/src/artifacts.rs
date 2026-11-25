//! P358. A card identifies a snapshot on its publisher's target. Its host action starts
//! only on a click, reads through upload/read, and materializes bytes off the UI thread.
use std::{
    collections::BTreeMap,
    io::Write,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

use eframe::egui;
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempPath};
use yaac_core::{ThreadKey, client::Client, state::AppState};
use yaac_protocol::GeneratedArtifact;

use crate::preview_controller::Browsers;

#[derive(Clone, Debug)]
pub(crate) struct ArtifactCard {
    pub worker: ThreadKey,
    pub artifact: GeneratedArtifact,
}

impl ArtifactCard {
    pub(crate) fn id(&self) -> String {
        format!(
            "artifact:{}:{}",
            self.worker.address(),
            self.artifact.upload_id
        )
    }

    fn extension(&self) -> Result<&str, String> {
        let ext = self
            .artifact
            .name
            .rsplit_once('.')
            .map(|(_, s)| s)
            .unwrap_or("");
        [
            "docx", "xlsx", "pptx", "csv", "svg", "md", "markdown", "txt", "mmd", "mermaid", "pdf",
            "png", "jpg", "jpeg", "gif", "webp",
        ]
        .into_iter()
        .find(|e| ext.eq_ignore_ascii_case(e))
        .ok_or_else(|| "Unsupported artifact format".into())
    }

    pub(crate) fn actions(&self, browser: bool) -> Vec<(Command, &'static str)> {
        let mut actions = vec![(Command::Download, "Download…"), (Command::Open, "Open")];
        if browser
            && matches!(
                self.extension(),
                Ok("md"
                    | "markdown"
                    | "txt"
                    | "csv"
                    | "mmd"
                    | "mermaid"
                    | "pdf"
                    | "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp")
            )
        {
            actions.push((Command::Browser, "Open in integrated browser"));
        }
        if self.artifact.size <= 64 * 1024 && matches!(self.extension(), Ok("mmd" | "mermaid")) {
            actions.push((Command::Diagram, "Preview diagram"));
        }
        actions
    }
}

pub(crate) fn cards(state: &AppState, viewing: &ThreadKey) -> Vec<ArtifactCard> {
    state
        .threads
        .iter()
        .filter(|(key, rec)| {
            rec.summary
                .artifacts
                .as_ref()
                .is_some_and(|a| !a.is_empty())
                && (*key == viewing
                    || yaac_core::copies::depth_below(state, key, viewing).is_some())
        })
        .flat_map(|(key, rec)| {
            rec.summary
                .artifacts
                .iter()
                .flatten()
                .cloned()
                .map(|artifact| ArtifactCard {
                    worker: key.clone(),
                    artifact,
                })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Command {
    Download,
    Open,
    Browser,
    Diagram,
}

struct Transfer {
    token: u64,
    viewing: ThreadKey,
    card: ArtifactCard,
    command: Command,
    bytes: Vec<u8>,
    deadline: Instant,
}
enum ResultFile {
    Saved,
    Cancelled,
    Opened(NamedTempFile),
    Browser(ThreadKey, NamedTempFile),
    Diagram(String),
}

pub(crate) struct Artifacts {
    serial: u64,
    transfer: Option<Transfer>,
    tx: mpsc::Sender<(String, Result<ResultFile, String>)>,
    rx: mpsc::Receiver<(String, Result<ResultFile, String>)>,
    pub status: BTreeMap<String, String>,
    pub diagrams: BTreeMap<String, String>,
    // Keep open documents available to their viewer for the lifetime of the window.
    opened: Vec<TempPath>,
}

impl Default for Artifacts {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            serial: 0,
            transfer: None,
            tx,
            rx,
            status: Default::default(),
            diagrams: Default::default(),
            opened: Vec::new(),
        }
    }
}

const TIMEOUT: Duration = Duration::from_secs(30);

impl Artifacts {
    pub(crate) fn start(
        &mut self,
        ctx: &egui::Context,
        client: &mut Client,
        viewing: ThreadKey,
        card: ArtifactCard,
        command: Command,
    ) {
        if self.transfer.is_some() {
            self.status
                .insert(card.id(), "A download is already in progress".into());
            return;
        }
        self.serial += 1;
        self.status.insert(card.id(), "Reading file…".into());
        client.read_artifact_chunk(
            self.serial,
            &card.worker.target,
            &card.artifact.upload_id,
            0,
        );
        self.transfer = Some(Transfer {
            token: self.serial,
            viewing,
            card,
            command,
            bytes: Vec::new(),
            deadline: Instant::now() + TIMEOUT,
        });
        ctx.request_repaint_after(TIMEOUT);
    }

    pub(crate) fn pump(
        &mut self,
        ctx: &egui::Context,
        client: &mut Client,
        browsers: &mut Browsers,
    ) {
        for (id, result) in self.rx.try_iter() {
            let message = match result {
                Err(e) => e,
                Ok(ResultFile::Cancelled) => String::new(),
                Ok(ResultFile::Saved) => "Downloaded".into(),
                Ok(ResultFile::Diagram(source)) => {
                    self.diagrams.insert(id.clone(), source);
                    String::new()
                }
                Ok(ResultFile::Opened(file)) => {
                    self.opened.push(file.into_temp_path());
                    "Opened".into()
                }
                Ok(ResultFile::Browser(key, file)) => {
                    match browsers.artifact(client, &key, file.path()) {
                        Ok(()) => {
                            self.opened.push(file.into_temp_path());
                            String::new()
                        }
                        Err(e) => e,
                    }
                }
            };
            self.status.insert(id, message);
        }
        for (token, reply) in std::mem::take(&mut client.artifact_chunks) {
            if !self.transfer.as_ref().is_some_and(|t| t.token == token) {
                continue;
            }
            let mut t = self.transfer.take().expect("matched transfer");
            match reply.and_then(|r| {
                if r.present && (r.size != t.card.artifact.size || r.size > 16 * 1024 * 1024) {
                    return Err("The snapshot size does not match its card".into());
                }
                crate::images::append_chunk(r, &mut t.bytes)
            }) {
                Err(e) => {
                    self.status.insert(t.card.id(), e);
                }
                Ok(true) => {
                    client.read_artifact_chunk(
                        token,
                        &t.card.worker.target,
                        &t.card.artifact.upload_id,
                        t.bytes.len() as u64,
                    );
                    t.deadline = Instant::now() + TIMEOUT;
                    self.transfer = Some(t);
                    ctx.request_repaint_after(TIMEOUT);
                }
                Ok(false) => {
                    let tx = self.tx.clone();
                    let wake = ctx.clone();
                    let id = t.card.id();
                    let error_id = id.clone();
                    if let Err(e) = std::thread::Builder::new()
                        .name("yaac-artifact".into())
                        .spawn(move || {
                            let _ = tx.send((id, materialize(t)));
                            wake.request_repaint();
                        })
                    {
                        self.status.insert(error_id, e.to_string());
                    }
                }
            }
        }
        if self
            .transfer
            .as_ref()
            .is_some_and(|t| t.deadline <= Instant::now())
            && let Some(t) = self.transfer.take()
        {
            self.status
                .insert(t.card.id(), "Timed out reading the file".into());
        }
    }
}

fn materialize(t: Transfer) -> Result<ResultFile, String> {
    let ext = t.card.extension()?;
    let expected = format!("u_artifact_{ext}_{:x}", Sha256::digest(&t.bytes));
    if t.card.artifact.upload_id.as_str() != expected {
        return Err("Artifact checksum mismatch".into());
    }
    if t.command == Command::Diagram {
        return String::from_utf8(t.bytes)
            .map(ResultFile::Diagram)
            .map_err(|e| e.to_string());
    }
    if t.command == Command::Download {
        let name = t
            .card
            .artifact
            .name
            .split(['/', '\\'])
            .next_back()
            .unwrap_or("artifact");
        let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() else {
            return Ok(ResultFile::Cancelled);
        };
        let mut file = NamedTempFile::new_in(path.parent().ok_or("Missing destination directory")?)
            .map_err(|e| e.to_string())?;
        file.write_all(&t.bytes).map_err(|e| e.to_string())?;
        file.persist(path).map_err(|e| e.to_string())?;
        return Ok(ResultFile::Saved);
    }
    let suffix = if t.command == Command::Browser
        && matches!(ext, "md" | "markdown" | "mmd" | "mermaid" | "csv")
    {
        "txt"
    } else {
        ext
    };
    let mut file = tempfile::Builder::new()
        .prefix("YAAC-document-")
        .suffix(&format!(".{suffix}"))
        .tempfile()
        .map_err(|e| e.to_string())?;
    // CEF's stream handler only sets the MIME type. Mark the temporary text view
    // as UTF-8 so Chromium does not guess the user's legacy locale encoding.
    if t.command == Command::Browser && suffix == "txt" && !t.bytes.starts_with(b"\xef\xbb\xbf") {
        file.write_all(b"\xef\xbb\xbf").map_err(|e| e.to_string())?;
    }
    file.write_all(&t.bytes).map_err(|e| e.to_string())?;
    if t.command == Command::Browser {
        return Ok(ResultFile::Browser(t.viewing, file));
    }
    open(file.path()).map_err(|e| e.to_string())?;
    Ok(ResultFile::Opened(file))
}

#[cfg(windows)]
fn open(file: &Path) -> std::io::Result<()> {
    yaac_winsys::open_document(file)
}

#[cfg(not(windows))]
fn open(file: &Path) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if std::process::Command::new(opener)
        .arg(file)
        .status()?
        .success()
    {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "The host could not open this document",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_browser_and_diagram_actions_follow_format_and_size() {
        let mut card = ArtifactCard {
            worker: yaac_core::demo::threads::index(),
            artifact: GeneratedArtifact {
                name: "sample.md".into(),
                upload_id: "test".into(),
                title: "Report".into(),
                size: 10,
            },
        };
        for (name, size, browser, diagram) in [
            ("diagram.mmd", 65_536, true, true),
            ("diagram.MERMAID", 65_537, true, false),
            ("report.md", 300, true, false),
            ("data.csv", 10, true, false),
            ("plot.SVG", 10, false, false),
            ("report.docx", 100, false, false),
        ] {
            card.artifact.name = name.into();
            card.artifact.size = size;
            let has = |command| {
                card.actions(true)
                    .iter()
                    .any(|(action, _)| *action == command)
            };
            assert_eq!(has(Command::Browser), browser);
            assert_eq!(has(Command::Diagram), diagram);
            assert!(
                !card
                    .actions(false)
                    .iter()
                    .any(|(action, _)| *action == Command::Browser)
            );
        }
    }

    #[test]
    fn artifact_snapshot_verification_precedes_materialization() {
        let key = yaac_core::demo::threads::notes();
        let bytes = b"<script>sample</script>".to_vec();
        let artifact = GeneratedArtifact {
            upload_id: format!("u_artifact_md_{:x}", Sha256::digest(&bytes)).into(),
            title: "Report".into(),
            name: "report.md".into(),
            size: bytes.len() as u64,
        };
        let transfer = |bytes| Transfer {
            token: 1,
            viewing: key.clone(),
            card: ArtifactCard {
                worker: key.clone(),
                artifact: artifact.clone(),
            },
            command: Command::Browser,
            bytes,
            deadline: Instant::now(),
        };
        assert!(materialize(transfer(vec![1])).is_err());
        let Ok(ResultFile::Browser(_, file)) = materialize(transfer(bytes.clone())) else {
            panic!("expected browser document")
        };
        assert_eq!(file.path().extension().unwrap(), "txt");
        assert_eq!(
            std::fs::read(file.path()).unwrap(),
            [b"\xef\xbb\xbf".as_slice(), &bytes].concat()
        );
    }
}
