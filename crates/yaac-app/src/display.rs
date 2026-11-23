//! Starting the window on the chosen renderer, and falling back (D128).
//!
//! Before the window exists the display file is read, a start is planned
//! ([`yaac_core::display::plan`]), and a "starting with X" marker is written. The window is then
//! created on X; if eframe returns an error before a first frame (no adapter, no OpenGL context,
//! …) it is created again, in this process, on the plan's fallback — software rendering,
//! Direct3D 12 on WARP on Windows. The first frame clears the marker, so a marker found at the
//! next start means X took the process down, and that start goes straight to software and says
//! why. What actually runs (renderer, backend, adapter, the reason for a fallback) is recorded
//! for Settings; it is never silent.
//!
//! `YAAC_RENDERER=<id>` (or `--renderer <id>`) overrides the stored choice for one start;
//! `YAAC_RENDERER_FAIL=1` makes the first attempt fail, to test the fallback for real.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use yaac_core::display::{Built, DisplayFile, Host, InUse, Plan, Renderer};

/// The renderers compiled into this build.
pub(crate) fn built() -> Built {
    Built { glow: cfg!(feature = "glow"), wgpu: cfg!(feature = "wgpu"), wpf: crate::wpf::available() }
}

/// Where the display file is kept; `None` keeps nothing (the demo, tests).
#[derive(Clone, Debug, Default)]
pub(crate) struct Store {
    path: Option<PathBuf>,
    /// The newest save asked for; an older background save that runs late is dropped.
    generation: Arc<AtomicU64>,
    lock: Arc<Mutex<()>>,
}

impl Store {
    pub(crate) fn at(path: Option<PathBuf>) -> Self {
        Self { path, ..Self::default() }
    }

    pub(crate) fn load(&self) -> DisplayFile {
        self.path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(DisplayFile::default, |t| DisplayFile::parse(&t))
    }

    /// Writes now (before the window exists, or from a worker thread): a temporary file, then
    /// a rename, so a crash never leaves half a file.
    fn write(&self, file: &DisplayFile, generation: u64) {
        let Some(path) = &self.path else { return };
        let Ok(_held) = self.lock.lock() else { return };
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, file.to_text()).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    pub(crate) fn save(&self, file: &DisplayFile) {
        let g = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.write(file, g);
    }

    /// Saves on a short-lived thread: the UI thread never waits on the disk.
    pub(crate) fn save_in_background(&self, file: DisplayFile) {
        if self.path.is_none() {
            return;
        }
        let g = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let store = self.clone();
        let _ = std::thread::Builder::new().name("yaac-display-file".into()).spawn(move || store.write(&file, g));
    }
}

/// What the window knows about its renderer (the Settings page shows it).
#[derive(Clone, Debug, Default)]
pub(crate) struct DisplayState {
    pub store: Store,
    /// The file as last written (its `renderer` is the choice Settings shows).
    pub file: DisplayFile,
    /// The renderer this start runs on.
    pub in_use: InUse,
    /// The choice this start was planned from (a change needs a restart).
    pub started_with: Renderer,
    first_frame: Arc<AtomicBool>,
}

impl DisplayState {
    /// The first frame was drawn: clear the start marker (off the UI thread).
    pub(crate) fn first_frame(&mut self) {
        if self.first_frame.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.file.starting.take().is_some() {
            self.store.save_in_background(self.file.clone());
        }
    }

    /// Settings picked a renderer: stored for the next start (a crash record is forgotten).
    pub(crate) fn choose(&mut self, r: Renderer) {
        self.file.choose(r);
        self.store.save_in_background(self.file.clone());
    }

