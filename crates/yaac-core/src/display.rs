//! The window's renderer (D128) and the tray (D127), as data: which renderers a build offers,
//! which one a start tries and what it falls back to, the line Settings shows about the one in
//! use, the window's own small display file, and what closing the window does.
//!
//! The decision table ([`plan`]):
//!
//! | Last start | First try | Then |
//! | --- | --- | --- |
//! | reached its first frame (or first start) | the choice | software, if that fails |
//! | the choice never reached a first frame (crashed or killed) | software | the choice |
//! | the choice is software | software | automatic |
//!
//! A renderer that did not reach its first frame stays *crashed* until the user picks a
//! renderer again in Settings, so a crashing driver cannot turn into a loop of crashing starts.

use serde_json::{Value, json};

/// A rendering backend the user can choose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Renderer {
    /// YAAC picks: OpenGL through glow when the build has it, else wgpu's own choice.
    #[default]
    Auto,
    /// wgpu on Direct3D 12 (Windows).
    Direct3D12,
    /// wgpu on Vulkan.
    Vulkan,
    /// wgpu on OpenGL.
    WgpuGl,
    /// OpenGL through glow (egui's lighter renderer).
    Glow,
    /// Software rendering: Direct3D 12 on WARP on Windows; wgpu's CPU adapter (lavapipe or
    /// llvmpipe) elsewhere.
    Software,
}

/// Which host the window runs on (the choices differ).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    /// Windows.
    Windows,
    /// Linux and the other Unixes.
    Unix,
}

impl Host {
    /// The host this was compiled for.
    pub const fn current() -> Host {
        if cfg!(windows) { Host::Windows } else { Host::Unix }
    }
}

/// The renderers compiled into this build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Built {
    /// OpenGL through glow.
    pub glow: bool,
    /// wgpu (Direct3D 12, Vulkan, OpenGL, and the software adapters).
    pub wgpu: bool,
}

impl Renderer {
    /// Every renderer, in the dropdown's order.
    pub const ALL: [Renderer; 6] =
        [Renderer::Auto, Renderer::Direct3D12, Renderer::Vulkan, Renderer::WgpuGl, Renderer::Glow, Renderer::Software];

    /// The stored id.
    pub fn id(self) -> &'static str {
        match self {
            Renderer::Auto => "auto",
            Renderer::Direct3D12 => "dx12",
            Renderer::Vulkan => "vulkan",
            Renderer::WgpuGl => "wgpu-gl",
            Renderer::Glow => "glow",
            Renderer::Software => "software",
        }
    }

    /// From a stored id.
    pub fn from_id(id: &str) -> Option<Renderer> {
        Renderer::ALL.into_iter().find(|r| r.id() == id)
    }

    /// The dropdown's label.
    pub fn label(self, host: Host) -> &'static str {
        match (self, host) {
            (Renderer::Auto, _) => "Automatic",
            (Renderer::Direct3D12, _) => "Direct3D 12",
            (Renderer::Vulkan, _) => "Vulkan",
            (Renderer::WgpuGl, _) => "OpenGL (wgpu)",
            (Renderer::Glow, _) => "OpenGL (glow)",
            (Renderer::Software, Host::Windows) => "Software (WARP)",
            (Renderer::Software, Host::Unix) => "Software",
        }
    }

    /// Whether `host` and `built` can run it.
    pub fn offered(self, host: Host, built: Built) -> bool {
        match self {
            Renderer::Auto => built.glow || built.wgpu,
            Renderer::Direct3D12 => host == Host::Windows && built.wgpu,
            Renderer::Vulkan | Renderer::WgpuGl | Renderer::Software => built.wgpu,
            Renderer::Glow => built.glow,
        }
    }

    /// What *Automatic* means in this build: glow, the lighter one (docs/02-architecture.md),
    /// else wgpu's own pick among the hardware backends (shown as its backend once known).
    pub fn resolve(self, built: Built) -> Renderer {
        match self {
            Renderer::Auto if built.glow => Renderer::Glow,
            other => other,
        }
    }
}

/// The renderers offered, in order.
pub fn offered(host: Host, built: Built) -> Vec<Renderer> {
    Renderer::ALL.into_iter().filter(|r| r.offered(host, built)).collect()
}

/// What one start tries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Tried first (never [`Renderer::Auto`] when glow is built).
    pub first: Renderer,
    /// Tried if the first fails to initialise.
    pub fallback: Option<Renderer>,
    /// Why the first is not the user's choice ("… did not finish starting last time").
    pub skipped: Option<String>,
}

/// Plans a start (the table in the module docs). `crashed` is the renderer that last failed to
/// reach a first frame, if any.
pub fn plan(choice: Renderer, host: Host, built: Built, crashed: Option<Renderer>) -> Plan {
    let choice = if choice.offered(host, built) { choice } else { Renderer::Auto };
    let first = choice.resolve(built);
    let software = Renderer::Software.offered(host, built).then_some(Renderer::Software);
    if first == Renderer::Software {
        let auto = Renderer::Auto.resolve(built);
        return Plan { first, fallback: (auto != Renderer::Software).then_some(auto), skipped: None };
    }
    if crashed == Some(first)
        && let Some(sw) = software
    {
        let skipped = format!("{} did not finish starting last time", first.label(host));
        return Plan { first: sw, fallback: Some(first), skipped: Some(skipped) };
    }
    Plan { first, fallback: software, skipped: None }
}

