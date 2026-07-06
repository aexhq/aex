/**
 * `aex outputs` — host CLI wrappers over the public output operations:
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
import { operations, type HttpClient, type Output, type OutputFileType, type OutputQuery, type OutputSearchHit, type OutputSearchQuery } from "@aexhq/contracts";
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
  makeHttpClient,
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
  const http = makeHttpClient(io, common.flags);

  switch (sub) {
    case "read":
      return outputsRead(io, http, args.slice(1));
    case "download":
      return outputsDownload(io, http, args.slice(1), common.flags);
    case "link":
      return outputsLink(io, http, args.slice(1));
    case "find":
      return outputsFind(io, http, args.slice(1));
    case "search":
      return outputsSearch(io, http, args.slice(1));
    default:
      return outputsList(io, http, args);
  }
}

/** `aex outputs <session-id>` — list every captured output as NDJSON. */
async function outputsList(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
  const positional = args.filter((a) => !a.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex outputs <session-id> [common flags]\n");
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;
  try {
    const outputs = await operations.listSessionOutputs(http, sessionId);
    for (const out of outputs) io.stdout(JSON.stringify(out) + "\n");
    return SUCCESS;
  } catch (err) {
    return outputsError(io, "outputs_failed", err, { sessionId });
  }
}

/** `aex outputs read <session-id> <path>` — read one file as capped text. */
async function outputsRead(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
  const positional = args.filter((a) => !a.startsWith("--"));
  if (positional.length !== 2) {
    io.stderr("usage: aex outputs read <session-id> <path> [common flags]\n");
    return USAGE_ERR;
  }
  const [sessionId, selector] = positional as [string, string];
  try {
    const text = await operations.readOutputText(http, sessionId, { path: selector });
    io.stdout(JSON.stringify(text) + "\n");
    return SUCCESS;
  } catch (err) {
    return outputsError(io, "outputs_read_failed", err, { sessionId, path: selector });
  }
}

/** `aex outputs download <session-id> <path> [--out file]` — one file's raw bytes. */
async function outputsDownload(io: CliIO, http: HttpClient, args: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
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
    bytes = (await operations.downloadOutput(http, sessionId, { path: selector })).bytes;
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
async function outputsLink(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
  const positional = args.filter((a) => !a.startsWith("--"));
  if (positional.length !== 2) {
    io.stderr("usage: aex outputs link <session-id> <path> [common flags]\n");
    return USAGE_ERR;
  }
  const [sessionId, selector] = positional as [string, string];
  try {
    const link = await operations.outputLink(http, sessionId, { path: selector });
    io.stdout(JSON.stringify(link) + "\n");
    return SUCCESS;
  } catch (err) {
    return outputsError(io, "outputs_link_failed", err, { sessionId, path: selector });
  }
}

/** `aex outputs find <session-id> [--name S] [--ext E] [--type T] [--content-type CT]`. */
async function outputsFind(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
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
    const hits = await searchSessionOutputs(http, sessionId, query);
    for (const hit of hits) io.stdout(JSON.stringify(hit) + "\n");
    return SUCCESS;
  } catch (err2) {
    return outputsError(io, "outputs_find_failed", err2, { sessionId });
  }
}

/** `aex outputs search [--query S] [--name S] [--ext E] [--content-type CT] [--run-id ID] [--limit N]` — cross-run. */
async function outputsSearch(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
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
    const page = await searchWorkspaceOutputs(http, search);
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

async function searchWorkspaceOutputs(
  http: HttpClient,
  query: OutputSearchQuery
): Promise<{ readonly hits: readonly OutputSearchHit[] }> {
  assertMetadataOnlyOutputSearch(query, "aex outputs search");
  const runIds = query.runIds && query.runIds.length > 0 ? [...query.runIds] : undefined;
  const limit = query.limit ?? 100;
  const hits: OutputSearchHit[] = [];
  const candidates = runIds ?? await listRecentRunIds(http, limit);
  for (const runId of candidates) {
    const outputs = await searchSessionOutputs(http, runId, query);
    for (const output of outputs) {
      hits.push(outputHit(runId, output));
      if (hits.length >= limit) return { hits };
    }
  }
  return { hits };
}

async function listRecentRunIds(http: HttpClient, limit: number): Promise<string[]> {
  const out: string[] = [];
  let cursor: string | undefined;
  while (out.length < limit) {
    const page = await operations.listSessions(http, { limit: Math.min(100, limit - out.length), ...(cursor ? { cursor } : {}) });
    out.push(...page.sessions.map((session) => session.id));
    if (!page.nextCursor) break;
    cursor = page.nextCursor;
  }
  return out;
}

async function searchSessionOutputs(
  http: HttpClient,
  sessionId: string,
  query: Omit<OutputSearchQuery, "runIds">
): Promise<readonly Output[]> {
  const listQuery: OutputQuery = {
    ...(query.extension !== undefined ? { extension: query.extension } : {}),
    ...(query.contentType !== undefined ? { contentType: query.contentType } : {})
  };
  const outputs = await operations.listSessionOutputs(
    http,
    sessionId,
    Object.keys(listQuery).length > 0 ? listQuery : undefined
  );
  if (query.filename === undefined) return outputs;
  const match = operations.toFilenameMatcher(query.filename);
  return outputs.filter((output) => typeof output.filename === "string" && match(output.filename));
}

function outputHit(runId: string, output: Output): OutputSearchHit {
  return {
    runId,
    outputId: output.id,
    ...(output.filename !== undefined ? { filename: output.filename } : {}),
    ...(output.sizeBytes !== undefined ? { sizeBytes: output.sizeBytes } : {}),
    ...(output.contentType !== undefined ? { contentType: output.contentType } : {})
  };
}

function assertMetadataOnlyOutputSearch(query: object, surface: string): void {
  for (const key of ["content", "text", "query", "grep", "body"]) {
    if (Object.prototype.hasOwnProperty.call(query, key)) {
      throw new Error(`${surface}: ${key} is not supported; output search is metadata-only`);
    }
  }
}

function baseName(p: string): string {
  const trimmed = p.replace(/[\\/]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return i >= 0 ? trimmed.slice(i + 1) : trimmed;
}
