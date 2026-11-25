// The user's umask for what yaacd starts on their behalf.
//
// Stage 0 runs under `umask 077` so the runtime and daemon it installs stay private, and it
// passes the umask the user had before in YAAC_USER_UMASK. yaacd restores that umask at start,
// so the harnesses, terminals and `!` commands it launches create files as the user's own shell
// would (644/755 under the usual 022), not 600/700. yaacd's own state stays private regardless:
// its directories are created with an explicit 0700 mode and secrets with an explicit 0600.

/** Parses an octal umask as `umask` prints it ("0022", "022", "77"); undefined if invalid. */
export function parseUmask(raw: string | undefined): number | undefined {
  if (raw === undefined) return undefined;
  const text = raw.trim();
  if (!/^[0-7]{1,4}$/.test(text)) return undefined;
  const value = parseInt(text, 8);
  return value <= 0o777 ? value : undefined;
}

/**
 * Restores the user's umask from YAAC_USER_UMASK. Leaves the process umask alone when the
 * variable is absent or invalid (an older stage 0, a daemon started by hand, Windows).
 * Returns the umask now in effect, or undefined when nothing was changed.
 */
export function restoreUserUmask(env: NodeJS.ProcessEnv = process.env): number | undefined {
  if (process.platform === "win32") return undefined;
  const mask = parseUmask(env.YAAC_USER_UMASK);
  if (mask === undefined) return undefined;
  process.umask(mask);
  return mask;
}
