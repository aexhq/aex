/**
 * `antpath delete-asset <assetId|hash>` — DELETE /assets/{assetId}.
 *
 * Removes a workspace asset blob from the shared content-addressed store.
 * Runs that already snapshotted the asset into their own prefix are
 * unaffected. Accepts `sha256:<hex>` or a bare 64-hex digest.
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

export async function runDeleteAssetCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "delete-asset")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const positional = common.rest.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: antpath delete-asset <hash> [common flags]\n");
    return USAGE_ERR;
  }
  const hash = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  try {
    await operations.deleteWorkspaceAsset(http, hash);
    io.stdout(JSON.stringify({ hash, deleted: true }) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitJsonError(io, "delete_asset_failed", (err as Error).message ?? "delete-asset failed", { hash });
  }
}
