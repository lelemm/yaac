//! Records the fixture escape streams in `fixtures/` from the real programs, on a PTY of a fixed
//! size, answering their queries (cursor position, device attributes, colours) with yaac-term
//! itself. Linux only (bash, vim, less, pwsh, htop must be installed or passed in `HTOP`).
//!
//! ```text
//! cargo run -p yaac-term --example record -- <cols> <rows> <out dir> [names…]
//! ```
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use yaac_term::{Frame, Terminal, Theme};

enum Step {
    /// Wait this long, answering queries.
    Wait(u64),
    /// Wait until the screen shows this text (at most 10 s).
    Until(&'static str),
    /// Type this.
    Send(&'static str),
}
use Step::{Send, Until, Wait};

struct Scenario {
    name: &'static str,
    argv: Vec<String>,
    env: Vec<(&'static str, String)>,
    steps: Vec<Step>,
}

const APP_RS: &str = r#"use crate::session::{Session, Store};
use std::time::{Duration, SystemTime};

/// How long a session lives without activity.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct App {
    store: Store,
    name: String,
}

impl App {
    pub fn new(name: &str) -> Self {
        Self { store: Store::default(), name: name.to_owned() }
    }

    /// Seconds until `session` expires, or zero.
    pub fn expires_in(&self, session: &Session) -> u64 {
        let idle = SystemTime::now()
            .duration_since(session.last_seen)
            .unwrap_or_default();
        IDLE_TIMEOUT.saturating_sub(idle).as_secs()
    }

    pub fn sign_in(&mut self, user: &str) -> Result<Session> {
        if user.is_empty() {
            return Err("empty user name".into());
        }
        Ok(self.store.open(user))
    }
}
"#;

fn setup(root: &Path) -> PathBuf {
    let home = root.join("home");
    let project = home.join("work/auth");
    let _ = std::fs::remove_dir_all(root);
    for d in ["src", "tests", "docs", ".config/htop"] {
        std::fs::create_dir_all(if d.starts_with('.') { home.join(d) } else { project.join(d) }).unwrap();
    }
    let files: [(&str, &str); 8] = [
        ("Cargo.toml", "[package]\nname = \"auth\"\nversion = \"0.3.1\"\nedition = \"2024\"\n\n[dependencies]\nserde = \"1\"\n"),
        ("README.md", "# auth\n\nSessions and sign-in for the billing service.\n\nRun `cargo test` before pushing; see https://example.com/auth/docs for the design.\n"),
        ("src/app.rs", APP_RS),
        ("src/session.rs", "pub struct Session;\n"),
        ("src/lib.rs", "pub mod app;\npub mod session;\n"),
        ("src/main.rs", "fn main() {\n    println!(\"auth\");\n}\n"),
        ("tests/sign_in.rs", "#[test]\nfn signs_in() {}\n"),
        ("docs/design.md", "# Design\n"),
    ];
    for (p, c) in files {
        std::fs::write(project.join(p), c).unwrap();
    }
    let long: String = (1..=120)
        .map(|i| format!("Line {i:>3}: session {} {}\n", ["opened", "refreshed", "expired", "closed"][i % 4], if i % 7 == 0 { "auth=denied" } else { "ok" }))
        .collect();
    std::fs::write(project.join("docs/sessions.log"), long).unwrap();
    std::fs::write(
        home.join(".config/htop/htoprc"),
        "htop_version=3.3.0\nconfig_reader_min_version=3\nfields=0 48 39 40 2 46 47 49 1\nhide_userland_threads=1\nhide_kernel_threads=1\nshow_program_path=0\nhighlight_base_name=1\ncolor_scheme=0\ndelay=15\nheader_layout=two_50_50\ncolumn_meters_0=CPU Memory Swap\ncolumn_meter_modes_0=1 1 1\ncolumn_meters_1=Tasks LoadAverage Uptime\ncolumn_meter_modes_1=2 2 2\ntree_view=0\nsort_key=46\nsort_direction=-1\nscreen:Main=PID USER M_RESIDENT STATE PERCENT_CPU PERCENT_MEM TIME Command\n.sort_key=PERCENT_CPU\n.sort_direction=-1\n",
    )
    .unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&project)
            .env("HOME", &home)
            .env("GIT_AUTHOR_DATE", "2026-11-20T10:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-11-20T10:00:00Z")
            .env("GIT_AUTHOR_NAME", "Dev")
            .env("GIT_AUTHOR_EMAIL", "dev@example.com")
            .env("GIT_COMMITTER_NAME", "Dev")
            .env("GIT_COMMITTER_EMAIL", "dev@example.com")
            .output()
            .unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Sessions expire after five minutes"]);
    git(&["config", "color.ui", "always"]);
    std::fs::write(project.join("src/app.rs"), APP_RS.replace("300", "600")).unwrap();
    std::fs::write(project.join("src/auth.rs"), "pub fn check() {}\n").unwrap();
    project
}

fn scenarios(project: &Path, htop: &str, pids: &str) -> Vec<Scenario> {
    let home = project.parent().unwrap().parent().unwrap().to_string_lossy().into_owned();
    let ps1 = r"\[\e[1;32m\]dev@devbox\[\e[0m\]:\[\e[1;34m\]\w\[\e[0m\]\$ ".to_owned();
    let bash = || vec!["bash".to_owned(), "--noprofile".into(), "--norc".into(), "-i".into()];
    let env = |extra: &[(&'static str, String)]| {
        let mut v = vec![
            ("HOME", home.clone()),
            ("TERM", "xterm-256color".into()),
            ("COLORTERM", "truecolor".into()),
            ("TERM_PROGRAM", "YAAC".into()),
            ("LANG", "C.UTF-8".into()),
            ("PS1", ps1.clone()),
            ("HISTFILE", "/dev/null".into()),
            ("PATH", std::env::var("PATH").unwrap_or_default()),
            ("GIT_PAGER", "cat".into()),
        ];
        v.extend_from_slice(extra);
        v
    };
    let shell_session = vec![
        Until("$ "),
        Send("ls -F --color=always\r"),
        Until("tests/"),
        Send("git status -sb\r"),
        Until("auth.rs"),
        Send("git log --oneline\r"),
        Until("five minutes"),
        Send("wc -l src/*.rs\r"),
        Until("total"),
        Send("grep -c denied docs/sessions.log\r"),
        Until("17"),
        Send("cargo build --rel"),
        Wait(300),
    ];
    let mut exited = vec![
        Until("$ "),
        Send("ls -F --color=always\r"),
        Until("tests/"),
        Send("grep -n denied docs/sessions.log | head -3\r"),
        Until("Line  21"),
        Send("exit 3\r"),
        Wait(500),
    ];
    exited.shrink_to_fit();
    let search = vec![
        Until("$ "),
        Send("tail -n 24 docs/sessions.log\r"),
        Until("Line 120"),
        Wait(200),
    ];
    vec![
        Scenario { name: "bash-prompt", argv: bash(), env: env(&[]), steps: shell_session },
        Scenario { name: "search", argv: bash(), env: env(&[]), steps: search },
        Scenario { name: "exited", argv: bash(), env: env(&[]), steps: exited },
        Scenario {
            name: "vim",
            argv: ["vim", "-u", "NONE", "-i", "NONE", "-N", "-c", "syntax on", "-c", "set number ruler laststatus=2 background=dark", "-c", "call cursor(20, 9)", "src/app.rs"]
                .map(String::from)
                .to_vec(),
            env: env(&[]),
            steps: vec![Until("app.rs"), Wait(1200)],
        },
        Scenario {
            name: "vim-light",
            argv: ["vim", "-u", "NONE", "-i", "NONE", "-N", "-c", "syntax on", "-c", "set number ruler laststatus=2 background=light", "-c", "call cursor(20, 9)", "src/app.rs"]
                .map(String::from)
                .to_vec(),
            env: env(&[]),
            steps: vec![Until("app.rs"), Wait(1200)],
        },
        Scenario {
            name: "vim-session",
            argv: ["vim", "-u", "NONE", "-i", "NONE", "-N", "-c", "syntax on", "src/app.rs"].map(String::from).to_vec(),
            env: env(&[]),
            steps: vec![Until("app.rs"), Wait(600), Send("jjjwwiNEW \x1b"), Wait(300), Send(":q!\r"), Wait(600)],
        },
        Scenario {
            name: "less",
            argv: ["less", "-R", "docs/sessions.log"].map(String::from).to_vec(),
            env: env(&[]),
            steps: vec![Until("Line"), Send(" "), Wait(300), Send("/denied\r"), Wait(300), Send("q"), Wait(300)],
        },
        Scenario {
            name: "htop",
            argv: vec![htop.to_owned(), format!("--pid={pids}"), "--no-mouse".into(), "--readonly".into()],
            env: env(&[("LD_LIBRARY_PATH", std::env::var("HTOP_LIBS").unwrap_or_default())]),
            steps: vec![Until("PID"), Wait(1800)],
        },
        Scenario {
            name: "powershell-prompt",
            argv: [
                "pwsh",
                "-NoLogo",
                "-NoProfile",
                "-NoExit",
                "-Command",
                "Set-PSReadLineOption -HistorySaveStyle SaveNothing -PredictionSource None; function prompt { \"PS ~/work/auth> \" }",
            ]
            .map(String::from)
            .to_vec(),
            env: env(&[]),
            steps: vec![
                Until("PS ~/work/auth> "),
                Send("Get-ChildItem -Name src\r"),
                Until("session.rs"),
                Wait(300),
                Send("$PSVersionTable.PSVersion.Major\r"),
                Wait(800),
                Send("Get-Content README.md | Select-String 'cargo'"),
                Wait(800),
            ],
        },
    ]
}

fn screen(term: &Terminal, frame: &mut Frame) -> String {
    term.snapshot(frame);
    frame.lines.iter().map(yaac_term::Row::text).collect::<Vec<_>>().join("\n")
}

fn record(s: &Scenario, cwd: &Path, cols: u16, rows: u16) -> Vec<u8> {
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new(&s.argv[0]);
    cmd.args(&s.argv[1..]);
    cmd.cwd(cwd);
    cmd.env_clear();
    for (k, v) in &s.env {
        cmd.env(k, v);
    }
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let term = Terminal::new(cols.into(), rows.into(), || {});
    term.set_theme(Theme::DARK, (8, 17));
    let mut frame = Frame::default();
    let mut out = Vec::new();
    let pump = |until: Instant, out: &mut Vec<u8>, writer: &mut Box<dyn Write + std::marker::Send>| {
        while let Ok(chunk) = rx.recv_timeout(until.saturating_duration_since(Instant::now()).max(Duration::from_millis(1))) {
            out.extend_from_slice(&chunk);
            term.feed(&chunk);
            while !term.idle() {
                std::thread::sleep(Duration::from_millis(1));
            }
            let replies = term.take_events().input;
            if !replies.is_empty() {
                let _ = writer.write_all(&replies);
            }
            if Instant::now() >= until {
                break;
            }
        }
    };
    for step in &s.steps {
        match step {
            Wait(ms) => pump(Instant::now() + Duration::from_millis(*ms), &mut out, &mut writer),
            Until(text) => {
                let end = Instant::now() + Duration::from_secs(10);
                while !screen(&term, &mut frame).contains(text) {
                    assert!(Instant::now() < end, "{}: timed out waiting for {text:?}:\n{}", s.name, screen(&term, &mut frame));
                    pump(Instant::now() + Duration::from_millis(50), &mut out, &mut writer);
                }
                pump(Instant::now() + Duration::from_millis(150), &mut out, &mut writer);
            }
            Send(text) => {
                for b in text.bytes() {
                    writer.write_all(&[b]).unwrap();
                    pump(Instant::now() + Duration::from_millis(15), &mut out, &mut writer);
                }
            }
        }
    }
    let _ = child.kill();
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cols), Some(rows), Some(dir)) = (args.first(), args.get(1), args.get(2)) else {
        eprintln!("record <cols> <rows> <out dir> [names…]");
        return;
    };
    let (cols, rows): (u16, u16) = (cols.parse().unwrap(), rows.parse().unwrap());
    let project = setup(Path::new("/tmp/yaac-fixture"));
    let htop = std::env::var("HTOP").unwrap_or_else(|_| "htop".into());
    // Processes for htop to list, named like a developer's (argv[0] only; they sleep, one spins).
    let sleepers: Vec<_> = ["cargo watch -x test", "rust-analyzer", "node web/server.js", "postgres -D data", "cargo test --workspace", "bash"]
        .iter()
        .filter_map(|name| {
            // `cat` on a pipe nobody writes waits forever; `yes` keeps one CPU busy. No arguments
            // besides the name, so htop's Command column shows just the name.
            let busy = name.starts_with("cargo test");
            std::process::Command::new(if busy { "yes" } else { "cat" })
                .arg0(name)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .spawn()
                .ok()
        })
        .collect();
    let pids = sleepers.iter().map(|c| c.id().to_string()).collect::<Vec<_>>().join(",");
    std::fs::create_dir_all(dir).unwrap();
    for s in scenarios(&project, &htop, &pids) {
        if args.len() > 3 && !args[3..].iter().any(|n| n == s.name) {
            continue;
        }
        let mut bytes = record(&s, &project, cols, rows);
        // The recording machine's user name is not part of the fixture.
        if let Ok(user) = std::env::var("USER") {
            let shown: String = user.chars().take(10).collect();
            let fake = format!("{:<width$}", "dev", width = shown.chars().count());
            bytes = String::from_utf8_lossy(&bytes).replace(&shown, &fake).into_bytes();
        }
        let path = Path::new(dir).join(format!("{}.vt", s.name));
        std::fs::write(&path, &bytes).unwrap();
        println!("{} {} bytes", path.display(), bytes.len());
    }
    for mut s in sleepers {
        let _ = s.kill();
    }
}
