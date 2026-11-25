// npm installs only the build machine's optional native dependency. Stage all target
// archives, verifying each against the same package-lock integrity used by npm ci.
import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, writeFile, rename, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const lock = JSON.parse(await readFile(join(root, "package-lock.json"), "utf8"));
const dest = join(root, "dist/pty");
await mkdir(dest, { recursive: true });
for (const target of ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "win32-x64"]) {
  const pkg = lock.packages["node_modules/@lydell/node-pty-" + target];
  const path = join(dest, target + ".tgz");
  const valid = data => "sha512-" + createHash("sha512").update(data).digest("base64") === pkg.integrity;
  if (await readFile(path).then(valid, () => false)) continue;
  const response = await fetch(pkg.resolved);
  if (!response.ok) throw new Error(pkg.resolved + ": " + response.status);
  const data = Buffer.from(await response.arrayBuffer());
  if (!valid(data)) throw new Error("node-pty archive integrity mismatch for " + target);
  const tmp = path + "." + randomUUID();
  try { await writeFile(tmp, data); await rename(tmp, path); } finally { await rm(tmp, { force: true }); }
}
