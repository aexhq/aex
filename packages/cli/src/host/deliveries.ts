/**
 * `aex deliveries <run-id>` — list a run's webhook delivery attempts via
 * GET /api/runs/{id}/webhook-deliveries and print them as JSON.
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

export async function runDeliveriesCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "deliveries")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const positional = common.rest.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex deliveries <run-id> [common flags]\n");
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  try {
    const deliveries = await operations.getRunWebhookDeliveries(http, runId);
    io.stdout(JSON.stringify(deliveries) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitJsonError(io, "deliveries_failed", (err as Error).message ?? "deliveries fetch failed", { runId });
  }
}
