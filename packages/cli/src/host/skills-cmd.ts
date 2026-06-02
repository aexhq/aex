/**
 * `antpath skills <verb>` — workspace skill bundle management. Mirrors
 * the SDK's `client.skills` namespace 1:1 so SDK and CLI never drift.
 *
 * Verbs:
 *   antpath skills upload --from-path <dir> --name <n>
 *   antpath skills upload --file <path> [--file <path> ...] --name <n>
 *   antpath skills list
 *   antpath skills get <skill-id>
 *   antpath skills delete <skill-id>
 *
 * `--file` stores each path verbatim (relative to cwd) under its
 * basename inside the bundle. For directory uploads, use `--from-path`.
 */
import { readFile, readdir, stat } from "node:fs/promises";
import { basename, join, posix, relative, resolve as resolvePath, sep } from "node:path";
import { zipSync, type Zippable } from "fflate";
import {
  SKILL_BUNDLE_LIMITS,
  operations,
  validateSkillBundleEntry
} from "@antpath/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  RUNTIME_ERR,
  SUCCESS,
  USAGE_ERR,
  collectRepeated,
  emitJsonError,
  makeHttpClient,
  parseCommonHostFlags,
  refuseInsideManagedRun,
  takeFlagValue
} from "./common.js";

export async function runSkillsCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "skills")) return USAGE_ERR;
  const [verb, ...rest] = argv;
  if (!verb) {
    io.stderr("usage: antpath skills <upload|list|get|delete> [flags]\n");
    return USAGE_ERR;
  }
  switch (verb) {
    case "upload": return runSkillsUpload(io, rest);
    case "list":   return runSkillsList(io, rest);
    case "get":    return runSkillsGet(io, rest);
    case "delete": return runSkillsDelete(io, rest);
    default:
      io.stderr(`unknown skills verb: ${verb}\n`);
      return USAGE_ERR;
  }
}

