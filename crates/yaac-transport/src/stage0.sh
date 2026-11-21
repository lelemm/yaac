# YAAC stage 0 (docs/02-architecture.md#stage-0). POSIX sh: dash, bash, busybox.
# Run as: sh -c "<this>" yaac-stage0 [YAAC_NAME=value ...]
# Reports what is installed, accepts the runtime and the daemon bundle in-band, then execs
# the daemon's bridge.
# Host lines: GET runtime <name> <url> <sha256>    (download it here, verify, unpack)
#             PUT runtime <name> <bytes> <sha256>  (then exactly <bytes> bytes of a .tar.gz)
#             PUT daemon <build> <bytes> <sha256>  (then exactly <bytes> bytes)
#             RUN <runtime> <build>
# Replies:    OK runtime | OK daemon | OK run (the next byte is the YAAC protocol) | ERR <reason>
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

node_version() { # binary: prints its version; fails if it does not run
  "$1" -e 'process.stdout.write(process.versions.node)' 2>/dev/null
}

# The runtime (D17, D25; docs/02-architecture.md#how-the-runtime-gets-there): YAAC's own
# pinned Node.js in $H/runtime/$YAAC_RUNTIME_VERSION/ when it is installed; otherwise a
# Node.js 22 or newer already installed for this user ($YAAC_NODE, then PATH, then the usual
# version managers' folders: a non-login shell has none of them on PATH), unless
# YAAC_SYSTEM_NODE=0; otherwise none, and the host sends GET or PUT runtime.
NODE=
NODE_VERSION=
FOUND=
PRIVATE=
RT=${YAAC_RUNTIME_VERSION:-}
if [ -n "$RT" ] && valid_build "$RT" && [ -x "$H/runtime/$RT/bin/node" ]; then
  if v=$(node_version "$H/runtime/$RT/bin/node") && [ -n "$v" ]; then
    NODE=$H/runtime/$RT/bin/node NODE_VERSION=$v PRIVATE=$RT
  fi
fi
best=0
if [ -z "$NODE" ] && [ "${YAAC_SYSTEM_NODE:-1}" != 0 ]; then
  for c in "${YAAC_NODE:-}" "$(command -v node 2>/dev/null)" \
    "$HOME"/.nvm/versions/node/*/bin/node "$HOME"/.volta/bin/node \
    "$HOME"/.local/share/fnm/node-versions/*/installation/bin/node "$HOME"/.asdf/shims/node \
    /usr/local/bin/node /usr/bin/node; do
    [ -n "$c" ] && [ -x "$c" ] || continue
    v=$(node_version "$c") || continue
    major=${v%%.*}
    case $major in '' | *[!0-9]*) continue ;; esac
    if [ "$major" -ge 22 ]; then
      if [ "$major" -gt "$best" ]; then best=$major; NODE=$c; NODE_VERSION=$v; fi
    elif [ -z "$FOUND" ]; then
      FOUND=$v
    fi
  done
fi

fetch=none
if command -v curl >/dev/null 2>&1; then
  fetch=curl
elif command -v wget >/dev/null 2>&1; then
  fetch=wget
fi

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
printf 'YAAC0 os=%s arch=%s libc=%s runtime=%s daemon=%s serving=%s found=%s private=%s fetch=%s\n' \
  "$os" "$arch" "$libc" "$runtime" "${daemons:-none}" "$serving" "${FOUND:-none}" "${PRIVATE:-none}" "$fetch"

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

take_lock() { # directory: 0 once it is ours
  tries=0
  until mkdir "$1" 2>/dev/null; do
    tries=$((tries + 1))
    [ "$tries" -gt 300 ] && return 1
    sleep 0.1 2>/dev/null || sleep 1
  done
  return 0
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
  if ! take_lock "$lock"; then
    rm -rf "$tmp"
    echo "ERR the install lock $lock is held"
    return
  fi
  if [ -f "$H/daemon/$build/yaacd.mjs" ]; then
    rm -rf "$tmp"
  else
    rm -rf "$H/daemon/$build"
    mv "$tmp" "$H/daemon/$build"
  fi
  rmdir "$lock"
  if [ -f "$H/daemon/$build/yaacd.mjs" ]; then echo "OK daemon"; else echo "ERR could not install daemon $build"; fi
}

