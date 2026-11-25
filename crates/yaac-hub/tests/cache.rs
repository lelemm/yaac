//! Items and position are never out of step: a failed commit changes nothing, and a writer
//! killed at an arbitrary moment leaves a cache whose items are exactly the events up to its
//! position.

use std::time::Duration;

use yaac_hub::cache::{Cache, Fault};
use yaac_protocol::{
    AgentMessageItem, DeltaField, Epoch, Item, ItemBody, ItemDeltaEvent, ItemEvent, ItemId, ItemStatus, JournalPosition,
    Seq, TargetId, ThreadEvent, ThreadEventParams, ThreadId, TurnId,
};

fn started(seq: u64) -> ThreadEventParams {
    let item = Item {
        id: ItemId::new("m"),
        turn_id: TurnId::new("u"),
        status: ItemStatus::InProgress,
        started_at: None,
        completed_at: None,
        body: ItemBody::AgentMessage(AgentMessageItem { text: String::new() }),
    };
    ThreadEventParams { thread_id: ThreadId::new("t"), epoch: Epoch(1), seq: Seq(seq), event: ThreadEvent::ItemStarted(ItemEvent { item }) }
}

/// Delta `n` appends `<n>`: the text at position `p` is `<2><3>…<p>`.
fn delta(seq: u64) -> ThreadEventParams {
    ThreadEventParams {
        thread_id: ThreadId::new("t"),
        epoch: Epoch(1),
        seq: Seq(seq),
        event: ThreadEvent::ItemDelta(ItemDeltaEvent { item_id: ItemId::new("m"), field: DeltaField::Text, delta: format!("<{seq}>") }),
    }
}

fn expected(position: u64) -> String {
    (2..=position).map(|n| format!("<{n}>")).collect()
}

fn text_and_position(cache: &mut Cache) -> Option<(String, u64)> {
    let t = cache.thread(&TargetId::new("x"), &ThreadId::new("t")).unwrap()?;
    let text = match &t.items.first()?.body {
        ItemBody::AgentMessage(m) => m.text.clone(),
        _ => panic!(),
    };
    Some((text, t.position.seq.0))
}

#[test]
fn a_failed_commit_changes_neither_items_nor_position() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let (x, t) = (TargetId::new("x"), ThreadId::new("t"));
    let mut cache = Cache::open(&path).unwrap();
    cache.commit_events(&x, &t, &[started(1), delta(2), delta(3)], delta(3).position_after()).unwrap();
    cache.inject_fault(Some(Fault::BeforePosition));
    assert!(cache.commit_events(&x, &t, &[delta(4), delta(5)], delta(5).position_after()).is_err());
    cache.inject_fault(None);
    drop(cache);
    let mut cache = Cache::open(&path).unwrap();
    assert_eq!(text_and_position(&mut cache), Some((expected(3), 3)));
    // A repeated batch is harmless; a later one continues exactly.
    cache.commit_events(&x, &t, &[delta(2), delta(3), delta(4)], delta(4).position_after()).unwrap();
    assert_eq!(text_and_position(&mut cache), Some((expected(4), 4)));
    let snapshot = cache.thread(&x, &t).unwrap().unwrap();
    assert_eq!(snapshot.position, JournalPosition { epoch: Epoch(1), seq: Seq(4) });
}

/// Run by `a_writer_killed_at_any_moment_leaves_items_and_position_in_step` in a child
/// process, which it kills. Does nothing when run normally.
#[test]
#[ignore = "child process of the crash test"]
fn crash_writer_child() {
    let Some(path) = std::env::var_os("YAAC_CRASH_DB") else { return };
    let (x, t) = (TargetId::new("x"), ThreadId::new("t"));
    let mut cache = Cache::open(std::path::Path::new(&path)).unwrap();
    cache.commit_events(&x, &t, &[started(1)], started(1).position_after()).unwrap();
    let mut seq = 2;
    loop {
        let batch: Vec<_> = (seq..seq + 3).map(delta).collect();
        let position = batch.last().unwrap().position_after();
        cache.commit_events(&x, &t, &batch, position).unwrap();
        seq += 3;
        if seq == 8 {
            // Two batches are in: tell the parent, which may kill us from now on.
            use std::io::Write;
            println!("committed");
            std::io::stdout().flush().ok();
        }
    }
}

#[test]
fn a_writer_killed_at_any_moment_leaves_items_and_position_in_step() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    for round in 0..3 {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_writer_child", "--ignored", "--nocapture", "--test-threads=1"])
            .env("YAAC_CRASH_DB", &path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // Wait until the writer says it has committed (a fixed sleep failed on a loaded CI
        // runner, where the child had not started writing yet), then let it run a little longer,
        // differently each round, so the kill lands at an arbitrary moment.
        {
            use std::io::BufRead;
            let out = child.stdout.take().unwrap();
            let mut lines = std::io::BufReader::new(out).lines();
            while let Some(Ok(line)) = lines.next() {
                if line.contains("committed") {
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20 + round * 35));
        child.kill().unwrap(); // SIGKILL, mid-transaction or between
        child.wait().unwrap();
        let mut cache = Cache::open(&path).unwrap();
        let (text, position) = text_and_position(&mut cache).expect("something was committed");
        assert!(position > 1, "the writer made progress");
        assert_eq!(text, expected(position), "items are exactly the events up to the position");
        drop(cache);
        std::fs::remove_file(&path).ok();
        for s in ["-wal", "-shm"] {
            std::fs::remove_file(format!("{}{s}", path.display())).ok();
        }
    }
}
