/**
 * `aex cancel <run-id>` — POST /api/runs/{id}/cancel.
 */
import { operations } from "@aexhq/contracts";
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

export async function runCancelCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "cancel")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const positional = common.rest.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex cancel <run-id> [common flags]\n");
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  try {
    await operations.cancelRun(http, runId);
    io.stdout(JSON.stringify({ runId, status: "cancel_requested" }) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitJsonError(io, "cancel_failed", (err as Error).message ?? "cancel failed", { runId });
  }
}
