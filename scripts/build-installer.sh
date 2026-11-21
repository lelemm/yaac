#!/bin/sh
# Builds the Windows installer, dist/windows/YAAC-Setup-<version>.exe, from WSL.
#
#   scripts/build-installer.sh              release yaac.exe (scripts/build-windows.sh), then setup
#   scripts/build-installer.sh --no-build   package the dist/windows/yaac.exe already built
#
# Needs what scripts/build-windows.sh needs, plus Inno Setup 6 on the Windows side, reached
# through WSL interop. ISCC.exe is looked for in $ISCC, then in the per-user install
# (%LOCALAPPDATA%\Programs\Inno Setup 6, what `winget install JRSoftware.InnoSetup --scope user`
# makes), then in Program Files. Every path and the version are passed to installer/yaac.iss
# as /D defines, so the script holds no path of its own. See docs/install.md.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
target=x86_64-pc-windows-gnu
out="$root/dist/windows"
build=1
case "${1:-}" in
  --no-build) build=0 ;;
  "") ;;
  *) echo "usage: $0 [--no-build]" >&2; exit 2 ;;
esac

command -v wslpath >/dev/null || { echo "run this from WSL: Inno Setup is a Windows program" >&2; exit 1; }

# The version: [workspace.package] version in Cargo.toml.
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\([^"]*\)".*/\1/p' "$root/Cargo.toml")
[ -n "$version" ] || { echo "no [workspace.package] version in Cargo.toml" >&2; exit 1; }
numeric=${version%%[-+]*} # the Windows file version takes digits and dots only

if [ "$build" = 1 ]; then
  "$root/scripts/build-windows.sh"
fi
[ -f "$out/yaac.exe" ] || { echo "$out/yaac.exe is missing: run scripts/build-windows.sh" >&2; exit 1; }
# The build id the exe was made with (scripts/build-windows.sh writes it beside the exe), so
# the installer carries the same one: `<version>+<id>`, file version `<numeric>.<id>`.
id=$(tr -cd '0-9' <"$out/build-id" 2>/dev/null || true)
[ -n "$id" ] || { echo "$out/build-id is missing: rebuild with scripts/build-windows.sh" >&2; exit 1; }
version="$version+$id"
numeric="$numeric.$id"

# The icon build.rs drew for the exe's resources (same drawing as the window's icon).
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
case "$CARGO_TARGET_DIR" in /*) ;; *) CARGO_TARGET_DIR="$PWD/$CARGO_TARGET_DIR" ;; esac
# (Cargo has laid build-script output out as build/yaac-app-<hash>/out and build/yaac-app/<hash>/out.)
icon=$(find "$CARGO_TARGET_DIR/$target/release/build" -path '*yaac-app*' -name yaac.ico -printf '%T@ %p\n' 2>/dev/null | sort -rn | head -n 1 | cut -d' ' -f2-)
[ -n "$icon" ] || { echo "no yaac.ico under $CARGO_TARGET_DIR/$target/release/build: build the exe first" >&2; exit 1; }

iscc=${ISCC:-}
if [ -z "$iscc" ]; then
  for base in "$(cmd.exe /d /c 'echo %LOCALAPPDATA%' 2>/dev/null | tr -d '\r')\\Programs" \
              "$(cmd.exe /d /c 'echo %ProgramFiles(x86)%' 2>/dev/null | tr -d '\r')" \
              "$(cmd.exe /d /c 'echo %ProgramFiles%' 2>/dev/null | tr -d '\r')"; do
    candidate=$(wslpath -u "$base\\Inno Setup 6\\ISCC.exe" 2>/dev/null) || continue
    if [ -f "$candidate" ]; then iscc=$candidate; break; fi
  done
fi
[ -n "$iscc" ] && [ -f "$iscc" ] || {
  echo "ISCC.exe not found: install Inno Setup 6 (winget install --id JRSoftware.InnoSetup -e --scope user) or set ISCC" >&2
  exit 1
}

setup="$out/YAAC-Setup-$version.exe"
rm -f "$setup"
"$iscc" /Q \
  "/DAppVersion=$version" \
  "/DNumericVersion=$numeric" \
  "/DSourceDir=$(wslpath -w "$out")" \
  "/DIconFile=$(wslpath -w "$icon")" \
  "/DOutputDir=$(wslpath -w "$out")" \
  "$(wslpath -w "$root/installer/yaac.iss")"
[ -f "$setup" ] || { echo "ISCC did not write $setup" >&2; exit 1; }
size=$(wc -c <"$setup")
echo "built $setup ($size bytes)"
sha256sum "$setup"
