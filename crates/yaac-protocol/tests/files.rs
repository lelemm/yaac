//! Shapes and traffic classes for the worktree browser.
use serde_json::json;
use yaac_protocol::attach::DaemonSummary;
use yaac_protocol::methods::{Direction, FsRead, FsTree, GitFileDiff, Method};
use yaac_protocol::schedule::Class;
use yaac_protocol::{FsTreeEntryKind, GitFileStatus};

fn roundtrip<M: Method>(params: serde_json::Value, result: serde_json::Value, class: Class) {
    assert_eq!(M::INFO.direction, Direction::HostToDaemon);
    assert_eq!(M::INFO.request_class, Class::Control);
    assert_eq!(M::INFO.response_class, class);
    assert!(!M::INFO.state_changing);
    assert_eq!((M::INFO.roundtrip_params)(params.clone()).unwrap(), params);
    assert_eq!((M::INFO.roundtrip_result)(result.clone()).unwrap(), result);
}

#[test]
fn files_methods_have_additive_wire_shapes() {
    roundtrip::<FsTree>(
        json!({"root":"/work","path":""}),
        json!({"entries":[
            {"name":"src","kind":"dir","ignored":false,"changed":true},
            {"name":"app.rs","kind":"file","size":123,"mtime":456,"git":"M","ignored":false,"changed":false},
            {"name":"link","kind":"symlink","ignored":true,"changed":false}
        ],"repo":true,"truncated":false}),
        Class::Control,
    );
    roundtrip::<FsRead>(
        json!({"root":"/work","path":"a","maxBytes":4194304,"offset":100}),
        json!({"text":"next page","size":42000,"truncated":false,"nextOffset":30100,"etag":"version"}),
        Class::Bulk,
    );
    roundtrip::<FsRead>(
        json!({"root":"/work","path":"a","maxBytes":4194304}),
        json!({"binary":true,"size":81,"truncated":false,"etag":"version"}),
        Class::Bulk,
    );
    roundtrip::<GitFileDiff>(
        json!({"root":"/work","path":"new","oldOffset":42}),
        json!({"old":"next old page","oldPath":"old","status":"R","binary":false,"nextOldOffset":88,"etag":"version"}),
        Class::Bulk,
    );
    roundtrip::<GitFileDiff>(
        json!({"root":"/work","path":"new"}),
        json!({"new":"untracked text","status":"U","binary":false,"nextNewOffset":30,"etag":"version"}),
        Class::Bulk,
    );
    assert_eq!(GitFileStatus::from_wire("!"), GitFileStatus::Conflict);
    assert_eq!(GitFileStatus::from_wire("future"), GitFileStatus::Unknown("future".into()));
    assert_eq!(FsTreeEntryKind::from_wire("future"), FsTreeEntryKind::Unknown("future".into()));
}

#[test]
fn an_older_hub_summary_defaults_to_no_file_view() {
    let legacy: DaemonSummary = serde_json::from_value(json!({"build":"old","older":true,"schemaHashMatches":true})).unwrap();
    assert!(!legacy.file_view);
    assert!(!legacy.terminal);
    assert_eq!(legacy.terminal_unavailable_reason, None);
}
