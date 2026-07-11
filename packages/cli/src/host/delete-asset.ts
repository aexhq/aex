/**
 * `aex delete-asset <assetId|hash>` - DELETE /api/assets/{assetId}.
 *
 * Removes a workspace asset blob from the shared content-addressed store.
 * Sessions that already snapshotted the asset into their own prefix are
 * unaffected. Accepts `sha256:<hex>` or a bare 64-hex digest.
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

export async function executeDeleteAssetCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "delete-asset")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const usage = "usage: aex delete-asset <hash> [common flags]";
  const unknown = rejectUnknownFlags(io, common.rest, usage);
  if (unknown) return unknown;
  const positional = common.rest;
  if (positional.length !== 1) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const hash = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  try {
    await operations.deleteWorkspaceAsset(http, hash);
    io.stdout(JSON.stringify({ hash, deleted: true }) + "\n");
    return SUCCESS;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "delete_asset_failed", d.message, {
      hash,
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}
