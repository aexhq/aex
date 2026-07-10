/**
 * aex files sync — IN-CONTAINER ONLY internal subcommand.
 *
 * This is NOT a user-facing verb. It is a legacy/internal directory walker:
 * callers pass explicit absolute directories, and the command emits a
 * structured JSON line per file to stdout. Managed sessions now capture files from
 * checkpointed workspace state; there is no default file directory and no
 * synthetic terminal agent turn.
 *
 * The subcommand:
 *  - REFUSES to run outside a managed session (no AEX_INDEX_PATH file).
 *  - Walks each provided directory recursively.
 *  - Emits one JSON line per file to stdout:
 *      {"dir":"/workspace/reports","path":"/workspace/reports/x.txt","sizeBytes":42}
 *  - Reports missing dirs via stderr (non-fatal). Exits 0 if at least
 *    one dir contained at least one file; 0 also when ALL dirs were
 *    empty (best-effort semantics — the hosted runtime still records the
 *    capture attempt either way).
 *
 * No flags. Each positional argument is one directory to walk.
 */
import { AEX_INDEX_PATH, type CliIO } from "./internal.js";
import { RUNTIME_ERR, SUCCESS, USAGE_ERR, type CliExitCode } from "./host/common.js";

export async function executeFilesSyncCmd(io: CliIO, dirs: readonly string[]): Promise<CliExitCode> {
  if (dirs.length === 0) {
    io.stderr("usage: aex files sync <dir> [<dir> ...]\n");
    return USAGE_ERR;
  }
  try {
    await io.readFile(AEX_INDEX_PATH);
  } catch {
    io.stderr(
      "`aex files sync` is an in-container internal command and cannot run on the host.\n"
    );
    return USAGE_ERR;
  }
  if (!io.walkDirectory) {
    io.stderr("aex files sync: walkDirectory IO is not available\n");
    return RUNTIME_ERR;
  }

  let scanned = 0;
  let missing = 0;
  for (const dir of dirs) {
    if (!dir.startsWith("/")) {
      io.stderr(
        JSON.stringify({ dir, error: "non_absolute_path", message: "skipping non-absolute file dir" }) + "\n"
      );
      missing++;
      continue;
    }
    let entries;
    try {
      entries = await io.walkDirectory(dir);
    } catch (err) {
      io.stderr(
        JSON.stringify({ dir, error: "walk_failed", message: (err as Error).message ?? "walk failed" }) + "\n"
      );
      missing++;
      continue;
    }
    if (entries === null) {
      io.stderr(JSON.stringify({ dir, error: "missing_or_unreadable" }) + "\n");
      missing++;
      continue;
    }
    for (const entry of entries) {
      io.stdout(JSON.stringify({ dir, path: entry.path, sizeBytes: entry.sizeBytes }) + "\n");
      scanned++;
    }
  }

  io.stdout(JSON.stringify({ summary: { dirs: dirs.length, files: scanned, missing } }) + "\n");
  return SUCCESS;
}
