#!/bin/sh
# Allocates the next build id and prints it: one increasing number per build of YAAC.
#
#   id=$(scripts/build-id.sh)
#
# Every artefact of that build carries the same id after its own version (`<version>+<id>`):
# the window and the hub (`yaac.exe`, through YAAC_BUILD_ID at `cargo build`), the `yaacd`
# bundle the hub installs on targets (through YAAC_BUILD_ID at `npm run build`), and the
# installer. YAAC compares these ids when it starts, to tell which components are older
# (docs/02-architecture.md#build-ids).
#
# The counter is kept in .build/counter at the repository root (not committed). A build made
# without this script (a plain `cargo build`, tests) is a development build: its id is a hash
# of its sources (`dev.<hash>`), never a number.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
dir="$root/.build"
counter="$dir/counter"
mkdir -p "$dir"

# One allocation at a time, even if two builds start together.
lock="$dir/counter.lock"
i=0
until mkdir "$lock" 2>/dev/null; do
  i=$((i + 1))
  [ "$i" -lt 100 ] || { echo "build-id: $lock is held; remove it if no build is running" >&2; exit 1; }
  sleep 0.1
done
trap 'rmdir "$lock"' EXIT INT TERM

last=0
if [ -f "$counter" ]; then
  last=$(tr -cd '0-9' <"$counter")
  [ -n "$last" ] || last=0
fi
next=$((last + 1))
printf '%s\n' "$next" >"$counter.tmp"
mv "$counter.tmp" "$counter"
printf '%s\n' "$next"
