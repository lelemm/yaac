//! `hub/wsl/distros`: the installed WSL distros for Settings → Targets, from the hub (the
//! window never runs `wsl.exe` itself). The parsers are tested in `yaac-transport`; this is the
//! method's shape and its errors, with a fixed list.

mod support;

use std::sync::Arc;

use serde_json::json;
use support::Home;
use yaac_hub::WslLister;
use yaac_hub::protocol::{WslDistroEntry, WslDistrosResult, codes, names as ext};
use yaac_transport::BoxFuture;
use yaac_transport::wsl::{Distro, ListError};

struct Fixed(Result<Vec<Distro>, ListError>);

impl WslLister for Fixed {
    fn list(&self) -> BoxFuture<'static, Result<Vec<Distro>, ListError>> {
        let out = self.0.clone();
        Box::pin(async move { out })
    }
}

fn distro(name: &str, default: bool, running: bool) -> Distro {
    Distro { name: name.into(), default, state: if running { "Running" } else { "Stopped" }.into(), version: Some(2), running }
}

fn call(list: Result<Vec<Distro>, ListError>) -> Result<serde_json::Value, yaac_protocol::rpc::ErrorObject> {
    let home = Home::new();
    let mut config = home.config();
    config.wsl_distros = Arc::new(Fixed(list));
    let hub = home.start_with(config);
    let c = hub.client();
    let out = c.call(ext::WSL_DISTROS, json!({}));
    drop(c);
    hub.shutdown();
    out
}

#[test]
fn lists_each_distro_with_the_default_and_whether_it_runs() {
    let v = call(Ok(vec![distro("Ubuntu", true, true), distro("Debian", false, false)])).expect("listed");
    assert_eq!(
        v,
        json!({ "distros": [
            { "name": "Ubuntu", "default": true, "running": true, "version": 2 },
            { "name": "Debian", "default": false, "running": false, "version": 2 },
        ] })
    );
    let r: WslDistrosResult = serde_json::from_value(v).unwrap();
    assert_eq!(r.distros[0], WslDistroEntry { name: "Ubuntu".into(), default: true, running: true, version: Some(2) });
}

#[test]
fn no_wsl_exe_and_a_failed_listing_are_plain_errors() {
    let e = call(Err(ListError::Unavailable("cannot run wsl.exe: not found".into()))).unwrap_err();
    assert_eq!(e.code.as_str(), codes::WSL_UNAVAILABLE);
    assert_eq!(e.message, "WSL is not installed on this PC");
    let e = call(Err(ListError::Failed("wsl.exe -l -v failed: Wsl/Service/E_UNEXPECTED".into()))).unwrap_err();
    assert_eq!(e.code.as_str(), codes::WSL_FAILED);
    assert!(e.message.contains("E_UNEXPECTED"), "{e:?}");
}

#[test]
fn an_empty_list_is_an_empty_list() {
    assert_eq!(call(Ok(Vec::new())).unwrap(), json!({ "distros": [] }));
}
