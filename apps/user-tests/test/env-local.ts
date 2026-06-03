import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { config as loadDotenv } from "dotenv";

let loaded = false;

export function loadLocalEnv(): void {
  if (loaded) return;
  loaded = true;

  const testRoot = dirname(fileURLToPath(import.meta.url));
  const appRoot = resolve(testRoot, "..");
  const repoRoot = resolve(appRoot, "..", "..");

  for (const dir of uniqueDirs([appRoot, repoRoot])) {
    const path = resolve(dir, ".env.local");
    if (existsSync(path)) {
      loadDotenv({ path, override: false });
    }
  }

  aliasEnv("ANTPATH_API_URL", ["ANTPATH_LIVE_API_BASE", "ANTPATH_API_BASE"]);
  aliasEnv("ANTPATH_API_TOKEN", ["ANTPATH_LIVE_API_TOKEN"]);
  aliasEnv("ANTHROPIC_API_KEY", ["ANTPATH_USER_TEST_ANTHROPIC_KEY"]);
  aliasEnv("DEEPSEEK_API_KEY", ["ANTPATH_USER_TEST_DEEPSEEK_KEY"]);
}

function uniqueDirs(dirs: readonly string[]): string[] {
  return Array.from(new Set(dirs));
}

function aliasEnv(target: string, sources: readonly string[]): void {
  if (process.env[target] && process.env[target]!.length > 0) return;
  for (const source of sources) {
    const value = process.env[source];
    if (value && value.length > 0) {
      process.env[target] = value;
      return;
    }
  }
}