    /// The choice differs from what this start was planned from.
    pub(crate) fn restart_needed(&self) -> bool {
        self.file.renderer != self.started_with
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0")
}

/// The wgpu backends and adapter for a renderer; `fail` is the test hook.
#[cfg(feature = "wgpu")]
fn wgpu_options(r: Renderer, fail: bool) -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    use eframe::wgpu::{self, Backends, DeviceType};
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    let software = r == Renderer::Software;
    setup.instance_descriptor.backends = match r {
        Renderer::Direct3D12 => Backends::DX12,
        Renderer::Vulkan => Backends::VULKAN,
        Renderer::WgpuGl => Backends::GL,
        // WARP is a Direct3D 12 adapter; elsewhere lavapipe (Vulkan) or llvmpipe (OpenGL).
        Renderer::Software if cfg!(windows) => Backends::DX12,
        Renderer::Software => Backends::VULKAN | Backends::GL,
        Renderer::Auto | Renderer::Glow | Renderer::WindowsWpf => Backends::PRIMARY | Backends::GL,
    };
    setup.native_adapter_selector = Some(Arc::new(move |adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>| {
        if fail {
            return Err("YAAC_RENDERER_FAIL is set".to_owned());
        }
        let usable: Vec<&wgpu::Adapter> = adapters.iter().filter(|a| surface.is_none_or(|s| a.is_surface_supported(s))).collect();
        // Hardware first, best first; the CPU adapter only for software (or when it is all
        // there is, and then Settings says "software").
        let rank = |t: DeviceType| match t {
            DeviceType::DiscreteGpu => 0,
            DeviceType::IntegratedGpu => 1,
            DeviceType::VirtualGpu => 2,
            DeviceType::Other => 3,
            DeviceType::Cpu => 4,
        };
        let pick = if software {
            usable.iter().find(|a| a.get_info().device_type == DeviceType::Cpu).copied()
        } else {
            usable.iter().min_by_key(|a| rank(a.get_info().device_type)).copied()
        };
        pick.cloned().ok_or_else(|| {
            let seen: Vec<String> = adapters.iter().map(|a| a.get_info().name).collect();
            if seen.is_empty() {
                "no graphics adapter was found".to_owned()
            } else if software {
                format!("no software adapter among {}", seen.join(", "))
            } else {
                format!("no adapter can draw to the window (found {})", seen.join(", "))
            }
        })
    }));
    WgpuConfiguration { wgpu_setup: WgpuSetup::CreateNew(setup), ..WgpuConfiguration::default() }
}

fn options_for(r: Renderer, base: &dyn Fn() -> eframe::NativeOptions, fail: bool) -> eframe::NativeOptions {
    let mut o = base();
    match r {
        #[cfg(feature = "glow")]
        Renderer::Glow => o.renderer = eframe::Renderer::Glow,
        #[cfg(feature = "wgpu")]
        _ => {
            o.renderer = eframe::Renderer::Wgpu;
            o.wgpu_options = wgpu_options(r, fail);
        }
        #[cfg(not(feature = "wgpu"))]
        _ => {
            let _ = fail;
        }
    }
    o
}

/// The backend wgpu is on, in words.
#[cfg(feature = "wgpu")]
fn backend_name(b: eframe::wgpu::Backend) -> String {
    use eframe::wgpu::Backend;
    match b {
        Backend::Dx12 => "Direct3D 12",
        Backend::Vulkan => "Vulkan",
        Backend::Gl => "OpenGL",
        Backend::Metal => "Metal",
        Backend::BrowserWebGpu => "WebGPU",
        Backend::Noop => "no backend",
    }
    .to_owned()
}

/// What eframe created, read from the creation context.
fn describe(cc: &eframe::CreationContext<'_>, r: Renderer) -> InUse {
    #[cfg_attr(not(feature = "wgpu"), allow(unused_mut))]
    let mut u = InUse { renderer: r, ..InUse::default() };
    #[cfg(feature = "wgpu")]
    if let Some(rs) = &cc.wgpu_render_state {
        let info = rs.adapter.get_info();
        u.backend = Some(backend_name(info.backend));
        u.adapter = Some(info.name);
        u.software = info.device_type == eframe::wgpu::DeviceType::Cpu;
    }
    #[cfg(not(feature = "wgpu"))]
    let _ = cc;
    u
}

