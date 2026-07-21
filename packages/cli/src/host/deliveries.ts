/**
 * `aex deliveries <session-id>` — list a session's webhook delivery attempts
 * via GET /api/sessions/{id}/webhook-deliveries and print them as JSON. (The
 * delivery ledger is keyed by the session id; the endpoint keeps its
 * session-namespaced path, matching the SDK's `session.webhooks.list()`.)
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  emitApiError,
  makeHttpClient,
  prepareHostCommand,
  rejectUnknownFlags,
} from "./common.js";

export async function executeDeliveriesCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "deliveries", auth: "data" });
  if (!common.ok) return common.exit;
  const usage = "usage: aex deliveries <session-id> [common flags]";
  const unknown = rejectUnknownFlags(io, common.rest, usage);
  if (unknown) return unknown;
  const positional = common.rest;
  if (positional.length !== 1) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  try {
    const deliveries = await operations.getSessionWebhookDeliveries(http, sessionId);
    io.stdout(JSON.stringify(deliveries) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "deliveries_failed", err, { sessionId });
  }
}
