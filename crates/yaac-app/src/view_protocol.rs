//! The view protocol over channels: the window controller that drives the WPF window, for an
//! embedder in this process (the Android library, `crates/yaac-android`).
//!
//! The controller is the same loop the WPF window runs (`wpf::serve`): UI commands go in as
//! JSON objects (`{"command": "sidebar.open", "payload": {…}}`, exactly what WPF sends), and
//! view frames come out as JSON objects (`{"views": {…}}` with only the views that changed,
//! the transcript as a diff, and the one-off frames such as `{"copy": …}` or `{"error": …}`).
//! Only the transport differs: [`ViewController`] runs the loop on its own thread and hands
//! the frames over a channel instead of a pipe.
//!
//! When the loop ends, one last frame says so, and no frame follows it:
//! `{"stopped": {"error": null}}`, or with the reason when it failed.

use std::io::{self, Write};

use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};
use yaac_core::demo::Scenario;

use crate::theme::ThemeChoice;
use crate::wpf::{Connection, Input};

/// What the controller is attached to.
#[derive(Clone, Debug)]
pub enum Mode {
    /// A scripted in-memory hub (`yaac --demo`).
    Demo(Scenario),
    /// A hub on another machine at `address` (`host:port`, its Tailscale attach listener).
    /// `data_dir` holds this client's own files.
    #[cfg(feature = "hub")]
    Hub { address: String, data_dir: std::path::PathBuf },
}

/// How the controller starts.
#[derive(Clone, Debug)]
pub struct Options {
    pub mode: Mode,
    /// `dark`, `light` or `system` (anything else is dark).
    pub theme: String,
    /// `touch` for a touch screen; anything else is a mouse and keyboard. The embedder can
    /// change it later with the `shell.inputMode` command (`{"mode": "pointer"|"touch"}`).
    pub input: String,
    /// Subscribe to the hub's request reminder list (`shell.requests` in the shell view;
    /// `shell.requestFeed`), for an embedder that raises its own notifications.
    pub requests: bool,
    /// `android` for the Android app (its tray and renderer settings); anything else is the
    /// desktop.
    pub platform: String,
    /// The embedder runs its own browser host (Android WebViews, P380): the controller sends
    /// it `browserHost` and `browserHostCall` frames and runs the tunnel proxy for it.
    pub browser_host: bool,
}

/// One controller, running on its own thread. Every method takes `&self` and the type is
/// `Sync`, so one controller can be shared between a thread that waits for frames and one that
/// sends commands and stops it.
#[derive(Debug)]
pub struct ViewController {
    /// Unbounded, so sending a command never blocks; a relay thread feeds the loop's own
    /// bounded input (which its wakes share).
    commands: mpsc::Sender<Input>,
    frames: Mutex<mpsc::Receiver<String>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Input::Command(_) => "Command",
            Input::Wake => "Wake",
            Input::BrowserWake => "BrowserWake",
            Input::Closed => "Closed",
        })
    }
}

fn theme(id: &str) -> ThemeChoice {
    match id {
        "light" => ThemeChoice::Light,
        "system" => ThemeChoice::System,
        _ => ThemeChoice::Dark,
    }
}

fn launch(options: Options) -> crate::Launch {
    let theme = theme(&options.theme);
    match options.mode {
        Mode::Demo(scenario) => crate::wpf::demo_launch(scenario, theme),
        #[cfg(feature = "hub")]
        Mode::Hub { address, data_dir } => {
            let paths = yaac_hub::HubPaths {
                config_dir: data_dir.join("config"),
                data_dir: data_dir.clone(),
                runtime_dir: data_dir.join("run"),
            };
            crate::Launch {
                demo: None,
                theme,
                shots: None,
                host_windows: false,
                attach: Some(crate::attach::AttachConfig::remote(address, paths)),
                display: crate::display::DisplayState::default(),
                shell: crate::tray::Shell::new(),
                tray: false,
                updater: crate::updater::Updater::off("updates come from the app store"),
            }
        }
    }
}

