//! The system tray and what closing the window does (D127).
//!
//! With "Keep running in the tray when closed" on (the default) and a tray icon in place, the
//! title bar's close button and Alt+F4 hide the window instead of closing it
//! ([`yaac_core::display::on_close`]). A hidden window runs no egui pass and paints nothing;
//! eframe only calls [`eframe::App::logic`] when something asks for a repaint (a hub message,
//! a tray click), so it costs no CPU while nothing happens. The tray icon's left click (or
//! double click) and its menu's *Open YAAC* show the window again; *Quit* closes the window
//! process. The hub is a separate process and runs on either way.
//!
//! The tray icon is Windows only (`tray-icon`, whose Win32 backend lives on the UI thread's
//! own message loop, so nothing blocks). On Linux no tray is built — WSLg, for one, has no
//! StatusNotifier host — and closing quits as before.
//!
//! A second `yaac` started while this window runs (perhaps hidden) asks it to show itself and
//! exits ([`yaac_winsys::claim_window_instance`]); that request arrives here as
//! [`ShellCmd::Show`] too.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};

use eframe::egui::{self, ViewportCommand};
use yaac_core::display::{CloseAction, on_close};

/// A request from outside the window's own input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code, reason = "only the Windows tray and single-instance waiter send them"))]
pub(crate) enum ShellCmd {
    /// Show the window and bring it to the front.
    Show,
    /// Close the window process.
    Quit,
}

/// The window's tray and close behaviour.
pub(crate) struct Shell {
    tx: Sender<ShellCmd>,
    rx: Receiver<ShellCmd>,
    /// Quit was asked for: the next close request is let through.
    quitting: bool,
    /// The window is hidden in the tray.
    pub hidden: bool,
    /// The presentation process hosts the tray on its own Windows message loop.
    pub external_tray: bool,
    #[cfg(windows)]
    tray: Option<WinTray>,
    /// Tests: pretend a tray icon is there.
    #[cfg(test)]
    pub fake_tray: bool,
    need_you: Option<usize>,
    /// The window's context once it exists, so a request from another thread can wake it.
    ctx: Arc<OnceLock<egui::Context>>,
}

impl std::fmt::Debug for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shell").field("hidden", &self.hidden).field("tray", &self.has_tray()).finish_non_exhaustive()
    }
}

impl Shell {
    /// Without a tray icon (tests, screenshots); [`Shell::install_tray`] adds one.
    pub(crate) fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            quitting: false,
            hidden: false,
            external_tray: false,
            #[cfg(windows)]
            tray: None,
            #[cfg(test)]
            fake_tray: false,
            need_you: None,
            ctx: Arc::default(),
        }
    }

    /// The window exists: requests from other threads wake it from now on.
    pub(crate) fn bind(&self, ctx: &egui::Context) {
        let _ = self.ctx.set(ctx.clone());
    }

    /// Queues a command from another thread and wakes the window (if it exists yet; if not,
    /// its first frame picks the command up).
    #[cfg_attr(not(windows), allow(dead_code, reason = "the single-instance waiter is Windows only"))]
    fn remote(&self) -> impl Fn(ShellCmd) + Send + 'static {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        move |cmd| {
            let _ = tx.send(cmd);
            if let Some(c) = ctx.get() {
                c.request_repaint();
            }
        }
    }

    /// Puts the icon in the tray (Windows). Failure only means close quits, as without it.
    pub(crate) fn install_tray(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        match WinTray::new(ctx, self.tx.clone()) {
            Ok(t) => self.tray = Some(t),
            Err(e) => eprintln!("yaac: no tray icon: {e}"),
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }

    /// A tray icon is in place, so hiding the window cannot lose it.
    pub(crate) fn has_tray(&self) -> bool {
        #[cfg(test)]
        let fake = self.fake_tray;
        #[cfg(not(test))]
        let fake = false;
        #[cfg(windows)]
        let real = self.tray.is_some();
        #[cfg(not(windows))]
        let real = false;
        fake || real || self.external_tray
    }

    /// Tests: as if the tray had sent `cmd`.
    #[cfg(test)]
    pub(crate) fn inject(&self, cmd: ShellCmd) {
        let _ = self.tx.send(cmd);
    }

    /// "Hide to the tray": hide the window now (only with a tray icon, which brings it back).
    pub(crate) fn hide(&mut self, ctx: &egui::Context) {
        if self.has_tray() {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            self.hidden = true;
        }
    }

    /// "Quit YAAC": close the window process; a close request is let through.
    pub(crate) fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    /// Whether this system can have a tray icon at all (Settings says so when it cannot).
    pub(crate) const fn supported() -> bool {
        cfg!(windows)
    }

    /// Called from `App::logic` (which also runs while the window is hidden): applies tray and
    /// single-instance requests, turns a close request into a hide when the option is on, and
    /// keeps the tray's badge and tooltip up to date.
    pub(crate) fn upkeep(&mut self, ctx: &egui::Context, tray_on_close: bool, need_you: impl FnOnce() -> usize) {
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                ShellCmd::Show => {
                    self.hidden = false;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                ShellCmd::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                    // A hidden window has no frame coming: ask for the pass that closes it.
                    ctx.request_repaint();
                }
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && on_close(tray_on_close, self.has_tray(), self.quitting) == CloseAction::Hide {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            self.hidden = true;
        }
        if self.has_tray() {
            let n = need_you();
            if self.need_you != Some(n) {
                self.need_you = Some(n);
                #[cfg(windows)]
                if let Some(t) = &self.tray {
                    t.set_need_you(n);
                }
            }
        }
    }
}

