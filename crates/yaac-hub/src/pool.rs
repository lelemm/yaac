//! The hub's small bounded worker pool (docs/02-architecture.md#processes): file reads and
//! hashing for uploads run here, never on the I/O thread and never on the store worker, so a
//! slow disk or a large file cannot stall the connections or the cache.
//!
//! A fixed number of threads take jobs from one queue; a finished job may hand an event back
//! to the core. Jobs carry their own cancellation (an upload's flag) and are short: a hash is
//! one pass over one file, a read is one bounded chunk.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;

use crate::core::CoreEvent;

type Job = Box<dyn FnOnce() -> Option<CoreEvent> + Send>;

/// Handle to the pool. Dropping it lets the threads finish their current job and exit.
#[derive(Debug)]
pub(crate) struct Pool {
    jobs: mpsc::Sender<Job>,
}

impl Pool {
    /// Starts `threads` workers (at least one); completions go to `core`.
    pub(crate) fn start(threads: usize, core: &UnboundedSender<CoreEvent>) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for n in 0..threads.max(1) {
            let (rx, core) = (rx.clone(), core.clone());
            std::thread::Builder::new().name(format!("yaac-hub-pool-{n}")).spawn(move || {
                loop {
                    // Hold the lock only to take a job, never while running it.
                    let job = match rx.lock() {
                        Ok(rx) => rx.recv(),
                        Err(_) => return,
                    };
                    let Ok(job) = job else { return };
                    if let Some(event) = job() {
                        let _ = core.send(event);
                    }
                }
            })?;
        }
        Ok(Self { jobs: tx })
    }

    /// Queues a job.
    pub(crate) fn submit(&self, job: impl FnOnce() -> Option<CoreEvent> + Send + 'static) {
        let _ = self.jobs.send(Box::new(job));
    }
}
