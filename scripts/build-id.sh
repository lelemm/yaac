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
#
# Release builds are numbered by CI, not by this counter: .github/workflows/release.yml sets
# YAAC_BUILD_ID to 1000 + the workflow's run number, and an id given in YAAC_BUILD_ID is
# printed back as it is, without touching the counter. CI ids are the release ids; local ids
# stay below 1000, so every release is newer than any local build (docs/install.md#releases).
set -eu

# Below the first release id (RELEASE_BASE + 1 in the workflow).
LOCAL_MAX=999

if [ -n "${YAAC_BUILD_ID:-}" ]; then
  case "$YAAC_BUILD_ID" in
    *[!0-9]*) echo "build-id: YAAC_BUILD_ID must be a number, not '$YAAC_BUILD_ID'" >&2; exit 1 ;;
  esac
  printf '%s\n' "$YAAC_BUILD_ID"
  exit 0
fi

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
[ "$next" -le "$LOCAL_MAX" ] || {
  echo "build-id: local build ids end at $LOCAL_MAX (release builds start above it); reset $counter" >&2
  exit 1
}
printf '%s\n' "$next" >"$counter.tmp"
mv "$counter.tmp" "$counter"
printf '%s\n' "$next"
