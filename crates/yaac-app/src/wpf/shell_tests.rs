//! `!command` in the WPF composer and transcript (P344, P346): what the controller sends.
//!
//! `YAAC_WPF_SHELL_FRAMES=<dir> cargo test -p yaac-app wpf::shell_tests::frames -- --ignored`
//! writes the view frames the screenshots of this feature are taken from.

use super::*;
use yaac_core::demo::threads;
use yaac_protocol::{
    AgentMessageItem, Capability, InputPart, Item, ItemBody, ItemStatus, QueuedMessage, TextPart, TextRole, ToolCallItem,
    ToolCallSource, ToolKind, TurnId, UserMessageItem,
};

/// Ids and turns under `sh-`, apart from the demo thread's own.
fn item(id: &str, turn: &str, status: ItemStatus, body: ItemBody) -> Item {
    Item { id: ItemId::new(format!("sh-{id}")), turn_id: TurnId::new(format!("sh-{turn}")), status, started_at: None, completed_at: None, body }
}

fn bang(id: &str, turn: &str, command: &str) -> Item {
    item(id, turn, ItemStatus::Completed, ItemBody::UserMessage(UserMessageItem {
        parts: vec![InputPart::Text(TextPart { text: format!("!{command}"), role: Some(TextRole::Command), label: None })],
        op_id: None,
    }))
}

fn run(id: &str, turn: &str, status: ItemStatus, command: &str, output: &str, exit: Option<i32>, ms: Option<u64>) -> Item {
    item(id, turn, status, ItemBody::ToolCall(ToolCallItem {
        tool: ToolKind::Command, title: command.into(), input: None, output: output.into(), output_truncated: false,
        output_bytes: None, exit_code: exit, cwd: Some("/home/dev/api".into()), agent_call: None, skill: None,
        images: Vec::new(), source: Some(ToolCallSource::UserShell), duration_ms: ms,
    }))
}

fn answer(id: &str, turn: &str, text: &str) -> Item {
    item(id, turn, ItemStatus::Completed, ItemBody::AgentMessage(AgentMessageItem { text: text.into() }))
}

fn test_output() -> String {
    let mut out = String::from("   Compiling yaac-core v0.1.0 (/home/dev/yaac/crates/yaac-core)\n    Finished `test` profile in 4.21s\n     Running unittests src/lib.rs\n\nrunning 28 tests\n");
    for name in ["compose::a_leading_bang_is_a_shell_command", "compose::the_field_hides_the_bang", "header::shell_mode_needs_the_capability",
        "queue::a_queued_command_keeps_its_text", "shell_block::a_live_command_pairs_its_message_with_its_call",
        "shell_block::a_finished_command_shows_its_exit_and_duration", "shell_block::codex_history_gives_a_lone_call_its_own_block",
        "shell_block::claude_history_reads_the_context_parts", "transcript::echo_replaces_sending", "transcript::queued_runs_after_the_turn",
        "activity::absorbed_items_are_not_drawn", "activity::a_lone_call_is_a_user_block", "timeline::bang_titles",
        "client::send_routes_a_bang_draft", "client::send_now_routes_a_bang_draft", "fold::twelve_lines_and_a_button",
        "fold::two_more_lines_are_shown", "fold::open_shows_all", "history::newest_first"] {
        out.push_str(&format!("test {name} ... ok\n"));
    }
    out.push_str("\ntest result: ok. 28 passed; 0 failed; 0 ignored; finished in 0.04s\n");
    out
}

/// A Codex demo thread that can run `!` commands, open, with `items` after its history.
fn open_with(app: &mut YaacApp, key: &ThreadKey, items: Vec<Item>, active: Option<&str>) {
    app.apply(vec![Action::Open(key.clone())]);
    app.client.pump();
    let t = app.client.state.transcripts.get_mut(key).expect("demo transcript");
    if let Some(e) = t.effective.as_mut()
        && !e.capabilities.contains(&Capability::UserShellCommand)
    {
        e.capabilities.push(Capability::UserShellCommand);
    }
    for (n, item) in items.into_iter().enumerate() {
        t.upsert(item, 10_000 + n as u64);
    }
    t.active_turn = active.map(|turn| TurnId::new(format!("sh-{turn}")));
}

fn set_draft(app: &mut YaacApp, key: &ThreadKey, text: &str) {
    app.client.state.drafts.entry(key.clone()).or_default().text = text.to_owned();
}

fn composer_view(app: &YaacApp) -> Value {
    let map = AttentionMap::compute(&app.client.state);
    composer::view(app, &map, None)
}

fn transcript_rows(app: &YaacApp) -> Vec<Value> {
    let map = AttentionMap::compute(&app.client.state);
    let mut markdown = markdown_view::MarkdownView::default();
    transcript::view(app, &map, &mut markdown)["rows"].as_array().cloned().unwrap_or_default()
}

