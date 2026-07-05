/**
 * `aex whoami` — resolve the API key to its workspace + scopes via the SDK's
 * `Aex.whoami()`. Lets agents confirm the key before submitting a real run.
 * Always emits JSON; `--json` is a globally-recognized no-op flag (consumed by
 * the common-flags parser) so `aex whoami --json` never errors.
 */
import { Aex } from "@aexhq/sdk";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  describeApiError,
  emitJsonError,
  resolveCommonHostFlags,
  refuseInsideManagedRun
} from "./common.js";

export async function runWhoamiCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "whoami")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  if (common.rest.length > 0) {
    io.stderr(`unexpected arguments: ${common.rest.join(" ")}\n`);
    return USAGE_ERR;
  }

  const aex = new Aex({ baseUrl: common.flags.aexUrl, apiKey: common.flags.apiKey, fetch: io.fetchImpl });
  try {
    const me = await aex.whoami();
    io.stdout(JSON.stringify(me) + "\n");
    return SUCCESS;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "whoami_failed", d.message, {
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}
