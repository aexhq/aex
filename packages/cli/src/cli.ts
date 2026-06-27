/**
 * aex CLI entrypoint. Wires the real IO surface and calls
 * {@link runCli}. The shipped bundle has a `#!/usr/bin/env bun` line
 * prepended by `scripts/finalize-bundle.mjs`.
 *
 * NO `process.env.AEX_*` reads here — paths to the manifest and
 * token file are fixed constants in `internal.ts`. The mechanical
 * enforcement test in Phase 9 greps the built artifact to confirm.
 */
import { readFile, writeFile, readdir, stat, mkdir } from "node:fs/promises";
import { resolve as resolvePath } from "node:path";
import { runCli } from "./run.js";
import type { CliIO, OutputsSyncFileEntry } from "./internal.js";

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
  walkDirectory
};

await runCli(io);
