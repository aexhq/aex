import { readFile } from "node:fs/promises";
import type { BundleMeta } from "./bundle.js";
import { walkDirectory, type IgnoreOptions } from "./node-walk.js";

/**
 * Fidelity-aware read of a local skill/tool directory into an in-memory files map
 * plus its bundle metadata. Delegates to the shared {@link walkDirectory} (the
 * SAME fidelity walk `File.fromPath` uses), so skills/tools now honor `.aexignore`
 * + the always-on defaults (`.git/`, `node_modules/`, …), capture executable bits
 * + symlinks into `meta` (destined for the `.aexmeta.json` sidecar), and never
 * silently vanish a non-regular file (surfaced in `dropped`).
 *
 * INVARIANT: for a directory with NO exec bits, NO symlinks, and NO ignored paths,
 * `meta` is empty → the downstream bundler emits NO sidecar, so the bundle bytes
 * are byte-identical to the pre-fidelity `readDirectoryAsFiles` output (dedup
 * continuity). Only metadata (or an `.aexignore`/ignored path) changes the bytes.
 *
 * Bun/Node filesystem runtimes only. Browser callers pass a pre-built files map to
 * `bundleSkillFiles` / `bundleToolFiles` directly and never reach this module.
 */
export interface DirectoryBundle {
  /** Regular-file bytes, keyed by forward-slash bundle-relative path. */
  readonly files: Record<string, Uint8Array>;
  /** Exec bits + symlinks → the `.aexmeta.json` sidecar (empty ⇒ no sidecar emitted). */
  readonly meta: BundleMeta;
  /** Non-regular files skipped (FIFO/socket/device) — never silently vanished. Sorted. */
  readonly dropped: readonly string[];
  /** Count of paths pruned by the ignore layers (`.aexignore` + defaults). */
  readonly ignoredCount: number;
}

export async function readDirectoryWithFidelity(rootDir: string, ignore?: IgnoreOptions): Promise<DirectoryBundle> {
  if (typeof rootDir !== "string" || !rootDir) {
    throw new Error("readDirectoryWithFidelity: rootDir is required");
  }
  const walk = await walkDirectory(rootDir, ignore);
  const files: Record<string, Uint8Array> = {};
  for (const entry of walk.entries) {
    files[entry.rel] = new Uint8Array(await readFile(entry.absPath));
  }
  return {
    files,
    meta: { exec: walk.exec, symlinks: walk.symlinks },
    dropped: walk.dropped,
    ignoredCount: walk.ignoredCount
  };
}
