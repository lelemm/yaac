// The four Unix native packages travel over the existing stage-0 connection. Rust-only
// development builds can still run the transport tests before the daemon has been built.
use std::{env, fs, path::PathBuf};

fn main() {
    let dist = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../daemon/dist/pty");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for target in ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64"] {
        let file = format!("{target}.tgz");
        let source = dist.join(&file);
        println!("cargo:rerun-if-changed={}", source.display());
        let bytes = match fs::read(source) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("cannot stage {file}: {e}"),
        };
        fs::write(out.join(file), bytes).expect("stage node-pty package");
    }
}
