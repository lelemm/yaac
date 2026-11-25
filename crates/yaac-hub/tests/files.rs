//! Worktree browser availability through the hub's routed methods.
mod support;

use serde_json::json;
use support::{FakeDaemon, Home};
use yaac_protocol::Capability;
use yaac_protocol::attach::ConnectionStatus;
use yaac_protocol::methods::names;
use yaac_protocol::rpc::ErrorCode;

#[test]
fn files_follow_the_advertised_capability_and_disable_when_a_method_is_missing() {
    let home = Home::new();
    let new = FakeDaemon::new("files");
    new.set_features(vec![Capability::FileView]);
    home.fakes.add(&new);
    home.fakes.add(&FakeDaemon::new("legacy"));
    let hub = home.start();
    let client = hub.client();
    let files = client.add_target("files").id;
    let legacy = client.add_target("legacy").id;
    for (target, expected) in [(&files, true), (&legacy, false)] {
        client.wait_status(target, ConnectionStatus::Connected);
        let entry = client.targets().into_iter().find(|t| t.id == *target).unwrap();
        assert_eq!(entry.state.daemon.unwrap().file_view, expected);
    }

    let params = json!({ "root": "/repo", "path": "" });
    let error = client.routed(&legacy, names::FS_TREE, params.clone()).unwrap_err();
    assert_eq!(error.code, ErrorCode::CapabilityMissing);
    assert_eq!(client.routed(&files, names::FS_TREE, params.clone()).unwrap(), json!({ "entries": [], "repo": true, "truncated": false }));
    // The fake advertises the feature and implements fs/tree but has no fs/read.
    let error = client.routed(&files, names::FS_READ, json!({ "root": "/repo", "path": "app", "maxBytes": 100 })).unwrap_err();
    assert_eq!(error.code, ErrorCode::CapabilityMissing);
    let entry = client.targets().into_iter().find(|t| t.id == files).unwrap();
    assert!(!entry.state.daemon.unwrap().file_view);
    assert_eq!(client.routed(&files, names::FS_TREE, params).unwrap_err().code, ErrorCode::CapabilityMissing);
    hub.shutdown();
}
