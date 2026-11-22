//! D159: operation ids never repeat across window runs. `yaacd` remembers every operation id;
//! a window that restarted its count at 1 had its messages refused ("opId … was used for
//! thread/archive") or answered with an earlier request's outcome.

use yaac_core::client::Client;
use yaac_core::link::DetachedLink;
use yaac_protocol::ClientId;

fn client(run: Option<&str>) -> Client {
    let mut c = Client::new(Box::new(DetachedLink), "b", ClientId::new("host1"));
    if let Some(r) = run {
        c.set_run(r);
    }
    c
}

#[test]
fn two_window_runs_never_make_the_same_operation_id() {
    let (mut a, mut b) = (client(Some("run1")), client(Some("run2")));
    let ids_a: Vec<String> = (0..5).map(|_| a.op_id().to_string()).collect();
    let ids_b: Vec<String> = (0..5).map(|_| b.op_id().to_string()).collect();
    assert!(ids_a.iter().all(|x| !ids_b.contains(x)), "{ids_a:?} vs {ids_b:?}");
    assert_eq!(ids_a[0], "op-host1-run1-1");
    // Within one run they are unique too.
    let mut all = ids_a;
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 5);
}
