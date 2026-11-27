//! Drafts kept on this device (P393): a client attached to a hub on another machine (the phone
//! over Tailscale) keeps its composers' text and chips in a file of its own, never in the hub's
//! draft store, so typing sends nothing over the link and no other client's draft appears here.
//!
//! The file is one JSON object, draft key → the stored form the core hands out
//! ([`yaac_core::client::Client::take_device_drafts`]). It is small (text and chip metadata;
//! attachment bytes stay where they are, uploaded or in the hub's blob store). Writes go to a
//! writer thread, newest snapshot wins, through a temporary file and a rename; the last one is
//! written in place when the controller stops.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use serde_json::Value;

/// The file's name in the client's data directory.
pub(crate) const FILE: &str = "drafts.json";

pub(crate) struct DeviceDrafts {
    path: PathBuf,
    drafts: BTreeMap<String, Value>,
    writer: Option<(mpsc::Sender<Vec<u8>>, std::thread::JoinHandle<()>)>,
}

impl std::fmt::Debug for DeviceDrafts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceDrafts").field("path", &self.path).field("drafts", &self.drafts.len()).finish()
    }
}

impl DeviceDrafts {
    /// Reads what is stored in `dir` (nothing, when the file is missing or unreadable).
    pub(crate) fn open(dir: &Path) -> Self {
        let path = dir.join(FILE);
        let drafts = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, Value>>(&bytes).ok())
            .unwrap_or_default();
        Self { path, drafts, writer: None }
    }

    /// The stored drafts, for [`yaac_core::client::Client::keep_drafts_on_device`].
    pub(crate) fn stored(&self) -> Vec<Value> {
        self.drafts.values().cloned().collect()
    }

    /// Applies the client's changes; writes them (off this thread) when something changed.
    pub(crate) fn absorb(&mut self, writes: Vec<(String, Option<Value>)>) {
        if self.apply(writes) {
            self.save_soon();
        }
    }

    /// Applies the client's changes and writes them now (the controller is stopping).
    pub(crate) fn absorb_now(&mut self, writes: Vec<(String, Option<Value>)>) {
        let changed = self.apply(writes);
        // Let the writer thread finish first: what it still holds is older than this.
        if let Some((tx, thread)) = self.writer.take() {
            drop(tx);
            let _ = thread.join();
        }
        if changed {
            let _ = write(&self.path, &self.bytes());
        }
    }

    fn apply(&mut self, writes: Vec<(String, Option<Value>)>) -> bool {
        let mut changed = false;
        for (key, value) in writes {
            changed |= match value {
                Some(v) => self.drafts.insert(key, v.clone()).is_none_or(|old| old != v),
                None => self.drafts.remove(&key).is_some(),
            };
        }
        changed
    }

    fn bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.drafts).unwrap_or_default()
    }

    fn save_soon(&mut self) {
        let bytes = self.bytes();
        if let Some((tx, _)) = &self.writer
            && tx.send(bytes.clone()).is_ok()
        {
            return;
        }
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let path = self.path.clone();
        let spawned = std::thread::Builder::new().name("yaac-device-drafts".into()).spawn(move || {
            while let Ok(mut bytes) = rx.recv() {
                // Only the newest snapshot matters.
                while let Ok(newer) = rx.try_recv() {
                    bytes = newer;
                }
                let _ = write(&path, &bytes);
            }
        });
        match spawned {
            Ok(thread) if tx.send(bytes.clone()).is_ok() => self.writer = Some((tx, thread)),
            _ => {
                let _ = write(&self.path, &bytes);
            }
        }
    }
}

fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn drafts_survive_a_restart_and_a_forgotten_one_stays_gone() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = DeviceDrafts::open(dir.path());
        assert!(d.stored().is_empty());
        d.absorb(vec![("t:a".into(), Some(json!({ "key": "t:a", "text": "one" }))), ("t:b".into(), Some(json!({ "key": "t:b", "text": "two" })))]);
        d.absorb_now(vec![("t:b".into(), None), ("t:a".into(), Some(json!({ "key": "t:a", "text": "one more" })))]);
        let again = DeviceDrafts::open(dir.path());
        assert_eq!(again.stored(), vec![json!({ "key": "t:a", "text": "one more" })]);
    }
}
