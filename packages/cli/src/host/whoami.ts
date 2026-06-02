/**
 * `antpath whoami` — GET /api/whoami. Lets agents confirm the token
 * resolves to a workspace + scopes before submitting a real run.
 *
 * `--workspace` is NOT required: the whoami endpoint resolves the
 * principal by the bearer alone and tells the caller which workspace
 * the token belongs to.
 */
import { operations } from "@antpath/contracts";
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

export async function runWhoamiCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "whoami")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
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
    return emitJsonError(io, "whoami_failed", (err as Error).message ?? "whoami failed");
  }
}
