//! # yaac-transport
//!
//! The transport layer has one job (docs/02-architecture.md#transports): produce a reliable,
//! binary-clean, bidirectional byte stream to a `yaacd` on the target, and report when it
//! drops. Exactly one child process per target; the [`Stream`] it yields is the target's one
//! connection, and dropping it kills that child.
//!
//! | Item | What |
//! | --- | --- |
//! | [`Connector`] | Opens the one stream to a target. Local now; SSH and WSL are later packages. |
//! | [`Stream`] | Reader, writer, and the [`ChildGuard`] that kills the child when dropped. |
//! | [`local::LocalConnector`] | `<private runtime> yaacd.mjs connect`, no shell in between. |
//!
//! The stream carries the YAAC protocol from its first byte on the local target (there is no
//! stage 0 locally). A remote connector will complete stage 0 inside `connect` and hand back a
//! stream positioned at the first protocol byte, so callers never see the bootstrap.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};

pub mod local;

pub use local::{LocalConfig, LocalConnector};

/// A boxed, sendable future (the trait stays object-safe without an extra dependency).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a connector is told about the connection it is opening.
#[derive(Clone, Debug, Default)]
pub struct ConnectCtx {
    /// The target's immutable id (for diagnostics only).
    pub target_id: String,
    /// The target's display name (for diagnostics only).
    pub target_name: String,
}

/// Opens the one connection to a target.
///
/// Implementations spawn exactly one child process (or, in tests, an in-memory peer) and
/// complete whatever bootstrap the transport needs before returning. The trait is
/// object-safe so the hub can hold a `Box<dyn Connector>` per target; SSH and WSL connectors
/// implement it the same way.
pub trait Connector: Send + Sync {
    /// Spawns the one child process for this target and completes the bootstrap handshake.
    fn connect<'a>(&'a self, ctx: &'a ConnectCtx) -> BoxFuture<'a, Result<Stream, ConnectError>>;

    /// A short description for diagnostics (`local: node …/yaacd.mjs connect`).
    fn describe(&self) -> String;
}

/// The byte stream to one `yaacd`.
pub struct Stream {
    /// Bytes from the target.
    pub reader: Box<dyn AsyncRead + Send + Unpin>,
    /// Bytes to the target.
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
    /// Kills the child when dropped.
    pub child: ChildGuard,
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream").field("child", &self.child).finish_non_exhaustive()
    }
}

/// Owns whatever sits on the other end of a [`Stream`]; dropping it ends that process.
#[derive(Default)]
pub struct ChildGuard {
    child: Option<tokio::process::Child>,
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

impl ChildGuard {
    /// Guards a child process: it is killed when the guard is dropped.
    pub fn process(child: tokio::process::Child) -> Self {
        Self { child: Some(child), on_drop: None }
    }

    /// A guard that runs `f` when dropped (in-memory peers in tests).
    pub fn on_drop(f: impl FnOnce() + Send + 'static) -> Self {
        Self { child: None, on_drop: Some(Box::new(f)) }
    }

    /// A guard that owns nothing.
    pub fn none() -> Self {
        Self::default()
    }

    /// The child's process id, while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(tokio::process::Child::id)
    }

    /// Kills the child now (also done on drop).
    pub fn kill(&mut self) {
        if let Some(child) = self.child.as_mut() {
            // The child may already have exited; nothing else to do then.
            let _ = child.start_kill();
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill();
        if let Some(f) = self.on_drop.take() {
            f();
        }
    }
}

impl fmt::Debug for ChildGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChildGuard").field("pid", &self.pid()).field("on_drop", &self.on_drop.is_some()).finish()
    }
}

/// Why a connection could not be opened. The message is shown to the user as-is.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// The runtime or daemon bundle is missing.
    #[error("{0}")]
    NotInstalled(String),
    /// Spawning the child failed.
    #[error("could not start {what}: {source}")]
    Spawn {
        /// What was being started.
        what: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// The transport is not available in this build or on this host.
    #[error("{0}")]
    Unsupported(String),
    /// Anything else, with a message to show as-is.
    #[error("{0}")]
    Other(String),
}
