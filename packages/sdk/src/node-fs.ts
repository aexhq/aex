import { readFile, readdir, stat } from "node:fs/promises";
import { join, posix, relative, sep } from "node:path";
import type { SkillFiles } from "./bundle.js";

/**
 * Walk a local directory and load every regular file into an in-memory
 * `SkillFiles` map. Symlinks and non-regular files are skipped (a
 * symlink that points outside the root would still be skipped because
 * we use `lstat` semantics). Paths are normalised to forward-slash
 * relative form so they can flow into `bundleSkillFiles` directly.
 *
 * Node-only. Browser callers should use `bundleSkillFiles` with a
 * pre-built files map instead.
 */
export async function readDirectoryAsFiles(rootDir: string): Promise<SkillFiles> {
  if (typeof rootDir !== "string" || !rootDir) {
    throw new Error("readDirectoryAsFiles: rootDir is required");
  }
  const rootStat = await stat(rootDir);
  if (!rootStat.isDirectory()) {
    throw new Error(`readDirectoryAsFiles: ${rootDir} is not a directory`);
  }
  const files: Record<string, Uint8Array> = {};
  await walk(rootDir, rootDir, files);
  return files;
}

async function walk(rootDir: string, currentDir: string, out: Record<string, Uint8Array>): Promise<void> {
  const entries = await readdir(currentDir, { withFileTypes: true });
  for (const dirent of entries) {
    const full = join(currentDir, dirent.name);
    if (dirent.isSymbolicLink()) {
      continue;
    }
    if (dirent.isDirectory()) {
      await walk(rootDir, full, out);
      continue;
    }
    if (!dirent.isFile()) {
      continue;
    }
    const rel = relative(rootDir, full);
    const posixPath = sep === "/" ? rel : rel.split(sep).join(posix.sep);
    out[posixPath] = await readFile(full);
  }
}
