/**
 * `aex outputs <run-id>` — list captured outputs for a run.
 * Prints one Output as JSON per line.
 *
 * `aex download <run-id> <output-id> [--out path]` is in a
 * separate file (download.ts) to keep concerns tight.
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

export async function runOutputsCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "outputs")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const positional = common.rest.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex outputs <run-id> [common flags]\n");
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  try {
    const outputs = await operations.listOutputs(http, runId);
    for (const out of outputs) {
      io.stdout(JSON.stringify(out) + "\n");
    }
    return SUCCESS;
  } catch (err) {
    return emitJsonError(io, "outputs_failed", (err as Error).message ?? "outputs fetch failed", { runId });
  }
}
