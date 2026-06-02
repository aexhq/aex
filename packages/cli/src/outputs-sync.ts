/**
 * antpath outputs sync — IN-CONTAINER ONLY internal subcommand.
 *
 * This is NOT a user-facing verb. The platform worker drives a
 * synthetic agent turn at session terminal that tells the in-
 * container agent to run:
 *
 *   node /mnt/session/uploads/antpath/antpath outputs sync /mnt/session/outputs
 *
 * The agent runs this via its bash tool. The CLI walks each directory
 * and emits a structured JSON line per file to stdout, so the worker
 * (observing the agent's `agent.tool_result` event) has a deterministic
 * record of what files exist. The bash output also acts as a hint to
 * Anthropic Managed Agents' built-in file-registration so the bytes
 * become listable via the Files API at terminal. Only directories under
 * `/mnt/session/...` are auto-registered by Anthropic, so the worker's
 * synthetic message should keep `outputDirs` inside that tree.
 *
 * The subcommand:
 *  - REFUSES to run outside a managed run (no ANTPATH_INDEX_PATH file).
 *  - Walks each provided directory recursively.
 *  - Emits one JSON line per file to stdout:
 *      {"dir":"/mnt/session/outputs","path":"/mnt/session/outputs/x.txt","sizeBytes":42}
 *  - Reports missing dirs via stderr (non-fatal). Exits 0 if at least
 *    one dir contained at least one file; 0 also when ALL dirs were
 *    empty (best-effort semantics — the worker still records the
 *    capture attempt either way).
 *
 * No flags. Each positional argument is one directory to walk.
 */
import { ANTPATH_INDEX_PATH, type CliIO } from "./internal.js";
import { RUNTIME_ERR, SUCCESS, USAGE_ERR, type CliExitCode } from "./host/common.js";

export async function runOutputsSyncCmd(io: CliIO, dirs: readonly string[]): Promise<CliExitCode> {
  if (dirs.length === 0) {
    io.stderr("usage: antpath outputs sync <dir> [<dir> ...]\n");
    return USAGE_ERR;
  }
  try {
    await io.readFile(ANTPATH_INDEX_PATH);
  } catch {
    io.stderr(
      "`antpath outputs sync` is an in-container internal command and cannot run on the host.\n"
    );
    return USAGE_ERR;
  }
  if (!io.walkDirectory) {
    io.stderr("antpath outputs sync: walkDirectory IO is not available\n");
    return RUNTIME_ERR;
  }

  let scanned = 0;
  let missing = 0;
  for (const dir of dirs) {
    if (!dir.startsWith("/")) {
      io.stderr(
        JSON.stringify({ dir, error: "non_absolute_path", message: "skipping non-absolute output dir" }) + "\n"
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
