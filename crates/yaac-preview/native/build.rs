fn main() {
    println!("cargo:rerun-if-env-changed=CEF_DIR");
    let directory = std::env::var("CEF_DIR").expect("set CEF_DIR to the staged runtime directory");
    println!("cargo:rustc-link-search=native={directory}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The archive must follow the binding rlibs for the MinGW linker.
        println!("cargo:rustc-link-arg={directory}/libcef.lib");
    } else {
        println!("cargo:rustc-link-lib=cef");
    }
}
