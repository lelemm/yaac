use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::Duration;

use super::*;

fn styled(path: &str, text: &str) -> Highlighted {
    highlight::highlight(path, text.to_owned(), &AtomicBool::new(false)).unwrap()
}

fn token(doc: &Highlighted, line: usize, text: &str, class: ScopeClass) {
    let spans: Vec<_> = doc.lines[line]
        .spans
        .iter()
        .map(|span| (&doc.line_text(line)[span.range.clone()], span.class))
        .collect();
    assert!(
        spans
            .iter()
            .any(|(part, style)| *style == class && part.contains(text)),
        "{text:?} {class:?}; found {spans:?}"
    );
}

#[test]
fn detects_names_compound_suffixes_and_first_lines() {
    for (path, first_line, language) in [
        (r"src\Main.RS", "#!/bin/bash", "Rust"),
        ("other/Dockerfile", "FROM scratch", "Dockerfile"),
        ("component.tsx", "const jsx = <div />;", "TypeScriptReact"),
        ("runner", "#!/usr/bin/env bash", "Bourne Again Shell (bash)"),
        ("runner", "#!/usr/bin/python3", "Python"),
        ("runner", "#!/usr/bin/env -S python3 -u", "Python"),
        ("runner", "# -*- mode: python; coding: utf-8 -*-", "Python"),
        ("runner", "// vim: set ft=rust:", "Rust"),
    ] {
        assert_eq!(
            styled(path, first_line).language.unwrap().name,
            language,
            "{path}: {first_line}"
        );
    }
    let unknown = styled("code.unknown", "fn unknown() {}");
    assert!(unknown.language.is_none());
    assert!(unknown.lines[0].spans.is_empty());
}

#[test]
fn styles_rust_tokens_with_utf8_ranges() {
    use ScopeClass::*;
    let doc = styled(
        "main.rs",
        "fn sample() -> u32 { let café = 42; café + 1 } // 🦀\r\n",
    );
    assert_eq!(doc.language.as_ref().unwrap().fence, "rust");
    assert_eq!(doc.lines.len(), 1);
    assert!(doc.line_text(0).ends_with("🦀"));
    for (text, class) in [
        ("fn", Keyword),
        ("sample", Function),
        ("42", Number),
        ("u32", Type),
        ("+", Operator),
        ("🦀", Comment),
    ] {
        token(&doc, 0, text, class);
    }
}

#[test]
fn styles_typescript_python_json_markdown_and_shell() {
    use ScopeClass::*;
    let doc = styled(
        "app.ts",
        "export function greet(x: string) { return 'hello'; }\n",
    );
    for (text, class) in [("export", Keyword), ("greet", Function), ("hello", String)] {
        token(&doc, 0, text, class);
    }
    let doc = styled("app.py", "def call(): return 123 # check\n");
    for (text, class) in [
        ("def", Keyword),
        ("call", Function),
        ("123", Number),
        ("check", Comment),
    ] {
        token(&doc, 0, text, class);
    }
    let doc = styled(
        "record.json",
        "{\"name\": \"value\", \"bool\": true, \"id\": 42}\n",
    );
    for (text, class) in [
        ("name", Attribute),
        ("value", String),
        ("true", Constant),
        ("42", Number),
    ] {
        token(&doc, 0, text, class);
    }
    let doc = styled("README.md", "# Heading\n**bold** and `code`\n");
    token(&doc, 0, "Heading", Heading);
    token(&doc, 1, "bold", Emphasis);
    token(&doc, 1, "code", String);
    let doc = styled(
        "script",
        "#!/usr/bin/env bash\nif true; then echo \"hi\"; fi # done\n",
    );
    token(&doc, 1, "then", Keyword);
    token(&doc, 1, "hi", String);
    token(&doc, 1, "done", Comment);
}

