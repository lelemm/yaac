//! The YAAC window controller as an Android native library (`libyaac_android.so`,
//! docs/14-android.md).
//!
//! The Compose app (`android/`) draws the view frames of the same controller that drives the
//! Windows WPF window ([`yaac_app::view_protocol`]); only the transport differs: commands and
//! frames cross JNI instead of a pipe. The Kotlin side is
//!
//! ```kotlin
//! package dev.yaac.android.core
//! object NativeController {
//!     external fun start(configJson: String): Long
//!     external fun command(handle: Long, json: String)
//!     external fun pollFrame(handle: Long): String?
//!     external fun waitFrame(handle: Long, timeoutMs: Long): String?
//!     external fun stop(handle: Long)
//! }
//! ```
//!
//! - `start` takes `{"mode": "demo"|"hub", "scenario": "fleet"|"fresh", "theme":
//!   "dark"|"light"|"system", "hubAddress": "host:port", "dataDir": "/data/…"}` (`scenario`
//!   for `demo`, `hubAddress` and `dataDir` for `hub`; optional `"input": "touch"|"pointer"`,
//!   default `touch`, changeable later with the `shell.inputMode` command; optional
//!   `"requests": false` to leave out the hub's request list, `shell.requests`, on by default;
//!   `"browserHost": true` once the app wires its WebView browser host, `core/BrowserHost.kt`;
//!   off by default, and then no browser action is offered, as in a build without one) and returns a handle (never 0). A bad
//!   config throws `IllegalArgumentException`.
//! - `command` queues one `{"command": …, "payload": …}` and never blocks. A malformed command
//!   throws `IllegalArgumentException`; a command to a stopped or unknown handle is dropped.
//! - `pollFrame` returns the next view frame or `null`, without blocking; `waitFrame` waits up
//!   to `timeoutMs` for one. The last frame of a controller is `{"stopped": {"error": …}}`.
//! - `stop` ends the controller and waits for it (drafts are flushed to the hub first). The
//!   handle is then unknown: later calls with it return `null` or do nothing.
//!
//! The controller's platform hooks call back into `dev.yaac.android.core.PlatformBridge`
//! (`bridge`), installed at the first `start`.
//!
//! Handles are keys into a table, not pointers, so a `waitFrame` that races a `stop` on another
//! thread is safe: it finishes on the controller it already holds.

mod bridge;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use yaac_app::view_protocol::{Mode, Options, ViewController};
use yaac_core::demo::Scenario;

/// `start`'s JSON.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    mode: String,
    #[serde(default)]
    scenario: Option<String>,
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    hub_address: Option<String>,
    #[serde(default)]
    data_dir: Option<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    requests: Option<bool>,
    #[serde(default)]
    browser_host: Option<bool>,
}

fn options(json: &str) -> Result<Options, String> {
    let c: Config = serde_json::from_str(json).map_err(|e| format!("bad controller config: {e}"))?;
    let mode = match c.mode.as_str() {
        "demo" => Mode::Demo(match c.scenario.as_deref() {
            None | Some("fleet") => Scenario::Fleet,
            Some("fresh") => Scenario::Fresh,
            Some(other) => return Err(format!("unknown demo scenario {other:?} (fleet or fresh)")),
        }),
        "hub" => Mode::Hub {
            address: c.hub_address.filter(|a| !a.is_empty()).ok_or("hub mode needs hubAddress (host:port)")?,
            data_dir: PathBuf::from(c.data_dir.filter(|d| !d.is_empty()).ok_or("hub mode needs dataDir")?),
        },
        other => return Err(format!("unknown mode {other:?} (demo or hub)")),
    };
    // A phone is a touch screen unless the app says a mouse is in use.
    let input = c.input.unwrap_or_else(|| "touch".into());
    // The app raises its own notifications from the hub's request list unless told not to.
    let requests = c.requests.unwrap_or(true);
    Ok(Options { mode, theme: c.theme.unwrap_or_else(|| "dark".into()), input, requests, platform: "android".into(), browser_host: c.browser_host.unwrap_or(false) })
}

struct Table {
    next: i64,
    live: BTreeMap<i64, Arc<ViewController>>,
}

static CONTROLLERS: Mutex<Table> = Mutex::new(Table { next: 0, live: BTreeMap::new() });

fn get(handle: i64) -> Option<Arc<ViewController>> {
    CONTROLLERS.lock().ok()?.live.get(&handle).cloned()
}

/// Starts a controller; the handle is never 0.
pub fn start(config: &str) -> Result<i64, String> {
    let controller = ViewController::start(options(config)?).map_err(|e| format!("could not start the controller: {e}"))?;
    let mut table = CONTROLLERS.lock().map_err(|_| "internal error")?;
    table.next += 1;
    let handle = table.next;
    table.live.insert(handle, Arc::new(controller));
    Ok(handle)
}

/// Queues one command. `Err` only for a malformed command; an unknown handle drops it.
pub fn command(handle: i64, json: &str) -> Result<(), String> {
    match get(handle) {
        Some(c) => match c.command(json) {
            Err(e) if e.starts_with("not a command") => Err(e),
            _ => Ok(()),
        },
        None => Ok(()),
    }
}

/// The next frame, without blocking.
pub fn poll_frame(handle: i64) -> Option<String> {
    get(handle)?.try_frame()
}

/// The next frame, waiting up to `timeout_ms` (a negative wait is no wait).
pub fn wait_frame(handle: i64, timeout_ms: i64) -> Option<String> {
    let controller = get(handle)?;
    controller.wait_frame(Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(0)))
}