/// The renderer that is running, for Settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InUse {
    /// The renderer started.
    pub renderer: Renderer,
    /// The graphics backend wgpu ended up on ("Direct3D 12", "Vulkan"), when known.
    pub backend: Option<String>,
    /// The adapter's name, when known ("NVIDIA GeForce RTX 3060", "Microsoft Basic Render Driver").
    pub adapter: Option<String>,
    /// It draws on the CPU.
    pub software: bool,
    /// Why this is not the choice: the failure, or the crash.
    pub fell_back: Option<String>,
}

/// "In use: Direct3D 12 (WARP, software) · fell back because …".
pub fn in_use_line(u: &InUse, host: Host) -> String {
    let name = match (u.renderer, &u.backend) {
        (Renderer::Software, _) if host == Host::Windows => "Direct3D 12 (WARP, software)".to_owned(),
        (Renderer::Software, Some(b)) => format!("{b} (software)"),
        (Renderer::Auto, Some(b)) => b.clone(),
        (r, _) => {
            let mut s = r.label(host).to_owned();
            if u.software {
                s.push_str(" (software)");
            }
            s
        }
    };
    let mut line = format!("In use: {name}");
    if let Some(a) = u.adapter.as_deref().filter(|a| !a.is_empty() && !(u.renderer == Renderer::Software && host == Host::Windows)) {
        line.push_str(&format!(" · {a}"));
    }
    if let Some(why) = &u.fell_back {
        line.push_str(&format!(" · fell back because {why}"));
    }
    line
}

/// The window's display file: the renderer choice, and the start marker that catches a
/// renderer crashing the process. The window writes it itself (it is not the hub's cache, and
/// it must be readable before the window exists).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayFile {
    /// The user's choice.
    pub renderer: Renderer,
    /// Written before a renderer is initialised, cleared after its first frame: present at
    /// the next start means that start never got there.
    pub starting: Option<Renderer>,
    /// The renderer that last failed to reach a first frame; skipped until the user picks a
    /// renderer again.
    pub crashed: Option<Renderer>,
}

impl DisplayFile {
    /// Parses the file; anything malformed is the default.
    pub fn parse(text: &str) -> DisplayFile {
        let Ok(v) = serde_json::from_str::<Value>(text) else { return DisplayFile::default() };
        let r = |k: &str| v.get(k).and_then(Value::as_str).and_then(Renderer::from_id);
        DisplayFile { renderer: r("renderer").unwrap_or_default(), starting: r("starting"), crashed: r("crashed") }
    }

    /// The file's text.
    pub fn to_text(&self) -> String {
        let mut v = json!({ "renderer": self.renderer.id() });
        if let Some(s) = self.starting {
            v["starting"] = json!(s.id());
        }
        if let Some(c) = self.crashed {
            v["crashed"] = json!(c.id());
        }
        v.to_string()
    }

    /// At start: a marker left by the previous start becomes a crash record.
    pub fn note_unfinished_start(&mut self) {
        if let Some(s) = self.starting.take() {
            self.crashed = Some(s);
        }
    }

    /// The user picked a renderer: a crash record is forgotten (they asked to try again).
    pub fn choose(&mut self, r: Renderer) {
        self.renderer = r;
        self.crashed = None;
    }
}

/// What closing the window does (the title bar's close button, Alt+F4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// Hide it to the tray; the process keeps running.
    Hide,
    /// Close the window process (the hub runs on either way).
    Quit,
}

/// D127: with the tray option on and a tray icon in place, close hides; Quit from the tray menu
/// (`quitting`) always quits.
pub fn on_close(tray_on_close: bool, tray_present: bool, quitting: bool) -> CloseAction {
    if tray_on_close && tray_present && !quitting { CloseAction::Hide } else { CloseAction::Quit }
}