/// The Windows tray icon and its menu.
#[cfg(windows)]
struct WinTray {
    icon: tray_icon::TrayIcon,
    plain: tray_icon::Icon,
    badged: tray_icon::Icon,
}

#[cfg(windows)]
impl WinTray {
    const SIZE: u32 = 32;

    fn new(ctx: &egui::Context, tx: Sender<ShellCmd>) -> Result<Self, String> {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
        use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
        let plain = tray_icon::Icon::from_rgba(crate::app_icon::rgba(Self::SIZE), Self::SIZE, Self::SIZE).map_err(|e| e.to_string())?;
        let badged = tray_icon::Icon::from_rgba(crate::app_icon::badged(Self::SIZE), Self::SIZE, Self::SIZE).map_err(|e| e.to_string())?;
        let open = MenuItem::new("Open YAAC", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit]).map_err(|e| e.to_string())?;
        let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
        // Both handlers run on the UI thread, inside its message loop: they only queue a
        // command and wake the window (which may be hidden; `logic` still runs).
        let (c, t) = (ctx.clone(), tx.clone());
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            let show = matches!(
                e,
                TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
                    | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. }
            );
            if show {
                let _ = t.send(ShellCmd::Show);
                c.request_repaint();
            }
        }));
        let c = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let cmd = if e.id == open_id {
                ShellCmd::Show
            } else if e.id == quit_id {
                ShellCmd::Quit
            } else {
                return;
            };
            let _ = tx.send(cmd);
            c.request_repaint();
        }));
        let icon = TrayIconBuilder::new()
            .with_icon(plain.clone())
            .with_tooltip(yaac_core::display::tray_tooltip(0))
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { icon, plain, badged })
    }

    /// The red badge and "YAAC · N need you" while some thread needs the user.
    fn set_need_you(&self, n: usize) {
        let _ = self.icon.set_icon(Some(if n > 0 { self.badged.clone() } else { self.plain.clone() }));
        let _ = self.icon.set_tooltip(Some(yaac_core::display::tray_tooltip(n)));
    }
}

/// The single-instance claim's outcome.
#[cfg(feature = "hub")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    /// This is the window (or the platform has no claim).
    First,
    /// Another window was open and was asked to show itself.
    AlreadyOpen,
}

/// The window's single-instance event name for this user session and hub profile (it follows
/// the hub's pipe, so a test profile has its own); the quit event is this plus `-quit`.
#[cfg(all(feature = "hub", windows))]
pub(crate) fn instance_name(paths: &yaac_hub::HubPaths) -> String {
    let pipe = paths.socket();
    let tail = pipe.file_name().map_or_else(|| "yaac-hub".into(), |n| n.to_string_lossy().into_owned());
    format!(r"Local\{}-window", tail.replace('\\', "-"))
}

/// Claims the window for this user session and hub profile. On Windows the first window keeps
/// a thread blocked on the claim's event; each signal from a later `yaac` shows the window. A
/// second event, `<name>-quit`, is signalled by `yaac quit` (the installer) and closes the
/// window as the tray's *Quit* does. Elsewhere it does nothing.
#[cfg(feature = "hub")]
pub(crate) fn claim_instance(paths: &yaac_hub::HubPaths, shell: &Shell) -> Claim {
    #[cfg(windows)]
    {
        let name = instance_name(paths);
        match yaac_winsys::claim_window_instance(&name) {
            Ok(yaac_winsys::InstanceClaim::Signalled) => return Claim::AlreadyOpen,
            Ok(yaac_winsys::InstanceClaim::First(event)) => {
                let show = shell.remote();
                let _ = std::thread::Builder::new().name("yaac-single-instance".into()).spawn(move || {
                    while event.wait() {
                        show(ShellCmd::Show);
                    }
                });
                match yaac_winsys::listen_named_event(&format!("{name}-quit")) {
                    Ok(event) => {
                        let quit = shell.remote();
                        let _ = std::thread::Builder::new().name("yaac-quit-request".into()).spawn(move || {
                            while event.wait() {
                                quit(ShellCmd::Quit);
                            }
                        });
                    }
                    Err(e) => eprintln!("yaac: cannot listen for quit requests: {e}"),
                }
            }
            Err(e) => eprintln!("yaac: cannot check for another window: {e}"),
        }
    }
    #[cfg(not(windows))]
    let _ = (paths, shell);
    Claim::First
}
