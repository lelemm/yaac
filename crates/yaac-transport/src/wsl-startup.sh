# Runs in the same wsl.exe as stage 0. Keep the profile's stdin, stdout and stderr off
# the connection. Bash's interactive flag lets ~/.bashrc pass the usual $- guard.
if [ "${CODEX_HOME+x}${CLAUDE_CONFIG_DIR+x}" != xx ] && [ -r "$HOME/.bashrc" ] &&
  command -v bash >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
  # The outer Bash reads NUL-terminated values without evaluating any shell text.
  # -p also keeps BASH_ENV from running in that shell before the redirects below.
  exec bash --noprofile --norc -p -c '
    import_profile() {
      local codex claude
      # Bound the reads too, in case a profile child keeps a pipe open.
      if IFS= read -r -d "" -t 3.5 codex && IFS= read -r -d "" -t 0.5 claude; then
        if [[ ! -v CODEX_HOME && $codex == CODEX_HOME=* ]]; then export "$codex"; fi
        if [[ ! -v CLAUDE_CONFIG_DIR && $claude == CLAUDE_CONFIG_DIR=* ]]; then export "$claude"; fi
      fi < <(exec timeout -k 1 2 bash --noprofile --norc -ic "$1" </dev/null 2>/dev/null)
    }
    import_profile "$1"
    shift
    exec "$@"
  ' yaac-wsl-profile '
    set +m
    builtin source "$HOME/.bashrc" >/dev/null || :
    set +u
    codex= claude=
    if [[ ${CODEX_HOME@a} == *x* ]]; then codex=CODEX_HOME=$CODEX_HOME; fi
    if [[ ${CLAUDE_CONFIG_DIR@a} == *x* ]]; then claude=CLAUDE_CONFIG_DIR=$CLAUDE_CONFIG_DIR; fi
    builtin printf "%s\0%s\0" "$codex" "$claude"
  ' "$@"
fi
exec "$@"
