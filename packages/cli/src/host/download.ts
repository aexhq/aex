/**
 * `aex download <run-id> [--only namespace] [--out path]` — download
 * a run's content as a zip, assembled client-side from the public read
 * endpoints (no per-output id required).
 *
 * Without `--only`, downloads everything public — organised into
 * `metadata/run.json`, typed `events/events.jsonl`, `outputs/<rel>`
 * (deliverables), plus `manifest.json`.
 *
 * `--only outputs|events|metadata` downloads just that one
 * namespace (files at the zip root).
 *
 * `--out` resolves relative to the host CWD; if omitted the file is
 * written to `aex-run-<run-id>[-<namespace>].zip` in the current
 * directory.
 */
import { resolve as resolvePath } from "node:path";
import { operations } from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  emitJsonError,
  makeHttpClient,
  parseCommonHostFlags,
  refuseInsideManagedRun,
  takeFlagValue
} from "./common.js";

type Namespace = "outputs" | "events" | "metadata";

const NAMESPACE_DOWNLOADERS = {
  outputs: operations.downloadOutputs,
  events: operations.downloadEvents,
  metadata: operations.downloadMetadata
} satisfies Record<Namespace, typeof operations.download>;

export async function runDownloadCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "download")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const outFlag = takeFlagValue(common.rest, "--out");
  if (outFlag.error) {
    io.stderr(`${outFlag.error}\n`);
    return USAGE_ERR;
  }
  const onlyFlag = takeFlagValue(outFlag.remaining, "--only");
  if (onlyFlag.error) {
    io.stderr(`${onlyFlag.error}\n`);
    return USAGE_ERR;
  }
  if (onlyFlag.value !== null && !(onlyFlag.value in NAMESPACE_DOWNLOADERS)) {
    io.stderr(`--only must be one of: ${Object.keys(NAMESPACE_DOWNLOADERS).join(", ")}\n`);
    return USAGE_ERR;
  }
  const namespace = onlyFlag.value as Namespace | null;

  const positional = onlyFlag.remaining.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex download <run-id> [--only outputs|events|metadata] [--out path] [common flags]\n");
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  const downloader = namespace ? NAMESPACE_DOWNLOADERS[namespace] : operations.download;

  let bytes: Uint8Array;
  try {
    bytes = await downloader(http, runId);
  } catch (err) {
    return emitJsonError(io, "download_failed", (err as Error).message ?? "download failed", { runId });
  }

  const destination = resolveDestination(io, outFlag.value, runId, namespace);
  try {
    await io.writeFile(destination, bytes);
  } catch (err) {
    return emitJsonError(io, "write_failed", `failed to write archive: ${(err as Error).message}`, { destination });
  }

  io.stdout(JSON.stringify({ runId, namespace: namespace ?? "all", path: destination, bytes: bytes.byteLength }) + "\n");
  return SUCCESS;
}

function resolveDestination(io: CliIO, out: string | null, runId: string, namespace: Namespace | null): string {
  if (out) {
    return resolvePath(io.cwd(), out);
  }
  const suffix = namespace ? `-${namespace}` : "";
  return resolvePath(io.cwd(), `aex-run-${runId}${suffix}.zip`);
}