/// The tray icon's tooltip.
pub fn tray_tooltip(need_you: usize) -> String {
    if need_you == 0 { "YAAC".to_owned() } else { format!("YAAC · {need_you} need you") }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Built = Built { glow: true, wgpu: true };

    #[test]
    fn the_dropdown_lists_what_the_build_and_host_offer() {
        let win = offered(Host::Windows, ALL);
        assert_eq!(win, Renderer::ALL);
        let labels: Vec<_> = win.iter().map(|r| r.label(Host::Windows)).collect();
        assert_eq!(labels, ["Automatic", "Direct3D 12", "Vulkan", "OpenGL (wgpu)", "OpenGL (glow)", "Software (WARP)"]);
        assert_eq!(offered(Host::Unix, ALL), [Renderer::Auto, Renderer::Vulkan, Renderer::WgpuGl, Renderer::Glow, Renderer::Software]);
        assert_eq!(offered(Host::Windows, Built { glow: true, wgpu: false }), [Renderer::Auto, Renderer::Glow]);
        for r in Renderer::ALL {
            assert_eq!(Renderer::from_id(r.id()), Some(r));
        }
    }

    #[test]
    fn a_start_tries_the_choice_then_software() {
        let p = plan(Renderer::Direct3D12, Host::Windows, ALL, None);
        assert_eq!(p, Plan { first: Renderer::Direct3D12, fallback: Some(Renderer::Software), skipped: None });
        // Automatic is glow in a build that has it.
        assert_eq!(plan(Renderer::Auto, Host::Windows, ALL, None).first, Renderer::Glow);
        assert_eq!(plan(Renderer::Auto, Host::Windows, Built { glow: false, wgpu: true }, None).first, Renderer::Auto);
        // Software chosen: automatic is what is left to try.
        assert_eq!(plan(Renderer::Software, Host::Windows, ALL, None).fallback, Some(Renderer::Glow));
        // Without wgpu there is nothing to fall back to.
        assert_eq!(plan(Renderer::Glow, Host::Windows, Built { glow: true, wgpu: false }, None).fallback, None);
        // A choice this host cannot run (a file copied from Windows) is automatic.
        assert_eq!(plan(Renderer::Direct3D12, Host::Unix, ALL, None).first, Renderer::Glow);
    }

    #[test]
    fn a_renderer_that_crashed_the_last_start_is_skipped_and_says_why() {
        let p = plan(Renderer::Vulkan, Host::Windows, ALL, Some(Renderer::Vulkan));
        assert_eq!(p.first, Renderer::Software);
        assert_eq!(p.fallback, Some(Renderer::Vulkan));
        assert_eq!(p.skipped.as_deref(), Some("Vulkan did not finish starting last time"));
        // A crash of some other renderer does not matter.
        assert_eq!(plan(Renderer::Glow, Host::Windows, ALL, Some(Renderer::Vulkan)).first, Renderer::Glow);
        // Automatic resolved to glow, and glow crashed.
        assert_eq!(plan(Renderer::Auto, Host::Windows, ALL, Some(Renderer::Glow)).first, Renderer::Software);
    }

    #[test]
    fn the_start_marker_becomes_a_crash_until_the_user_picks_again() {
        let mut f = DisplayFile { renderer: Renderer::Direct3D12, starting: None, crashed: None };
        // Before initialising: the marker.
        f.starting = Some(Renderer::Direct3D12);
        let written = f.to_text();
        // That start never reached its first frame; the next one reads it back.
        let mut next = DisplayFile::parse(&written);
        next.note_unfinished_start();
        assert_eq!(next.crashed, Some(Renderer::Direct3D12));
        assert_eq!(next.starting, None);
        assert_eq!(plan(next.renderer, Host::Windows, ALL, next.crashed).first, Renderer::Software);
        // Picking a renderer in Settings clears it.
        next.choose(Renderer::Direct3D12);
        assert_eq!(next.crashed, None);
        assert_eq!(DisplayFile::parse("not json"), DisplayFile::default());
        assert_eq!(DisplayFile::parse(r#"{"renderer":"metal"}"#).renderer, Renderer::Auto);
    }

    #[test]
    fn the_in_use_line_is_never_silent_about_a_fallback() {
        let u = InUse {
            renderer: Renderer::Software,
            backend: Some("Direct3D 12".into()),
            adapter: Some("Microsoft Basic Render Driver".into()),
            software: true,
            fell_back: Some("Direct3D 12 failed to start: no adapter".into()),
        };
        assert_eq!(in_use_line(&u, Host::Windows), "In use: Direct3D 12 (WARP, software) · fell back because Direct3D 12 failed to start: no adapter");
        let gpu = InUse { renderer: Renderer::Vulkan, adapter: Some("Intel Arc".into()), ..InUse::default() };
        assert_eq!(in_use_line(&gpu, Host::Windows), "In use: Vulkan · Intel Arc");
        let lava = InUse { renderer: Renderer::Software, backend: Some("Vulkan".into()), adapter: Some("llvmpipe".into()), software: true, fell_back: None };
        assert_eq!(in_use_line(&lava, Host::Unix), "In use: Vulkan (software) · llvmpipe");
    }

    #[test]
    fn close_hides_to_the_tray_only_when_asked_and_possible() {
        assert_eq!(on_close(true, true, false), CloseAction::Hide);
        assert_eq!(on_close(true, true, true), CloseAction::Quit, "Quit in the tray menu");
        assert_eq!(on_close(true, false, false), CloseAction::Quit, "no tray on this desktop");
        assert_eq!(on_close(false, true, false), CloseAction::Quit);
        assert_eq!(tray_tooltip(0), "YAAC");
        assert_eq!(tray_tooltip(2), "YAAC · 2 need you");
    }
}
