# YAAC SSH setup, on the server (docs/02-architecture.md#setting-up-an-ssh-connection). POSIX sh.
# Run as: sh -c "<this>" yaac-ssh <install|revoke> <args...>
#   install <line> <authorized_keys|''> <yaac home|''> <id>
#       Append YAAC's line (only if its key is not there), print "INSTALLED <0|1>", then wait
#       for the host's decision on standard input: KEEP (the key-only login worked: keep it,
#       and record it in <yaac home>/ssh/<id>) or anything else, or the end of the stream
#       (remove the line again and confirm it is gone).
#   revoke <key> <authorized_keys|''> <yaac home|''> <id>
#       Remove every line holding exactly this key, and YAAC's record.
# Never rewrites, reorders, or removes any other line. Replies: OK ... | ERR <reason>
umask 077
op=$1
ak=${3:-$HOME/.ssh/authorized_keys}
rec=${4:-$HOME/.yaac}/ssh
id=$5

remove_key() { # key
  if [ ! -f "$ak" ] || ! grep -qF -- "$1" "$ak"; then
    echo "OK absent"
    return 0
  fi
  tmp=$ak.yaac-tmp.$$
  if ! awk -v k="$1" '{ for (i = 1; i <= NF; i++) if ($i == k) next; print }' "$ak" >"$tmp"; then
    rm -f "$tmp"
    echo "ERR cannot rewrite $ak"
    return 1
  fi
  chmod 600 "$tmp"
  if [ -L "$ak" ]; then
    cat "$tmp" >"$ak" && rm -f "$tmp"
  else
    mv -f "$tmp" "$ak"
  fi
  if grep -qF -- "$1" "$ak"; then
    echo "ERR the line is still in $ak"
    return 1
  fi
  echo "OK removed"
}

case $id in '' | *[!A-Za-z0-9._-]*) echo "ERR invalid id"; exit 1 ;; esac

case $op in
  install)
    line=$2
    key=$(printf '%s\n' "$line" | cut -d ' ' -f 2)
    [ -n "$key" ] || { echo "ERR invalid line"; exit 1; }
    d=$(dirname "$ak")
    if [ ! -d "$d" ]; then
      mkdir -p "$d" && chmod 700 "$d" || { echo "ERR cannot create $d"; exit 1; }
    fi
    if [ ! -e "$ak" ]; then
      { : >"$ak" && chmod 600 "$ak"; } || { echo "ERR cannot create $ak"; exit 1; }
    fi
    added=0
    if ! grep -qF -- "$key" "$ak"; then
      # Keep the last existing line whole.
      if [ -s "$ak" ] && [ -n "$(tail -c 1 "$ak")" ]; then printf '\n' >>"$ak"; fi
      printf '%s\n' "$line" >>"$ak" || { echo "ERR cannot write to $ak"; exit 1; }
      added=1
    fi
    echo "INSTALLED $added"
    decision=
    read -r decision || decision=
    if [ "$decision" = KEEP ]; then
      mkdir -p "$rec" 2>/dev/null
      printf '%s\n' "$line" >"$rec/$id" 2>/dev/null
      echo "OK kept"
    else
      remove_key "$key"
    fi
    ;;
  revoke)
    [ -n "$2" ] || { echo "ERR invalid key"; exit 1; }
    remove_key "$2" && rm -f "$rec/$id"
    ;;
  *)
    echo "ERR unknown operation"
    exit 1
    ;;
esac