async function runSkillsUpload(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = parseCommonHostFlags(argv);
  if (!common.ok) { io.stderr(`${common.reason}\n`); return USAGE_ERR; }
  let rest = common.rest;

  const nameFlag = takeFlagValue(rest, "--name");
  if (nameFlag.error) { io.stderr(`${nameFlag.error}\n`); return USAGE_ERR; }
  rest = nameFlag.remaining;
  if (!nameFlag.value) {
    io.stderr("--name is required\n");
    return USAGE_ERR;
  }

  const fromPath = takeFlagValue(rest, "--from-path");
  if (fromPath.error) { io.stderr(`${fromPath.error}\n`); return USAGE_ERR; }
  rest = fromPath.remaining;

  const fileFlags = collectRepeated(rest, "--file");
  if (fileFlags.error) { io.stderr(`${fileFlags.error}\n`); return USAGE_ERR; }
  rest = fileFlags.remaining;

  const unknown = rest.filter((a) => a.startsWith("--"));
  if (unknown.length > 0) {
    io.stderr(`unknown flag: ${unknown[0]}\n`);
    return USAGE_ERR;
  }

  if ((fromPath.value && fileFlags.values.length > 0) || (!fromPath.value && fileFlags.values.length === 0)) {
    io.stderr("antpath skills upload requires exactly one of: --from-path <dir> | --file <path> [--file ...]\n");
    return USAGE_ERR;
  }

  let zip: Uint8Array;
  try {
    if (fromPath.value) {
      zip = await zipDirectory(resolvePath(io.cwd(), fromPath.value));
    } else {
      zip = await zipFiles(io.cwd(), fileFlags.values);
    }
  } catch (err) {
    io.stderr(`failed to build skill bundle: ${(err as Error).message}\n`);
    return USAGE_ERR;
  }

  const http = makeHttpClient(io, common.flags);
  try {
    const skill = await operations.createSkillBundle(http, {
      name: nameFlag.value,
      body: zip,
      contentType: "application/zip",
      filename: `${nameFlag.value}.zip`
    });
    io.stdout(JSON.stringify(skill) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitJsonError(io, "skill_upload_failed", (err as Error).message ?? "upload failed");
  }
}

async function runSkillsList(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = parseCommonHostFlags(argv);
  if (!common.ok) { io.stderr(`${common.reason}\n`); return USAGE_ERR; }
  const positional = common.rest.filter((a) => !a.startsWith("--"));
  const unknown = common.rest.filter((a) => a.startsWith("--"));
  if (unknown.length > 0) {
    io.stderr(`unknown flag: ${unknown[0]}\n`);
    return USAGE_ERR;
  }
  if (positional.length > 0) {
    io.stderr(`antpath skills list takes no positional arguments\n`);
    return USAGE_ERR;
  }
  const http = makeHttpClient(io, common.flags);
  try {
    const skills = await operations.listSkills(http);
    for (const s of skills) io.stdout(JSON.stringify(s) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitJsonError(io, "skills_list_failed", (err as Error).message ?? "list failed");
  }
}

async function runSkillsGet(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = parseCommonHostFlags(argv);
  if (!common.ok) { io.stderr(`${common.reason}\n`); return USAGE_ERR; }
  const positional = common.rest.filter((a) => !a.startsWith("--"));
  const unknown = common.rest.filter((a) => a.startsWith("--"));
  if (unknown.length > 0) {
    io.stderr(`unknown flag: ${unknown[0]}\n`);
    return USAGE_ERR;
  }
  if (positional.length !== 1) {
    io.stderr("usage: antpath skills get <skill-id>\n");
    return USAGE_ERR;
  }
  const skillId = positional[0]!;
  const http = makeHttpClient(io, common.flags);
  try {
    const skill = await operations.getSkill(http, skillId);
    io.stdout(JSON.stringify(skill) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitJsonError(io, "skill_get_failed", (err as Error).message ?? "get failed");
  }
}

async function runSkillsDelete(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = parseCommonHostFlags(argv);
  if (!common.ok) { io.stderr(`${common.reason}\n`); return USAGE_ERR; }
  const positional = common.rest.filter((a) => !a.startsWith("--"));
  const unknown = common.rest.filter((a) => a.startsWith("--"));
  if (unknown.length > 0) {
    io.stderr(`unknown flag: ${unknown[0]}\n`);
    return USAGE_ERR;
  }
  if (positional.length !== 1) {
    io.stderr("usage: antpath skills delete <skill-id>\n");
    return USAGE_ERR;
  }
  const skillId = positional[0]!;
  const http = makeHttpClient(io, common.flags);
  try {
    await operations.deleteSkill(http, skillId);
    io.stdout(JSON.stringify({ skillId, deleted: true }) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitJsonError(io, "skill_delete_failed", (err as Error).message ?? "delete failed");
  }
}

/* ---------- bundle building ---------- */

/** Stable mtime for every zip entry — keeps the bundle byte-deterministic
 * across machines and re-runs so SDK/CLI hashes match the BFF's
 * canonical hash. ZIP's legacy DOS date encoding requires year >= 1980,
 * so we anchor to 1980-01-01 UTC rather than the unix epoch. */
const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));

function zipEntryFor(bytes: Uint8Array): [Uint8Array, { mtime: Date }] {
  return [bytes, { mtime: ZIP_EPOCH }];
}

async function zipDirectory(rootDir: string): Promise<Uint8Array> {
  const rootStat = await stat(rootDir);
  if (!rootStat.isDirectory()) {
    throw new Error(`${rootDir} is not a directory`);
  }
  const collected = new Map<string, Uint8Array>();
  let totalDecompressed = 0;
  let hasSkillMd = false;
  await walk(rootDir, rootDir, async (relPathPosix, bytes) => {
    const entry = validateSkillBundleEntry({ path: relPathPosix, size: bytes.byteLength });
    if (entry.path === "SKILL.md") hasSkillMd = true;
    totalDecompressed += bytes.byteLength;
    if (totalDecompressed > SKILL_BUNDLE_LIMITS.maxDecompressedBytes) {
      throw new Error(`skill bundle exceeds decompressed cap of ${SKILL_BUNDLE_LIMITS.maxDecompressedBytes} bytes`);
    }
    if (collected.has(entry.path)) {
      throw new Error(`skill bundle contains duplicate path: ${entry.path}`);
    }
    if (collected.size >= SKILL_BUNDLE_LIMITS.maxFiles) {
      throw new Error(`skill bundle exceeds ${SKILL_BUNDLE_LIMITS.maxFiles} file limit`);
    }
    collected.set(entry.path, bytes);
  });
  if (collected.size === 0) {
    throw new Error(`${rootDir} contains no regular files`);
  }
  if (!hasSkillMd) {
    throw new Error(
      'skill bundle must contain a "SKILL.md" file at the root. ' +
        "For AGENTS.md / generic files use the corresponding `antpath agentsmd` / " +
        "`antpath files` commands instead."
    );
  }
  // Sort entries for deterministic ordering (zipSync preserves insertion order).
  const sorted = [...collected.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  const zippable: Zippable = {};
  for (const [path, bytes] of sorted) zippable[path] = zipEntryFor(bytes);
  const out = zipSync(zippable, { level: 6 });
  if (out.byteLength > SKILL_BUNDLE_LIMITS.maxCompressedBytes) {
    throw new Error(`skill bundle exceeds compressed cap of ${SKILL_BUNDLE_LIMITS.maxCompressedBytes} bytes (got ${out.byteLength})`);
  }
  return out;
}

async function zipFiles(cwd: string, paths: readonly string[]): Promise<Uint8Array> {
  if (paths.length > SKILL_BUNDLE_LIMITS.maxFiles) {
    throw new Error(`skill bundle exceeds ${SKILL_BUNDLE_LIMITS.maxFiles} file limit`);
  }
  const collected = new Map<string, Uint8Array>();
  let totalDecompressed = 0;
  let hasSkillMd = false;
  for (const p of paths) {
    const abs = resolvePath(cwd, p);
    const bytes = await readFile(abs);
    const name = basename(p);
    const entry = validateSkillBundleEntry({ path: name, size: bytes.byteLength });
    if (entry.path === "SKILL.md") hasSkillMd = true;
    totalDecompressed += bytes.byteLength;
    if (totalDecompressed > SKILL_BUNDLE_LIMITS.maxDecompressedBytes) {
      throw new Error(`skill bundle exceeds decompressed cap of ${SKILL_BUNDLE_LIMITS.maxDecompressedBytes} bytes`);
    }
    if (collected.has(entry.path)) {
      throw new Error(`skill bundle contains duplicate path (basenames must be unique): ${entry.path}`);
    }
    collected.set(entry.path, bytes);
  }
  if (!hasSkillMd) {
    throw new Error('skill bundle must contain a "SKILL.md" file at the root');
  }
  const sorted = [...collected.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  const zippable: Zippable = {};
  for (const [path, bytes] of sorted) zippable[path] = zipEntryFor(bytes);
  const out = zipSync(zippable, { level: 6 });
  if (out.byteLength > SKILL_BUNDLE_LIMITS.maxCompressedBytes) {
    throw new Error(`skill bundle exceeds compressed cap of ${SKILL_BUNDLE_LIMITS.maxCompressedBytes} bytes`);
  }
  return out;
}

async function walk(
  rootDir: string,
  currentDir: string,
  visit: (relPathPosix: string, bytes: Uint8Array) => Promise<void>
): Promise<void> {
  const entries = await readdir(currentDir, { withFileTypes: true });
  for (const dirent of entries) {
    const full = join(currentDir, dirent.name);
    if (dirent.isSymbolicLink()) continue;
    if (dirent.isDirectory()) {
      await walk(rootDir, full, visit);
      continue;
    }
    if (!dirent.isFile()) continue;
    const rel = relative(rootDir, full);
    const posixPath = sep === "/" ? rel : rel.split(sep).join(posix.sep);
    const bytes = await readFile(full);
    await visit(posixPath, bytes);
  }
}
