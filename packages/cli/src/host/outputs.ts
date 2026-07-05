/**
 * `aex outputs` — a THIN pass-through over the SDK's outputs accessor
 * (`aex.sessions.outputs(id)` / cross-run `aex.outputs`), so the CLI mirrors the
 * SDK's per-file output surface 1:1 instead of only listing:
 *
 *   aex outputs <session-id>                          List captured outputs (NDJSON)
 *   aex outputs read <session-id> <path>              Read one file as capped text (JSON)
 *   aex outputs download <session-id> <path> [--out]  Download one file's raw bytes
 *   aex outputs link <session-id> <path>              Mint a temporary download URL (JSON)
 *   aex outputs find <session-id> [--name S] [--ext E] [--type T] [--content-type CT]
 *   aex outputs search [--query S] [--name S] [--ext E] [--content-type CT] [--run-id ID] [--limit N]
 *
 * `aex outputs search` (no session id) is the CROSS-RUN metadata search
 * (`aex.outputs.search`); the whole-namespace zip stays `aex download`.
 */
import { Aex, type OutputFileType, type OutputQuery, type OutputSearchQuery } from "@aexhq/sdk";
import { resolve as resolvePath } from "node:path";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  type CommonHostFlags,
  SUCCESS,
  USAGE_ERR,
  collectRepeated,
  describeApiError,
  emitJsonError,
  refuseInsideManagedRun,
  resolveCommonHostFlags,
  takeFlagValue
} from "./common.js";

const SUBVERBS = new Set(["read", "download", "link", "find", "search"]);

export async function runOutputsCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "outputs")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const args = common.rest;
  const sub = args[0];
  const aex = new Aex({ baseUrl: common.flags.aexUrl, apiKey: common.flags.apiKey, fetch: io.fetchImpl });

  switch (sub) {
    case "read":
      return outputsRead(io, aex, args.slice(1));
    case "download":
      return outputsDownload(io, aex, args.slice(1), common.flags);
    case "link":
      return outputsLink(io, aex, args.slice(1));
    case "find":
      return outputsFind(io, aex, args.slice(1));
    case "search":
      return outputsSearch(io, aex, args.slice(1));
    default:
      return outputsList(io, aex, args);
  }
}

