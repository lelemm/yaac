use crate::diagnostics::Diagnostics;
use crate::wire::*;
use memmap2::Mmap;
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, Weak,
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

fn runtime() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("YAAC_CEF_DIR") {
        return PathBuf::from(dir).canonicalize();
    }
    Ok(std::env::current_exe()?.with_file_name("cef"))
}
/// Dispatch before console attachment or YAAC initialization. Missing libraries fail
/// only the helper process. The ordinary window and hub never load Chromium.
pub fn subprocess_entry() -> Option<i32> {
    if !std::env::args_os()
        .skip(1)
        .any(|s| s == "--yaac-browser-host" || s.to_string_lossy().starts_with("--type="))
    {
        return None;
    }
    let result = (|| -> Result<i32, Box<dyn std::error::Error>> {
        let path = runtime()?.join(libloading::library_filename("yaac_preview_native"));
        // SAFETY: this is the pinned native adapter installed beside the trusted YAAC binary.
        let lib = unsafe { libloading::Library::new(path)? };
        // SAFETY: this symbol is exported by native/src/lib.rs with this exact ABI.
        let run = unsafe { lib.get::<extern "C" fn() -> i32>(b"yaac_cef_main")? };
        Ok(run())
    })();
    Some(result.unwrap_or_else(|err| {
        eprintln!("CEF unavailable: {err}");
        1
    }))
}

/// Check the staged adapter and runtime files. Call on a worker before enabling the UI.
pub fn runtime_available() -> Result<(), String> {
    let dir = runtime().map_err(|e| e.to_string())?;
    let mut files = vec![
        libloading::library_filename("yaac_preview_native")
            .into_string()
            .unwrap(),
        libloading::library_filename("cef").into_string().unwrap(),
        "icudtl.dat".into(),
        "resources.pak".into(),
        "locales/en-US.pak".into(),
    ];
    #[cfg(windows)]
    {
        files[1] = "libcef.dll".into();
        files.push("yaac_preview_native.exe".into());
    }
    for file in files.drain(..) {
        if !dir.join(&file).is_file() {
            return Err(format!(
                "Missing CEF runtime file {}",
                dir.join(file).display()
            ));
        }
    }
    Ok(())
}
/// A read-only, premultiplied BGRA surface. Keep it alive until the front end has
/// finished reading. Drop sends Consumed and allows the helper to reuse the mapping.
pub struct Frame {
    pub size: [usize; 2],
    pub dirty: Vec<[usize; 4]>,
    pub popup: bool,
    pub path: PathBuf,
    pixels: Option<Mmap>,
    _files: Arc<SurfaceFiles>,
    tab: u64,
    commands: Sender<Request>,
}
impl Frame {
    pub fn bytes(&self) -> &[u8] {
        self.pixels.as_deref().unwrap()
    }
}
impl Drop for Frame {
    fn drop(&mut self) {
        self.pixels.take();
        let _ = self.commands.send(Request {
            tab: self.tab,
            action: Action::Consumed { popup: self.popup },
        });
    }
}

pub enum Update {
    Frame(Frame),
    Event(Event),
}
#[derive(Clone)]
struct Route {
    diagnostics: Arc<Mutex<Diagnostics>>,
    events: Sender<Update>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
#[derive(Default)]
struct Routes {
    tabs: HashMap<u64, Route>,
    error: Option<String>,
}
static WORKERS: (Mutex<usize>, std::sync::Condvar) = (Mutex::new(0), std::sync::Condvar::new());
struct WorkerLifetime;
impl WorkerLifetime {
    fn start() -> Self {
        *WORKERS.0.lock().unwrap() += 1;
        Self
    }
}
impl Drop for WorkerLifetime {
    fn drop(&mut self) {
        *WORKERS.0.lock().unwrap() -= 1;
        WORKERS.1.notify_all();
    }
}
/// Wait for browser hosts and frame leases after the front end has exited. Tab close and the
/// rendering thread never wait on CEF. Returns false if a helper fails to exit within five seconds.
pub fn finish_after_window_exit() -> bool {
    let (count, _timeout) = WORKERS
        .1
        .wait_timeout_while(
            WORKERS.0.lock().unwrap(),
            std::time::Duration::from_secs(5),
            |count| *count != 0,
        )
        .unwrap();
    *count == 0
}

struct Session {
    commands: Sender<Request>,
    routes: Arc<Mutex<Routes>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.commands.send(Request {
            tab: 0,
            action: Action::Shutdown,
        });
    }
}

// Keep closed-tab files available to an outstanding WPF frame lease. A worker removes
// them after the final lease drops, and removes the session after the helper exits.
struct SurfaceFiles {
    tab: u64,
    cleanup: Sender<u64>,
}
impl Drop for SurfaceFiles {
    fn drop(&mut self) {
        let _ = self.cleanup.send(self.tab);
    }
}

