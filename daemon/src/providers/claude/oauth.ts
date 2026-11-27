import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

interface ClaudeCredentials {
  claudeAiOauth?: { accessToken?: unknown };
}

/**
 * Read the access token from Claude Code's own credential source. YAAC never copies, writes,
 * refreshes, or logs it; callers should retain it only for the WebSocket handshake.
 */
export async function claudeAccessToken(env: NodeJS.ProcessEnv): Promise<string | null> {
  const injected = env.CLAUDE_CODE_OAUTH_TOKEN?.trim();
  if (injected) return injected;

  const defaultDir = path.join(env.HOME ?? env.USERPROFILE ?? os.homedir(), ".claude");
  const secureDir = env.CLAUDE_SECURESTORAGE_CONFIG_DIR !== undefined
    ? (env.CLAUDE_SECURESTORAGE_CONFIG_DIR || defaultDir)
    : (env.CLAUDE_CONFIG_DIR ?? defaultDir);
  try {
    const raw = await fs.readFile(path.join(secureDir.normalize("NFC"), ".credentials.json"), "utf8");
    const token = (JSON.parse(raw) as ClaudeCredentials).claudeAiOauth?.accessToken;
    return typeof token === "string" && token.trim() ? token : null;
  } catch {
    return null;
  }
}
