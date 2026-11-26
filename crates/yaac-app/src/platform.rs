//! What the window asks of the operating system: file dialogs, the clipboard, opening a URL
//! or a document.
//!
//! A desktop build (the `desktop` feature) answers these itself: rfd for the dialogs, arboard
//! for the clipboard, the system opener for URLs. A build without it (the Android library,
//! `crates/yaac-android`) has none of those, and each call goes to the [`PlatformHooks`] the
//! embedder installed with [`set_hooks`]. Until one is installed — and for every method an
//! embedder leaves at its default — the call answers "nothing": no file was picked, the
//! clipboard is empty, the URL was not opened. Every caller already handles that answer (a
//! cancelled dialog), so a missing hook is never an error.
//!
//! All of these block and run on worker threads only, never on the UI thread.

use std::path::PathBuf;

/// A file-type filter for a dialog: a label and the extensions it accepts (no dots).
#[derive(Clone, Copy, Debug)]
pub struct Filter<'a> {
    pub name: &'a str,
    pub extensions: &'a [&'a str],
}

/// What an embedder without a desktop provides. Every method has a "nothing happened"
/// default, so an embedder fills only what it has.
///
/// Paths are local files the controller can open: an embedder whose system hands out
/// something else (Android's `content://` URIs) copies the picked file into its own storage
/// first and returns that copy's path.
pub trait PlatformHooks: Send + Sync {
    /// Pick one or more files to attach. `None` when cancelled.
    fn pick_files(&self, title: &str) -> Option<Vec<PathBuf>> {
        let _ = title;
        None
    }

    /// Pick one file, optionally of one type. `None` when cancelled.
    fn pick_file(&self, title: &str, filter: Option<Filter<'_>>) -> Option<PathBuf> {
        let _ = (title, filter);
        None
    }

    /// Pick a folder. `None` when cancelled.
    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        let _ = title;
        None
    }

    /// Let the user choose where to save a file named `name`, and copy `staged` (the file's
    /// content, already written) there. `Ok(false)` when cancelled. `staged` is deleted
    /// afterwards by the caller.
    fn save_file(&self, name: &str, filter: Option<Filter<'_>>, staged: &std::path::Path) -> std::io::Result<bool> {
        let _ = (name, filter, staged);
        Ok(false)
    }

    /// Tell the user something went wrong with what they asked (a failed save).
    fn message(&self, title: &str, text: &str) {
        let _ = (title, text);
    }

    /// The clipboard's text, if it holds any.
    fn clipboard_text(&self) -> Option<String> {
        None
    }

    /// Files on the clipboard (a copied picture or document), as local copies. Asked only
    /// when pasting into a composer, before the text.
    fn clipboard_files(&self) -> Option<Vec<PathBuf>> {
        None
    }

    /// Open an `http`/`https` URL in the user's browser.
    fn open_url(&self, url: &str) -> std::io::Result<()> {
        let _ = url;
        Err(unsupported())
    }

    /// Open a local document with the system's viewer.
    fn open_document(&self, path: &std::path::Path) -> std::io::Result<()> {
        let _ = path;
        Err(unsupported())
    }
}

fn unsupported() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Unsupported, "this device cannot do that yet")
}

static HOOKS: std::sync::OnceLock<Box<dyn PlatformHooks>> = std::sync::OnceLock::new();

/// Installs the embedder's hooks, once per process. Returns `false` if hooks were already
/// installed (the first ones stay).
pub fn set_hooks(hooks: Box<dyn PlatformHooks>) -> bool {
    HOOKS.set(hooks).is_ok()
}

#[cfg(not(feature = "desktop"))]
fn hooks() -> Option<&'static dyn PlatformHooks> {
    HOOKS.get().map(Box::as_ref)
}

#[cfg(feature = "desktop")]
mod native {
    use super::Filter;
    use std::path::PathBuf;

    fn dialog(title: Option<&str>, filter: Option<Filter<'_>>) -> rfd::FileDialog {
        let mut d = rfd::FileDialog::new();
        if let Some(title) = title {
            d = d.set_title(title);
        }
        if let Some(f) = filter {
            d = d.add_filter(f.name, f.extensions);
        }
        d
    }

