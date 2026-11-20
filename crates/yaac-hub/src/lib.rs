//! # yaac-hub
//!
//! The hub (docs/02-architecture.md#processes, docs/10-agent-mesh.md#the-hub): the headless
//! process on the host that owns the **one connection to each target**, the host cache, the
//! durable draft store, the attach server windows connect to, and the mesh relay queue.
//!
//! ```text
//! window ──attach socket──► hub ──one stream each──► yaacd on every target
//! ```
//!
//! | Module | What |
//! | --- | --- |
//! | [`run_hub`] | The entry point (`yaac hub` calls it): one I/O thread, one store thread. |
//! | [`config`] | [`HubConfig`], connector factory, timings. |
//! | [`paths`] | Where files go and the environment variables that move them. |
//! | [`settings`] | `hub.json`: targets (id, name, transport, enabled) and hub settings. |
//! | [`protocol`] | The hub's additions to `yaac_protocol::attach`. |
//! | [`client`] | The window side: connect to the socket, `send` / `try_recv`, never blocking. |
//! | [`cache`] | The disposable SQLite cache; items and position commit together. |
//! | [`drafts`] | The durable draft store (own file, own migrations). |
//! | [`relay`] | The durable mesh relay queue (own file, own migrations). |
//!
//! Threads: one tokio current-thread runtime on the calling thread does all I/O and owns all
//! protocol state; one store thread does all SQLite work, in submission order. Nothing else.

pub mod autostart;
pub mod cache;
pub mod client;
pub mod config;
mod conn;
mod core;
pub mod drafts;
mod lock;
mod migrate;
pub mod paths;
pub mod protocol;
pub mod relay;
mod server;
pub mod settings;
mod worker;

use std::time::Duration;

use tokio::sync::mpsc;

pub use crate::config::{Backoff, ConnectorFactory, DefaultConnectors, HubConfig};
pub use crate::core::ExitReason;
pub use crate::lock::{HubLock, LockError};
pub use crate::migrate::MigrateError;
pub use crate::paths::HubPaths;
pub use crate::server::ListenError;
use crate::core::{Core, CoreEvent, Loaded};
use crate::settings::Settings;

/// Why `run_hub` returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubExit {
    /// The reason.
    pub reason: ExitReason,
}

