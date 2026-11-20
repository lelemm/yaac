//! `yaac-core`: the window's state, pure.
//!
//! * [`state`] — the application state ([`state::AppState`]): targets, providers, the thread
//!   index, transcripts, hints, budgets, drafts — and the reducers that apply hub messages.
//! * [`client::Client`] — turns user actions into attach-protocol messages and applies the
//!   hub's answers, through a [`link::HubLink`].
//! * View models, each a pure function of the state: [`sidebar`] (tree and Needs you),
//!   [`attention`], [`header`] (header, banners, composer), [`controls`] (the control row),
//!   [`agents`] (the agents panel), [`requests`] (request cards), [`images`] (image routing),
//!   [`permission`] (the agents permission prompt, a seam), [`activity`] (which items are
//!   muted activity lines), [`markstyle`] (markdown styled in place).
//! * [`ssh_setup`] (SSH setup and revoke: the step machine and its backend trait),
//!   [`settings`] (the settings pages), [`fork`] (the fork dialog), [`first_run`] (screen 07 and
//!   the project picker), [`mode_confirm`] (confirming a mode that bypasses approvals),
//!   [`prefs`] (window-side choices persisted through the hub).
//! * [`demo`] — **demo only**: a scripted in-memory hub playing the fleet of the screen brief.
//!
//! No I/O, no clock, no UI toolkit. Nothing here branches on provider kind.

pub mod activity;
pub mod agents;
pub mod attention;
pub mod client;
pub mod controls;
pub mod demo;
pub mod diff;
pub mod first_run;
pub mod fork;
pub mod header;
pub mod hints;
pub mod hover;
pub mod images;
pub mod key;
pub mod link;
pub mod markstyle;
pub mod mode_confirm;
pub mod prefs;
pub mod settings;
pub mod ssh_setup;
pub mod requests;
pub mod sidebar;
pub mod permission;
pub mod projects;
pub mod state;
pub mod time;
pub mod transcript;

pub use key::ThreadKey;
