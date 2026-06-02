/**
 * `antpath status <run-id>` — fetch a single run record via GET
 * /api/runs/{id} and print it as JSON.
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

export async function runStatusCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "status")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const positional = common.rest.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: antpath status <run-id> [common flags]\n");
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  try {
    const run = await operations.getRun(http, runId);
    io.stdout(JSON.stringify(run) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitJsonError(io, "status_failed", (err as Error).message ?? "status fetch failed", { runId });
  }
}
