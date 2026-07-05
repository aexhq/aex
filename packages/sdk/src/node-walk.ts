/**
 * Node/Bun-only directory walk with fidelity capture + `.aexignore` filtering.
 *
 * Shared by `File.fromPath` (customer files) and `readDirectoryAsFiles`
 * (skills/tools). Captures, in ONE deterministic walk:
 *   - regular files (bytes),
 *   - executable bits (`mode & 0o111` → normalised 0o755 bucket, listed in `exec`),
 *   - symlinks (raw `readlink` target, verbatim — the SDK captures faithfully; the
 *     container restore decides whether a target may be recreated),
 *   - dropped non-regular files (FIFO / socket / char / block device) — never
 *     silently vanished, surfaced in `dropped`.
 *
 * Ignore semantics are gitignore-compatible via the `ignore` package: always-on
 * built-in {@link DEFAULT_IGNORES} (`.git/`, `node_modules/`, …), `.gitignore`
 * honored only when opted in, root `.aexignore` on by default and highest
 * precedence, plus programmatic `extraPatterns`. The ignored SET is a pure
 * function of the layered patterns + the sorted path list, so the resulting
 * bundle is deterministic. An ignored directory is pruned wholesale (never
 * descended) — `node_modules` is never walked.
 *
 * Node-only: imports `node:fs` + `ignore`. Browser callers pass pre-built maps to
 * the bundlers and never reach this module.
 */

import { readFile, readdir, readlink, stat } from "node:fs/promises";
import { join, posix, relative, sep } from "node:path";
import ignore from "ignore";
import { MAX_SYMLINK_TARGET_LENGTH, RESERVED_META_ENTRY, type BundleSymlink } from "@aexhq/contracts";

type Ignore = ReturnType<typeof ignore>;

/** Caller-facing ignore controls, threaded from `File.fromPath` / `Tool.fromPath` / skill factories. */
export interface IgnoreOptions {
  /** Apply the always-on built-in {@link DEFAULT_IGNORES}. Default true. */
  readonly useDefaults?: boolean;
  /** Also honor the root `.gitignore`. Default false (least-surprise: no silent data loss). */
  readonly useGitignore?: boolean;
  /** Honor the root `.aexignore`. Default true. */
  readonly useAexignore?: boolean;
  /** Extra patterns appended LAST (highest precedence). */
  readonly extraPatterns?: readonly string[];
}

/**
 * Always-on ignore defaults (unless `useDefaults:false`): VCS metadata, dependency
 * caches, editor/OS cruft, plus the two aex control files (`.aexignore` and the
 * reserved sidecar). `.gitignore` itself is real repo content and is NOT excluded.
 */
export const DEFAULT_IGNORES: readonly string[] = [
  ".git/",
  ".hg/",
  ".svn/",
  "node_modules/",
  "__pycache__/",
  "*.pyc",
  "*.pyo",
  ".DS_Store",
  "Thumbs.db"
];

/** aex control files always excluded from bundle content (regardless of `useDefaults`). */
const CONTROL_IGNORES: readonly string[] = [".aexignore", RESERVED_META_ENTRY];

export interface WalkedEntry {
  /** Forward-slash bundle-relative path. */
  readonly rel: string;
  /** Absolute on-disk path (for lazy reads on the streaming path). */
  readonly absPath: string;
  /** Raw byte size from `stat` (drives the small-vs-streaming path decision). */
  readonly size: number;
}

export interface WalkResult {
  /** Regular-file descriptors (bytes NOT yet read), globally sorted by `rel`. */
  readonly entries: WalkedEntry[];
  /** Bundle-relative paths that restore executable (0o755). Sorted. */
  readonly exec: string[];
  /** Captured symlinks (raw target). Sorted by path. */
  readonly symlinks: BundleSymlink[];
  /** Non-regular files skipped (FIFO/socket/device), for a non-silent summary. Sorted. */
  readonly dropped: string[];
  /** Count of files/dirs excluded by the ignore layers (surfaced, not silent). */
  readonly ignoredCount: number;
  /** Sum of `entries[i].size` — the total raw input size. */
  readonly totalSize: number;
}

