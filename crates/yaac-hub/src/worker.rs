//! The store worker: the one thread that touches SQLite.
//!
//! The I/O thread never blocks on a disk write. It hands each write to this worker as a job;
//! jobs run strictly in the order they were submitted (so a thread's commits never reorder),
//! and each job may hand an event back to the hub core when it finishes — for example "these
//! events are committed; forward them and acknowledge".

use std::sync::mpsc;
use std::thread::JoinHandle;

use tokio::sync::mpsc::UnboundedSender;

use crate::cache::Cache;
use crate::core::CoreEvent;
use crate::drafts::DraftStore;
use crate::relay::RelayQueue;

/// The stores the worker owns.
#[derive(Debug)]
pub(crate) struct Stores {
    pub(crate) cache: Cache,
    pub(crate) drafts: DraftStore,
    pub(crate) relay: RelayQueue,
}

type Job = Box<dyn FnOnce(&mut Stores) -> Option<CoreEvent> + Send>;

/// Handle to the worker thread.
#[derive(Debug)]
pub(crate) struct Worker {
    jobs: Option<mpsc::Sender<Job>>,
    thread: Option<JoinHandle<Stores>>,
}

impl Worker {
    /// Starts the worker; completions go to `core`.
    pub(crate) fn start(mut stores: Stores, core: UnboundedSender<CoreEvent>) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new().name("yaac-hub-store".into()).spawn(move || {
            while let Ok(job) = rx.recv() {
                if let Some(event) = job(&mut stores) {
                    // The core is gone only while the hub is shutting down.
                    let _ = core.send(event);
                }
            }
            stores
        })?;
        Ok(Self { jobs: Some(tx), thread: Some(thread) })
    }

    /// Queues a job.
    pub(crate) fn submit(&self, job: impl FnOnce(&mut Stores) -> Option<CoreEvent> + Send + 'static) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Box::new(job));
        }
    }

    /// Runs every queued job, checkpoints the stores, and stops the worker.
    pub(crate) fn finish(mut self) -> Option<Stores> {
        self.jobs.take();
        let mut stores = self.thread.take()?.join().ok()?;
        let _ = stores.relay.checkpoint();
        let _ = stores.drafts.checkpoint();
        let _ = stores.cache.checkpoint();
        Some(stores)
    }
}
