//! CEF browser host. All CEF objects live exclusively on its main thread.
mod tools_title;
#[path = "../../src/wire.rs"]
mod wire;
use cef::{args::Args, *};
use memmap2::MmapMut;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io::{BufRead, Write},
    path::PathBuf,
    rc::Rc as Shared,
};
use wire::{Action, Event, Notice, Request};
thread_local! {
 static TABS:RefCell<HashMap<u64,Tab>>=RefCell::new(HashMap::new());
 static TOOL_WINDOWS:RefCell<HashMap<i32,(Browser,String)>>=RefCell::new(HashMap::new());
 static STOPPING:Cell<bool>=const{Cell::new(false)};
}
fn emit(tab: u64, event: Event) {
    let mut out = std::io::stdout().lock();
    if serde_json::to_writer(&mut out, &Notice { tab, event }).is_ok() {
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}
fn root() -> PathBuf {
    PathBuf::from(std::env::var_os("YAAC_PREVIEW_SESSION").unwrap())
}
struct Tab {
    ready: bool,
    pending: Vec<Action>,
    browser: Browser,
    state: Shared<State>,
    _observer: Option<Registration>,
}
struct State {
    id: u64,
    isolated_html: bool,
    ctrl_click: Cell<Option<std::time::Instant>>,
    bounds: Cell<(i32, i32, f32)>,
    surface: [RefCell<Surface>; 2],
    popup: Cell<[i32; 4]>,
    file_dialog: RefCell<Option<FileDialogCallback>>,
    menu: RefCell<Option<RunContextMenuCallback>>,
    dialog: RefCell<Option<JsdialogCallback>>,
}
#[derive(Default)]
struct Surface {
    map: Option<MmapMut>,
    size: [usize; 2],
    busy: bool,
    missed: bool,
}
impl State {
    fn paint(
        &self,
        popup: bool,
        size: [usize; 2],
        rects: &[Rect],
        pixels: &[u8],
    ) -> std::io::Result<()> {
        let mut surf = self.surface[usize::from(popup)].borrow_mut();
        if surf.busy {
            surf.missed = true;
            return Ok(());
        }
        let mut full = surf.missed;
        if surf.size != size || surf.map.is_none() {
            let path = root().join(format!("{}-{}.bgra", self.id, u8::from(popup)));
            surf.map.take();
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .read(true)
                .truncate(true)
                .open(path)?;
            file.set_len(pixels.len() as u64)?;
            // SAFETY: the controller releases every outstanding view before sending Consumed.
            surf.map = Some(unsafe { MmapMut::map_mut(&file)? });
            surf.size = size;
            full = true;
        }
        let dirty = if full || rects.is_empty() {
            vec![[0, 0, size[0], size[1]]]
        } else {
            rects
                .iter()
                .filter_map(|r| {
                    let [x, y, w, h] =
                        [r.x, r.y, r.width, r.height].map(|i| usize::try_from(i).ok());
                    let (x, y, w, h) = (x?, y?, w?, h?);
                    (x + w <= size[0] && y + h <= size[1] && w > 0 && h > 0).then_some([x, y, w, h])
                })
                .collect()
        };
        if dirty.is_empty() { return Ok(()); }
        let map = surf.map.as_mut().unwrap();
        for &[x, y, w, h] in &dirty {
            for row in y..y + h {
                let start = (row * size[0] + x) * 4;
                map[start..start + w * 4].copy_from_slice(&pixels[start..start + w * 4]);
            }
        }
        surf.busy = true;
        surf.missed = false;
        emit(self.id, Event::Paint { popup, size, dirty });
        Ok(())
    }
}
wrap_render_handler! {
    struct Render {
        s: Shared<State>,
    }
    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(r) = rect {
                (r.width, r.height, _) = self.s.bounds.get();
            }
        }
        fn screen_info(&self, _browser: Option<&mut Browser>, info: Option<&mut ScreenInfo>) -> i32 {
            if let Some(i) = info {
                i.device_scale_factor = self.s.bounds.get().2;
                1
            } else {
                0
            }
        }
        fn on_ime_composition_range_changed(
            &self,
            _browser: Option<&mut Browser>,
            range: Option<&Range>,
            bounds: Option<&[Rect]>,
        ) {
            if let Some(rects) = bounds
                && !rects.is_empty()
            {
                let index = range.map_or(0, |r| r.to.saturating_sub(r.from) as usize);
                let caret = &rects[index.min(rects.len() - 1)];
                emit(
                    self.s.id,
                    Event::CompositionCaret([caret.x, caret.y, caret.width, caret.height]),
                );
            }
        }
        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            if let Some(r) = rect {
                let at = [r.x, r.y, r.width, r.height];
                self.s.popup.set(at);
                emit(self.s.id, Event::Popup(Some(at)));
            }
        }
        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: i32) {
            emit(
                self.s.id,
                Event::Popup((show != 0).then(|| self.s.popup.get())),
            );
        }
        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            kind: PaintElementType,
            dirty: Option<&[Rect]>,
            buffer: *const u8,
            width: i32,
            height: i32,
        ) {
            if buffer.is_null() || !(1..=16384).contains(&width) || !(1..=16384).contains(&height) {
                return;
            }
            let size = [width as usize, height as usize];
            // SAFETY: CEF supplies width * height tightly packed BGRA pixels for this callback.
            let bytes = unsafe { std::slice::from_raw_parts(buffer, size[0] * size[1] * 4) };
            if let Err(e) = self.s.paint(
                kind == PaintElementType::POPUP,
                size,
                dirty.unwrap_or_default(),
                bytes,
            ) {
                emit(self.s.id, Event::Error(e.to_string()));
            }
        }
    }
}
wrap_life_span_handler! {
    struct Life {
        s: Shared<State>,
    }
    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _id: i32,
            url: Option<&CefString>,
            _name: Option<&CefString>,
            disposition: WindowOpenDisposition,
            user_gesture: i32,
            _features: Option<&PopupFeatures>,
            _info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra: Option<&mut Option<DictionaryValue>>,
            _no_js: Option<&mut i32>,
        ) -> i32 {
            if self.s.external_link(url, disposition, user_gesture) {
                return 1;
            }
            if user_gesture != 0 {
                if let Some(frame) = browser.and_then(|b| b.main_frame()) {
                    frame.load_url(url);
                }
            }
            1
        }
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            TABS.with_borrow_mut(|tabs| {
                tabs.remove(&self.s.id);
            });
            emit(self.s.id, Event::Closed);
            maybe_quit();
        }
    }
}
fn maybe_quit() {
    if STOPPING.get()
        && TOOL_WINDOWS.with_borrow(|windows| windows.is_empty())
        && TABS.with_borrow(|tabs| tabs.is_empty())
    {
        quit_message_loop()
    }
}
wrap_download_image_callback! {
    struct Icon {
        id: u64,
    }
    impl DownloadImageCallback {
        fn on_download_image_finished(
            &self,
            _url: Option<&CefString>,
            _status: i32,
            image: Option<&mut Image>,
        ) {
            let (mut w, mut h) = (0, 0);
            if let Some(data) = image.and_then(|i| i.as_png(1., 1, Some(&mut w), Some(&mut h))) {
                if data.size() <= 65536 {
                    let mut png = vec![0; data.size()];
                    if data.data(Some(&mut png), 0) == png.len() {
                        emit(self.id, Event::Favicon(png));
                    }
                }
            }
        }
    }
}
wrap_display_handler! {
    struct Display {
        id: u64,
    }
    impl DisplayHandler {
        fn on_favicon_urlchange(
            &self,
            browser: Option<&mut Browser>,
            icons: Option<&mut CefStringList>,
        ) {
            if let (Some(h), Some(url)) = (
                browser.and_then(|b| b.host()),
                icons.and_then(|i| i.clone().into_iter().next()),
            ) {
                h.download_image(
                    Some(&url.as_str().into()),
                    1,
                    32,
                    0,
                    Some(&mut Icon::new(self.id)),
                );
            }
        }
        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: CursorHandle,
            kind: CursorType,
            custom: Option<&CursorInfo>,
        ) -> i32 {
            if kind == CursorType::CUSTOM
                && let Some(info) = custom
            {
                let (w, h) = (info.size.width, info.size.height);
                if (1..=256).contains(&w) && (1..=256).contains(&h) && !info.buffer.is_null() {
                    // SAFETY: CEF supplies the BGRA bitmap for the life of this callback.
                    let bgra = unsafe {
                        std::slice::from_raw_parts(info.buffer.cast::<u8>(), (w * h * 4) as usize)
                    };
                    let mut rgba = bgra.to_vec();
                    for pixel in rgba.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    emit(
                        self.id,
                        Event::CustomCursor {
                            rgba,
                            size: [w as u16, h as u16],
                            hotspot: [
                                info.hotspot.x.clamp(0, w - 1) as u16,
                                info.hotspot.y.clamp(0, h - 1) as u16,
                            ],
                        },
                    );
                    return 1;
                }
            }
            emit(self.id, Event::Cursor(kind.get_raw() as u32));
            1
        }
        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            if frame.is_some_and(|f| f.is_main() != 0) {
                emit(
                    self.id,
                    Event::Url(url.map(ToString::to_string).unwrap_or_default()),
                );
            }
        }
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            emit(
                self.id,
                Event::Title(title.map(ToString::to_string).unwrap_or_default()),
            );
        }
    }
}
wrap_load_handler! {
    struct Load {
        id: u64,
    }
    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: i32,
            can_go_back: i32,
            can_go_forward: i32,
        ) {
            emit(
                self.id,
                Event::Loading {
                    busy: is_loading != 0,
                    back: can_go_back != 0,
                    forward: can_go_forward != 0,
                },
            );
        }
    }
}
wrap_dialog_handler! {
    struct Files {
        s: Shared<State>,
    }
    impl DialogHandler {
        fn on_file_dialog(
            &self,
            _browser: Option<&mut Browser>,
            mode: FileDialogMode,
            title: Option<&CefString>,
            default_path: Option<&CefString>,
            filters: Option<&mut CefStringList>,
            _extensions: Option<&mut CefStringList>,
            _descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>,
        ) -> i32 {
            self.s.file_dialog.replace(callback.cloned());
            emit(
                self.s.id,
                Event::FileDialog {
                    mode: mode.get_raw() as u32,
                    title: title.map(ToString::to_string).unwrap_or_default(),
                    default_path: default_path.map(ToString::to_string).unwrap_or_default(),
                    filters: filters
                        .map(|s| s.clone().into_iter().collect())
                        .unwrap_or_default(),
                },
            );
            1
        }
    }
}
wrap_jsdialog_handler! {
    struct Dialogs {
        s: Shared<State>,
    }
    impl JsdialogHandler {
        fn on_reset_dialog_state(&self, _browser: Option<&mut Browser>) {
            if self.s.dialog.take().is_some() {
                emit(self.s.id, Event::DismissDialog);
            }
        }
        fn on_jsdialog(
            &self,
            _browser: Option<&mut Browser>,
            _origin: Option<&CefString>,
            kind: JsdialogType,
            message: Option<&CefString>,
            text: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>,
            _suppress: Option<&mut i32>,
        ) -> i32 {
            self.s.dialog.replace(callback.cloned());
            emit(
                self.s.id,
                Event::Dialog {
                    kind: kind.get_raw() as u32,
                    message: message.map(ToString::to_string).unwrap_or_default(),
                    text: text.map(ToString::to_string).unwrap_or_default(),
                },
            );
            1
        }
    }
}
wrap_context_menu_handler! {
    struct Menus {
        s: Shared<State>,
    }
    impl ContextMenuHandler {
        fn on_context_menu_dismissed(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
        ) {
            if self.s.menu.take().is_some() {
                emit(self.s.id, Event::DismissMenu);
            }
        }
        fn run_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> i32 {
            let (Some(p), Some(m)) = (params, model) else {
                return 0;
            };
            let items = (0..m.count())
                .filter(|&i| matches!(m.type_at(i), MenuItemType::COMMAND | MenuItemType::CHECK | MenuItemType::RADIO))
                .map(|i| {
                    (
                        m.command_id_at(i),
                        menu_label(&CefString::from(&m.label_at(i)).to_string()),
                        m.is_enabled_at(i) != 0,
                    )
                })
                .filter(|(_, s, _)| !s.is_empty())
                .collect();
            self.s.menu.replace(callback.cloned());
            emit(
                self.s.id,
                Event::Menu {
                    at: [p.xcoord(), p.ycoord()],
                    items,
                },
            );
            1
        }
    }
}