impl ViewController {
    /// Starts the controller. The first view frame follows at once.
    pub fn start(options: Options) -> io::Result<Self> {
        let connection = Connection::channel();
        let input = connection.sender();
        if options.browser_host {
            let _ = input.send(Input::Command(json!({ "command": "shell.browserHost", "payload": {} })));
        }
        if options.platform == "android" {
            let platform = json!({ "command": "shell.platform", "payload": { "platform": "android" } });
            let _ = input.send(Input::Command(platform));
        }
        if options.input == "touch" {
            // Read before any other command; the first frame is drawn before it, for pointer.
            let mode = json!({ "command": "shell.inputMode", "payload": { "mode": "touch" } });
            let _ = input.send(Input::Command(mode));
        }
        if options.requests {
            let _ = input.send(Input::Command(json!({ "command": "shell.requestFeed", "payload": {} })));
        }
        let (commands, relay) = mpsc::channel::<Input>();
        std::thread::Builder::new().name("yaac-view-commands".into()).spawn(move || {
            for command in relay {
                let last = matches!(command, Input::Closed);
                if input.send(command).is_err() || last {
                    return;
                }
            }
            // Every handle is gone without a stop: end the loop all the same.
            let _ = input.send(Input::Closed);
        })?;
        let (sink, frames) = FrameSink::new();
        let thread = std::thread::Builder::new().name("yaac-view-controller".into()).spawn(move || {
            let mut sink = sink;
            let ended = crate::wpf::serve(launch(options), connection, &mut sink, None);
            let error = match ended {
                Err(e) if e.kind() != io::ErrorKind::BrokenPipe => Some(e.to_string()),
                _ => None,
            };
            let _ = sink.frames.send(json!({ "stopped": { "error": error } }).to_string());
        })?;
        Ok(Self { commands, frames: Mutex::new(frames), thread: Mutex::new(Some(thread)) })
    }

    /// Queues one UI command (`{"command": …, "payload": …}`). Never blocks.
    pub fn command(&self, json: &str) -> Result<(), String> {
        let frame: Value = serde_json::from_str(json).map_err(|e| format!("not a command: {e}"))?;
        if !frame["command"].is_string() {
            return Err("not a command: no \"command\" field".into());
        }
        self.commands.send(Input::Command(frame)).map_err(|_| "the controller has stopped".to_owned())
    }

    /// The next view frame, if one is waiting. Never blocks: `None` also while another
    /// thread is in [`ViewController::wait_frame`].
    pub fn try_frame(&self) -> Option<String> {
        self.frames.try_lock().ok()?.try_recv().ok()
    }

    /// The next view frame, waiting up to `timeout`. `None` on timeout, or once the
    /// controller has stopped and its last frame was taken.
    pub fn wait_frame(&self, timeout: Duration) -> Option<String> {
        let frames = self.frames.lock().ok()?;
        frames.recv_timeout(timeout).ok()
    }

    /// Ends the loop (drafts are flushed to the hub first) and waits for its thread. Frames
    /// written before the end, and the `stopped` frame, can still be taken afterwards.
    pub fn stop(&self) {
        self.close();
        let thread = self.thread.lock().ok().and_then(|mut t| t.take());
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }

    fn close(&self) {
        let _ = self.commands.send(Input::Closed);
    }
}

/// The controller's output as a [`Write`]: `wpf::serve` writes each frame as one line and
/// flushes it; each complete line becomes one frame on the channel.
pub(crate) struct FrameSink {
    line: Vec<u8>,
    frames: mpsc::Sender<String>,
}

impl FrameSink {
    pub(crate) fn new() -> (Self, mpsc::Receiver<String>) {
        let (frames, receiver) = mpsc::channel();
        (Self { line: Vec::new(), frames }, receiver)
    }
}

