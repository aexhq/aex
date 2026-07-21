/**
 * `aex download <session-id> [--only namespace] [--out path]` — download
 * a session's content as a zip, assembled client-side from the public read
 * endpoints (no per-file id required), matching the SDK's
 * `session.download()`.
 *
 * Without `--only`, downloads everything public — organised into
 * `metadata/session.json`, typed `events/events.jsonl`, `files/<rel>`,
 * plus `manifest.json`.
 *
 * `--only files|events|metadata` downloads just that one
 * namespace (files plus `manifest.json` at the zip root).
 *
 * `--out` resolves relative to the host CWD; if omitted the file is
 * written to `aex-session-<session-id>[-<namespace>].zip` in the current
 * directory.
 */
import { resolve as resolvePath } from "node:path";
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  emitApiError,
  emitJsonError,
  makeHttpClient,
  prepareHostCommand,
  rejectUnknownFlags,
  takeOptionFlag
} from "./common.js";

type Namespace = "files" | "events" | "metadata";

const NAMESPACE_DOWNLOADERS = {
  files: operations.downloadSessionFiles,
  events: operations.downloadEvents,
  metadata: operations.downloadMetadata
} satisfies Record<Namespace, typeof operations.download>;

export async function executeDownloadCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "download", auth: "data" });
  if (!common.ok) return common.exit;
  const outFlag = takeOptionFlag(common.rest, "--out");
  if (outFlag.error) {
    io.stderr(`${outFlag.error}\n`);
    return USAGE_ERR;
  }
  const onlyFlag = takeOptionFlag(outFlag.remaining, "--only");
  if (onlyFlag.error) {
    io.stderr(`${onlyFlag.error}\n`);
    return USAGE_ERR;
  }
  if (onlyFlag.value !== undefined && !Object.hasOwn(NAMESPACE_DOWNLOADERS, onlyFlag.value)) {
    io.stderr(`--only must be one of: ${Object.keys(NAMESPACE_DOWNLOADERS).join(", ")}\n`);
    return USAGE_ERR;
  }
  const namespace = onlyFlag.value as Namespace | undefined;

  const usage = "usage: aex download <session-id> [--only files|events|metadata] [--out path] [common flags]";
  const unknown = rejectUnknownFlags(io, onlyFlag.remaining, usage);
  if (unknown) return unknown;
  const positional = onlyFlag.remaining;
  if (positional.length !== 1) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  const downloader = namespace ? NAMESPACE_DOWNLOADERS[namespace] : operations.download;

  let bytes: Uint8Array;
  try {
    bytes = await downloader(http, sessionId);
  } catch (err) {
    return emitApiError(io, "download_failed", err, { sessionId });
  }

  const destination = resolveDestination(io, outFlag.value, sessionId, namespace);
  try {
    await io.writeFile(destination, bytes);
  } catch (err) {
    return emitJsonError(io, "write_failed", `failed to write archive: ${(err as Error).message}`, { destination });
  }

  io.stdout(JSON.stringify({ sessionId, namespace: namespace ?? "all", path: destination, bytes: bytes.byteLength }) + "\n");
  return SUCCESS;
}

function resolveDestination(io: CliIO, out: string | undefined, sessionId: string, namespace: Namespace | undefined): string {
  if (out) {
    return resolvePath(io.cwd(), out);
  }
  const suffix = namespace ? `-${namespace}` : "";
  return resolvePath(io.cwd(), `aex-session-${sessionId}${suffix}.zip`);
}
