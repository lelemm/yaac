//! Read-only worktree pane. All target access uses the window's existing hub client.
use crate::preview_controller::{Browsers, Pending};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};
use yaac_core::{ThreadKey, attachments::DraftRef, client::Client};
use yaac_protocol::{FsReadResult, FsTreeResult, GitFileDiffResult, attach, methods::names};

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub(crate) enum TabKind {
    #[default]
    Browser,
    Files,
    File(String),
    Diff(String),
    /// A terminal session ([`crate::terminal::Terminals`]).
    Terminal(u64),
}

impl TabKind {
    pub(crate) fn path(&self) -> Option<&str> {
        match self {
            Self::File(p) | Self::Diff(p) => Some(p),
            _ => None,
        }
    }
}

pub(crate) enum Document {
    File(yaac_code::Highlighted),
    Diff(Box<yaac_code::Diff>),
    Notice(String),
}
pub(crate) enum Work {
    File(yaac_code::Job<yaac_code::Highlighted>),
    Diff(yaac_code::Job<yaac_code::DiffResult>),
}

#[derive(Default)]
pub(crate) struct FileTree {
    pub(crate) folders: HashMap<String, FsTreeResult>,
    pub(crate) expanded: HashSet<String>,
    pub(crate) show_ignored: bool,
    pub(crate) errors: HashMap<String, String>,
    pending: HashSet<String>,
    due: Option<Instant>,
    last_turn: Option<yaac_protocol::TurnId>,
}

impl FileTree {
    pub(crate) fn expand(&mut self, path: &str, on: bool) {
        if on {
            self.expanded.insert(path.to_owned());
        } else {
            self.expanded
                .retain(|p| p != path && !p.strip_prefix(path).is_some_and(|s| s.starts_with('/')));
        }
    }
    pub(crate) fn refresh(&mut self) {
        self.due = None;
    }
    /// Demo screenshots: the tree as loaded, with no refresh due for an hour.
    pub(crate) fn hold(&mut self, turn: Option<yaac_protocol::TurnId>) {
        self.due = Some(Instant::now() + Duration::from_secs(3600));
        self.last_turn = turn;
    }
    pub(crate) fn collapse_all(&mut self) {
        self.expanded.clear();
    }
    fn requests(&mut self, now: Instant) -> Vec<String> {
        let refresh = self.due.is_none_or(|t| now >= t);
        if refresh {
            self.due = Some(now + Duration::from_secs(5));
        }
        std::iter::once(String::new())
            .chain(self.expanded.iter().cloned())
            .filter(|p| refresh || (!self.folders.contains_key(p) && !self.errors.contains_key(p)))
            .filter(|p| self.pending.insert(p.clone()))
            .collect()
    }
}

pub(crate) fn supported(client: &Client, key: &ThreadKey) -> bool {
    client.state.target(&key.target).is_some_and(|t| {
        t.connected() && t.entry.state.daemon.as_ref().is_some_and(|d| d.file_view)
    })
}

pub(crate) struct FileRequest {
    key: ThreadKey,
    root: String,
    path: String,
    action: Read,
}

enum Read {
    Folder(u64),
    Text {
        id: u64,
        generation: u64,
        pages: Pages,
        diff: bool,
    },
}

#[derive(Default)]
struct Pages {
    etag: Option<String>,
    old: Option<String>,
    new: Option<String>,
    old_offset: Option<u64>,
    new_offset: Option<u64>,
}
fn append(dest: &mut Option<String>, chunk: Option<String>) {
    if let Some(s) = chunk {
        dest.get_or_insert_with(String::new).push_str(&s);
    }
}

fn root(client: &Client, key: &ThreadKey) -> Option<String> {
    let s = client.state.summary(key)?;
    Some(
        s.worktree
            .as_ref()
            .map(|w| w.path.as_str())
            .or_else(|| client.state.project_of(key).map(|p| p.folder.as_str()))
            .unwrap_or(s.cwd.as_str())
            .to_owned(),
    )
}