#[test]
fn a_bang_draft_on_a_capable_thread_is_a_run_composer() {
    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    let key = threads::ci();
    open_with(&mut app, &key, vec![bang("u1", "s1", "cargo test"), run("c1", "s1", ItemStatus::Completed, "cargo test", "ok", Some(0), Some(900))], None);

    set_draft(&mut app, &key, "!cargo build");
    let v = composer_view(&app);
    assert_eq!(v["text"], "!cargo build", "the draft keeps its `!`; WPF hides it in the field");
    assert_eq!(v["shellCapable"], true);
    assert!(v["shell"]["prompt"].as_str().is_some_and(|p| !p.is_empty()));
    assert_eq!(v["shell"]["history"], json!(["cargo test"]));
    assert_eq!(v["shell"]["run"]["label"], "Run");
    assert_eq!(v["shell"]["run"]["enabled"], true);
    assert!(v["shell"]["hint"].as_str().is_some_and(|h| h.starts_with("Runs here, then ")));
    // What the composer goes back to when the user leaves the mode (WPF hides attach and
    // Send now while in it, and swaps these back in at once).
    assert_eq!(v["send"]["label"], "Send");
    assert!(!v["attach"].is_null());
    assert!(v["hint"].is_null());
    assert_eq!(v["markdown"], json!([]), "a command is not Markdown");

    // A bare `!` is the mode with nothing to run yet.
    set_draft(&mut app, &key, "!");
    assert_eq!(composer_view(&app)["shell"]["run"]["enabled"], false);

    // Leaving the mode (WPF sends the field without the `!`): the normal composer.
    let draft = DraftRef::Thread(key.clone()).store_key();
    composer::command(&mut app, "composer.edit", &json!({ "key": draft, "text": "cargo build", "caret": 0 })).unwrap();
    let v = composer_view(&app);
    assert!(v["shell"].is_null());
    assert_eq!(v["send"]["label"], "Send");
    assert!(!v["attach"].is_null());
}

#[test]
fn while_a_turn_runs_the_button_queues_the_run_and_queued_commands_are_marked() {
    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    let key = threads::ci();
    open_with(&mut app, &key, vec![], Some("busy"));
    let t = app.client.state.transcripts.get_mut(&key).unwrap();
    t.queued.push(QueuedMessage {
        queue_id: "q1".into(), input: vec![InputPart::text("!git status")], queued_at: yaac_core::time::timestamp(1_800_000_000),
        model: None, effort: None, mode: None, fast: None, ultrafast: None, shell_command: Some("git status".into()),
    });
    set_draft(&mut app, &key, "!npm test");
    let v = composer_view(&app);
    assert_eq!(v["shell"]["run"]["label"], "Queue run");
    assert_eq!(v["send"]["label"], "Queue");
    let row = &v["queue"][0];
    assert_eq!(row["label"], "git status");
    assert_eq!(row["shell"], true);
    assert_eq!(v["shell"]["history"][0], "git status", "newest first");
}

#[test]
fn a_leading_bang_is_plain_text_where_commands_cannot_run() {
    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    let key = threads::ci();
    app.apply(vec![Action::Open(key.clone())]);
    app.client.pump();
    let t = app.client.state.transcripts.get_mut(&key).unwrap();
    t.active_turn = None;
    if let Some(e) = t.effective.as_mut() {
        e.capabilities.retain(|c| c != &Capability::UserShellCommand);
    }
    set_draft(&mut app, &key, "!important");
    let v = composer_view(&app);
    assert_eq!(v["shellCapable"], false);
    assert!(v["shell"].is_null());
    assert_eq!(v["send"]["label"], "Send");
}

#[test]
fn a_command_is_one_block_with_its_output_folded() {
    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    let key = threads::ci();
    let output = test_output();
    let lines = output.lines().count();
    open_with(&mut app, &key, vec![
        bang("u1", "s1", "cargo test -p yaac-core"),
        run("c1", "s1", ItemStatus::Completed, "cargo test -p yaac-core", &output, Some(0), Some(4800)),
        answer("a1", "s1", "All 28 tests pass."),
        bang("u2", "s2", "false"),
        run("c2", "s2", ItemStatus::Completed, "false", "", Some(1), Some(30)),
    ], None);
    let rows = transcript_rows(&app);
    let shells: Vec<_> = rows.iter().filter(|r| r["kind"] == "shell").collect();
    assert_eq!(shells.len(), 2, "each `!cmd` and its call are one block: {shells:#?}");
    assert!(!rows.iter().any(|r| r["actionId"] == "sh-c1" || r["actionId"] == "sh-c2"), "the call is not drawn again");
    let first = shells[0];
    assert_eq!(first["command"], "cargo test -p yaac-core");
    assert_eq!(first["ok"], true);
    assert_eq!(first["badge"], "exit 0 · 4.8s");
    assert_eq!(first["showAll"], format!("Show all {lines} lines"));
    assert_eq!(first["shown"].as_str().unwrap().lines().count(), yaac_core::shell_block::FOLD_LINES);
    assert!(first["stop"].is_null());
    assert_eq!(shells[1]["ok"], false);
    assert_eq!(shells[1]["badge"], "exit 1 · 0.0s");
    assert!(shells[1]["showAll"].is_null());
    let answer_at = rows.iter().position(|r| r["actionId"] == "sh-a1").unwrap();
    let block_at = rows.iter().position(|r| r["actionId"] == "sh-u1").unwrap();
    assert!(block_at < answer_at, "then the agent's answer");
}