#[test]
fn preserves_multiline_scopes_and_skips_long_lines() {
    let doc = styled(
        "app.py",
        &format!(
            "x = \"\"\"first\nsecond\n{}\"\"\"\npass\n",
            "x".repeat(MAX_LINE_BYTES)
        ),
    );
    token(&doc, 1, "second", ScopeClass::String);
    assert!(doc.line_text(2).len() > MAX_LINE_BYTES);
    assert!(doc.lines[2].spans.is_empty());
    token(&doc, 3, "pass", ScopeClass::Keyword);
}

fn diff_input(old: Option<&str>, new: Option<&str>) -> DiffInput {
    DiffInput {
        path: "src/main.rs".into(),
        old_path: None,
        old: old.map(str::to_owned),
        new: new.map(str::to_owned),
        notice: None,
    }
}

fn diff(old: Option<&str>, new: Option<&str>) -> Box<Diff> {
    let DiffResult::Text(diff) =
        diff::build(diff_input(old, new), &AtomicBool::new(false)).unwrap()
    else {
        panic!("expected text diff")
    };
    diff
}

fn row(old: Option<usize>, new: Option<usize>) -> DiffRow {
    DiffRow::Line { old, new }
}

#[test]
fn pairs_replacement_lines_and_marks_word_ranges_on_both_sides() {
    let diff = diff(
        Some("fn test() {\n    let item = \"olá\";\n}\n"),
        Some("fn test() {\n    let item = \"tchau\";\n    drop(item);\n}\n"),
    );
    assert_eq!(
        diff.unified.rows,
        [
            row(Some(0), Some(0)),
            row(Some(1), None),
            row(None, Some(1)),
            row(None, Some(2)),
            row(Some(2), Some(3))
        ]
    );
    assert_eq!(
        diff.side_by_side.rows,
        [
            row(Some(0), Some(0)),
            row(Some(1), Some(1)),
            row(None, Some(2)),
            row(Some(2), Some(3))
        ]
    );
    assert_eq!(diff.unified.change_indices, [1]);
    assert_eq!(diff.side_by_side.change_indices, [1]);
    assert_eq!(diff.old_inline[&1].len(), 1);
    assert_eq!(diff.new_inline[&1].len(), 1);
    assert_eq!(
        &diff.old.line_text(1)[diff.old_inline[&1][0].clone()],
        "olá"
    );
    assert_eq!(
        &diff.new.line_text(1)[diff.new_inline[&1][0].clone()],
        "tchau"
    );
    token(&diff.old, 1, "olá", ScopeClass::String);
    token(&diff.new, 1, "tchau", ScopeClass::String);
}

#[test]
fn supports_untracked_deleted_identical_and_empty_files() {
    let text = "one\ntwo\n";
    let added = diff(None, Some(text));
    let removed = diff(Some(text), None);
    let same = diff(Some(text), Some(text));
    let empty = diff(None, None);
    for view in [&added.unified, &added.side_by_side] {
        assert_eq!(view.rows, [row(None, Some(0)), row(None, Some(1))]);
        assert_eq!(view.change_indices, [0]);
    }
    for view in [&removed.unified, &removed.side_by_side] {
        assert_eq!(view.rows, [row(Some(0), None), row(Some(1), None)]);
        assert_eq!(view.change_indices, [0]);
    }
    for view in [&same.unified, &same.side_by_side] {
        assert_eq!(view.rows, [row(Some(0), Some(0)), row(Some(1), Some(1))]);
        assert!(view.change_indices.is_empty());
    }
    assert!(empty.unified.rows.is_empty());
}

#[test]
fn folds_long_runs_and_indexes_each_change_in_both_layouts() {
    let unchanged: String = (1..=9).map(|i| format!("{i}\n")).collect();
    let old = format!("old\n{unchanged}old\n");
    let new = format!("new\n{unchanged}new\n");
    let result = diff(Some(&old), Some(&new));
    let fold = DiffRow::Fold {
        old_start: 1,
        new_start: 1,
        count: 9,
    };
    assert_eq!(result.unified.rows[2], fold);
    assert_eq!(result.side_by_side.rows[1], fold);
    assert_eq!(result.unified.change_indices, [0, 3]);
    assert_eq!(result.side_by_side.change_indices, [0, 2]);
    assert_eq!(result.old.line_text(9), "9");
    assert_eq!(result.new.line_text(9), "9");

    let exactly_eight = "equal\n".repeat(8);
    assert_eq!(
        diff(Some(&exactly_eight), Some(&exactly_eight))
            .unified
            .rows
            .len(),
        8
    );
}