/// One controller per YAAC window process. Each open tab keeps the browser host alive.
#[derive(Default)]
pub struct BrowserHost {
    session: Weak<Session>,
    next: u64,
}
impl BrowserHost {
    pub fn open(
        &mut self,
        url: &str,
        socks: Option<u16>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Tab {
        self.next += 1;
        let id = self.next;
        let (events, updates) = mpsc::channel();
        let diagnostics = Arc::new(Mutex::new(Diagnostics::default()));
        let route = Route {
            diagnostics: diagnostics.clone(),
            events,
            wake: Arc::new(wake),
        };
        let session = if let Some(s) = self.session.upgrade().filter(|s| {
            let mut routes = s.routes.lock().unwrap();
            if routes.error.is_some() {
                return false;
            }
            routes.tabs.insert(id, route.clone());
            true
        }) {
            s
        } else {
            let routes = Arc::new(Mutex::new(Routes {
                tabs: HashMap::from([(id, route)]),
                error: None,
            }));
            let (commands, inbox) = mpsc::channel();
            let s = Arc::new(Session {
                commands: commands.clone(),
                routes: routes.clone(),
            });
            let lifetime = WorkerLifetime::start();
            thread::spawn(move || {
                let err = run_host(inbox, commands, &routes, lifetime)
                    .err()
                    .map_or_else(|| "Browser host closed".to_owned(), |e| e.to_string());
                let targets = {
                    let mut r = routes.lock().unwrap();
                    r.error = Some(err.clone());
                    r.tabs.values().cloned().collect::<Vec<_>>()
                };
                for route in targets {
                    let _ = route.events.send(Update::Event(Event::Error(err.clone())));
                    (route.wake)();
                }
            });
            self.session = Arc::downgrade(&s);
            s
        };
        let tab = Tab {
            id,
            session,
            updates,
            call: 2,
            diagnostics,
        };
        let _ = tab.session.commands.send(Request {
            tab: id,
            action: Action::Open {
                url: url.into(),
                socks,
            },
        });
        tab
    }
}
/// Renderer-neutral tab. Use try_update from the frame callback and send from either UI.
pub struct Tab {
    id: u64,
    session: Arc<Session>,
    updates: Receiver<Update>,
    call: i32,
    diagnostics: Arc<Mutex<Diagnostics>>,
}
impl Tab {
    pub(crate) fn choose_files(
        &self,
        mode: u32,
        title: String,
        default_path: String,
        filters: Vec<String>,
    ) {
        let commands = self.session.commands.clone();
        let id = self.id;
        thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            let paths = Path::new(&default_path);
            if let Some(parent) = paths.parent() {
                dialog = dialog.set_directory(parent);
            }
            if let Some(name) = paths.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy());
            }
            let exts: Vec<_> = filters.iter().filter_map(|s| s.strip_prefix('.')).collect();
            if !exts.is_empty() {
                dialog = dialog.add_filter("Accepted files", &exts);
            }
            let result = match mode {
                0 => dialog.pick_file().map(|p| vec![p]),
                1 => dialog.pick_files(),
                2 => dialog.pick_folder().map(|p| vec![p]),
                3 => dialog.save_file().map(|p| vec![p]),
                _ => None,
            };
            let paths = result.map(|ps| {
                ps.into_iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
            });
            let _ = commands.send(Request {
                tab: id,
                action: Action::FileDialog(paths),
            });
        });
    }
    pub(crate) fn add_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        if let Some(route) = self.session.routes.lock().unwrap().tabs.get_mut(&self.id) {
            let previous = route.wake.clone();
            route.wake = Arc::new(move || {
                previous();
                wake()
            });
        }
    }
    /// Console, exception and network events in sequence order. Read on a controller
    /// worker. Use the last returned sequence as the next cursor.
    pub fn diagnostics_since(&self, cursor: u64) -> Vec<(u64, serde_json::Value)> {
        let entries = self.diagnostics.lock().unwrap().since(cursor);
        entries
            .into_iter()
            .map(|(n, v)| (n, (*v).clone()))
            .collect()
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn send(&self, action: Action) {
        // Only Frame::drop can release the mapped storage, and the controller owns
        // process lifetime. Public input commands cannot invalidate a borrowed slice.
        if matches!(
            action,
            Action::Consumed { .. } | Action::Open { .. } | Action::Shutdown
        ) {
            return;
        }
        let _ = self.session.commands.send(Request {
            tab: self.id,
            action,
        });
    }
    pub fn try_update(&self) -> Option<Update> {
        self.updates.try_recv().ok()
    }
    /// Responses carry the returned id in Event::DevTools. Unsolicited console/exception/network
    /// events carry a CDP method. There is no remote debugging port.
    pub fn devtools_command(&mut self, method: &str, params: serde_json::Value) -> i32 {
        self.call += 1;
        self.send(Action::DevTools {
            id: self.call,
            method: method.into(),
            params,
        });
        self.call
    }
}
impl Drop for Tab {
    fn drop(&mut self) {
        self.session.routes.lock().unwrap().tabs.remove(&self.id);
        self.send(Action::Close);
    }
}
fn run_host(
    inbox: Receiver<Request>,
    commands: Sender<Request>,
    routes: &Mutex<Routes>,
    lifetime: WorkerLifetime,
) -> io::Result<()> {
    let dir = runtime()?;
    let adapter = dir.join(libloading::library_filename("yaac_preview_native"));
    if !adapter.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("CEF unavailable: {}", adapter.display()),
        ));
    }
    let mut temp = tempfile::Builder::new();
    temp.prefix("yaac-browser-");
    #[cfg(unix)]
    temp.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700));
    let profile = temp.tempdir()?;
    #[cfg(not(windows))]
    let mut command = Command::new(std::env::current_exe()?);
    #[cfg(windows)]
    let mut command = Command::new(dir.join("yaac_preview_native.exe"));
    command
        .arg("--yaac-browser-host")
        .env("YAAC_CEF_DIR", &dir)
        .env("YAAC_PREVIEW_SESSION", profile.path());
    #[cfg(target_os = "linux")]
    {
        let paths = std::env::var_os("LD_LIBRARY_PATH").unwrap_or_default();
        let mut libs = vec![dir.clone()];
        libs.extend(std::env::split_paths(&paths).filter(|p| !p.as_os_str().is_empty()));
        command.env(
            "LD_LIBRARY_PATH",
            std::env::join_paths(libs).map_err(io::Error::other)?,
        );
    }
    #[cfg(windows)]
    {
        let paths = std::env::var_os("PATH").unwrap_or_default();
        let mut libs = vec![dir.clone()];
        libs.extend(std::env::split_paths(&paths));
        command.env(
            "PATH",
            std::env::join_paths(libs).map_err(io::Error::other)?,
        );
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    thread::spawn(move || {
        for req in inbox {
            if serde_json::to_writer(&mut stdin, &req).is_err()
                || stdin.write_all(b"\n").is_err()
                || matches!(req.action, Action::Shutdown)
            {
                break;
            }
        }
    });
    let output = BufReader::new(child.stdout.take().unwrap());
    let directory = profile.path().to_path_buf();
    let (cleanup, retired) = mpsc::channel::<u64>();
    thread::spawn(move || {
        for tab in retired {
            for surface in [0, 1] {
                let _ = std::fs::remove_file(profile.path().join(format!("{tab}-{surface}.bgra")));
            }
        }
        drop(profile);
        drop(lifetime);
    });
    let mut files = HashMap::new();
    let read = (|| -> io::Result<()> {
        for line in output.lines() {
            let line = line?;
            let notice: Notice = serde_json::from_str(&line)?;
            if matches!(notice.event, Event::Closed) {
                files.remove(&notice.tab);
            }
            let target = routes.lock().unwrap().tabs.get(&notice.tab).cloned();
            if let Some(route) = target {
                let update = match notice.event {
                    Event::Paint { popup, size, dirty } => Update::Frame(read_frame(
                        &directory,
                        files
                            .entry(notice.tab)
                            .or_insert_with(|| {
                                Arc::new(SurfaceFiles {
                                    tab: notice.tab,
                                    cleanup: cleanup.clone(),
                                })
                            })
                            .clone(),
                        notice.tab,
                        popup,
                        size,
                        dirty,
                        &commands,
                    )?),
                    Event::DevTools(value) if value.get("method").is_some() => {
                        route.diagnostics.lock().unwrap().push(value, line.len());
                        continue;
                    }
                    event => Update::Event(event),
                };
                if route.events.send(update).is_ok() {
                    (route.wake)();
                }
            }
        }
        Ok(())
    })();
    if read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    read?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("CEF host exited with {status}")))
    }
}
fn read_frame(
    dir: &Path,
    files: Arc<SurfaceFiles>,
    tab: u64,
    popup: bool,
    size: [usize; 2],
    dirty: Vec<[usize; 4]>,
    commands: &Sender<Request>,
) -> io::Result<Frame> {
    let path = dir.join(format!("{tab}-{}.bgra", u8::from(popup)));
    let file = std::fs::File::open(&path)?;
    let len = size[0].checked_mul(size[1]).and_then(|n| n.checked_mul(4));
    if len != usize::try_from(file.metadata()?.len()).ok()
        || size.iter().any(|&n| n == 0 || n > 16384)
    {
        return Err(io::Error::other("invalid CEF surface size"));
    }
    // SAFETY: the browser host suspends writes and resize until this frame sends Consumed.
    let pixels = unsafe { Mmap::map(&file)? };
    Ok(Frame {
        size,
        dirty,
        popup,
        path,
        pixels: Some(pixels),
        _files: files,
        tab,
        commands: commands.clone(),
    })
}