/// The renderer choice for this start: the override, else the file's.
pub(crate) fn choice(file: &DisplayFile, arg: Option<&str>) -> Renderer {
    let over = arg.map(str::to_owned).or_else(|| std::env::var("YAAC_RENDERER").ok());
    over.as_deref().and_then(Renderer::from_id).unwrap_or(file.renderer)
}

/// Runs the window to its end on the planned renderer, falling back once. `base` builds the
/// options every attempt shares.
pub(crate) fn run_window(base: &dyn Fn() -> eframe::NativeOptions, plan: &Plan, choice: Renderer, mut file: DisplayFile, store: &Store, launch: crate::Launch) -> ExitCode {
    let host = Host::current();
    let fail_first = env_flag("YAAC_RENDERER_FAIL");
    let launch = RefCell::new(Some(launch));
    let mut why = plan.skipped.clone();
    let attempts: Vec<Renderer> = std::iter::once(plan.first).chain(plan.fallback).collect();
    for (i, r) in attempts.iter().copied().enumerate() {
        let fail = fail_first && i == 0;
        file.starting = Some(r);
        store.save(&file);
        #[cfg(windows)]
        if r == Renderer::WindowsWpf {
            let frontend = if fail {
                Err(std::io::Error::other("YAAC_RENDERER_FAIL is set"))
            } else {
                crate::wpf::Frontend::start(launch.borrow().as_ref().expect("the WPF window has not started"))
            };
            match frontend {
                Ok(frontend) => {
                    let mut launch = launch.borrow_mut().take().expect("the WPF window has not started");
                    launch.display = DisplayState {
                        store: store.clone(), file: file.clone(), started_with: choice, first_frame: Arc::default(),
                        in_use: InUse { renderer: r, software: true, fell_back: why, ..InUse::default() },
                    };
                    return match frontend.run(launch) {
                        Ok(()) => ExitCode::SUCCESS,
                        Err(e) => { eprintln!("yaac: WPF window: {e}"); ExitCode::FAILURE }
                    };
                }
                Err(e) => {
                    eprintln!("yaac: {} failed to start: {e}", r.label(host));
                    why = Some(format!("{} failed to start: {e}", r.label(host)));
                    continue;
                }
            }
        }
        let first_frame = Arc::new(AtomicBool::new(false));
        let (ff, reason, f) = (first_frame.clone(), why.clone(), file.clone());
        let created = std::cell::Cell::new(false);
        let result = eframe::run_native(
            "YAAC",
            options_for(r, base, fail),
            Box::new(|cc| {
                if fail && r == Renderer::Glow {
                    return Err("YAAC_RENDERER_FAIL is set".into());
                }
                let mut launch = launch.borrow_mut().take().ok_or("the window was already created")?;
                let mut in_use = describe(cc, r);
                in_use.fell_back = reason;
                eprintln!("yaac: renderer {}", yaac_core::display::in_use_line(&in_use, host));
                launch.display = DisplayState { store: store.clone(), file: f, in_use, started_with: choice, first_frame: ff };
                created.set(true);
                Ok(Box::new(crate::app::YaacApp::new(cc, launch)))
            }),
        );
        match result {
            Ok(()) => return ExitCode::SUCCESS,
            Err(e) if !created.get() && !first_frame.load(Ordering::SeqCst) => {
                eprintln!("yaac: {} failed to start: {e}", r.label(host));
                why = Some(format!("{} failed to start: {e}", r.label(host)));
            }
            Err(e) => {
                eprintln!("yaac: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    // Nothing could start; that was not a crash, so the next start tries again.
    file.starting = None;
    store.save(&file);
    eprintln!("yaac: no renderer could start");
    ExitCode::FAILURE
}
