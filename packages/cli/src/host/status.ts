/**
 * `aex status <session-id>` — fetch a single session record via GET
 * /api/sessions/{id} and print it as JSON.
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

export async function executeStatusCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "status", auth: "data" });
  if (!common.ok) return common.exit;
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
    return emitApiError(io, "status_failed", err, { sessionId });
  }
}