#[test]
fn detects_newline_only_changes_and_uses_previous_name_for_renames() {
    let mut input = diff_input(Some("# same\r\n"), Some("# same"));
    input.old_path = Some("old.py".into());
    let DiffResult::Text(diff) = diff::build(input, &AtomicBool::new(false)).unwrap() else {
        panic!("expected text diff")
    };
    assert_eq!(diff.unified.rows, [row(Some(0), None), row(None, Some(0))]);
    assert_eq!(diff.side_by_side.rows, [row(Some(0), Some(0))]);
    assert!(diff.old_inline[&0].is_empty());
    assert!(diff.new_inline[&0].is_empty());
    token(&diff.old, 0, "same", ScopeClass::Comment);
    assert_eq!(diff.old.line_text(0), diff.new.line_text(0));
}

#[test]
fn handles_binary_and_oversized_notices() {
    let mut notice = diff_input(None, None);
    notice.notice = Some(Notice::Binary { size: 123 });
    let size = MAX_FILE_BYTES + 1;
    let large = diff_input(Some(&"x".repeat(size)), None);
    for (input, expected) in [
        (notice, Notice::Binary { size: 123 }),
        (diff_input(Some("a\0b"), None), Notice::Binary { size: 3 }),
        (large, Notice::TooLarge { size }),
    ] {
        let result = diff::build(input, &AtomicBool::new(false)).unwrap();
        assert!(matches!(result, DiffResult::Notice(notice) if notice == expected));
    }
}

#[test]
fn file_excerpt_uses_relative_path_language_and_a_safe_fence() {
    let excerpt = file_excerpt("src/app.rs", Some("rust"), 120..=148, "let a = \"````\":\n");
    assert_eq!(excerpt.label, "app.rs:120–148");
    assert_eq!(
        excerpt.text,
        "src/app.rs:120–148\n`````rust\nlet a = \"````\":\n`````\n"
    );
    assert_eq!(
        file_excerpt("src/file", None, 2..=2, "text").text,
        "src/file:2\n```\ntext\n```\n"
    );
}

#[test]
fn diff_excerpt_includes_removed_added_and_context_markers() {
    use DiffLineKind::*;
    let excerpt = diff_excerpt(
        "src/app.rs",
        120..=148,
        [
            (Context, "  fn foo() {"),
            (Removed, "    old();"),
            (Added, "    new();"),
        ],
    );
    assert_eq!(excerpt.label, "app.rs diff 120–148");
    assert_eq!(
        excerpt.text,
        "src/app.rs:120–148\n```diff\n   fn foo() {\n-    old();\n+    new();\n```\n"
    );
}

#[test]
fn worker_publishes_before_waking() {
    let (wake, ready) = mpsc::channel();
    let job = spawn_highlight("src/main.rs".into(), "fn main() {}".into(), move || {
        wake.send(()).unwrap();
    })
    .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    token(&job.try_recv().unwrap(), 0, "main", ScopeClass::Function);
}

#[test]
fn closing_a_job_cancels_work_without_waiting() {
    let (start, started) = mpsc::channel();
    let (closed, check) = mpsc::channel();
    let (seen, cancelled) = mpsc::channel();
    let job = spawn(
        || panic!("a cancelled job woke the front end"),
        move |flag| {
            start.send(()).unwrap();
            check.recv_timeout(Duration::from_secs(5)).unwrap();
            seen.send(is_cancelled(flag)).unwrap();
            Some(())
        },
    )
    .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(job);
    closed.send(()).unwrap();
    assert!(cancelled.recv_timeout(Duration::from_secs(5)).unwrap());
}
