//! The demo hub's terminals (docs/13-terminal.md): a line-echo "shell" behind the real
//! `hub/terminal/*` methods and data streams, so `--demo` has working terminals and tests can
//! follow a terminal's whole path (open, credit, input, output, exit).
use std::collections::{HashMap, VecDeque};

use serde_json::{Value, json};
use yaac_protocol::StreamId;

const PROMPT: &str = "\x1b[1;32mdev@demo\x1b[0m:\x1b[1;34m~/work\x1b[0m$ ";

/// One demo shell.
#[derive(Debug)]
struct Shell {
    terminal: String,
    line: String,
}

/// Every demo terminal and what has crossed its streams.
#[derive(Debug, Default)]
pub(crate) struct Terminals {
    next: u32,
    shells: HashMap<StreamId, Shell>,
    /// Output for the window.
    pub(crate) out: VecDeque<(StreamId, Vec<u8>)>,
    /// Notifications the window sent (`stream/credit`), newest last.
    pub(crate) notes: Vec<(String, Value)>,
    /// Input bytes received, per stream.
    pub(crate) input: HashMap<StreamId, Vec<u8>>,
    /// Notifications for the window (`hub/terminal/exited`…).
    pub(crate) events: VecDeque<(String, Value)>,
}

impl Terminals {
    /// Answer a `hub/terminal/*` request.
    pub(crate) fn request(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let target = params.get("target").cloned().unwrap_or(Value::Null);
        match method.rsplit('/').next().unwrap_or_default() {
            "shells" => Ok(json!({ "shells": [
                { "path": "/bin/bash", "name": "bash" },
                { "path": "/usr/bin/zsh", "name": "zsh" },
                { "path": "/usr/bin/fish", "name": "fish" },
            ]})),
            "open" | "attach" => {
                self.next += 1;
                let stream = StreamId(self.next);
                let terminal = params.get("terminal").and_then(Value::as_str).map_or_else(|| format!("demo-{}", self.next), str::to_owned);
                self.shells.insert(stream, Shell { terminal: terminal.clone(), line: String::new() });
                let banner = format!("Demo terminal: lines are echoed back; `exit N` ends it.\r\n{PROMPT}");
                self.out.push_back((stream, banner.into_bytes()));
                Ok(json!({ "terminal": terminal, "stream": stream, "initialCredit": 65536, "target": target }))
            }
            "resize" | "close" => Ok(json!({})),
            "list" => Ok(json!({ "terminals": [] })),
            _ => Err("unknown terminal method".into()),
        }
    }

    /// Input from the window: echo it like a line discipline would.
    pub(crate) fn data(&mut self, stream: StreamId, bytes: &[u8], target: &str) {
        self.input.entry(stream).or_default().extend_from_slice(bytes);
        let Some(sh) = self.shells.get_mut(&stream) else { return };
        let mut echo = String::new();
        for ch in String::from_utf8_lossy(bytes).chars() {
            match ch {
                '\r' => {
                    let line = std::mem::take(&mut sh.line);
                    echo.push_str("\r\n");
                    if let Some(code) = line.trim().strip_prefix("exit") {
                        let code: i64 = code.trim().parse().unwrap_or(0);
                        self.out.push_back((stream, echo.clone().into_bytes()));
                        self.events.push_back(("hub/terminal/exited".into(), json!({ "target": target, "terminal": sh.terminal, "code": code })));
                        self.shells.remove(&stream);
                        return;
                    }
                    if !line.trim().is_empty() {
                        echo.push_str(&format!("{line}\r\n"));
                    }
                    echo.push_str(PROMPT);
                }
                '\u{7f}' | '\u{8}' => {
                    if sh.line.pop().is_some() {
                        echo.push_str("\u{8} \u{8}");
                    }
                }
                c if !c.is_control() => {
                    sh.line.push(c);
                    echo.push(c);
                }
                _ => {}
            }
        }
        if !echo.is_empty() {
            self.out.push_back((stream, echo.into_bytes()));
        }
    }
}
