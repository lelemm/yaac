//! Live controller integration test invoked by tests/socks_probe.py. The same
//! executable acts as CEF's subprocess, like the normal YAAC binary.
use serde_json::{Value, json};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use yaac_preview::{
    BrowserHost, Tab, Update, subprocess_entry,
    wire::{Action, Event},
};

struct Client {
    tab: Tab,
    wakes: mpsc::Receiver<()>,
}
impl Client {
    fn open(host: &mut BrowserHost, port: u16) -> Self {
        let (tx, wakes) = mpsc::channel();
        Self {
            tab: host.open(
                &std::env::var("PROBE_ORIGIN").unwrap(),
                Some(port),
                move || {
                    let _ = tx.send(());
                },
            ),
            wakes,
        }
    }
    fn wait(&self, mut found: impl FnMut(Update) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            while let Some(update) = self.tab.try_update() {
                if let Update::Event(Event::Error(msg)) = &update {
                    panic!("{msg}");
                }
                if found(update) {
                    return;
                }
            }
            self.wakes
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("timed out waiting for CEF");
        }
    }
    fn page_painted(&self) {
        self.wait(|event| match event {
            Update::Frame(frame) => frame
                .bytes()
                .as_chunks::<4>()
                .0
                .contains(&[51, 34, 17, 255]),
            _ => false,
        });
    }
    fn eval(&mut self, source: &str) -> Value {
        let id = self.tab.devtools_command(
            "Runtime.evaluate",
            json!({"expression":source,"returnByValue":true,"awaitPromise":true}),
        );
        let mut result = Value::Null;
        self.wait(|event| {
            if let Update::Event(Event::DevTools(v)) = event
                && v["id"] == id
            {
                assert!(
                    v["error"].is_null() && v["result"]["exceptionDetails"].is_null(),
                    "{v}"
                );
                result = v["result"]["result"]["value"].clone();
                return true;
            }
            false
        });
        result
    }
}
fn main() {
    if let Some(code) = subprocess_entry() {
        std::process::exit(code);
    }
    let port = std::env::var("PROBE_PORT").unwrap().parse().unwrap();
    let mut host = BrowserHost::default();
    let mut first = Client::open(&mut host, port);
    first.page_painted();
    first.eval("localStorage.setItem('test','first')");
    let mut second = Client::open(&mut host, port);
    second.page_painted();
    assert_eq!(second.eval("localStorage.getItem('test')"), Value::Null);
    drop(second);
    first.eval("document.body.appendChild(document.createElement('input')).focus()");
    first.tab.send(Action::Focus(true));
    first.tab.send(Action::Text("á€".into()));
    assert_eq!(first.eval("new Promise(resolve => setTimeout(()=>resolve(document.querySelector('input').value),150))"),"á€");
    first.tab.send(Action::Compose("preedit".into()));
    first.tab.send(Action::Commit("字".into()));
    assert_eq!(first.eval("new Promise(resolve => setTimeout(()=>resolve(document.querySelector('input').value),150))"), "á€字");
    first.eval("console.log('preview probe')");
    assert!(
        first
            .tab
            .diagnostics_since(0)
            .iter()
            .any(|(_, v)| v.to_string().contains("preview probe"))
    );
    drop(first);
    let mut next = Client::open(&mut host, port);
    next.page_painted();
    assert_eq!(next.eval("localStorage.getItem('test')"), Value::Null);
    println!("shared paint, per-tab storage, text and IME, console and fresh host passed");
}
