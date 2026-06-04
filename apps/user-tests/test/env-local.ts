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

}

function uniqueDirs(dirs: readonly string[]): string[] {
  return Array.from(new Set(dirs));
}
