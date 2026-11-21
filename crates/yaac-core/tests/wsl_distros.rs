//! Settings → Targets, WSL: the distro dropdown is filled from the hub (`hub/wsl/distros`),
//! preselects Windows' default distro unless it already has a target, keeps the user's choice
//! across a refresh, and falls back to typing when listing fails.

mod common;

use common::{boot_with, settle};
use serde_json::json;
use yaac_core::demo::Scenario;
use yaac_core::settings::{DistroOption, DistroPicker, distro_picker};

fn picker(c: &yaac_core::client::Client) -> DistroPicker {
    distro_picker(&c.state)
}

fn selected(p: &DistroPicker) -> Option<String> {
    match p {
        DistroPicker::Choose { selected, .. } => selected.clone(),
        other => panic!("not a dropdown: {other:?}"),
    }
}

#[test]
fn the_default_distro_is_preselected() {
    let (mut c, hub) = boot_with(Scenario::Fresh);
    assert_eq!(picker(&c), DistroPicker::Loading, "not asked yet: the dropdown says Loading…");
    c.load_wsl_distros();
    assert_eq!(picker(&c), DistroPicker::Loading);
    settle(&mut c, &hub);
    let p = picker(&c);
    assert_eq!(selected(&p).as_deref(), Some("Ubuntu"));
    let DistroPicker::Choose { options, .. } = p else { unreachable!() };
    assert_eq!(
        options,
        vec![
            DistroOption { name: "Ubuntu".into(), label: "Ubuntu · default".into(), state: "running", added: false },
            DistroOption { name: "Debian".into(), label: "Debian".into(), state: "stopped", added: false },
            DistroOption { name: "Ubuntu-24.04".into(), label: "Ubuntu-24.04".into(), state: "stopped", added: false },
        ]
    );
}

#[test]
fn nothing_is_selected_when_the_default_distro_already_has_a_target() {
    // The standard fleet has `wsl: Ubuntu`.
    let (mut c, hub) = boot_with(Scenario::Fleet);
    c.load_wsl_distros();
    settle(&mut c, &hub);
    let p = picker(&c);
    assert_eq!(selected(&p), None, "the user picks another");
    let DistroPicker::Choose { options, .. } = p else { unreachable!() };
    let ubuntu = options.iter().find(|o| o.name == "Ubuntu").unwrap();
    assert!(ubuntu.added, "still listed (another user is allowed), marked already added");
    assert!(!options.iter().find(|o| o.name == "Debian").unwrap().added);
}

#[test]
fn a_refresh_keeps_the_users_choice_while_it_is_installed() {
    let (mut c, hub) = boot_with(Scenario::Fleet);
    c.load_wsl_distros();
    settle(&mut c, &hub);
    c.choose_wsl_distro(Some("Debian".into()));
    c.load_wsl_distros();
    assert_eq!(picker(&c), DistroPicker::Loading);
    settle(&mut c, &hub);
    assert_eq!(selected(&picker(&c)).as_deref(), Some("Debian"));
    // Debian was removed meanwhile: back to the rule (the default has a target: nothing).
    hub.set_wsl_distros(Ok(json!({ "distros": [
        { "name": "Ubuntu", "default": true, "running": true },
        { "name": "Ubuntu-24.04", "default": false, "running": false },
    ] })));
    c.load_wsl_distros();
    settle(&mut c, &hub);
    assert_eq!(selected(&picker(&c)), None);
}

#[test]
fn a_failed_listing_falls_back_to_typing_the_distro() {
    let (mut c, hub) = boot_with(Scenario::Fresh);
    hub.set_wsl_distros(Err("wsl.exe -l -v failed: Wsl/Service/E_UNEXPECTED".into()));
    c.load_wsl_distros();
    settle(&mut c, &hub);
    assert_eq!(picker(&c), DistroPicker::Typed { message: "wsl.exe -l -v failed: Wsl/Service/E_UNEXPECTED".into() });
    // Asking again after it is fixed gives the dropdown back.
    hub.set_wsl_distros(Ok(json!({ "distros": [{ "name": "Ubuntu", "default": true, "running": false }] })));
    c.load_wsl_distros();
    settle(&mut c, &hub);
    assert_eq!(selected(&picker(&c)).as_deref(), Some("Ubuntu"));
    // No distros at all: typed, saying so.
    hub.set_wsl_distros(Ok(json!({ "distros": [] })));
    c.load_wsl_distros();
    settle(&mut c, &hub);
    assert_eq!(picker(&c), DistroPicker::Typed { message: "No distros were found".into() });
}
