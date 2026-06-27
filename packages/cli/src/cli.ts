/**
 * aex CLI entrypoint. Wires the real IO surface and calls
 * {@link runCli}. The shipped bundle has a `#!/usr/bin/env bun` line
 * prepended by `scripts/finalize-bundle.mjs`.
 *
 * NO `process.env.AEX_*` reads here — paths to the manifest and
 * token file are fixed constants in `internal.ts`. The mechanical
 * enforcement test in Phase 9 greps the built artifact to confirm.
 *
 * This IS the single host-only file allowed to touch the OS/home/env:
 * the persistent config store (`aex login`) resolves its path from
 * `XDG_CONFIG_HOME` / `APPDATA` / `os.homedir()` here — so the command
 * layer (`run.ts` / `host/*`) stays pure and the `no-env-vars` bundle
 * grep (which only forbids `process.env.AEX_*`) stays green.
 */
import { readFile, writeFile, readdir, stat, mkdir, chmod, rm } from "node:fs/promises";
import { resolve as resolvePath, join, dirname } from "node:path";
import { homedir } from "node:os";
import { runCli } from "./run.js";
import type { CliIO, CliConfigStore, StoredCliConfig, OutputsSyncFileEntry } from "./internal.js";

async function walkDirectory(root: string): Promise<readonly OutputsSyncFileEntry[] | null> {
  try {
    const rootStat = await stat(root);
    if (!rootStat.isDirectory()) return null;
  } catch {
    return null;
  }
  const out: OutputsSyncFileEntry[] = [];
  async function visit(dir: string): Promise<void> {
    const entries = await readdir(dir, { withFileTypes: true });
    for (const entry of entries) {
      const full = resolvePath(dir, entry.name);
      if (entry.isDirectory()) {
        await visit(full);
      } else if (entry.isFile()) {
        const s = await stat(full);
        out.push({ path: full, sizeBytes: s.size });
      }
    }
  }
  await visit(root);
  return out;
}

// `--debug` is detected from argv here (NOT an AEX_* env read) so the config
// store can emit non-secret diagnostics on the same channel as the commands.
const debug = process.argv.includes("--debug");
const debugLog = (line: string): void => {
  if (debug) process.stderr.write(`${line}\n`);
};

/**
 * Resolve the persistent config path. POSIX honors `XDG_CONFIG_HOME`; Windows
 * honors `APPDATA`; both fall back to `~/.config/aex/config.json`. None of
 * these is a `process.env.AEX_*` read, so the bundle lockdown grep stays clean.
 */
function resolveConfigPath(): string {
  const xdg = process.env.XDG_CONFIG_HOME;
  if (xdg && xdg.length > 0) return join(xdg, "aex", "config.json");
  if (process.platform === "win32") {
    const appData = process.env.APPDATA;
    if (appData && appData.length > 0) return join(appData, "aex", "config.json");
  }
  return join(homedir(), ".config", "aex", "config.json");
}

const configPath = resolveConfigPath();

const configStore: CliConfigStore = {
  location: () => configPath,
  read: async () => {
    try {
      const text = await readFile(configPath, "utf8");
      const parsed = JSON.parse(text) as StoredCliConfig;
      debugLog(`[aex] config: read ${configPath} (present=true)`);
      return parsed;
    } catch (err) {
      const code = (err as NodeJS.ErrnoException | undefined)?.code;
      if (code === "ENOENT") {
        debugLog(`[aex] config: read ${configPath} (present=false)`);
        return null;
      }
      // Corrupt / unreadable file: visibly ignore rather than crash auth.
      debugLog(`[aex] config: ${configPath} unreadable (${code ?? "parse"}) — ignoring`);
      return null;
    }
  },
  write: async (config) => {
    // writeFile's `mode` only applies to a NEWLY created file; chmod after to
    // enforce 0600 on overwrite (near no-op on Windows, harmless).
    await mkdir(dirname(configPath), { recursive: true, mode: 0o700 });
    await writeFile(configPath, JSON.stringify(config, null, 2) + "\n", { mode: 0o600 });
    try {
      await chmod(configPath, 0o600);
    } catch {
      // best-effort (Windows / unusual fs)
    }
    debugLog(`[aex] config: wrote ${configPath} (mode=600)`);
  },
  clear: async () => {
    await rm(configPath, { force: true });
    debugLog(`[aex] config: cleared ${configPath}`);
  }
};

const io: CliIO = {
  readFile: (path) => readFile(path, "utf8"),
  writeFile: (path, data) => writeFile(path, data),
  mkdirp: async (path) => {
    await mkdir(path, { recursive: true });
  },
  fetchImpl: fetch,
  stdout: (chunk) => process.stdout.write(chunk),
  stderr: (chunk) => process.stderr.write(chunk),
  exit: (code) => process.exit(code),
  argv: process.argv,
  cwd: () => process.cwd(),
  walkDirectory,
  configStore
};

await runCli(io);
