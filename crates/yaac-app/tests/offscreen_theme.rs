#![cfg(feature = "wgpu")]

#[test]
#[ignore = "Requires an offscreen graphics adapter"]
fn captures_the_demo_ui_with_the_supplied_palette() {
    let dir = tempfile::tempdir().unwrap();
    let palette = dir.path().join("theme.json");
    let output = dir.path().join("capture.png");
    std::fs::write(
        &palette,
        r##"{"name":"Capture proof","base":"dark","colors":{"bg":"#173246","panel":"#1A354C"}}"##,
    )
    .unwrap();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_yaac"))
        .args(["--demo", "--theme-file"])
        .arg(&palette)
        .arg("--capture-ui")
        .arg(&output)
        .args(["--renderer", "software"])
        .status()
        .unwrap();
    assert!(status.success());
    let screenshot = image::open(output).unwrap().to_rgb8();
    assert_eq!(screenshot.dimensions(), (1280, 800));
    let themed = screenshot
        .pixels()
        .filter(|p| p.0 == [0x17, 0x32, 0x46])
        .count();
    assert!(themed > 10_000, "the window uses the supplied canvas color");
}