fn menu_label(label: &str) -> String {
    let mut chars = label.chars().peekable();
    let mut text = String::new();
    while let Some(ch) = chars.next() {
        if ch != '&' || chars.next_if_eq(&'&').is_some() {
            text.push(ch);
        }
    }
    text
}
wrap_life_span_handler! {
    struct ToolsLife {
        title: String,
    }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                let id = browser.identifier();
                TOOL_WINDOWS.with_borrow_mut(|windows| {
                    windows.insert(id, (browser.clone(), self.title.clone()))
                });
                post(Job::SetTitle(id, 0));
            }
        }
        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                TOOL_WINDOWS.with_borrow_mut(|windows| windows.remove(&browser.identifier()));
            }
            maybe_quit();
        }
    }
}
wrap_client! {
    struct Tools {
        title: String,
    }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(ToolsLife::new(self.title.clone()))
        }
    }
}
wrap_client! {
    struct Page {
        s: Shared<State>,
    }
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(Render::new(self.s.clone()))
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(PageNavigation::new(self.s.clone()))
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(Life::new(self.s.clone()))
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(Load::new(self.s.id))
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(Display::new(self.s.id))
        }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(Menus::new(self.s.clone()))
        }
        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(Files::new(self.s.clone()))
        }
        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            Some(Dialogs::new(self.s.clone()))
        }
    }
}