    pub(super) fn pick_files(title: &str) -> Option<Vec<PathBuf>> {
        dialog(Some(title), None).pick_files()
    }

    pub(super) fn pick_file(title: Option<&str>, filter: Option<Filter<'_>>) -> Option<PathBuf> {
        dialog(title, filter).pick_file()
    }

    pub(super) fn pick_folder(title: &str) -> Option<PathBuf> {
        dialog(Some(title), None).pick_folder()
    }

    pub(super) fn save_file(name: &str, filter: Option<Filter<'_>>) -> Option<std::path::PathBuf> {
        dialog(None, filter).set_file_name(name).save_file()
    }

    pub(super) fn message(title: &str, text: &str) {
        rfd::MessageDialog::new().set_title(title).set_description(text).show();
    }

    pub(super) fn clipboard_text() -> Option<String> {
        arboard::Clipboard::new().and_then(|mut c| c.get_text()).ok()
    }
}

/// Pick files to attach. `None` when cancelled.
pub(crate) fn pick_files(title: &str) -> Option<Vec<PathBuf>> {
    #[cfg(feature = "desktop")]
    return native::pick_files(title);
    #[cfg(not(feature = "desktop"))]
    hooks()?.pick_files(title)
}

/// Pick one file. The desktop dialog shows no title when `title` is `None`.
pub(crate) fn pick_file(title: Option<&str>, filter: Option<Filter<'_>>) -> Option<PathBuf> {
    #[cfg(feature = "desktop")]
    return native::pick_file(title, filter);
    #[cfg(not(feature = "desktop"))]
    hooks()?.pick_file(title.unwrap_or_default(), filter)
}

/// Pick a folder. `None` when cancelled.
pub(crate) fn pick_folder(title: &str) -> Option<PathBuf> {
    #[cfg(feature = "desktop")]
    return native::pick_folder(title);
    #[cfg(not(feature = "desktop"))]
    hooks()?.pick_folder(title)
}

/// Asks where to save `name` and has `write` put the content there. `Ok(false)` when
/// cancelled. On the desktop `write` writes the chosen file; without it, `write` writes a
/// staged copy in the temporary folder that the embedder then saves where the user chose.
pub(crate) fn save_with(
    name: &str,
    filter: Option<Filter<'_>>,
    write: impl FnOnce(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<bool> {
    #[cfg(feature = "desktop")]
    {
        let Some(path) = native::save_file(name, filter) else { return Ok(false) };
        write(&path)?;
        Ok(true)
    }
    #[cfg(not(feature = "desktop"))]
    {
        let Some(h) = hooks() else { return Ok(false) };
        let staged = tempfile::Builder::new().prefix("yaac-save-").tempfile()?.into_temp_path();
        write(&staged)?;
        h.save_file(name, filter, &staged)
    }
}

/// Show an error about something the user asked for.
pub(crate) fn message(title: &str, text: &str) {
    #[cfg(feature = "desktop")]
    native::message(title, text);
    #[cfg(not(feature = "desktop"))]
    if let Some(h) = hooks() {
        h.message(title, text);
    }
}

/// The clipboard's text (plain text only: no file list or image without the desktop
/// clipboard).
pub(crate) fn clipboard_text() -> Option<String> {
    #[cfg(feature = "desktop")]
    return native::clipboard_text();
    #[cfg(not(feature = "desktop"))]
    hooks()?.clipboard_text()
}

/// Files on the clipboard, as local copies (the embedder's; the desktop reads its own file
/// list in `files`).
#[cfg(not(feature = "desktop"))]
pub(crate) fn clipboard_files() -> Option<Vec<PathBuf>> {
    hooks()?.clipboard_files()
}

/// Open a URL through the embedder (`browser::open` checks it is `http`/`https` first).
#[cfg(not(feature = "desktop"))]
pub(crate) fn open_url(url: &str) -> std::io::Result<()> {
    hooks().ok_or_else(unsupported)?.open_url(url)
}

/// Open a local document through the embedder.
#[cfg(not(feature = "desktop"))]
pub(crate) fn open_document(path: &std::path::Path) -> std::io::Result<()> {
    hooks().ok_or_else(unsupported)?.open_document(path)
}
