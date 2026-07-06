/**
 * `aex status <session-id>` — fetch a single session record via GET
 * /api/sessions/{id} and print it as JSON.
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
  rejectUnknownFlags,
  resolveCommonHostFlags,
  refuseInsideManagedRun
} from "./common.js";

export async function runStatusCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "status")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const usage = "usage: aex status <session-id> [common flags]";
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
    const session = await operations.getSession(http, sessionId);
    io.stdout(JSON.stringify(session) + "\n");
    return SUCCESS;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "status_failed", d.message, {
      sessionId,
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}