const ARTIFACT_URL: &str = "https://artifact.invalid/document";

wrap_request_handler! {
    struct PageNavigation { s: Shared<State> }
    impl RequestHandler {
        fn on_before_browse(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
            request: Option<&mut cef::Request>, _user_gesture: i32, _is_redirect: i32) -> i32 {
            if !self.s.isolated_html { return 0; }
            let Some(request) = request else { return 1 };
            let url = CefString::from(&request.url()).to_string();
            let page = url.split('#').next().unwrap_or_default();
            i32::from(!matches!(page, ARTIFACT_URL | "about:blank"))
        }
        fn on_open_urlfrom_tab(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
            url: Option<&CefString>, disposition: WindowOpenDisposition, user_gesture: i32) -> i32 {
            i32::from(self.s.external_link(url, disposition, user_gesture))
        }
    }
}

impl State {
    fn external_link(&self, url: Option<&CefString>, disposition: WindowOpenDisposition, user_gesture: i32) -> bool {
        if user_gesture == 0 || !matches!(disposition, WindowOpenDisposition::NEW_BACKGROUND_TAB | WindowOpenDisposition::NEW_FOREGROUND_TAB)
            || !self.ctrl_click.take().is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(2)) {
            return false;
        }
        if let Some(url) = url {
            emit(self.id, Event::ExternalLink(url.to_string()));
        }
        true
    }
}
wrap_dev_tools_message_observer! {
    struct Observer {
        id: u64,
    }
    impl DevToolsMessageObserver {
        fn on_dev_tools_message(&self, _browser: Option<&mut Browser>, message: Option<&[u8]>) -> i32 {
            if let Some(mut v) =
                message.and_then(|s| serde_json::from_slice::<serde_json::Value>(s).ok())
            {
                if v.get("id").is_none() {
                    let params = &v["params"];
                    let method = v["method"].as_str().unwrap_or_default();
                    let summary = match method {
                        "Network.requestWillBeSent" => serde_json::json!({
                            "requestId": params["requestId"], "timestamp": params["timestamp"],
                            "request": {"url":params["request"]["url"],"method":params["request"]["method"]}
                        }),
                        "Network.responseReceived" => serde_json::json!({
                            "requestId": params["requestId"], "timestamp": params["timestamp"],
                            "response": {"url":params["response"]["url"],"status":params["response"]["status"],"timing":params["response"]["timing"]}
                        }),
                        "Runtime.consoleAPICalled"
                        | "Runtime.exceptionThrown"
                        | "Network.loadingFailed"
                        | "Network.loadingFinished" => params.clone(),
                        _ => return 1,
                    };
                    v["params"] = summary;
                }
                emit(self.id, Event::DevTools(v));
            }
            1
        }
    }
}
fn context_proxy(ctx: &RequestContext, socks: u16) -> Result<(), String> {
    let mut d = dictionary_value_create().ok_or("dictionary unavailable")?;
    // Every host, loopback included, through the target (P323); see wire::proxy_settings.
    for (k, v) in wire::proxy_settings(socks) {
        if d.set_string(Some(&k.into()), Some(&v.as_str().into())) == 0 {
            return Err(format!("cannot set proxy {k}"));
        }
    }
    let mut val = value_create().ok_or("value unavailable")?;
    val.set_dictionary(Some(&mut d));
    let mut error = CefString::from("");
    if ctx.set_preference(Some(&"proxy".into()), Some(&mut val), Some(&mut error)) == 0 {
        return Err(error.to_string());
    }
    // WebRTC would otherwise open UDP sockets from the host; a tab that cannot be kept off the
    // host's network is not opened.
    let (name, policy) = wire::WEBRTC_POLICY;
    let mut val = value_create().ok_or("value unavailable")?;
    val.set_string(Some(&policy.into()));
    if ctx.set_preference(Some(&name.into()), Some(&mut val), Some(&mut error)) == 0 {
        Err(format!("cannot set {name}: {error}"))
    } else {
        Ok(())
    }
}
// Only the private controller pipe supplies this path. No part of a browser URL
// is interpreted as a host filename, and other tabs have separate request contexts.
wrap_resource_handler! {
    struct ArtifactResponse { stream: ResourceHandler, html: bool }
    impl ResourceHandler {
        fn open(&self, request: Option<&mut cef::Request>, handle: Option<&mut i32>, callback: Option<&mut Callback>) -> i32 {
            self.stream.open(request, handle, callback)
        }
        fn response_headers(&self, response: Option<&mut Response>, length: Option<&mut i64>, redirect: Option<&mut CefString>) {
            let Some(response) = response else { return };
            self.stream.response_headers(Some(response), length, redirect);
            response.set_header_by_name(Some(&"X-Content-Type-Options".into()), Some(&"nosniff".into()), 1);
            response.set_header_by_name(Some(&"Cache-Control".into()), Some(&"no-store".into()), 1);
            if self.html {
                response.set_charset(Some(&"utf-8".into()));
                response.set_header_by_name(Some(&"Content-Security-Policy".into()),
                    Some(&"sandbox allow-popups; default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'".into()), 1);
                response.set_header_by_name(Some(&"Referrer-Policy".into()), Some(&"no-referrer".into()), 1);
            }
        }
        fn read(&self, output: *mut u8, count: i32, read: Option<&mut i32>, callback: Option<&mut ResourceReadCallback>) -> i32 {
            self.stream.read(output, count, read, callback)
        }
    }
}

