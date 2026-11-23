#!/bin/sh
# Stage the pinned Node.js runtime and its license for the native Windows installer.
set -eu

dest=${1:?usage: stage-windows-runtime.sh <installer source directory>}/runtime
mkdir -p "$dest"

fetch() {
  file=$1
  url=$2
  sum=$3
  if [ -f "$dest/$file" ] && printf '%s  %s\n' "$sum" "$dest/$file" | sha256sum --check --status; then
    return
  fi
  curl --fail --silent --show-error --location --retry 2 -o "$dest/$file.part" "$url"
  printf '%s  %s.part\n' "$sum" "$dest/$file" | sha256sum --check
  mv "$dest/$file.part" "$dest/$file"
}

# Keep this release in sync with PINNED_NODE in crates/yaac-transport/src/runtime.rs.
# node.exe's checksum is from the release's signed SHASUMS256.txt.
version=22.22.3
fetch node.exe "https://nodejs.org/dist/v$version/win-x64/node.exe" 780f44f2c53c108bae261ada21a525b4bfe733c020ac85e41bfe94479090ac9b
fetch LICENSE-Node.js.txt "https://raw.githubusercontent.com/nodejs/node/v$version/LICENSE" c738ae413cf561f174e34f6961f8ca458aae2369a73640dda6234c629b98bcc4
