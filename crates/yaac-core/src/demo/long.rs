//! A long conversation for the timeline (D153), the hover times (D154) and the "Worked for"
//! fold (D156): three weeks of prompts on *Try the new parser*, each turn with reasoning, tool
//! calls and a final answer, one turn that failed and one interrupted by an update.

use yaac_protocol::{InputPart, Item, ItemBody, ItemEvent, ItemId, ItemStatus, NoticeKind, ThreadEvent, ToolKind, TurnCompletedEvent, TurnId, TurnOrigin, TurnStartedEvent, TurnStatus, UserMessageItem};

use super::build::{agent, notice, reasoning, tool};
use crate::time::timestamp;

/// The prompts, oldest first: (what the user asked, what the agent answered).
const TURNS: &[(&str, &str)] = &[
    ("Run the new parser over the sample files and tell me what breaks.", "Two of the 40 samples fail: `nested.toml` (a table inside an inline array) and `crlf.toml` (Windows line endings)."),
    ("Fix the CRLF case first.", "The lexer now treats `\\r\\n` as one newline. `crlf.toml` parses."),
    ("Now the nested inline tables.", "Inline tables can nest now. The recursion is bounded at 128 levels, like the spec suggests."),
    ("Add a test for each of those.", "Added `lexer::crlf_is_one_newline` and `parser::inline_tables_nest`. Both pass."),
    ("How fast is it compared with the old one?", "On the 40 samples: 3.1 ms against 5.4 ms, about 43% faster. Most of the gain is the lexer no longer allocating per token."),
    ("Can we stream instead of reading the whole file?", "Yes: the lexer works on a `BufRead` now and the parser pulls tokens on demand. Memory stays flat on the 200 MB sample."),
    ("Show me the public API as it stands.", "`parse(reader) -> Result<Document>`, `Document::get(path)`, `Document::tables()`, and `Error` with a line and column."),
    ("Rename Document to Doc everywhere.", "Renamed in 14 files; the docs and examples follow."),
    ("Actually, revert that rename.", "Reverted. `Document` is back."),
    ("What does the error look like for a missing quote?", "`error: unterminated string at 12:18` followed by the line with a caret under the opening quote."),
    ("Make errors show the line above too.", "Errors now print one line of context above and below, dimmed."),
    ("Run clippy on the whole crate.", "Clippy is clean after two fixes: a needless borrow and a manual `strip_prefix`."),
    ("Check the dates in the samples parse as dates.", "All 9 date samples parse as `Datetime`; offset dates keep their offset."),
    ("Does it handle multiline basic strings?", "Yes, including the line-ending backslash that trims leading whitespace on the next line."),
    ("Fuzz it for a few minutes.", "Ten minutes of fuzzing found one panic on a lone `[` at end of input. Fixed and added as a regression test."),
    ("Write a short README section for the parser.", "Added *Parsing* to the README: one example, the streaming note, and how errors look."),
    ("Port the old config loader to the new parser.", "The loader uses the new parser now; its 22 tests pass unchanged."),
    ("Profile the loader start-up.", "Start-up went from 41 ms to 18 ms. The rest is reading the file."),
    ("Try it on the big monorepo config.", "Parses in 9 ms. One warning: a duplicate key `[tool.lint]`, which the old parser silently kept the last of."),
    ("Make duplicate keys an error, like the spec says.", "Duplicate keys are an error now, with both locations in the message."),
    ("Is there anything left from the old parser?", "Only `legacy::escape`, used by the writer. I left it for now."),
    ("Move escape into the new crate and delete legacy.", "Moved and deleted. The workspace builds without `legacy`."),
    ("Run the whole test suite.", "All 312 tests pass."),
    ("Bump the version and draft the changelog.", "Version 0.4.0. The changelog lists streaming, nested inline tables, duplicate-key errors and the 43% speed-up."),
    ("Open a pull request.", "Opened #128 *New streaming parser*."),
    ("CI says the Windows job fails, can you look?", "The Windows job checked out the samples with `autocrlf`, so every sample had CRLF and one test compared bytes. The test compares lines now."),
    ("Re-run CI.", "CI is green on all three platforms."),
    ("Address the review comment about the recursion limit.", "The limit is a constant now, `MAX_DEPTH = 128`, documented, with a test at 129."),
    ("Summarise what changed for the release notes.", "A faster, streaming TOML parser: nested inline tables, clear errors with context, and duplicate keys reported instead of dropped."),
    ("Rebase on main and resolve conflicts.", "Rebased; one conflict in `Cargo.lock`, regenerated."),
    ("Check the benchmark again after the rebase.", "Same numbers within noise: 3.0 ms against 5.4 ms."),
    ("Merge it when CI is green.", "Merged #128."),
];

