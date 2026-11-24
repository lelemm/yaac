#!/bin/sh
# Stage the runtime matching crates/yaac-preview/native's Rust bindings.
set -eu
platform=${1:?usage: stage-cef-runtime.sh linux64|windows64 <empty destination>}
dest=${2:?specify an empty destination directory}
case "$platform" in
linux64) sum=a6c3452c47f344f9f125c71dfc9d8965149bac7c8df1e23ffd80442a28ce1b62 ;;
windows64) sum=c691636862b5f6bab05e76d77bd1b57799659552a25a5bc8b7eaa365de6e63fd ;;
*) echo "unsupported CEF platform: $platform" >&2; exit 2 ;;
esac
root=$(cd "$(dirname "$0")/.." && pwd)
name=cef_binary_156.0.31+ge603d97+chromium-156.0.8078.100_${platform}_minimal
cache="$root/.build/cef"
mkdir -p "$cache" "$(dirname "$dest")"
archive="$cache/$name.tar.bz2"
if [ -f "$dest/.cef-version" ] && [ "$(cat "$dest/.cef-version")" = "$sum" ]; then
  exit 0
fi
if ! (printf '%s  %s\n' "$sum" "$archive" | sha256sum --status -c) 2>/dev/null; then
  part=$(mktemp "$cache/archive.XXXXXXXX")
  trap 'rm -f "$part"' EXIT
  curl --fail --location --retry 2 --output "$part" "https://cef-builds.spotifycdn.com/$name.tar.bz2"
  printf '%s  %s\n' "$sum" "$part" | sha256sum -c
  mv "$part" "$archive"
fi
tmp=$(mktemp -d "$(dirname "$dest")/cef-stage.XXXXXXXX")
trap 'rm -rf "$tmp"' EXIT
# Some bzip2 builds fail at random on this 330 MB archive ("I/O or other error … Success")
# although its checksum is right; Python's decoder is used when tar's bzip2 fails.
if ! tar -xjf "$archive" -C "$tmp" "$name/Release" "$name/Resources" "$name/LICENSE.txt"; then
  echo "tar/bzip2 failed; extracting with Python" >&2
  rm -rf "${tmp:?}/$name"
  python3 - "$archive" "$tmp" "$name" <<'PY'
import sys, tarfile
archive, out, name = sys.argv[1:]
keep = (f"{name}/Release", f"{name}/Resources", f"{name}/LICENSE.txt")
with tarfile.open(archive, "r|bz2") as t:
    for m in t:
        if m.name in keep or m.name.startswith((keep[0] + "/", keep[1] + "/")):
            t.extract(m, out, filter="tar")
PY
fi
mv "$tmp/$name/Release" "$tmp/runtime"
cp -R "$tmp/$name/Resources/." "$tmp/runtime/"
cp "$tmp/$name/LICENSE.txt" "$tmp/runtime/LICENSE-CEF.txt"
# Rename only into an absent or empty directory. Leave existing installs alone.
printf '%s\n' "$sum" > "$tmp/runtime/.cef-version"
mv -T "$tmp/runtime" "$dest"
