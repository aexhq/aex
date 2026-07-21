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
  resolveCommonHostFlags,
  refuseInsideManagedSession
} from "./common.js";

export async function executeWhoamiCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "whoami")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
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