#[test]
fn a_running_command_offers_stop_and_stop_interrupts_the_turn() {
    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    let key = threads::ci();
    open_with(&mut app, &key, vec![
        bang("u1", "s1", "npm test"),
        run("c1", "s1", ItemStatus::InProgress, "npm test", "> yaac@0.1.0 test\n> vitest run\n", None, None),
    ], Some("s1"));
    let rows = transcript_rows(&app);
    let block = rows.iter().find(|r| r["kind"] == "shell").unwrap();
    assert_eq!(block["running"], true);
    assert!(block["ok"].is_null() && block["badge"].is_null());
    assert_eq!(block["stop"]["id"], "stopShell");
    transcript::command(&mut app, "transcript.action", &json!({ "id": "sh-u1", "action": "stopShell", "threadId": key.address() })).unwrap();
}

/// Lone user-run calls (Codex history) render as the same block.
#[test]
fn a_lone_history_call_is_a_command_block() {
    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    let key = threads::ci();
    open_with(&mut app, &key, vec![run("c9", "h1", ItemStatus::Completed, "ls", "a\nb", Some(0), None)], None);
    let rows = transcript_rows(&app);
    let block = rows.iter().find(|r| r["actionId"] == "sh-c9").unwrap();
    assert_eq!(block["kind"], "shell");
    assert_eq!(block["command"], "ls");
}

fn frame(app: &YaacApp) -> String {
    let mut markdown = markdown_view::MarkdownView::default();
    let mut preview = markdown_view::MarkdownView::default();
    let views = view(app, None, None, 0, &mut markdown, &mut preview, None);
    serde_json::to_string(&json!({ "views": views })).unwrap()
}

#[test]
#[ignore = "writes screenshot frames; set YAAC_WPF_SHELL_FRAMES"]
fn frames() {
    let Some(dir) = std::env::var_os("YAAC_WPF_SHELL_FRAMES") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = egui::Context::default();
    for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
        let name = theme.id();
        let key = threads::ci();
        let finished = vec![
            bang("u1", "s1", "cargo test -p yaac-core"),
            run("c1", "s1", ItemStatus::Completed, "cargo test -p yaac-core", &test_output(), Some(0), Some(4800)),
            answer("a1", "s1", "All 28 tests in `yaac-core` pass, including the new `shell_block` ones. Nothing to fix."),
            bang("u2", "s2", "git diff --stat"),
            run("c2", "s2", ItemStatus::Completed, "git diff --stat", "fatal: not a git repository (or any of the parent directories): .git", Some(128), Some(40)),
        ];
        let write = |file: &str, app: &YaacApp| std::fs::write(dir.join(format!("{name}-{file}.jsonl")), frame(app) + "\n").unwrap();

        // Idle: a command typed, the finished blocks above.
        let mut app = crate::app::tests::demo_app(&ctx);
        app.ui.theme = theme;
        open_with(&mut app, &key, finished.clone(), None);
        set_draft(&mut app, &key, "!cargo clippy -p yaac-core");
        write("idle", &app);
        // The same draft out of the mode: the normal composer, keeping the text.
        set_draft(&mut app, &key, "cargo clippy -p yaac-core");
        write("reverted", &app);

        // A finished block on its own: the header, the folded output, then the answer.
        let mut app = crate::app::tests::demo_app(&ctx);
        app.ui.theme = theme;
        open_with(&mut app, &key, finished[..3].to_vec(), None);
        write("finished", &app);

        // A turn runs: the command block streams, and a new one would queue.
        let mut app = crate::app::tests::demo_app(&ctx);
        app.ui.theme = theme;
        let mut items = finished;
        items.push(bang("u3", "s3", "npm test"));
        items.push(run("c3", "s3", ItemStatus::InProgress, "npm test", "> yaac@0.1.0 test\n> vitest run\n\n RUN  v3.2.4 /home/dev/yaac\n\n ✓ src/compose.test.ts (12 tests) 31ms\n ✓ src/queue.test.ts (8 tests) 22ms\n", None, None));
        open_with(&mut app, &key, items, Some("s3"));
        let t = app.client.state.transcripts.get_mut(&key).unwrap();
        t.queued.clear();
        t.queued.push(QueuedMessage {
            queue_id: "sh-q1".into(), input: vec![InputPart::text("!cargo fmt --check")],
            queued_at: yaac_core::time::timestamp(1_800_000_000), model: None, effort: None, mode: None, fast: None,
            ultrafast: None, shell_command: Some("cargo fmt --check".into()),
        });
        set_draft(&mut app, &key, "!git status");
        write("running", &app);
    }
}

