#!/bin/sh
# Run on Linux/WSL. Build and stage a sandboxed CEF adapter for development or packaging.
set -eu
platform=${1:?usage: build-preview-runtime.sh linux64|windows64 <cef directory>}
dest=${2:?specify the CEF runtime output directory}
root=$(cd "$(dirname "$0")/.." && pwd)
sh "$root/scripts/stage-cef-runtime.sh" "$platform" "$dest"
CEF_DIR=$(cd "$dest" && pwd)
export CEF_DIR
case "$platform" in
  linux64) target=x86_64-unknown-linux-gnu; adapter=libyaac_preview_native.so ;;
  windows64) target=x86_64-pc-windows-gnu; adapter=yaac_preview_native.dll ;;
esac
if ! command -v cargo >/dev/null 2>&1; then PATH="$HOME/.cargo/bin:$PATH"; fi
export CARGO_TARGET_DIR="$root/.build/cef/adapter"
cargo build --release --locked --manifest-path "$root/crates/yaac-preview/native/Cargo.toml" --target "$target"
cp "$CARGO_TARGET_DIR/$target/release/$adapter" "$dest/$adapter.part"
mv "$dest/$adapter.part" "$dest/$adapter"
if [ "$platform" = windows64 ]; then
  # CEF supplies sandbox information through RunWinMain in the same-named DLL.
  cp "$dest/bootstrap.exe" "$dest/yaac_preview_native.exe"
fi
