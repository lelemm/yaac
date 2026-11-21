//! # yaac-winsys
//!
//! The only place YAAC calls the Windows API directly. Every other crate forbids `unsafe`;
//! this one wraps the few calls Windows needs in safe functions and is empty on every other
//! platform.
//!
//! | Function | Used by |
//! | --- | --- |
//! | [`attach_parent_console`] | `yaac.exe` is a GUI-subsystem program (no console window pops up); `yaac hub` and `--screenshots` reattach to the terminal they were started from. |
//! | [`current_user_sid`] | The attach pipe's name and access control. |
//! | [`create_pipe_server`] | The hub's attach pipe: one instance, accessible only to the current user, never to remote clients. |
//! | [`owned_by_current_user`] | The window checks the pipe it opened belongs to its own user (no squatting by another user). |
//! | [`claim_window_instance`], [`WindowInstance`] | One window per user session: a second `yaac` shows the running window (it may be hidden in the tray, D127) and exits. |
//! | [`registry_set_string`], [`registry_get_string`], [`registry_delete_value`], [`registry_delete_tree`] | The per-user `Run` value that starts the hub at login (under `HKEY_CURRENT_USER` only). |

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::*;