/// The hub could not start (or failed while running).
#[derive(Debug, thiserror::Error)]
pub enum HubError {
    /// Another hub is running for this user. The caller should attach to it instead.
    #[error(transparent)]
    AlreadyRunning(LockError),
    /// The lock file could not be used.
    #[error(transparent)]
    Lock(LockError),
    /// A directory could not be created.
    #[error("cannot create {path}: {source}")]
    Dir {
        /// The directory.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// The settings file.
    #[error(transparent)]
    Settings(#[from] settings::SettingsError),
    /// The cache could not be opened even after rebuilding it.
    #[error(transparent)]
    Cache(#[from] cache::CacheError),
    /// The draft store (it is never dropped; a newer schema refuses to start).
    #[error(transparent)]
    Drafts(#[from] drafts::DraftsError),
    /// The relay queue (it is never dropped; a newer schema refuses to start).
    #[error(transparent)]
    Relay(#[from] relay::RelayError),
    /// The attach socket.
    #[error(transparent)]
    Listen(#[from] ListenError),
    /// The runtime or a thread.
    #[error("hub runtime: {0}")]
    Runtime(#[from] std::io::Error),
}

/// Runs a hub until it is idle long enough, asked to stop (`hub/shutdown`), or signalled.
/// Blocks the calling thread, which becomes the hub's one I/O thread.
///
/// Fails at once with [`HubError::AlreadyRunning`] if another hub holds the lock.
pub fn run_hub(config: HubConfig) -> Result<HubExit, HubError> {
    let paths = config.paths.clone();
    for dir in [&paths.config_dir, &paths.data_dir, &paths.runtime_dir] {
        paths::ensure_private_dir(dir).map_err(|source| HubError::Dir { path: dir.display().to_string(), source })?;
    }
    let lock = HubLock::acquire(&paths.lock_file()).map_err(|e| match e {
        LockError::Held(_) => HubError::AlreadyRunning(e),
        LockError::Io { .. } => HubError::Lock(e),
    })?;

    let settings_file = paths.settings_file();
    let mut settings = match Settings::load(&settings_file)? {
        Some(settings) => settings,
        None => Settings::first_run(config.seed_local_target, config.first_run_always),
    };
    if settings.client_id.is_none() {
        settings.client_id = Some(yaac_protocol::ClientId::new(settings::new_id("c")));
    }
    settings.save(&settings_file)?;

    let mut cache = cache::Cache::open(&paths.cache_file())?;
    let mut drafts = drafts::DraftStore::open(&paths.drafts_file())?;
    let relay = relay::RelayQueue::open(&paths.relay_file())?;
    // Operations that were in flight when the previous hub stopped have an unknown outcome.
    drafts.mark_all_uncertain()?;
    // Rows of targets that no longer exist (removed while the cache was being written).
    let known: Vec<_> = settings.targets.iter().map(|t| t.id.clone()).collect();
    let loaded = Loaded {
        positions: cache.positions()?.into_iter().filter(|(t, _, _)| known.contains(t)).collect(),
        tokens: cache.sync_tokens()?.into_iter().filter(|(t, _)| known.contains(t)).collect(),
        states: cache.thread_states()?.into_iter().filter(|(t, _, _)| known.contains(t)).collect(),
        in_flight: relay.in_flight()?,
    };
    let _ = cache.checkpoint();

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let reason = runtime.block_on(async move {
        let listener = server::Listener::bind(&paths.socket())?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let worker = worker::Worker::start(worker::Stores { cache, drafts, relay }, tx.clone())?;
        let core_handles_signals = config.handle_signals;
        let mut core = Core::new(config, settings, worker, tx.clone(), loaded);
        let (writers_tx, mut writers_rx) = mpsc::unbounded_channel();
        let reason = {
            let serve = server::serve(&listener, tx.clone(), writers_tx);
            tokio::pin!(serve);
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let handle_signals = core_handles_signals;
            let signals = signals(tx.clone(), handle_signals);
            tokio::pin!(signals);
            let mut serving = true;
            loop {
                tokio::select! {
                    () = &mut serve, if serving => serving = false,
                    () = &mut signals => {}
                    event = rx.recv() => match event {
                        Some(event) => core.handle(event),
                        None => break ExitReason::Signal,
                    },
                    _ = tick.tick() => core.tick(),
                }
                if let Some(reason) = core.exit.clone() {
                    break reason;
                }
            }
        };
        crate::core::log(format!("loop ended: {reason:?}"));
        listener.remove();
        let (worker, clients) = core.finish();
        drop(clients);
        // Let each window's writer send what is queued (the `hub/shutdown` answer).
        while let Ok(writer) = writers_rx.try_recv() {
            let _ = tokio::time::timeout(Duration::from_millis(500), writer).await;
        }
        // Give aborted supervisors a moment to drop their streams (killing the children).
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(rx);
        let _ = tokio::task::spawn_blocking(move || worker.finish()).await;
        crate::core::log("worker finished");
        Ok::<_, HubError>(reason)
    })?;
    drop(runtime);
    drop(lock);
    Ok(HubExit { reason })
}

/// Tells the core when SIGINT or SIGTERM arrives. Never resolves.
async fn signals(core: mpsc::UnboundedSender<CoreEvent>, enabled: bool) {
    if !enabled {
        return std::future::pending().await;
    }
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        if tokio::signal::ctrl_c().await.is_err() {
            return std::future::pending().await;
        }
    }
    let _ = core.send(CoreEvent::Signal);
    std::future::pending::<()>().await;
}
