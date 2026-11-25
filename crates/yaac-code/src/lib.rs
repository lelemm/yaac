//! Renderer-neutral syntax colours, diffs, and composer excerpts for the Files pane.

mod diff;
mod excerpt;
mod highlight;

pub use diff::{Diff, DiffInput, DiffResult, DiffRow, DiffView, MAX_FILE_BYTES, Notice};
pub use excerpt::{DiffLineKind, Excerpt, diff_excerpt, file_excerpt, terminal_excerpt};
pub use highlight::{Highlighted, Language, Line, MAX_LINE_BYTES, ScopeClass, Span};

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// A background result. Poll after the wake callback and drop on close or reload.
#[derive(Debug)]
pub struct Job<T> {
    result: Receiver<T>,
    cancelled: Arc<AtomicBool>,
}

impl<T> Job<T> {
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.result.try_recv()
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

/// Highlight a UTF-8 file already checked against the file size and binary limits.
/// The callback runs on the worker after publishing the result.
pub fn spawn_highlight(
    path: String,
    text: String,
    wake: impl FnOnce() + Send + 'static,
) -> io::Result<Job<Highlighted>> {
    spawn(wake, move |cancelled| {
        highlight::highlight(&path, text, cancelled)
    })
}

/// Build both diff layouts and highlight both sources on a worker.
pub fn spawn_diff(
    input: DiffInput,
    wake: impl FnOnce() + Send + 'static,
) -> io::Result<Job<DiffResult>> {
    spawn(wake, move |cancelled| diff::build(input, cancelled))
}

fn spawn<T: Send + 'static>(
    wake: impl FnOnce() + Send + 'static,
    work: impl FnOnce(&AtomicBool) -> Option<T> + Send + 'static,
) -> io::Result<Job<T>> {
    let (sender, result) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    std::thread::Builder::new()
        .name("yaac-code".into())
        .spawn(move || {
            let value = work(&flag);
            // Publish the result before notifying the front end.
            if let Some(value) = value.filter(|_| !is_cancelled(&flag)) {
                let _ = sender.send(value);
            }
            drop(sender);
            if !is_cancelled(&flag) {
                wake();
            }
        })?;
    Ok(Job { result, cancelled })
}

fn is_cancelled(cancelled: &AtomicBool) -> bool {
    cancelled.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests;