/// Turn 12 failed, turn 20 was interrupted by an update.
const FAILED: usize = 11;
const INTERRUPTED: usize = 19;

fn at(id: &str, turn: &TurnId, status: ItemStatus, body: ItemBody, start: i64, end: i64) -> ThreadEvent {
    let item = Item { id: ItemId::new(id), turn_id: turn.clone(), status, body, started_at: Some(timestamp(start)), completed_at: Some(timestamp(end)) };
    ThreadEvent::ItemCompleted(ItemEvent { item })
}

/// The events, oldest first. `base` is the story's "now"; the newest prompt is a few minutes
/// before it, the oldest three weeks before.
#[allow(clippy::cast_possible_wrap, reason = "a few dozen turns")]
pub(super) fn events(base: i64) -> Vec<ThreadEvent> {
    let n = TURNS.len() as i64;
    let mut out = Vec::new();
    for (i, (ask, answer)) in TURNS.iter().enumerate() {
        let k = i as i64;
        // Spread over 21 days, the last few today, a few minutes apart.
        let start = if k >= n - 4 { base - (n - k) * 7 * 60 } else { base - 21 * 86_400 + k * 21 * 86_400 / n };
        let turn = TurnId::new(format!("p-long-{i}"));
        let worked = 8 + (k * 37) % 230;
        out.push(ThreadEvent::TurnStarted(TurnStartedEvent { turn_id: turn.clone(), origin: TurnOrigin::User, at: Some(timestamp(start)) }));
        let user = ItemBody::UserMessage(UserMessageItem { parts: vec![InputPart::text(*ask)] });
        out.push(at(&format!("pl{i}-u"), &turn, ItemStatus::Completed, user, start, start));
        out.push(at(&format!("pl{i}-r"), &turn, ItemStatus::Completed, reasoning("Look at the lexer and the samples first."), start + 1, start + 4));
        out.push(at(&format!("pl{i}-t1"), &turn, ItemStatus::Completed, tool(ToolKind::Command, "Ran `cargo test -p toml-next`", Some(0), "test result: ok. 312 passed"), start + 5, start + worked / 2));
        if i % 3 == 0 {
            out.push(at(&format!("pl{i}-m"), &turn, ItemStatus::Completed, agent("Tests pass; checking the samples next."), start + worked / 2, start + worked / 2));
            out.push(at(&format!("pl{i}-t2"), &turn, ItemStatus::Completed, tool(ToolKind::Command, "Ran `./scripts/samples.sh`", Some(0), "40 of 40 parsed"), start + worked / 2, start + worked - 2));
        }
        let status = match i {
            FAILED => {
                out.push(at(&format!("pl{i}-e"), &turn, ItemStatus::Completed, notice(NoticeKind::Error, "The model is overloaded. Try again in a moment.", None), start + worked, start + worked));
                TurnStatus::Failed
            }
            INTERRUPTED => {
                out.push(at(&format!("pl{i}-x"), &turn, ItemStatus::Completed, notice(NoticeKind::InterruptedByUpdate, "This turn was interrupted because yaacd was updated.", None), start + worked, start + worked));
                TurnStatus::InterruptedByUpdate
            }
            _ => {
                out.push(at(&format!("pl{i}-a"), &turn, ItemStatus::Completed, agent(answer), start + worked, start + worked));
                TurnStatus::Completed
            }
        };
        out.push(ThreadEvent::TurnCompleted(TurnCompletedEvent { turn_id: turn, status, usage: None, error: None, at: Some(timestamp(start + worked)) }));
    }
    out
}
