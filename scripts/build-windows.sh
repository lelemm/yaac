#!/bin/sh
# Builds the Windows yaac.exe on Linux (or WSL) into dist/windows/.
#
#   scripts/build-windows.sh            release build, all default features
#   scripts/build-windows.sh --demo     window-only build (no hub), for UI work
#
# Needs, all user-space or already on a typical Ubuntu:
#   - rustup with the x86_64-pc-windows-gnu target (added here if missing)
#   - the MinGW-w64 cross toolchain: x86_64-w64-mingw32-gcc and -windres
#     (Ubuntu: the gcc-mingw-w64-x86-64 and binutils-mingw-w64-x86-64 packages)
#   - Node.js 22 and npm, to bundle the yaacd daemon that the hub uploads to WSL targets
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
target=x86_64-pc-windows-gnu
out="$root/dist/windows"
features=""
case "${1:-}" in
  --demo) features="--no-default-features --features glow" ;;
  "") ;;
  *) echo "usage: $0 [--demo]" >&2; exit 2 ;;
esac

if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
  PATH="$HOME/.cargo/bin:$PATH"
fi
command -v cargo >/dev/null || { echo "cargo not found (install Rust with rustup)" >&2; exit 1; }
command -v x86_64-w64-mingw32-gcc >/dev/null || {
  echo "x86_64-w64-mingw32-gcc not found: install the MinGW-w64 cross compiler" >&2
  exit 1
}
if ! rustup target list --installed | grep -qx "$target"; then
  rustup target add "$target"
fi

# One build id for everything this build makes (scripts/build-id.sh): the exe (window and
# hub) and the daemon bundle it embeds. A caller that already allocated one passes it on.
YAAC_BUILD_ID=${YAAC_BUILD_ID:-$("$root/scripts/build-id.sh")}
export YAAC_BUILD_ID

# The hub embeds the daemon bundle it installs on WSL targets.
if [ -z "$features" ]; then
  (cd "$root/daemon" && { [ -d node_modules ] || npm ci; } && npm run --silent build)
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
# shellcheck disable=SC2086 # $features is a list of words
cargo build --release --locked --target "$target" -p yaac-app $features

mkdir -p "$out"
cp "$CARGO_TARGET_DIR/$target/release/yaac.exe" "$out/yaac.exe"
printf '%s\n' "$YAAC_BUILD_ID" >"$out/build-id"
size=$(wc -c <"$out/yaac.exe")
echo "built $out/yaac.exe ($size bytes), build $YAAC_BUILD_ID"
sha256sum "$out/yaac.exe"
