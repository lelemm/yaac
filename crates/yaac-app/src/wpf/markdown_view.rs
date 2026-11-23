//! The shared CommonMark parser's output for the native transcript.
use crate::markdown::{Block, Parsed, Span};
use serde_json::{Value, json};
use std::collections::HashMap;
use yaac_core::ThreadKey;
use yaac_protocol::ItemId;

#[derive(Default)]
pub(super) struct MarkdownView {
    thread: Option<ThreadKey>,
    entries: HashMap<(ItemId, String), Entry>,
    pub(super) diffs: crate::thread::DiffCache,
}

#[derive(Default)]
struct Entry {
    text: String,
    complete: bool,
    parsed: Parsed,
    blocks: Value,
}

impl MarkdownView {
    pub(super) fn select(&mut self, thread: Option<&ThreadKey>) {
        // A tool item can become several file rows within this projection.
        self.diffs = crate::thread::DiffCache::default();
        if self.thread.as_ref() != thread {
            self.entries.clear();
            self.thread = thread.cloned();
        }
    }

    pub(super) fn blocks(&mut self, item: &ItemId, field: &str, text: &str, typed: bool, complete: bool) -> Value {
        if text.is_empty() {
            return json!([]);
        }
        let entry = self.entries.entry((item.clone(), field.to_owned())).or_default();
        if entry.text != text || entry.complete != complete {
            if typed {
                entry.blocks = Value::Array(crate::markdown::parse_typed(text).iter().map(block).collect());
            } else {
                if !text.starts_with(&entry.text) {
                    entry.parsed = Parsed::default();
                }
                entry.parsed.update(text, complete);
                entry.blocks = Value::Array(entry.parsed.blocks().map(block).collect());
            }
            entry.text = text.to_owned();
            entry.complete = complete;
        }
        entry.blocks.clone()
    }
}

fn spans(spans: &[Span]) -> Vec<Value> {
    spans.iter().map(|span| json!({
        "text": span.text, "bold": span.bold, "italic": span.italic,
        "code": span.code, "link": span.link,
    })).collect()
}

fn block(block: &Block) -> Value {
    match block {
        Block::Para(s) => json!({ "kind": "paragraph", "spans": spans(s) }),
        Block::Heading(level, s) => json!({ "kind": "heading", "level": level, "spans": spans(s) }),
        Block::Code { lang, text } => json!({ "kind": "code", "language": lang, "text": text }),
        Block::Item { depth, marker, spans: s } => json!({ "kind": "item", "depth": depth, "marker": marker, "spans": spans(s) }),
        Block::Quote(s) => json!({ "kind": "quote", "spans": spans(s) }),
        Block::Rule => json!({ "kind": "rule" }),
        Block::Table { head, rows } => {
            let cells = |row: &[Vec<Span>]| row.iter().map(|s| spans(s)).collect::<Vec<_>>();
            json!({ "kind": "table", "head": cells(head), "rows": rows.iter().map(|row| cells(row)).collect::<Vec<_>>() })
        }
    }
}
