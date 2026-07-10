/**
 * `aex files` — host CLI wrappers over the public captured-file operations:
 *
 *   aex files <session-id>                          List captured files (NDJSON)
 *   aex files read <session-id> <path>              Read one file as capped text (JSON)
 *   aex files download <session-id> <path> [--out]  Download one file's raw bytes
 *   aex files link <session-id> <path>              Mint a temporary download URL (JSON)
 *   aex files find <session-id> [--name S] [--ext E] [--type T] [--content-type CT]
 *   aex files search [--query S] [--name S] [--ext E] [--content-type CT] [--session-id ID] [--limit N]
 *
 * `aex files search` (no session id) is the cross-session metadata search
 * (`aex.files.search`); the whole-namespace zip stays `aex download`.
 */
import { operations, type HttpClient, type SessionFile, type SessionFileType, type SessionFileQuery, type SessionFileSearchHit, type SessionFileSearchQuery } from "@aexhq/contracts";
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
  rejectUnknownFlags,
  refuseInsideManagedSession,
  resolveCommonHostFlags,
  takeFlagValue
} from "./common.js";

const SUBVERBS = new Set(["read", "download", "link", "find", "search"]);

export async function executeSessionFilesCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "files")) return USAGE_ERR;

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
      return filesRead(io, http, args.slice(1));
    case "download":
      return filesDownload(io, http, args.slice(1), common.flags);
    case "link":
      return filesLink(io, http, args.slice(1));
    case "find":
      return filesFind(io, http, args.slice(1));
    case "search":
      return filesSearch(io, http, args.slice(1));
    default:
      return filesList(io, http, args);
  }
}

/** `aex files <session-id>` — list every captured file as NDJSON. */
async function filesList(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
  const usage = "usage: aex files <session-id> [common flags]";
  const unknown = rejectUnknownFlags(io, args, usage);
  if (unknown) return unknown;
  const positional = args;
  if (positional.length !== 1) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;
  try {
    const files = await operations.listSessionFiles(http, sessionId);
    for (const file of files) io.stdout(JSON.stringify(file) + "\n");
    return SUCCESS;
  } catch (err) {
    return filesError(io, "files_failed", err, { sessionId });
  }
}

/** `aex files read <session-id> <path>` — read one file as capped text. */
async function filesRead(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
  const usage = "usage: aex files read <session-id> <path> [common flags]";
  const unknown = rejectUnknownFlags(io, args, usage);
  if (unknown) return unknown;
  const positional = args;
  if (positional.length !== 2) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const [sessionId, selector] = positional as [string, string];
  try {
    const text = await operations.readSessionFileText(http, sessionId, { path: selector });
    io.stdout(JSON.stringify(text) + "\n");
    return SUCCESS;
  } catch (err) {
    return filesError(io, "files_read_failed", err, { sessionId, path: selector });
  }
}