impl Write for FrameSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.line.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        while let Some(end) = self.line.iter().position(|&b| b == b'\n') {
            let rest = self.line.split_off(end + 1);
            let mut line = std::mem::replace(&mut self.line, rest);
            line.pop();
            let frame = String::from_utf8(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            self.frames.send(frame).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first frame a fresh demo-fleet controller emits, without any command.
    fn first_frame_through_channel() -> Value {
        let controller = ViewController::start(Options { mode: Mode::Demo(Scenario::Fleet), theme: "dark".into(), input: "pointer".into(), requests: false, platform: String::new(), browser_host: false }).unwrap();
        let frame = controller.wait_frame(Duration::from_secs(60)).expect("a first frame");
        controller.stop();
        serde_json::from_str(&frame).unwrap()
    }

    fn first_frame_through_pipe() -> Value {
        let launch = crate::wpf::demo_launch(Scenario::Fleet, ThemeChoice::Dark);
        // An input that ends at once: the loop writes its first frame, then sees the close.
        let commands = Connection::new(io::BufReader::new(io::empty()));
        let mut out = Vec::new();
        crate::wpf::serve(launch, commands, &mut out, None).unwrap();
        let first = out.split(|&b| b == b'\n').next().unwrap();
        serde_json::from_slice(first).unwrap()
    }

    #[test]
    fn the_demo_fleet_emits_the_same_first_frame_through_a_channel_as_through_the_pipe() {
        let channel = first_frame_through_channel();
        let pipe = first_frame_through_pipe();
        assert!(channel["views"]["sidebar"].is_object(), "the first frame carries every view");
        assert_eq!(channel, pipe);
    }

    #[test]
    fn commands_reach_the_controller_and_the_last_frame_says_it_stopped() {
        let controller = ViewController::start(Options { mode: Mode::Demo(Scenario::Fleet), theme: "light".into(), input: "pointer".into(), requests: false, platform: String::new(), browser_host: false }).unwrap();
        assert!(controller.wait_frame(Duration::from_secs(60)).is_some());
        assert!(controller.command("not json").is_err());
        controller.command(r#"{"command":"shell.settings","payload":{}}"#).unwrap();
        let mut settings = false;
        while let Some(frame) = controller.wait_frame(Duration::from_secs(60)) {
            let frame: Value = serde_json::from_str(&frame).unwrap();
            if frame["views"]["shell"]["settings"] == true {
                settings = true;
                break;
            }
        }
        assert!(settings, "Settings opened");
        controller.command(r#"{"command":"no.such","payload":{}}"#).unwrap();
        let error = std::iter::from_fn(|| controller.wait_frame(Duration::from_secs(60)))
            .map(|f| serde_json::from_str::<Value>(&f).unwrap())
            .find(|f| f.get("error").is_some())
            .unwrap();
        assert!(error["error"].as_str().unwrap().contains("no.such"));
        controller.close();
        // The loop ends, its sink is dropped, and the channel closes after the last frame.
        let last: Value = std::iter::from_fn(|| controller.wait_frame(Duration::from_secs(60)))
            .last()
            .map(|f| serde_json::from_str(&f).unwrap())
            .unwrap();
        assert_eq!(last, json!({ "stopped": { "error": null } }));
    }

    /// Frames until `done` holds for the merged shell view (or the controller stops).
    fn shell_until(controller: &ViewController, shell: &mut Value, mut done: impl FnMut(&Value, &Value) -> bool) -> bool {
        while let Some(frame) = controller.wait_frame(Duration::from_secs(30)) {
            let frame: Value = serde_json::from_str(&frame).unwrap();
            if let Some(next) = frame["views"].get("shell") {
                *shell = next.clone();
            }
            if done(&frame, shell) {
                return true;
            }
        }
        false
    }

    #[test]
    fn the_request_list_is_sent_only_when_asked_and_answers_with_its_revision() {
        let plain = first_frame_through_channel();
        assert!(plain["views"]["shell"].get("requests").is_none(), "the WPF frames do not change");

        let options = Options { mode: Mode::Demo(Scenario::Fleet), theme: "dark".into(), input: "touch".into(), requests: true, platform: String::new(), browser_host: false };
        let controller = ViewController::start(options).unwrap();
        let mut shell = Value::Null;
        assert!(shell_until(&controller, &mut shell, |_, s| s["requests"]["items"].as_array().is_some_and(|i| !i.is_empty())));
        let requests = shell["requests"].clone();
        let revision = requests["revision"].as_u64().unwrap();
        let items = requests["items"].as_array().unwrap();
        let answerable = items.iter().find(|i| i["decisions"].as_array().is_some_and(|d| d.len() == 2)).expect("a plain approval").clone();
        assert_eq!(answerable["decisions"][0], json!({ "id": "allow", "label": "Allow once" }));
        assert!(answerable["threadId"].as_str().unwrap().contains('/'));
        let pick = |item: &Value, extra: Value| {
            let mut payload = json!({ "threadId": item["threadId"], "requestId": item["requestId"] });
            payload.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            payload
        };
        let gone = |s: &Value| !s["requests"]["items"].as_array().unwrap().iter().any(|i| i["requestId"] == answerable["requestId"] && i["threadId"] == answerable["threadId"]);
        // A stale revision is refused, and the request stays.
        let stale = json!({ "command": "shell.requestAction", "payload": pick(&answerable, json!({ "decision": "allow", "revision": revision + 9 })) });
        controller.command(&stale.to_string()).unwrap();
        // (the hub's refusal shows as the status line, and the request's card opens with it)
        assert!(shell_until(&controller, &mut shell, |_, s| s["status"]["text"].as_str().is_some_and(|e| e.contains("changed"))));
        assert!(!gone(&shell));
        // The current revision answers it: it leaves the list.
        let answer = json!({ "command": "shell.requestAction", "payload": pick(&answerable, json!({ "decision": "deny", "revision": revision })) });
        controller.command(&answer.to_string()).unwrap();
        assert!(shell_until(&controller, &mut shell, |_, s| gone(s)));
        // Open goes to the request's thread and marks it seen.
        if let Some(other) = shell["requests"]["items"].as_array().unwrap().first().cloned() {
            let open = json!({ "command": "shell.requestOpen", "payload": pick(&other, json!({})) });
            controller.command(&open.to_string()).unwrap();
            let seen = |s: &Value| !s["requests"]["items"].as_array().unwrap().iter().any(|i| i["requestId"] == other["requestId"] && i["threadId"] == other["threadId"]);
            assert!(shell_until(&controller, &mut shell, |_, s| seen(s)));
        }
        controller.stop();
    }

    #[test]
    fn the_side_pane_scene_puts_a_screenshot_a_diagram_and_published_files_in_the_transcript() {
        let controller = ViewController::start(Options { mode: Mode::Demo(Scenario::Fleet), theme: "dark".into(), input: "touch".into(), requests: false, platform: "android".into(), browser_host: false }).unwrap();
        let scene = yaac_core::demo::Scene::ALL.iter().position(|s| *s == yaac_core::demo::Scene::SidePanes).unwrap();
        controller.command(&json!({ "command": "shell.scene", "payload": { "id": scene } }).to_string()).unwrap();
        let users = yaac_core::demo::threads::users().address();
        controller.command(&json!({ "command": "sidebar.open", "payload": { "id": users.as_str() } }).to_string()).unwrap();
        let mut seen = String::new();
        let found = std::iter::from_fn(|| controller.wait_frame(Duration::from_secs(30))).any(|frame| {
            if frame.contains("\"transcript\"") {
                seen.push_str(&frame);
            }
            ["login.png", "sequenceDiagram", "Auth migration report"].iter().all(|w| seen.contains(w))
        });
        controller.stop();
        assert!(found, "screenshot, diagram and report in the transcript frames");
    }

    /// A diff opened from the Files tree arrives without any further command (the wakes of the
    /// file request, the diff worker and the document encoder all reach the loop).
    #[test]
    fn a_diff_opened_from_the_tree_arrives_without_another_command() {
        let controller = ViewController::start(Options { mode: Mode::Demo(Scenario::Fleet), theme: "dark".into(), input: "touch".into(), requests: false, platform: "android".into(), browser_host: false }).unwrap();
        let send = |c: &str, p: Value| controller.command(&json!({ "command": c, "payload": p }).to_string()).unwrap();
        let scene = yaac_core::demo::Scene::ALL.iter().position(|s| *s == yaac_core::demo::Scene::SidePanes).unwrap();
        send("shell.scene", json!({ "id": scene }));
        let users = yaac_core::demo::threads::users().address();
        send("sidebar.open", json!({ "id": users.as_str() }));
        send("shell.visible", json!({ "shown": true }));
        send("files.new", json!({ "thread": users.as_str() }));
        let mut browser = Value::Null;
        let mut wait = |browser: &mut Value, what: &str, done: &dyn Fn(&Value, &Value) -> bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while std::time::Instant::now() < deadline {
                let Some(frame) = controller.wait_frame(Duration::from_millis(500)) else { continue };
                let frame: Value = serde_json::from_str(&frame).unwrap();
                if let Some(b) = frame["views"].get("browser") {
                    *browser = b.clone();
                }
                if done(&frame, browser) {
                    return;
                }
            }
            panic!("timed out waiting for {what}: {browser}");
        };
        wait(&mut browser, "the Files tab", &|_, b| b["files"]["folders"].get("").is_some());
        let tab = browser["active"].clone();
        for dir in ["services", "services/users", "services/users/src"] {
            send("files.expand", json!({ "thread": users.as_str(), "tab": tab, "data": { "path": dir, "expanded": true } }));
            wait(&mut browser, dir, &|_, b| b["files"]["folders"].get(dir).is_some());
        }
        send("files.diff", json!({ "thread": users.as_str(), "tab": tab, "data": { "path": "services/users/src/handlers.rs" } }));
        wait(&mut browser, "the diff document", &|f, _| f.get("filesDocument").is_some());
        controller.stop();
    }

    #[test]
    fn a_frame_sink_splits_lines_and_reports_a_closed_channel() {
        let (mut sink, frames) = FrameSink::new();
        sink.write_all(b"{\"a\":1}\n{\"b\"").unwrap();
        sink.flush().unwrap();
        assert_eq!(frames.try_recv().unwrap(), "{\"a\":1}");
        assert!(frames.try_recv().is_err());
        sink.write_all(b":2}\n").unwrap();
        sink.flush().unwrap();
        assert_eq!(frames.try_recv().unwrap(), "{\"b\":2}");
        drop(frames);
        sink.write_all(b"{}\n").unwrap();
        assert_eq!(sink.flush().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }
}
