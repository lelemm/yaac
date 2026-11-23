import * as fs from "node:fs";
import * as path from "node:path";

function executable(file: string): boolean {
  try {
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}

/** Search executable files and native npm packages before falling back to the desktop's CLI. */
export function windowsExecutable(env: NodeJS.ProcessEnv, name: string, launcherDirs: string[], npmPackages: string[], desktop: string): string | null {
  const local = env.LOCALAPPDATA;
  const dirs = (env.PATH ?? env.Path ?? "").split(path.delimiter).map((dir) => dir.replace(/^"(.*)"$/, "$1")).filter(Boolean);
  if (env.APPDATA) dirs.push(path.join(env.APPDATA, "npm"));
  if (local) dirs.push(path.join(local, "Microsoft", "WinGet", "Links"));
  dirs.push(...launcherDirs);
  for (const dir of new Set(dirs)) {
    const direct = path.join(dir, name);
    if (executable(direct)) return direct;
    for (const pkg of npmPackages) {
      const packaged = path.join(dir, "node_modules", pkg, name);
      if (executable(packaged)) return packaged;
    }
  }
  if (!local) return null;
  const bundled = fs.globSync(desktop, { cwd: local }).flatMap((file) => {
    const full = path.join(local, file);
    try {
      const stat = fs.statSync(full);
      return stat.isFile() ? [{ full, modified: stat.mtimeMs }] : [];
    } catch {
      return [];
    }
  });
  bundled.sort((a, b) => b.modified - a.modified);
  return bundled[0]?.full ?? null;
}
