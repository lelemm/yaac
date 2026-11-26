//! The demo hub's terminals (docs/13-terminal.md): a line-echo "shell" behind the real
//! `hub/terminal/*` methods and data streams, so `--demo` has working terminals and tests can
//! follow a terminal's whole path (open, credit, input, output, exit).
use std::collections::{HashMap, VecDeque};

use serde_json::{Value, json};
use yaac_protocol::StreamId;

const PROMPT: &str = "\x1b[1;32mdev@demo\x1b[0m:\x1b[1;34m~/work\x1b[0m$ ";

/// What a few commands print in the demo (colour, a bell, a longer stream); others echo.
fn scripted(command: &str) -> Option<String> {
    Some(match command {
        "bell" => "\x07".into(),
        "git status" => concat!(
            "On branch main\r\nChanges not staged for commit:\r\n",
            "\t\x1b[31mmodified:   README.md\x1b[0m\r\n",
            "\t\x1b[31mmodified:   services/users/src/handlers.rs\x1b[0m\r\n",
            "\t\x1b[31mdeleted:    services/users/src/legacy.rs\x1b[0m\r\n",
            "Unmerged paths:\r\n\t\x1b[31mboth modified:   migrations/0007_users_tokens.sql\x1b[0m\r\n",
            "Untracked files:\r\n\t\x1b[31mnotes.txt\x1b[0m\r\n",
        ).into(),
        "cargo test" => {
            let mut out = String::from("\x1b[1;32m   Compiling\x1b[0m users v0.3.0 (/home/dev/api/services/users)\r\n\x1b[1;32m    Finished\x1b[0m `test` profile in 4.21s\r\n\r\nrunning 14 tests\r\n");
            for name in ["me_returns_the_subject", "list", "get", "create", "update", "deactivate", "reactivate", "search", "export", "delete_revokes_tokens", "tokens_newest_first", "bearer_is_required", "expired_token_is_refused", "schema_round_trips"] {
                out.push_str(&format!("test handlers::{name} ... \x1b[32mok\x1b[0m\r\n"));
            }
            out.push_str("\r\ntest result: \x1b[32mok\x1b[0m. 14 passed; 0 failed\r\n\x07");
            out
        }
        _ => return None,
    })
}

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
                let banner = format!("Demo terminal: lines are echoed back; try `git status`, `cargo test`, `bell`; `exit N` ends it.\r\n{PROMPT}");
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
                    match line.trim() {
                        "" => {}
                        command => echo.push_str(&scripted(command).unwrap_or_else(|| format!("{line}\r\n"))),
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