# Verify, unpack, test, and rename a runtime archive into $H/runtime/<name>/, under the lock.
install_runtime() { # name archive sha256
  name=$1 file=$2 want=$3
  if ! valid_build "$name"; then
    rm -f "$file"
    echo "ERR invalid runtime name"
    return
  fi
  sum=$(hash_of "$file")
  if [ "$sum" = none ]; then
    rm -f "$file"
    echo "ERR the target has no sha256sum or shasum to verify the runtime"
    return
  fi
  if [ "$sum" != "$want" ]; then
    rm -f "$file"
    echo "ERR hash mismatch for runtime $name"
    return
  fi
  dir=$H/runtime/.tmp.$$
  rm -rf "$dir"
  if ! mkdir -p "$dir" 2>/dev/null; then
    rm -f "$file"
    echo "ERR cannot write to $H/runtime"
    return
  fi
  if ! gzip -dc "$file" 2>/dev/null | (cd "$dir" && tar -xf - 2>/dev/null); then
    rm -rf "$dir" "$file"
    echo "ERR could not unpack runtime $name (disk full, or no gzip and tar?)"
    return
  fi
  rm -f "$file"
  top=
  for t in "$dir"/*; do [ -x "$t/bin/node" ] && top=$t; done
  v=
  [ -n "$top" ] && v=$(node_version "$top/bin/node")
  case ${v%%.*} in
    '' | *[!0-9]*)
      rm -rf "$dir"
      echo "ERR runtime $name does not run on this machine"
      return
      ;;
  esac
  if ! take_lock "$H/runtime/.lock"; then
    rm -rf "$dir"
    echo "ERR the install lock $H/runtime/.lock is held"
    return
  fi
  if [ -x "$H/runtime/$name/bin/node" ]; then
    rm -rf "$dir"
  else
    rm -rf "$H/runtime/$name"
    mv "$top" "$H/runtime/$name"
    rm -rf "$dir"
  fi
  rmdir "$H/runtime/.lock"
  if [ -x "$H/runtime/$name/bin/node" ]; then
    NODE=$H/runtime/$name/bin/node
    NODE_VERSION=$(node_version "$NODE")
    echo "OK runtime"
  else
    echo "ERR could not install runtime $name"
  fi
}

get_runtime() { # name url sha256
  mkdir -p "$H/runtime" 2>/dev/null
  f=$H/runtime/.dl.$$
  rm -f "$f"
  case $fetch in
    curl) curl -fsSL --retry 2 -o "$f" "$2" </dev/null 2>/dev/null ;;
    wget) wget -q -O "$f" "$2" </dev/null 2>/dev/null ;;
    *) false ;;
  esac || {
    rm -f "$f"
    echo "ERR the target could not download the runtime (with $fetch)"
    return
  }
  install_runtime "$1" "$f" "$3"
}

put_runtime() { # name bytes sha256
  case $2 in
    '' | *[!0-9]*)
      echo "ERR invalid byte count"
      exit 1
      ;;
  esac
  mkdir -p "$H/runtime" 2>/dev/null
  f=$H/runtime/.up.$$
  if ! : >"$f" 2>/dev/null; then
    head -c "$2" >/dev/null
    echo "ERR cannot write to $H/runtime"
    return
  fi
  head -c "$2" >"$f"
  got=$(wc -c <"$f" | tr -d ' ')
  if [ "$got" != "$2" ]; then
    rm -f "$f"
    echo "ERR short upload: $got of $2 bytes (disk full?)"
    return
  fi
  install_runtime "$1" "$f" "$3"
}

while read -r cmd a b c d; do
  case $cmd in
    GET)
      case $a in
        runtime) get_runtime "$b" "$c" "$d" ;;
        *) echo "ERR cannot GET $a" ;;
      esac
      ;;
    PUT)
      case $a:$b:$c in
        daemon:*:*) put_daemon "$b" "$c" "$d" ;;
        runtime:*:*) put_runtime "$b" "$c" "$d" ;;
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