impl Browsers {
    pub(crate) fn set_file_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        if self.file_wake.is_none() {
            let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
            self.terms.set_wake(wake.clone());
            self.file_wake = Some(wake);
        }
    }
    pub(crate) fn open_file_tab(
        &mut self,
        client: &mut Client,
        key: &ThreadKey,
        kind: TabKind,
        duplicate: bool,
    ) -> Result<u64, String> {
        if !supported(client, key) || root(client, key).is_none() {
            return Err("Worktree files unavailable".into());
        }
        if kind == TabKind::Browser {
            return Err("Expected a Files, File or Diff tab".into());
        }
        if !duplicate
            && let Some(area) = self.areas.get_mut(key)
            && let Some(t) = area.tabs.iter().find(|t| t.kind == kind)
        {
            area.active = t.id;
            area.chat = false;
            return Ok(t.id);
        }
        let id = self.insert(key);
        let tab = self.tab_mut(key, id).unwrap();
        let basename = kind
            .path()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("Files");
        tab.title = if matches!(kind, TabKind::Diff(_)) {
            format!("{basename} diff")
        } else {
            basename.into()
        };
        tab.kind = kind;
        self.reload_file(client, key, id);
        Ok(id)
    }
    pub(crate) fn reload_file(&mut self, client: &mut Client, key: &ThreadKey, id: u64) {
        let Some(t) = self.tab_mut(key, id) else {
            return;
        };
        let Some(path) = t.kind.path().map(str::to_owned) else {
            return;
        };
        t.generation += 1;
        t.document = None;
        t.work = None;
        t.error = None;
        let Some(root) = root(client, key).filter(|_| supported(client, key)) else {
            t.loading = false;
            t.error = Some("Target files unavailable".into());
            return;
        };
        t.loading = true;
        let action = Read::Text {
            id,
            generation: t.generation,
            pages: Pages::default(),
            diff: matches!(t.kind, TabKind::Diff(_)),
        };
        self.request_file(
            client,
            FileRequest {
                key: key.clone(),
                root,
                path,
                action,
            },
        );
    }
    fn request_file(&mut self, client: &mut Client, request: FileRequest) {
        let mut params = json!({"root":request.root,"path":request.path});
        let method = match &request.action {
            Read::Folder(_) => names::FS_TREE,
            Read::Text {
                pages, diff: true, ..
            } => {
                if let Some(offset) = pages.old_offset {
                    params["oldOffset"] = json!(offset);
                }
                if let Some(offset) = pages.new_offset {
                    params["newOffset"] = json!(offset);
                }
                names::GIT_FILE_DIFF
            }
            Read::Text { pages, .. } => {
                params["maxBytes"] = json!(4 * 1024 * 1024);
                if let Some(offset) = pages.new_offset {
                    params["offset"] = json!(offset);
                }
                names::FS_READ
            }
        };
        let rpc = client.preview_request(
            attach::names::REQUEST,
            json!({"target":request.key.target,"method":method,"params":params}),
        );
        self.pending.insert(rpc, Pending::File(request));
    }
    pub(crate) fn tick_files(
        &mut self,
        client: &mut Client,
        key: &ThreadKey,
        now: Instant,
    ) -> Option<Duration> {
        if !supported(client, key) {
            return None;
        }
        let root = root(client, key)?;
        let area = self.areas.get_mut(key)?;
        if !area
            .tabs
            .iter()
            .any(|t| t.id == area.active && t.kind == TabKind::Files)
        {
            return None;
        }
        let tree = &mut area.tree;
        let turn = client
            .state
            .transcripts
            .get(key)
            .and_then(|t| t.last_completed_turn.clone());
        if tree.last_turn != turn {
            tree.refresh();
        }
        tree.last_turn = turn;
        let epoch = area.epoch;
        let paths = tree.requests(now);
        let next = tree.due.map(|t| t.saturating_duration_since(now));
        for path in paths {
            self.request_file(
                client,
                FileRequest {
                    key: key.clone(),
                    root: root.clone(),
                    path,
                    action: Read::Folder(epoch),
                },
            );
        }
        next
    }
    pub(crate) fn file_reply(
        &mut self,
        client: &mut Client,
        req: FileRequest,
        result: Result<Value, String>,
    ) {
        let FileRequest {
            key,
            root,
            path,
            action,
        } = req;
        match action {
            Read::Folder(epoch) => {
                let Some(area) = self.areas.get_mut(&key).filter(|a| a.epoch == epoch) else {
                    return;
                };
                let tree = &mut area.tree;
                tree.pending.remove(&path);
                let reply = result.and_then(|v| {
                    serde_json::from_value::<FsTreeResult>(v).map_err(|e| e.to_string())
                });
                match reply {
                    Ok(folder) => {
                        tree.errors.remove(&path);
                        let previous = tree.folders.insert(path.clone(), folder);
                        if let Some(previous) = previous {
                            let next = &tree.folders[&path];
                            let changed: Vec<_> = area
                                .tabs
                                .iter()
                                .filter_map(|t| {
                                    let (dir, name) = t
                                        .kind
                                        .path()?
                                        .rsplit_once('/')
                                        .unwrap_or(("", t.kind.path()?));
                                    if dir != path {
                                        return None;
                                    }
                                    let entry = |f: &FsTreeResult| {
                                        f.entries.iter().find(|e| e.name == name).cloned()
                                    };
                                    (entry(&previous) != entry(next)).then_some(t.id)
                                })
                                .collect();
                            for id in changed {
                                self.reload_file(client, &key, id);
                            }
                        }
                    }
                    Err(e) => {
                        tree.errors.insert(path, e);
                    }
                }
            }
            Read::Text {
                id,
                generation,
                mut pages,
                diff,
            } => {
                let wake = self.file_wake.clone();
                let Some(t) = self
                    .tab_mut(&key, id)
                    .filter(|t| t.generation == generation)
                else {
                    return;
                };
                let apply = (|| -> Result<(), String> {
                    let v = result?;
                    let wake = move || {
                        if let Some(f) = wake {
                            f();
                        }
                    };
                    if diff {
                        let r: GitFileDiffResult =
                            serde_json::from_value(v).map_err(|e| e.to_string())?;
                        pages.check(r.etag)?;
                        append(&mut pages.old, r.old);
                        append(&mut pages.new, r.new);
                        pages.old_offset = r.next_old_offset;
                        pages.new_offset = r.next_new_offset;
                        if r.binary {
                            t.document = Some(Arc::new(Document::Notice("Binary file".into())));
                        } else if pages.old_offset.is_none() && pages.new_offset.is_none() {
                            let input = yaac_code::DiffInput {
                                path: path.clone(),
                                old_path: r.old_path.map(|p| p.as_str().into()),
                                old: pages.old.take(),
                                new: pages.new.take(),
                                notice: None,
                            };
                            t.work = Some(Work::Diff(
                                yaac_code::spawn_diff(input, wake).map_err(|e| e.to_string())?,
                            ));
                        }
                    } else {
                        let r: FsReadResult =
                            serde_json::from_value(v).map_err(|e| e.to_string())?;
                        pages.check(r.etag)?;
                        append(&mut pages.new, r.text);
                        pages.new_offset = r.next_offset;
                        if r.binary || r.truncated {
                            let why = if r.binary {
                                "Binary file"
                            } else {
                                "File exceeds 4 MiB"
                            };
                            t.document = Some(Arc::new(Document::Notice(format!(
                                "{why} ({} bytes)",
                                r.size
                            ))));
                        } else if pages.new_offset.is_none() {
                            t.work = Some(Work::File(
                                yaac_code::spawn_highlight(
                                    path.clone(),
                                    pages.new.take().unwrap_or_default(),
                                    wake,
                                )
                                .map_err(|e| e.to_string())?,
                            ));
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = apply {
                    t.error = Some(e);
                    t.loading = false;
                } else if t.document.is_some() {
                    t.loading = false;
                } else if t.work.is_none() {
                    self.request_file(
                        client,
                        FileRequest {
                            key,
                            root,
                            path,
                            action: Read::Text {
                                id,
                                generation,
                                pages,
                                diff,
                            },
                        },
                    );
                }
            }
        }
    }
    pub(crate) fn finish_file_jobs(&mut self) {
        for t in self
            .areas
            .values_mut()
            .flat_map(|a| a.tabs.iter_mut())
            .filter(|t| t.work.is_some())
        {
            let result = match t.work.as_ref().unwrap() {
                Work::File(j) => j.try_recv().map(Document::File),
                Work::Diff(j) => j.try_recv().map(|d| match d {
                    yaac_code::DiffResult::Text(d) => Document::Diff(d),
                    yaac_code::DiffResult::Notice(n) => Document::Notice(match n {
                        yaac_code::Notice::Binary { size } => format!("Binary file ({size} bytes)"),
                        yaac_code::Notice::TooLarge { size } => {
                            format!("File exceeds 4 MiB ({size} bytes)")
                        }
                    }),
                }),
            };
            match result {
                Ok(d) => t.document = Some(Arc::new(d)),
                Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                Err(e) => t.error = Some(e.to_string()),
            }
            t.work = None;
            t.loading = false;
        }
    }
}
impl Pages {
    fn check(&mut self, etag: String) -> Result<(), String> {
        if self.etag.as_ref().is_some_and(|e| e != &etag) {
            return Err("File changed during loading. Reload to read the current version.".into());
        }
        self.etag = Some(etag);
        Ok(())
    }
}

pub(crate) fn insert_excerpt(
    client: &mut Client,
    key: &ThreadKey,
    caret: usize,
    label: String,
    formatted: String,
) -> bool {
    if !client.state.may_act(key) || client.state.queue_edits.contains_key(key) {
        return false;
    }
    client
        .state
        .drafts
        .entry(key.clone())
        .or_default()
        .insert_excerpt(caret, label, formatted);
    client.draft_changed(&DraftRef::Thread(key.clone()));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_core::demo::{Scenario, threads};

    fn daemon(client: &mut Client, key: &ThreadKey) {
        client
            .state
            .targets
            .iter_mut()
            .find(|t| t.entry.id == key.target)
            .unwrap()
            .entry
            .state
            .daemon = Some(attach::DaemonSummary {
            build: "test".into(),
            older: false,
            can_update: false,
            schema_hash_matches: true,
            port_forward: true,
            file_view: true,
            terminal: false,
            terminal_unavailable_reason: None,
        });
    }
    #[test]
    fn tree_is_lazy_and_polls_only_while_visible() {
        let (mut client, _) = crate::app::demo_client(Scenario::Fleet);
        let key = threads::orchestrator();
        daemon(&mut client, &key);
        let mut b = Browsers::default();
        let id = b
            .open_file_tab(&mut client, &key, TabKind::Files, false)
            .unwrap();
        let now = Instant::now();
        b.tick(&mut client, None, now);
        assert!(b.pending.is_empty());
        b.tick(&mut client, Some(&key), now);
        let area = b.areas.get_mut(&key).unwrap();
        assert_eq!(area.tree.pending, HashSet::from([String::new()]));
        area.tree.expand("src", true);
        b.tick(&mut client, Some(&key), now);
        let area = b.areas.get_mut(&key).unwrap();
        assert!(area.tree.pending.contains("src"));
        area.tree.expand("src/deep", true);
        area.tree.expand("src", false);
        assert!(area.tree.expanded.is_empty());
        let file = TabKind::File("src/app.rs".into());
        let original = b
            .open_file_tab(&mut client, &key, file.clone(), false)
            .unwrap();
        let duplicate = b
            .open_file_tab(&mut client, &key, file.clone(), true)
            .unwrap();
        assert_ne!(original, duplicate);
        assert_eq!(
            b.open_file_tab(&mut client, &key, file, false).unwrap(),
            original
        );
        let diff = b
            .open_file_tab(&mut client, &key, TabKind::Diff("src/app.rs".into()), false)
            .unwrap();
        assert_ne!(diff, original);
        assert!(
            b.tick(&mut client, Some(&key), now + Duration::from_secs(8))
                .is_none()
        );
        let other = ThreadKey::new("other", "same");
        let browser = b.insert(&other);
        assert_eq!(b.tab(&other, browser).unwrap().kind, TabKind::Browser);
        assert!(b.tab(&other, id).is_none());
    }
    #[test]
    fn excerpt_inserted_at_unicode_caret_is_sent_as_plain_text() {
        let (mut client, hub) = crate::app::demo_client(Scenario::Fleet);
        let key = threads::billing();
        let draft = client.state.drafts.entry(key.clone()).or_default();
        draft.text = "λ left right".into();
        let code = yaac_code::file_excerpt("src/app.rs", Some("rust"), 7..=7, "let x = 4;");
        assert!(insert_excerpt(
            &mut client,
            &key,
            6,
            code.label,
            code.text.clone()
        ));
        assert!(client.state.drafts[&key].text.contains("⟦app.rs:7⟧"));
        assert!(client.send(&key));
        let (_, _, params) = hub
            .requests()
            .into_iter()
            .find(|(_, m, _)| m == "turn/start")
            .unwrap();
        assert_eq!(
            params["input"],
            json!([{"type":"text","text":format!("λ left{} right",code.text)}])
        );
    }
}