wrap_scheme_handler_factory! {
    struct SnapshotResource { path: String, mime: String }
    impl SchemeHandlerFactory {
        fn create(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
            _scheme: Option<&CefString>, request: Option<&mut cef::Request>) -> Option<ResourceHandler> {
            use cef::wrapper::stream_resource_handler::StreamResourceHandler;
            if CefString::from(&request?.url()).to_string() != ARTIFACT_URL {
                return Some(StreamResourceHandler::new(404, "Not Found".into(), "text/plain".into(), None, None));
            }
            let stream = stream_reader_create_for_file(Some(&self.path.as_str().into()))?;
            Some(ArtifactResponse::new(
                StreamResourceHandler::new_with_stream(self.mime.clone(), stream), self.mime == "text/html"))
        }
    }
}
wrap_request_context_handler! {
    struct ContextReady {
        id: u64,
        url: String,
        socks: Option<u16>,
        snapshot: Option<PathBuf>,
    }
    impl RequestContextHandler {
        fn on_request_context_initialized(&self, ctx: Option<&mut RequestContext>) {
            let Some(ctx) = ctx else { emit(self.id, Event::Error("Missing browser context".into())); return; };
            if let Some(path) = &self.snapshot {
                let mime = match path.extension().and_then(|s| s.to_str()) {
                    Some("txt") => "text/plain", Some("pdf") => "application/pdf",
                    Some("html" | "htm") => "text/html",
                    Some("png") => "image/png", Some("gif") => "image/gif",
                    Some("webp") => "image/webp", Some("jpg" | "jpeg") => "image/jpeg",
                    _ => { emit(self.id, Event::Error("Unsupported document type".into())); return; }
                };
                let mut factory = SnapshotResource::new(path.to_string_lossy().into_owned(), mime.into());
                if ctx.register_scheme_handler_factory(Some(&"https".into()), Some(&"artifact.invalid".into()), Some(&mut factory)) == 0 {
                    emit(self.id, Event::Error("Cannot display this document".into()));
                    return;
                }
            }
            if let Some(port) = self.socks {
                if let Err(e) = context_proxy(ctx, port)
                {
                    emit(self.id, Event::Error(e));
                    enqueue(Request {
                        tab: self.id,
                        action: Action::Close,
                    });
                    return;
                }
            }
            // Queue navigation so create_browser_sync has returned before we access the tab.
            post(Job::Ready(self.id, self.url.clone()));
        }
    }
}
fn open(id: u64, url: String, socks: Option<u16>, snapshot: Option<PathBuf>) -> Result<(), String> {
    let isolated_html = snapshot.as_deref().and_then(|path| path.extension()).and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "html" | "htm"));
    let mut ctx = request_context_create_context(
        Some(&RequestContextSettings::default()),
        Some(&mut ContextReady::new(id, url, socks, snapshot)),
    )
    .ok_or("request context unavailable")?;
    let state = Shared::new(State {
        id,
        isolated_html,
        ctrl_click: Cell::new(None),
        bounds: Cell::new((800, 600, 1.0)),
        surface: Default::default(),
        popup: Default::default(),
        file_dialog: Default::default(),
        menu: Default::default(),
        dialog: Default::default(),
    });
    let browser = browser_host_create_browser_sync(
        Some(&WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            windowless_rendering_enabled: 1,
            ..Default::default()
        }),
        Some(&mut Page::new(state.clone())),
        Some(&"about:blank".into()),
        Some(&BrowserSettings {
            javascript: if isolated_html { cef::State::DISABLED } else { cef::State::DEFAULT },
            ..Default::default()
        }),
        None,
        Some(&mut ctx),
    )
    .ok_or("browser creation failed")?;
    let host = browser.host().ok_or("browser host unavailable")?;
    let observer = host.add_dev_tools_message_observer(Some(&mut Observer::new(id)));
    for (id, method) in [(1, "Runtime.enable"), (2, "Network.enable")] {
        let msg = serde_json::json!({"id":id,"method":method}).to_string();
        host.send_dev_tools_message(Some(msg.as_bytes()));
    }
    TABS.with_borrow_mut(|tabs| {
        tabs.insert(
            id,
            Tab {
                ready: false,
                pending: Vec::new(),
                browser,
                state,
                _observer: observer,
            },
        )
    });
    Ok(())
}
enum Job {
    SetTitle(i32, u8),
    Command(Request),
    Ready(u64, String),
}
type PendingJob = std::sync::Arc<std::sync::Mutex<Option<Job>>>;
wrap_task! {
    struct Run {
        job: PendingJob,
    }
    impl Task {
        fn execute(&self) {
            let job = self.job.lock().unwrap().take();
            match job {
                Some(Job::SetTitle(id, attempt)) => {
                    let target = TOOL_WINDOWS.with_borrow(|w| w.get(&id).cloned());
                    if let Some((mut browser, title)) = target {
                        if !tools_title::apply(&mut browser, &title) && attempt < 10 {
                            let job = std::sync::Arc::new(std::sync::Mutex::new(Some(Job::SetTitle(
                                id,
                                attempt + 1,
                            ))));
                            post_delayed_task(ThreadId::UI, Some(&mut Run::new(job)), 50);
                        }
                    }
                }
                Some(Job::Ready(tab, url)) => {
                    let pending = TABS.with_borrow_mut(|tabs| {
                        tabs.get_mut(&tab).map(|t| {
                            t.ready = true;
                            std::mem::take(&mut t.pending)
                        })
                    });
                    if let Some(actions) = pending {
                        handle(Request {
                            tab,
                            action: Action::Navigate(url),
                        });
                        for action in actions {
                            handle(Request { tab, action })
                        }
                    }
                }
                Some(Job::Command(req)) => handle(req),
                None => {}
            }
        }
    }
}
fn post(job: Job) {
    post_task(
        ThreadId::UI,
        Some(&mut Run::new(std::sync::Arc::new(std::sync::Mutex::new(
            Some(job),
        )))),
    );
}
fn enqueue(req: Request) {
    post(Job::Command(req));
}
fn handle(req: Request) {
    use Action::*;
    let id = req.tab;
    match req.action {
        Open { url, socks, snapshot } => {
            if let Err(e) = open(id, url, socks, snapshot) {
                emit(id, Event::Error(e))
            }
            return;
        }
        Shutdown => {
            STOPPING.set(true);
            let browsers = TABS
                .with_borrow(|tabs| tabs.values().map(|t| t.browser.clone()).collect::<Vec<_>>());
            for b in browsers {
                if let Some(h) = b.host() {
                    h.close_dev_tools();
                    h.close_browser(1)
                }
            }
            maybe_quit();
            return;
        }
        _ => {}
    }
    if !matches!(req.action, Close | Consumed { .. })
        && TABS.with_borrow(|tabs| tabs.get(&id).is_some_and(|t| !t.ready))
    {
        TABS.with_borrow_mut(|tabs| tabs.get_mut(&id).unwrap().pending.push(req.action));
        return;
    }
    let Some((b, s)) =
        TABS.with_borrow(|tabs| tabs.get(&id).map(|t| (t.browser.clone(), t.state.clone())))
    else {
        return;
    };
    let Some(h) = b.host() else { return };
    match req.action {
        Close => {
            h.close_dev_tools();
            h.close_browser(1)
        }
        Navigate(url) => {
            if let Some(f) = b.main_frame() {
                f.load_url(Some(&url.as_str().into()));
            }
        }
        Back => b.go_back(),
        Forward => b.go_forward(),
        Reload => b.reload(),
        Stop => b.stop_load(),
        Bounds {
            width,
            height,
            scale,
        } => {
            s.bounds.set((
                width.clamp(1, 8192),
                height.clamp(1, 8192),
                scale.clamp(0.5, 4.0),
            ));
            h.notify_screen_info_changed();
            h.was_resized();
        }
        Invalidate => {
            s.surface[0].borrow_mut().missed = true;
            h.invalidate(PaintElementType::VIEW);
        }
        Hidden(v) => {
            h.was_hidden(v.into());
            if !v {
                s.surface[0].borrow_mut().missed = true;
                h.invalidate(PaintElementType::VIEW);
            }
        }
        Focus(v) => {
            if !v && let Some(menu) = s.menu.take() {
                menu.cancel();
                emit(id, Event::DismissMenu);
            }
            h.set_focus(v.into());
        }
        Inspect { title, at } => {
            #[cfg(not(windows))]
            let win = WindowInfo {
                window_name: title.as_str().into(),
                bounds: Rect {
                    x: 0,
                    y: 0,
                    width: 1000,
                    height: 750,
                },
                ..Default::default()
            };
            #[cfg(windows)]
            let win = WindowInfo::default().set_as_popup(Default::default(), &title);
            let p = at.map(|[x, y]| Point { x, y });
            h.show_dev_tools(
                Some(&win),
                Some(&mut Tools::new(title)),
                Some(&BrowserSettings::default()),
                p.as_ref(),
            );
        }
        Consumed { popup } => {
            let mut surf = s.surface[usize::from(popup)].borrow_mut();
            surf.busy = false;
            let missed = surf.missed;
            drop(surf);
            if missed {
                h.invalidate(if popup {
                    PaintElementType::POPUP
                } else {
                    PaintElementType::VIEW
                })
            }
        }
        Pointer { x, y, flags, kind } => {
            let e = MouseEvent {
                x,
                y,
                modifiers: flags,
            };
            match kind {
                wire::Pointer::Move => h.send_mouse_move_event(Some(&e), 0),
                wire::Pointer::Leave => h.send_mouse_move_event(Some(&e), 1),
                wire::Pointer::Button { button, up, count } => {
                    s.ctrl_click.set((button == 0 && up && flags & 4 != 0).then(std::time::Instant::now));
                    h.send_mouse_click_event(
                    Some(&e),
                    match button {
                        1 => MouseButtonType::MIDDLE,
                        2 => MouseButtonType::RIGHT,
                        _ => MouseButtonType::LEFT,
                    },
                    up.into(),
                    count,
                    );
                }
                wire::Pointer::Wheel(dx, dy) => h.send_mouse_wheel_event(Some(&e), dx, dy),
            }
        }
        Key { vk, flags, pressed } => h.send_key_event(Some(&KeyEvent {
            windows_key_code: vk,
            modifiers: flags,
            type_: if pressed {
                KeyEventType::RAWKEYDOWN
            } else {
                KeyEventType::KEYUP
            },
            ..Default::default()
        })),
        Text(t) => {
            for ch in t.encode_utf16() {
                h.send_key_event(Some(&KeyEvent {
                    windows_key_code: i32::from(ch),
                    character: ch,
                    unmodified_character: ch,
                    type_: KeyEventType::CHAR,
                    ..Default::default()
                }));
            }
        }
        Compose(t) => {
            let end = t.encode_utf16().count() as u32;
            h.ime_set_composition(
                Some(&t.as_str().into()),
                None,
                Some(&Range {
                    from: u32::MAX,
                    to: u32::MAX,
                }),
                Some(&Range { from: end, to: end }),
            );
        }
        Commit(t) => h.ime_commit_text(
            Some(&t.as_str().into()),
            Some(&Range {
                from: u32::MAX,
                to: u32::MAX,
            }),
            0,
        ),
        CancelComposition => h.ime_cancel_composition(),
        FileDialog(paths) => {
            if let Some(cb) = s.file_dialog.take() {
                if let Some(paths) = paths {
                    let mut list = CefStringList::new();
                    for path in paths {
                        list.append(&path);
                    }
                    cb.cont(Some(&mut list));
                } else {
                    cb.cancel();
                }
            }
        }
        Menu(cmd) => {
            if let Some(cb) = s.menu.take() {
                if let Some(i) = cmd {
                    cb.cont(i, EventFlags::default())
                } else {
                    cb.cancel()
                }
            }
        }
        Dialog { accept, text } => {
            if let Some(cb) = s.dialog.take() {
                cb.cont(accept.into(), Some(&text.as_str().into()));
            }
        }
        DevTools {
            id: call,
            method,
            params,
        } => {
            let msg = serde_json::json!({"id":call,"method":method,"params":params}).to_string();
            if h.send_dev_tools_message(Some(msg.as_bytes())) == 0 {
                emit(
                    id,
                    Event::DevTools(
                        serde_json::json!({"id":call,"error":{"message":"command rejected"}}),
                    ),
                );
            }
        }
        Open { .. } | Shutdown => unreachable!(),
    }
}
/// Called only in a dedicated browser host or a CEF subprocess.
#[unsafe(no_mangle)]
pub extern "C" fn yaac_cef_main() -> i32 {
    main_with_sandbox(std::ptr::null_mut())
}
/// Entry point for the CEF distribution's sandbox bootstrap.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "C" fn RunWinMain(
    _instance: *mut std::ffi::c_void,
    _cmd: *mut u16,
    _show: i32,
    sandbox: *mut std::ffi::c_void,
    _version: *const std::ffi::c_void,
) -> i32 {
    main_with_sandbox(sandbox)
}
fn main_with_sandbox(sandbox: *mut std::ffi::c_void) -> i32 {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = Args::new();
    let ret = execute_process(Some(args.as_main_args()), None, sandbox.cast());
    if ret >= 0 {
        return ret;
    }
    let (Ok(dir), Some(session)) = (
        std::env::var("YAAC_CEF_DIR"),
        std::env::var_os("YAAC_PREVIEW_SESSION"),
    ) else {
        eprintln!("Browser host requires a controller session");
        return 1;
    };
    let profile = PathBuf::from(session).join("profile");
    let settings = Settings {
        windowless_rendering_enabled: 1,
        resources_dir_path: dir.as_str().into(),
        locales_dir_path: std::path::Path::new(&dir)
            .join("locales")
            .to_string_lossy()
            .as_ref()
            .into(),
        root_cache_path: profile.to_string_lossy().as_ref().into(),
        ..Default::default()
    };
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        None,
        sandbox.cast(),
    ) == 0
    {
        return 1;
    }
    std::thread::spawn(|| {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if let Ok(request) = serde_json::from_str(&line) {
                enqueue(request)
            }
        }
        enqueue(Request {
            tab: 0,
            action: Action::Shutdown,
        });
    });
    run_message_loop();
    shutdown();
    0
}