/** `aex outputs <session-id>` — list every captured output as NDJSON. */
async function outputsList(io: CliIO, aex: Aex, args: readonly string[]): Promise<CliExitCode> {
  const positional = args.filter((a) => !a.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex outputs <session-id> [common flags]\n");
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;
  try {
    const outputs = await aex.sessions.outputs(sessionId).list();
    for (const out of outputs) io.stdout(JSON.stringify(out) + "\n");
    return SUCCESS;
  } catch (err) {
    return outputsError(io, "outputs_failed", err, { sessionId });
  }
}

/** `aex outputs read <session-id> <path>` — read one file as capped text. */
async function outputsRead(io: CliIO, aex: Aex, args: readonly string[]): Promise<CliExitCode> {
  const positional = args.filter((a) => !a.startsWith("--"));
  if (positional.length !== 2) {
    io.stderr("usage: aex outputs read <session-id> <path> [common flags]\n");
    return USAGE_ERR;
  }
  const [sessionId, selector] = positional as [string, string];
  try {
    const text = await aex.sessions.outputs(sessionId).read({ path: selector });
    io.stdout(JSON.stringify(text) + "\n");
    return SUCCESS;
  } catch (err) {
    return outputsError(io, "outputs_read_failed", err, { sessionId, path: selector });
  }
}

/** `aex outputs download <session-id> <path> [--out file]` — one file's raw bytes. */
async function outputsDownload(io: CliIO, aex: Aex, args: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  const outFlag = takeFlagValue(args, "--out");
  if (outFlag.error) { io.stderr(`${outFlag.error}\n`); return USAGE_ERR; }
  void flags;
  const positional = outFlag.remaining.filter((a) => !a.startsWith("--"));
  if (positional.length !== 2) {
    io.stderr("usage: aex outputs download <session-id> <path> [--out file] [common flags]\n");
    return USAGE_ERR;
  }
  const [sessionId, selector] = positional as [string, string];
  let bytes: Uint8Array;
  try {
    bytes = await aex.sessions.outputs(sessionId).download({ path: selector });
  } catch (err) {
    return outputsError(io, "outputs_download_failed", err, { sessionId, path: selector });
  }
  const destination = resolvePath(io.cwd(), outFlag.value ?? baseName(selector));
  try {
    await io.writeFile(destination, bytes);
  } catch (err) {
    return emitJsonError(io, "write_failed", `failed to write file: ${(err as Error).message}`, { destination });
  }
  io.stdout(JSON.stringify({ sessionId, path: selector, out: destination, bytes: bytes.byteLength }) + "\n");
  return SUCCESS;
}

/** `aex outputs link <session-id> <path>` — mint a temporary download URL. */
async function outputsLink(io: CliIO, aex: Aex, args: readonly string[]): Promise<CliExitCode> {
  const positional = args.filter((a) => !a.startsWith("--"));
  if (positional.length !== 2) {
    io.stderr("usage: aex outputs link <session-id> <path> [common flags]\n");
    return USAGE_ERR;
  }
  const [sessionId, selector] = positional as [string, string];
  try {
    const link = await aex.sessions.outputs(sessionId).link({ path: selector });
    io.stdout(JSON.stringify(link) + "\n");
    return SUCCESS;
  } catch (err) {
    return outputsError(io, "outputs_link_failed", err, { sessionId, path: selector });
  }
}

/** `aex outputs find <session-id> [--name S] [--ext E] [--type T] [--content-type CT]`. */
async function outputsFind(io: CliIO, aex: Aex, args: readonly string[]): Promise<CliExitCode> {
  const name = takeFlagValue(args, "--name");
  const ext = takeFlagValue(name.remaining, "--ext");
  const type = takeFlagValue(ext.remaining, "--type");
  const contentType = takeFlagValue(type.remaining, "--content-type");
  const err = name.error ?? ext.error ?? type.error ?? contentType.error;
  if (err) { io.stderr(`${err}\n`); return USAGE_ERR; }
  const positional = contentType.remaining.filter((a) => !a.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex outputs find <session-id> [--name S] [--ext E] [--type T] [--content-type CT] [common flags]\n");
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;
  const query: OutputQuery = {
    ...(name.value !== null ? { filename: name.value } : {}),
    ...(ext.value !== null ? { extension: ext.value } : {}),
    ...(type.value !== null ? { type: type.value as OutputFileType } : {}),
    ...(contentType.value !== null ? { contentType: contentType.value } : {})
  };
  try {
    const hits = await aex.sessions.outputs(sessionId).find(query);
    for (const hit of hits) io.stdout(JSON.stringify(hit) + "\n");
    return SUCCESS;
  } catch (err2) {
    return outputsError(io, "outputs_find_failed", err2, { sessionId });
  }
}

/** `aex outputs search [--query S] [--name S] [--ext E] [--content-type CT] [--run-id ID] [--limit N]` — cross-run. */
async function outputsSearch(io: CliIO, aex: Aex, args: readonly string[]): Promise<CliExitCode> {
  const query = takeFlagValue(args, "--query");
  const name = takeFlagValue(query.remaining, "--name");
  const ext = takeFlagValue(name.remaining, "--ext");
  const contentType = takeFlagValue(ext.remaining, "--content-type");
  const limit = takeFlagValue(contentType.remaining, "--limit");
  const runIds = collectRepeated(limit.remaining, "--run-id");
  const err = query.error ?? name.error ?? ext.error ?? contentType.error ?? limit.error ?? runIds.error;
  if (err) { io.stderr(`${err}\n`); return USAGE_ERR; }
  // `--query`/`--name` are filename substring matches (metadata-only search).
  const filename = query.value ?? name.value;
  let limitValue: number | undefined;
  if (limit.value !== null) {
    const n = Number(limit.value);
    if (!Number.isInteger(n) || n < 1) {
      io.stderr(`--limit must be a positive integer (got: ${limit.value})\n`);
      return USAGE_ERR;
    }
    limitValue = n;
  }
  const search: OutputSearchQuery = {
    ...(filename !== null ? { filename } : {}),
    ...(ext.value !== null ? { extension: ext.value } : {}),
    ...(contentType.value !== null ? { contentType: contentType.value } : {}),
    ...(runIds.values.length > 0 ? { runIds: [...runIds.values] } : {}),
    ...(limitValue !== undefined ? { limit: limitValue } : {})
  };
  try {
    const page = await aex.outputs.search(search);
    io.stdout(JSON.stringify(page) + "\n");
    return SUCCESS;
  } catch (err2) {
    return outputsError(io, "outputs_search_failed", err2, {});
  }
}

function outputsError(io: CliIO, code: string, err: unknown, extra: Record<string, unknown>): CliExitCode {
  const d = describeApiError(err);
  return emitJsonError(io, code, d.message, {
    ...extra,
    ...(d.status !== undefined ? { status: d.status } : {}),
    ...(d.remedy ? { remedy: d.remedy } : {})
  });
}

function baseName(p: string): string {
  const trimmed = p.replace(/[\\/]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return i >= 0 ? trimmed.slice(i + 1) : trimmed;
}
