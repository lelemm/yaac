// Which forge hosts a repository, from its remote URL (D119).
//
// GitHub: `https://github.com/o/r(.git)`, `git@github.com:o/r.git`, `ssh://git@github.com/o/r`,
// and the same forms on a GitHub Enterprise host — which is GitHub only when the target's `gh`
// knows the host (`gh auth status`), since any host can serve git.
//
// Azure DevOps: `https://dev.azure.com/{org}/{project}/_git/{repo}` (also with `{user}@`),
// `https://{org}.visualstudio.com/[DefaultCollection/]{project}/_git/{repo}`,
// `git@ssh.dev.azure.com:v3/{org}/{project}/{repo}`, and
// `{org}@vs-ssh.visualstudio.com:v3/{org}/{project}/{repo}`.

export type ParsedRemote =
  | { kind: "github"; host: string; owner: string; repo: string }
  | { kind: "azureDevOps"; host: string; org: string; project: string; repo: string };

/** Host and path of any git URL form: scp-like `user@host:path`, or `scheme://[user@]host[:port]/path`. */
export function splitUrl(url: string): { host: string; path: string } | null {
  const u = url.trim();
  if (!u) return null;
  const scheme = /^([a-z][a-z0-9+.-]*):\/\/(?:[^@/]*@)?([^/:]+)(?::\d+)?(\/.*)?$/i.exec(u);
  if (scheme) return { host: scheme[2]!.toLowerCase(), path: (scheme[3] ?? "").replace(/^\/+/, "") };
  const scp = /^(?:[^@/]+@)?([^:/]+):(?!\/)(.+)$/.exec(u);
  if (scp) return { host: scp[1]!.toLowerCase(), path: scp[2]!.replace(/^\/+/, "") };
  return null;
}

function clean(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

/**
 * Parses a remote URL. `githubHosts` are the hosts the target's `gh` is signed in to (GitHub
 * Enterprise); `github.com` is always GitHub. Returns null for anything else.
 */
export function parseRemote(url: string, githubHosts: Iterable<string> = []): ParsedRemote | null {
  const s = splitUrl(url);
  if (!s) return null;
  const parts = s.path.replace(/\.git\/?$/, "").replace(/\/+$/, "").split("/").filter(Boolean);
  const host = s.host;
  // Azure DevOps.
  if (host === "dev.azure.com" && parts.length >= 4 && parts[2] === "_git") {
    return { kind: "azureDevOps", host, org: clean(parts[0]!), project: clean(parts[1]!), repo: clean(parts[3]!) };
  }
  if (host === "ssh.dev.azure.com" && parts.length >= 4 && parts[0] === "v3") {
    return { kind: "azureDevOps", host: "dev.azure.com", org: clean(parts[1]!), project: clean(parts[2]!), repo: clean(parts[3]!) };
  }
  if (host === "vs-ssh.visualstudio.com" && parts.length >= 4 && parts[0] === "v3") {
    return { kind: "azureDevOps", host: "dev.azure.com", org: clean(parts[1]!), project: clean(parts[2]!), repo: clean(parts[3]!) };
  }
  const vs = /^([a-z0-9-]+)\.visualstudio\.com$/.exec(host);
  if (vs) {
    const p = parts[0]?.toLowerCase() === "defaultcollection" ? parts.slice(1) : parts;
    if (p.length >= 3 && p[1] === "_git") return { kind: "azureDevOps", host: "dev.azure.com", org: vs[1]!, project: clean(p[0]!), repo: clean(p[2]!) };
    return null;
  }
  // GitHub (and GitHub Enterprise hosts gh knows).
  const known = new Set([...githubHosts].map((h) => h.toLowerCase()));
  if ((host === "github.com" || known.has(host)) && parts.length === 2) {
    return { kind: "github", host, owner: parts[0]!, repo: parts[1]! };
  }
  return null;
}

/** The slug shown for a parsed remote: `owner/repo` or `org/project/repo`. */
export function slugOf(r: ParsedRemote): string {
  return r.kind === "github" ? `${r.owner}/${r.repo}` : `${r.org}/${r.project}/${r.repo}`;
}

/** The repository's web page. */
export function webUrlOf(r: ParsedRemote): string {
  if (r.kind === "github") return `https://${r.host}/${r.owner}/${r.repo}`;
  const e = encodeURIComponent;
  return `https://dev.azure.com/${e(r.org)}/${e(r.project)}/_git/${e(r.repo)}`;
}
