# YAAC stage 0 (docs/02-architecture.md#stage-0). POSIX sh: dash, bash, busybox.
# Run as: sh -c "<this>" yaac-stage0 [YAAC_NAME=value ...]
# Reports what is installed, accepts the daemon bundle in-band, then execs its bridge.
# Host lines: PUT daemon <build> <bytes> <sha256> (then exactly <bytes> bytes) | RUN <runtime> <build>
# Replies:    OK daemon | OK run (the next byte is the YAAC protocol) | ERR <reason>
umask 077
for kv in "$@"; do
  case $kv in
    YAAC_[A-Z0-9_]*=*) export "$kv" ;;
  esac
done
H=${YAAC_HOME:-$HOME/.yaac}

valid_build() {
  case $1 in
    '' | .* | *[!A-Za-z0-9._+-]*) return 1 ;;
  esac
  return 0
}

# SEAM(private-runtime): YAAC's own pinned runtime (D17, D25) is not shipped yet. Until it is,
# stage 0 uses a Node.js 22 or newer already installed for this user: $YAAC_NODE, then PATH,
# then the usual version managers' folders (a non-login shell has none of them on PATH).
NODE=
NODE_VERSION=
FOUND=
best=0
for c in "${YAAC_NODE:-}" "$(command -v node 2>/dev/null)" \
  "$HOME"/.nvm/versions/node/*/bin/node "$HOME"/.volta/bin/node \
  "$HOME"/.local/share/fnm/node-versions/*/installation/bin/node "$HOME"/.asdf/shims/node \
  /usr/local/bin/node /usr/bin/node; do
  [ -n "$c" ] && [ -x "$c" ] || continue
  v=$("$c" -e 'process.stdout.write(process.versions.node)' 2>/dev/null) || continue
  major=${v%%.*}
  case $major in '' | *[!0-9]*) continue ;; esac
  if [ "$major" -ge 22 ]; then
    if [ "$major" -gt "$best" ]; then best=$major; NODE=$c; NODE_VERSION=$v; fi
  elif [ -z "$FOUND" ]; then
    FOUND=$v
  fi
done

os=$(uname -s 2>/dev/null) || os=unknown
arch=$(uname -m 2>/dev/null) || arch=unknown
libc=glibc
for f in /lib/ld-musl-*; do [ -e "$f" ] && libc=musl; done

daemons=
if [ -d "$H/daemon" ]; then
  for d in "$H"/daemon/*; do
    b=${d##*/}
    valid_build "$b" && [ -f "$d/yaacd.mjs" ] || continue
    daemons=${daemons:+$daemons,}$b
  done
fi

# The serving daemon's build, from the bundle path in its command line.
serving=none
if [ -r "$H/run/yaacd.pid" ]; then
  read -r pid <"$H/run/yaacd.pid" 2>/dev/null
  if [ -n "${pid:-}" ] && kill -0 "$pid" 2>/dev/null; then
    serving=unknown
    if [ -r "/proc/$pid/cmdline" ]; then
      b=$(tr '\000' '\n' <"/proc/$pid/cmdline" 2>/dev/null | sed -n 's|.*/daemon/\([^/]*\)/yaacd\.mjs$|\1|p' | head -n 1)
      valid_build "$b" && serving=$b
    fi
  fi
fi

runtime=none
[ -n "$NODE" ] && runtime=node-$NODE_VERSION
printf 'YAAC0 os=%s arch=%s libc=%s runtime=%s daemon=%s serving=%s found=%s\n' \
  "$os" "$arch" "$libc" "$runtime" "${daemons:-none}" "$serving" "${FOUND:-none}"

hash_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  elif [ -n "$NODE" ]; then
    "$NODE" -e 'const c=require("crypto"),f=require("fs");process.stdout.write(c.createHash("sha256").update(f.readFileSync(process.argv[1])).digest("hex"))' "$1"
  else
    echo none
  fi
}

put_daemon() { # build bytes sha256
  build=$1 n=$2 want=$3
  case $n in
    '' | *[!0-9]*)
      echo "ERR invalid byte count"
      exit 1
      ;;
  esac
  # Read the bytes first, whatever happens next: they are on the stream either way.
  mkdir -p "$H/daemon" 2>/dev/null
  tmp=$H/daemon/.tmp.$$
  rm -rf "$tmp"
  if ! mkdir -p "$tmp" 2>/dev/null; then
    head -c "$n" >/dev/null
    echo "ERR cannot write to $H/daemon"
    return
  fi
  head -c "$n" >"$tmp/yaacd.mjs"
  got=$(wc -c <"$tmp/yaacd.mjs" | tr -d ' ')
  if [ "$got" != "$n" ]; then
    rm -rf "$tmp"
    echo "ERR short upload: $got of $n bytes (disk full?)"
    return
  fi
  if ! valid_build "$build"; then
    rm -rf "$tmp"
    echo "ERR invalid build name"
    return
  fi
  sum=$(hash_of "$tmp/yaacd.mjs")
  if [ "$sum" != "$want" ]; then
    rm -rf "$tmp"
    echo "ERR hash mismatch for daemon $build"
    return
  fi
  # Install under the lock: rename the verified folder into place unless another host
  # already did.
  lock=$H/daemon/.lock
  tries=0
  until mkdir "$lock" 2>/dev/null; do
    tries=$((tries + 1))
    if [ "$tries" -gt 300 ]; then
      rm -rf "$tmp"
      echo "ERR the install lock $lock is held"
      return
    fi
    sleep 0.1 2>/dev/null || sleep 1
  done
  if [ -f "$H/daemon/$build/yaacd.mjs" ]; then
    rm -rf "$tmp"
  else
    rm -rf "$H/daemon/$build"
    mv "$tmp" "$H/daemon/$build"
  fi
  rmdir "$lock"
  if [ -f "$H/daemon/$build/yaacd.mjs" ]; then echo "OK daemon"; else echo "ERR could not install daemon $build"; fi
}

while read -r cmd a b c d; do
  case $cmd in
    PUT)
      case $a:$b:$c in
        daemon:*:*) put_daemon "$b" "$c" "$d" ;;
        *) echo "ERR cannot PUT $a" ;;
      esac
      ;;
    RUN)
      if [ "$a" != node ] || [ -z "$NODE" ]; then
        echo "ERR no runtime $a"
      elif ! valid_build "$b" || [ ! -f "$H/daemon/$b/yaacd.mjs" ]; then
        echo "ERR daemon $b is not installed"
      else
        echo "OK run"
        YAAC_HOME=$H
        export YAAC_HOME
        exec "$NODE" "$H/daemon/$b/yaacd.mjs" connect
      fi
      ;;
    '') ;;
    *) echo "ERR unknown command $cmd" ;;
  esac
done
echo "ERR the host closed the stream"
exit 1