/** Read a root-level ignore control file's patterns (CRLF-normalised), or [] when absent. */
async function readIgnoreFile(rootDir: string, name: string): Promise<string[]> {
  try {
    const raw = await readFile(join(rootDir, name), "utf8");
    return raw.replace(/\r\n/g, "\n").split("\n");
  } catch {
    return [];
  }
}

/** Build the layered ignore matcher (last layer wins, git semantics). */
async function buildIgnore(rootDir: string, options: IgnoreOptions | undefined): Promise<Ignore> {
  const ig = ignore();
  if (options?.useDefaults !== false) ig.add([...DEFAULT_IGNORES]);
  ig.add([...CONTROL_IGNORES]);
  if (options?.useGitignore === true) ig.add(await readIgnoreFile(rootDir, ".gitignore"));
  if (options?.useAexignore !== false) ig.add(await readIgnoreFile(rootDir, ".aexignore"));
  if (options?.extraPatterns && options.extraPatterns.length > 0) ig.add([...options.extraPatterns]);
  return ig;
}

function toPosixRel(rootDir: string, full: string): string {
  const rel = relative(rootDir, full);
  return sep === "/" ? rel : rel.split(sep).join(posix.sep);
}

/**
 * Walk `rootDir`, capturing files + fidelity metadata + applying the ignore
 * layers. Deterministic: each directory level is read then processed in sorted
 * order, and the returned metadata arrays are sorted.
 */
export async function walkDirectory(rootDir: string, options?: IgnoreOptions): Promise<WalkResult> {
  const ig = await buildIgnore(rootDir, options);
  const entries: WalkedEntry[] = [];
  const exec: string[] = [];
  const symlinks: BundleSymlink[] = [];
  const dropped: string[] = [];
  let ignoredCount = 0;
  let totalSize = 0;

  const visit = async (currentDir: string): Promise<void> => {
    const dirents = await readdir(currentDir, { withFileTypes: true });
    // Sort each level for a deterministic ignored-set + walk order.
    dirents.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
    for (const dirent of dirents) {
      const full = join(currentDir, dirent.name);
      const rel = toPosixRel(rootDir, full);

      if (dirent.isSymbolicLink()) {
        if (ig.ignores(rel)) {
          ignoredCount += 1;
          continue;
        }
        const target = await readlink(full);
        if (target.includes("\0")) {
          throw new Error(`File.fromPath: symlink ${JSON.stringify(rel)} target contains a NUL byte`);
        }
        if (target.length > MAX_SYMLINK_TARGET_LENGTH) {
          throw new Error(
            `File.fromPath: symlink ${JSON.stringify(rel)} target exceeds ${MAX_SYMLINK_TARGET_LENGTH} bytes`
          );
        }
        symlinks.push({ path: rel, target });
        continue;
      }

      if (dirent.isDirectory()) {
        // Directory pruning (git-correct + fast): an ignored dir is never descended.
        if (ig.ignores(`${rel}/`)) {
          ignoredCount += 1;
          continue;
        }
        await visit(full);
        continue;
      }

      if (!dirent.isFile()) {
        // FIFO / socket / char / block device — skipped, never silently vanished.
        dropped.push(rel);
        continue;
      }

      if (ig.ignores(rel)) {
        ignoredCount += 1;
        continue;
      }
      const st = await stat(full);
      entries.push({ rel, absPath: full, size: st.size });
      totalSize += st.size;
      if ((st.mode & 0o111) !== 0) exec.push(rel);
    }
  };

  await visit(rootDir);

  // Global lexicographic order (per-level sort is NOT globally sorted across the
  // tree, e.g. `a.txt` vs `a/b.txt`), matching the canonical zip entry order.
  entries.sort((a, b) => byString(a.rel, b.rel));
  exec.sort(byString);
  symlinks.sort((a, b) => byString(a.path, b.path));
  dropped.sort(byString);
  return { entries, exec, symlinks, dropped, ignoredCount, totalSize };
}

function byString(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}
