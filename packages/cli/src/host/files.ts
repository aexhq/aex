/**
 * `aex files` — host CLI wrappers over the public captured-file operations:
 *
 *   aex files <session-id>                          List captured files (NDJSON)
 *   aex files read <session-id> <path>              Read one file as capped text (JSON)
 *   aex files download <session-id> <path> [--out]  Download one file's raw bytes
 *   aex files link <session-id> <path>              Mint a temporary download URL (JSON)
 *   aex files find <session-id> [--name S] [--ext E] [--type T] [--content-type CT]
 */
import { type HttpClient, type SessionFileType, type SessionFileQuery } from "@aexhq/contracts";
import { operations } from "@aexhq/contracts/internal";
import { resolve as resolvePath } from "node:path";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  type CommonHostFlags,
  SUCCESS,
  USAGE_ERR,
  emitApiError,
  emitJsonError,
  makeHttpClient,
  rejectUnknownFlags,
  refuseInsideManagedSession,
  resolveCommonHostFlags,
  takeOptionFlag
} from "./common.js";
import { portableBasename } from "./command-primitives.js";

const SUBVERBS = new Set(["read", "download", "link", "find"]);

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
    const snapshot = await operations.listSessionFiles(http, sessionId);
    for (const file of snapshot.files) io.stdout(JSON.stringify(file) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "files_failed", err, { sessionId });
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
    return emitApiError(io, "files_read_failed", err, { sessionId, path: selector });
  }
}

/** `aex files download <session-id> <path> [--out file]` — one file's raw bytes. */
async function filesDownload(io: CliIO, http: HttpClient, args: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  const outFlag = takeOptionFlag(args, "--out");
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
    return emitApiError(io, "files_download_failed", err, { sessionId, path: selector });
  }
  const destination = resolvePath(io.cwd(), outFlag.value ?? portableBasename(selector));
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
    return emitApiError(io, "files_link_failed", err, { sessionId, path: selector });
  }
}

/** `aex files find <session-id> [--name S] [--ext E] [--type T] [--content-type CT]`. */
async function filesFind(io: CliIO, http: HttpClient, args: readonly string[]): Promise<CliExitCode> {
  const name = takeOptionFlag(args, "--name");
  const ext = takeOptionFlag(name.remaining, "--ext");
  const type = takeOptionFlag(ext.remaining, "--type");
  const contentType = takeOptionFlag(type.remaining, "--content-type");
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
    ...(name.value !== undefined ? { filename: name.value } : {}),
    ...(ext.value !== undefined ? { extension: ext.value } : {}),
    ...(type.value !== undefined ? { type: type.value as SessionFileType } : {}),
    ...(contentType.value !== undefined ? { contentType: contentType.value } : {})
  };
  try {
    const hits = await searchSessionFiles(http, sessionId, query);
    for (const hit of hits) io.stdout(JSON.stringify(hit) + "\n");
    return SUCCESS;
  } catch (err2) {
    return emitApiError(io, "files_find_failed", err2, { sessionId });
  }
}

async function searchSessionFiles(
  http: HttpClient,
  sessionId: string,
  query: SessionFileQuery
) {
  const listQuery: SessionFileQuery = {
    ...(query.extension !== undefined ? { extension: query.extension } : {}),
    ...(query.contentType !== undefined ? { contentType: query.contentType } : {})
  };
  const snapshot = await operations.listSessionFiles(http, sessionId, listQuery);
  if (query.filename === undefined) return snapshot.files;
  const match = operations.toFilenameMatcher(query.filename);
  return snapshot.files.filter((file) => typeof file.filename === "string" && match(file.filename));
}