/** `aex files download <session-id> <path> [--out file]` — one file's raw bytes. */
async function filesDownload(io: CliIO, http: HttpClient, args: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  const outFlag = takeFlagValue(args, "--out");
  if (outFlag.error) { io.stderr(`${outFlag.error}\n`); return USAGE_ERR; }
  void flags;
  const usage = "usage: aex files download <session-id> <path> [--out file] [common flags]";
  const unknown = rejectUnknownFlags(io, outFlag.remaining, usage);
  if (unknown) return unknown;
  const positional = outFlag.remaining;
  if (positional.length !== 2) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const [sessionId, selector] = positional as [string, string];
  let bytes: Uint8Array;
  try {
    bytes = (await operations.downloadSessionFile(http, sessionId, { path: selector })).bytes;
  } catch (err) {
    return filesError(io, "files_download_failed", err, { sessionId, path: selector });
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

/** `aex files link <session-id> <path>` — mint a temporary download URL. */
async function filesLink(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
  const usage = "usage: aex files link <session-id> <path> [common flags]";
  const unknown = rejectUnknownFlags(io, args, usage);
  if (unknown) return unknown;
  const positional = args;
  if (positional.length !== 2) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const [sessionId, selector] = positional as [string, string];
  try {
    const link = await operations.sessionFileLink(http, sessionId, { path: selector });
    io.stdout(JSON.stringify(link) + "\n");
    return SUCCESS;
  } catch (err) {
    return filesError(io, "files_link_failed", err, { sessionId, path: selector });
  }
}

/** `aex files find <session-id> [--name S] [--ext E] [--type T] [--content-type CT]`. */
async function filesFind(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
  const name = takeFlagValue(args, "--name");
  const ext = takeFlagValue(name.remaining, "--ext");
  const type = takeFlagValue(ext.remaining, "--type");
  const contentType = takeFlagValue(type.remaining, "--content-type");
  const err = name.error ?? ext.error ?? type.error ?? contentType.error;
  if (err) { io.stderr(`${err}\n`); return USAGE_ERR; }
  const usage = "usage: aex files find <session-id> [--name S] [--ext E] [--type T] [--content-type CT] [common flags]";
  const unknown = rejectUnknownFlags(io, contentType.remaining, usage);
  if (unknown) return unknown;
  const positional = contentType.remaining;
  if (positional.length !== 1) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;
  const query: SessionFileQuery = {
    ...(name.value !== null ? { filename: name.value } : {}),
    ...(ext.value !== null ? { extension: ext.value } : {}),
    ...(type.value !== null ? { type: type.value as SessionFileType } : {}),
    ...(contentType.value !== null ? { contentType: contentType.value } : {})
  };
  try {
    const hits = await searchSessionFiles(http, sessionId, query);
    for (const hit of hits) io.stdout(JSON.stringify(hit) + "\n");
    return SUCCESS;
  } catch (err2) {
    return filesError(io, "files_find_failed", err2, { sessionId });
  }
}

/** `aex files search [--query S] [--name S] [--ext E] [--content-type CT] [--session-id ID] [--limit N]` — cross-session. */
async function filesSearch(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
  const query = takeFlagValue(args, "--query");
  const name = takeFlagValue(query.remaining, "--name");
  const ext = takeFlagValue(name.remaining, "--ext");
  const contentType = takeFlagValue(ext.remaining, "--content-type");
  const limit = takeFlagValue(contentType.remaining, "--limit");
  const sessionIds = collectRepeated(limit.remaining, "--session-id");
  const err = query.error ?? name.error ?? ext.error ?? contentType.error ?? limit.error ?? sessionIds.error;
  if (err) { io.stderr(`${err}\n`); return USAGE_ERR; }
  const usage = "usage: aex files search [--query S] [--name S] [--ext E] [--content-type CT] [--session-id ID] [--limit N] [common flags]";
  const unknown = rejectUnknownFlags(io, sessionIds.remaining, usage);
  if (unknown) return unknown;
  if (sessionIds.remaining.length > 0) {
    io.stderr(`unexpected arguments: ${sessionIds.remaining.join(" ")}\n`);
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
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
  const search: SessionFileSearchQuery = {
    ...(filename !== null ? { filename } : {}),
    ...(ext.value !== null ? { extension: ext.value } : {}),
    ...(contentType.value !== null ? { contentType: contentType.value } : {}),
    ...(sessionIds.values.length > 0 ? { sessionIds: [...sessionIds.values] } : {}),
    ...(limitValue !== undefined ? { limit: limitValue } : {})
  };
  try {
    const page = await searchWorkspaceFiles(http, search);
    io.stdout(JSON.stringify(page) + "\n");
    return SUCCESS;
  } catch (err2) {
    return filesError(io, "files_search_failed", err2, {});
  }
}

function filesError(io: CliIO, code: string, err: unknown, extra: Record<string, unknown>): CliExitCode {
  const d = describeApiError(err);
  return emitJsonError(io, code, d.message, {
    ...extra,
    ...(d.status !== undefined ? { status: d.status } : {}),
    ...(d.remedy ? { remedy: d.remedy } : {})
  });
}

async function searchWorkspaceFiles(
  http: HttpClient,
  query: SessionFileSearchQuery
): Promise<{ readonly hits: readonly SessionFileSearchHit[] }> {
  assertMetadataOnlySessionFileSearch(query, "aex files search");
  const sessionIds = query.sessionIds && query.sessionIds.length > 0 ? [...query.sessionIds] : undefined;
  const limit = query.limit ?? 100;
  const hits: SessionFileSearchHit[] = [];
  const candidates = sessionIds ?? await listRecentSessionIds(http, limit);
  for (const sessionId of candidates) {
    const files = await searchSessionFiles(http, sessionId, query);
    for (const file of files) {
      hits.push(fileHit(sessionId, file));
      if (hits.length >= limit) return { hits };
    }
  }
  return { hits };
}

async function listRecentSessionIds(http: HttpClient, limit: number): Promise<string[]> {
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

async function searchSessionFiles(
  http: HttpClient,
  sessionId: string,
  query: Omit<SessionFileSearchQuery, "sessionIds">
): Promise<readonly SessionFile[]> {
  const listQuery: SessionFileQuery = {
    ...(query.extension !== undefined ? { extension: query.extension } : {}),
    ...(query.contentType !== undefined ? { contentType: query.contentType } : {})
  };
  const files = await operations.listSessionFiles(
    http,
    sessionId,
    Object.keys(listQuery).length > 0 ? listQuery : undefined
  );
  if (query.filename === undefined) return files;
  const match = operations.toFilenameMatcher(query.filename);
  return files.filter((file) => typeof file.filename === "string" && match(file.filename));
}

function fileHit(sessionId: string, file: SessionFile): SessionFileSearchHit {
  return {
    sessionId,
    fileId: file.id,
    ...(file.filename !== undefined ? { filename: file.filename } : {}),
    ...(file.sizeBytes !== undefined ? { sizeBytes: file.sizeBytes } : {}),
    ...(file.contentType !== undefined ? { contentType: file.contentType } : {})
  };
}

function assertMetadataOnlySessionFileSearch(query: object, surface: string): void {
  for (const key of ["content", "text", "query", "grep", "body"]) {
    if (Object.prototype.hasOwnProperty.call(query, key)) {
      throw new Error(`${surface}: ${key} is not supported; file search is metadata-only`);
    }
  }
}

function baseName(p: string): string {
  const trimmed = p.replace(/[\\/]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return i >= 0 ? trimmed.slice(i + 1) : trimmed;
}
