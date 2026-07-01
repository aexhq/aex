/**
 * `aex deliveries <session-id>` — list a session's webhook delivery attempts
 * via GET /api/runs/{id}/webhook-deliveries and print them as JSON. (The
 * delivery ledger is keyed by the session id; the endpoint keeps its
 * run-namespaced path, matching the SDK's `session.webhooks().list()`.)
 */
import { operations } from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  describeApiError,
  emitJsonError,
  makeHttpClient,
  resolveCommonHostFlags,
  refuseInsideManagedRun
} from "./common.js";

export async function runDeliveriesCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "deliveries")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const positional = common.rest.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex deliveries <session-id> [common flags]\n");
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  try {
    const deliveries = await operations.getRunWebhookDeliveries(http, sessionId);
    io.stdout(JSON.stringify(deliveries) + "\n");
    return SUCCESS;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "deliveries_failed", d.message, {
      sessionId,
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}
