/**
 * `aex whoami` — resolve the API key to its workspace + scopes via the public
 * whoami operation. Lets agents confirm the key before submitting a real session.
 * Always emits JSON; `--json` is a globally-recognized no-op flag (consumed by
 * the common-flags parser) so `aex whoami --json` never errors.
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  emitApiError,
  makeHttpClient,
  prepareHostCommand
} from "./common.js";

export async function executeWhoamiCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "whoami", auth: "data" });
  if (!common.ok) return common.exit;
  if (common.rest.length > 0) {
    io.stderr(`unexpected arguments: ${common.rest.join(" ")}\n`);
    return USAGE_ERR;
  }

  const http = makeHttpClient(io, common.flags);
  try {
    const me = await operations.whoami(http);
    io.stdout(JSON.stringify(me) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "whoami_failed", err);
  }
}
