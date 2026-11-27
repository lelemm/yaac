//! `yaac-core`: the window's state, pure.
//!
//! * [`state`] — the application state ([`state::AppState`]): targets, providers, the thread
//!   index, transcripts, hints, budgets, drafts — and the reducers that apply hub messages.
//! * [`client::Client`] — turns user actions into attach-protocol messages and applies the
//!   hub's answers, through a [`link::HubLink`].
//! * View models, each a pure function of the state: [`sidebar`] (tree and Needs you),
//!   [`attention`], [`header`] (header, banners, composer), [`controls`] (the control row),
//!   [`agents`] (the agents panel), [`requests`] (request cards), [`images`] (image routing), [`attachments`] (composer chips and uploads),
//!   [`permission`] (the agents permission prompt, a seam), [`activity`] (which items are
//!   muted activity lines), [`markstyle`] (markdown styled in place).
//! * [`ssh_setup`] (SSH setup and revoke: the step machine and its backend trait),
//!   [`settings`] (the settings pages), [`fork`] (the fork dialog), [`first_run`] (screen 07 and
//!   the project picker), [`mode_confirm`] (confirming a mode that bypasses approvals),
//!   [`prefs`] (window-side choices persisted through the hub), [`display`] (the renderer
//!   choice and its fallback, the tray).
//! * [`demo`] — **demo only**: a scripted in-memory hub playing the fleet of the screen brief.
//!
//! No I/O, no clock, no UI toolkit. Nothing here branches on provider kind.

pub mod activity;
pub mod agentcall;
pub mod agents;
pub mod attachments;
pub mod attention;
pub mod bulk;
pub mod client;
pub mod compose;
pub mod commands;
pub mod copies;
pub mod components;
pub mod controls;
pub mod demo;
pub mod diff;
pub mod display;
pub mod first_run;
pub mod fork;
pub mod header;
pub mod hints;
pub mod history;
pub mod hover;
pub mod images;
pub mod input;
pub mod key;
pub mod limits;
pub mod keys;
pub mod link;
pub mod markstyle;
pub mod message;
pub mod mode_confirm;
pub mod moving;
pub mod mute;
pub mod pr;
pub mod prefs;
pub mod themes;
pub mod settings;
pub mod ssh_setup;
pub mod requests;
pub mod running_commands;
pub mod shell_block;
pub mod selection;
pub mod sending;
pub mod sidebar;
pub mod skills;
pub mod permission;
pub mod projects;
pub mod queue;
pub mod state;
pub mod strip;
pub mod thread_menu;
pub mod time;
pub mod timeline;
pub mod transcript;
pub mod turns;
pub mod update;
pub mod voice;
pub mod voice_call;
pub mod voice_language;

pub use key::ThreadKey;