/// Ends the controller and forgets the handle.
pub fn stop(handle: i64) {
    let controller = CONTROLLERS.lock().ok().and_then(|mut t| t.live.remove(&handle));
    if let Some(c) = controller {
        c.stop();
    }
}

/// The JNI symbols of `dev.yaac.android.core.NativeController`. Each one converts its
/// arguments, calls the safe function above inside `catch_unwind` (an unwinding panic must not
/// cross into the JVM), and turns an error into a Java exception.
#[allow(unsafe_code, reason = "`#[unsafe(no_mangle)]` is how the JVM finds a native method")]
pub mod exports {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use jni::JNIEnv;
    use jni::objects::{JObject, JString};
    use jni::sys::{jlong, jstring};

    const ILLEGAL_ARGUMENT: &str = "java/lang/IllegalArgumentException";
    const ILLEGAL_STATE: &str = "java/lang/IllegalStateException";

    fn guarded<T>(env: &mut JNIEnv<'_>, fallback: T, f: impl FnOnce(&mut JNIEnv<'_>) -> T) -> T {
        match catch_unwind(AssertUnwindSafe(|| f(env))) {
            Ok(value) => value,
            Err(_) => {
                let _ = env.throw_new(ILLEGAL_STATE, "the YAAC controller panicked");
                fallback
            }
        }
    }

    fn read(env: &mut JNIEnv<'_>, s: &JString<'_>) -> Option<String> {
        match env.get_string(s) {
            Ok(s) => Some(s.into()),
            Err(e) => {
                let _ = env.throw_new(ILLEGAL_ARGUMENT, format!("not a string: {e}"));
                None
            }
        }
    }

    fn frame(env: &mut JNIEnv<'_>, frame: Option<String>) -> jstring {
        let Some(frame) = frame else { return std::ptr::null_mut() };
        env.new_string(frame).map_or(std::ptr::null_mut(), jni::objects::JString::into_raw)
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_yaac_android_core_NativeController_start<'l>(
        mut env: JNIEnv<'l>,
        _this: JObject<'l>,
        config: JString<'l>,
    ) -> jlong {
        guarded(&mut env, 0, |env| {
            let Some(config) = read(env, &config) else { return 0 };
            // The platform hooks (file pickers, clipboard, opening links), from this Java
            // thread so the app's class loader finds `PlatformBridge`.
            super::bridge::install(env);
            match super::start(&config) {
                Ok(handle) => handle,
                Err(e) => {
                    let _ = env.throw_new(ILLEGAL_ARGUMENT, e);
                    0
                }
            }
        })
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_yaac_android_core_NativeController_command<'l>(
        mut env: JNIEnv<'l>,
        _this: JObject<'l>,
        handle: jlong,
        json: JString<'l>,
    ) {
        guarded(&mut env, (), |env| {
            let Some(json) = read(env, &json) else { return };
            if let Err(e) = super::command(handle, &json) {
                let _ = env.throw_new(ILLEGAL_ARGUMENT, e);
            }
        });
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_yaac_android_core_NativeController_pollFrame<'l>(
        mut env: JNIEnv<'l>,
        _this: JObject<'l>,
        handle: jlong,
    ) -> jstring {
        guarded(&mut env, std::ptr::null_mut(), |env| frame(env, super::poll_frame(handle)))
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_yaac_android_core_NativeController_waitFrame<'l>(
        mut env: JNIEnv<'l>,
        _this: JObject<'l>,
        handle: jlong,
        timeout_ms: jlong,
    ) -> jstring {
        guarded(&mut env, std::ptr::null_mut(), |env| frame(env, super::wait_frame(handle, timeout_ms)))
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_yaac_android_core_NativeController_stop<'l>(
        mut env: JNIEnv<'l>,
        _this: JObject<'l>,
        handle: jlong,
    ) {
        guarded(&mut env, (), |_| super::stop(handle));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_are_read_or_refused_with_a_reason() {
        assert!(matches!(options(r#"{"mode":"demo"}"#).unwrap().mode, Mode::Demo(Scenario::Fleet)));
        assert!(matches!(options(r#"{"mode":"demo","scenario":"fresh","theme":"light"}"#).unwrap().mode, Mode::Demo(Scenario::Fresh)));
        let hub = options(r#"{"mode":"hub","hubAddress":"desk.tailnet.ts.net:7461","dataDir":"/data/x"}"#).unwrap();
        assert!(matches!(hub.mode, Mode::Hub { ref address, .. } if address == "desk.tailnet.ts.net:7461"));
        assert!(options(r#"{"mode":"hub","dataDir":"/data/x"}"#).unwrap_err().contains("hubAddress"));
        assert!(options(r#"{"mode":"hub","hubAddress":"h:1"}"#).unwrap_err().contains("dataDir"));
        assert!(options(r#"{"mode":"window"}"#).is_err());
        assert!(options("nope").is_err());
    }

    #[test]
    fn a_demo_controller_runs_through_its_handle_and_a_stopped_handle_is_inert() {
        let handle = start(r#"{"mode":"demo","scenario":"fleet","theme":"dark"}"#).unwrap();
        assert_ne!(handle, 0);
        let first: serde_json::Value = serde_json::from_str(&wait_frame(handle, 60_000).unwrap()).unwrap();
        assert!(first["views"]["sidebar"].is_object());
        assert!(command(handle, "{").is_err());
        command(handle, r#"{"command":"shell.settings","payload":{}}"#).unwrap();
        stop(handle);
        assert!(poll_frame(handle).is_none());
        assert!(wait_frame(handle, 10).is_none());
        command(handle, r#"{"command":"shell.settings","payload":{}}"#).unwrap();
        stop(handle);
    }
}
