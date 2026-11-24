#[cfg(target_os = "linux")]
#[link(name = "cef")]
unsafe extern "C" {}
use cef::{args::Args, *};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
static PAINT: AtomicBool = AtomicBool::new(false);
static CLOSED: AtomicBool = AtomicBool::new(false);
wrap_render_handler! {
    struct Paint;
    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(r) = rect {
                r.width = 320;
                r.height = 240
            }
        }
        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            _kind: PaintElementType,
            _dirty: Option<&[Rect]>,
            buffer: *const u8,
            w: i32,
            h: i32,
        ) {
            if !buffer.is_null() && w == 320 && h == 240 {
                // SAFETY: CEF owns the full BGRA surface for the duration of OnPaint.
                let pixels = unsafe { std::slice::from_raw_parts(buffer, (w * h * 4) as usize) };
                if pixels.chunks_exact(4).any(|px| px == [51, 34, 17, 255]) {
                    PAINT.store(true, Ordering::Release);
                }
            }
        }
    }
}
wrap_life_span_handler! {
    struct Life;
    impl LifeSpanHandler {
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            CLOSED.store(true, Ordering::Release);
        }
    }
}
static READY: AtomicBool = AtomicBool::new(false);
wrap_request_context_handler! {
    struct Ready;
    impl RequestContextHandler {
        fn on_request_context_initialized(&self, _ctx: Option<&mut RequestContext>) {
            READY.store(true, Ordering::Release);
        }
    }
}
thread_local! {static TOOLS:std::cell::RefCell<Option<Browser>>=const{std::cell::RefCell::new(None)};}
wrap_life_span_handler! {
    struct ToolsLife;
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            TOOLS.with_borrow_mut(|b| *b = browser.cloned());
        }
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            TOOLS.with_borrow_mut(|b| b.take());
        }
    }
}
wrap_client! {
    struct Tools;
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(ToolsLife::new())
        }
    }
}
wrap_client! {
    struct Page;
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(Paint::new())
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(Life::new())
        }
    }
}
fn pump_until(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < Duration::from_secs(20), "timed out");
        do_message_loop_work();
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn main() {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = Args::new();
    let ret = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    if ret >= 0 {
        std::process::exit(ret)
    }
    let runtime = std::env::var("CEF_DIR").unwrap();
    let settings = Settings {
        windowless_rendering_enabled: 1,
        resources_dir_path: runtime.as_str().into(),
        locales_dir_path: format!("{runtime}/locales").as_str().into(),
        root_cache_path: std::env::var("PROBE_PROFILE").unwrap().as_str().into(),
        ..Default::default()
    };
    assert_eq!(
        initialize(
            Some(args.as_main_args()),
            Some(&settings),
            None,
            std::ptr::null_mut()
        ),
        1
    );
    {
        let mut ctx = request_context_create_context(
            Some(&RequestContextSettings::default()),
            Some(&mut Ready::new()),
        )
        .unwrap();
        let win = WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            windowless_rendering_enabled: 1,
            ..Default::default()
        };
        let b = browser_host_create_browser_sync(
            Some(&win),
            Some(&mut Page::new()),
            Some(&"about:blank".into()),
            Some(&BrowserSettings::default()),
            None,
            Some(&mut ctx),
        )
        .unwrap();
        pump_until(|| READY.load(Ordering::Acquire));
        let mut d = dictionary_value_create().unwrap();
        let server = format!(
            "socks5://127.0.0.1:{}",
            std::env::var("PROBE_PORT").unwrap()
        );
        for (k, v) in [
            ("mode", "fixed_servers"),
            ("server", &server),
            ("bypass_list", "*;<-loopback>"),
        ] {
            assert_eq!(d.set_string(Some(&k.into()), Some(&v.into())), 1);
        }
        let mut val = value_create().unwrap();
        val.set_dictionary(Some(&mut d));
        let mut error = CefString::from("");
        assert_eq!(
            ctx.set_preference(Some(&"proxy".into()), Some(&mut val), Some(&mut error)),
            1,
            "{error}"
        );
        b.main_frame().unwrap().load_url(Some(
            &std::env::var("PROBE_ORIGIN").unwrap().as_str().into(),
        ));
        pump_until(|| PAINT.load(Ordering::Acquire));
        println!("received target page paint");
        let host = b.host().unwrap();
        host.show_dev_tools(
            Some(&WindowInfo::default()),
            Some(&mut Tools::new()),
            Some(&BrowserSettings::default()),
            None,
        );
        pump_until(|| {
            host.has_dev_tools() == 1
                && TOOLS.with_borrow(|b| {
                    b.as_ref().and_then(|b| b.host()).is_some_and(|h| {
                        h.is_window_rendering_disabled() == 0
                            && h.window_handle() != Default::default()
                    })
                })
        });
        println!("DevTools opened with a native window handle");
        std::thread::sleep(Duration::from_millis(100));
        host.close_dev_tools();
        pump_until(|| TOOLS.with_borrow(|b| b.is_none()));
        host.close_browser(1);
        pump_until(|| CLOSED.load(Ordering::Acquire));
    }
    shutdown();
}
