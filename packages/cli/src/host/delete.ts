/**
 * `aex delete <session-id>` — DELETE /api/sessions/{id}.
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  describeApiError,
  emitJsonError,
  makeHttpClient,
  rejectUnknownFlags,
  resolveCommonHostFlags,
  refuseInsideManagedSession
} from "./common.js";

export async function executeDeleteCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "delete")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const usage = "usage: aex delete <session-id> [common flags]";
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
    await operations.deleteSession(http, sessionId);
    io.stdout(JSON.stringify({ sessionId, deleted: true }) + "\n");
    return SUCCESS;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "delete_failed", d.message, {
      sessionId,
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}
