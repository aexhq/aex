/**
 * `antpath delete <run-id>` — DELETE /api/runs/{id}.
 */
import { operations } from "@antpath/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  emitJsonError,
  makeHttpClient,
  parseCommonHostFlags,
  refuseInsideManagedRun
} from "./common.js";

export async function runDeleteCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "delete")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const positional = common.rest.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: antpath delete <run-id> [common flags]\n");
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  try {
    await operations.deleteRun(http, runId);
    io.stdout(JSON.stringify({ runId, deleted: true }) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitJsonError(io, "delete_failed", (err as Error).message ?? "delete failed", { runId });
  }
}
