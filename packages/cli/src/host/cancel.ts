/**
 * `aex cancel <session-id>` — POST /api/sessions/{id}/cancel.
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  emitApiError,
  makeHttpClient,
  rejectUnknownFlags,
  resolveCommonHostFlags,
  refuseInsideManagedSession
} from "./common.js";

export async function executeCancelCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "cancel")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const usage = "usage: aex cancel <session-id> [common flags]";
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
    const accepted = await operations.cancelSession(http, sessionId);
    io.stdout(JSON.stringify({ sessionId, status: accepted.session.status }) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "cancel_failed", err, { sessionId });
  }
}
