/**
 * `aex login` / `aex logout` / `aex auth status` (DX1).
 *
 * Persist the API key + default `--aex-url` to a 0600 config file so a dev
 * stops re-passing `--api-key` on every command. `login` validates the token
 * against `whoami` BEFORE writing (a bad token is never persisted). The token
 * value is never printed by any of these verbs.
 *
 * These are host-only and require `io.configStore` (wired by `cli.ts`); they
 * read no env and are unavailable inside a managed session container.
 */
import { operations } from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  describeApiError,
  emitJsonError,
  extractCommonHostFlags,
  makeHttpClient,
  rejectUnknownFlags,
  refuseInsideManagedSession
} from "./common.js";
import { AEX_DEFAULT_BASE_URL } from "@aexhq/contracts";

export async function executeLoginCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "login")) return USAGE_ERR;
  if (!io.configStore) {
    return emitJsonError(io, "config_store_unavailable", "config store unavailable in this environment");
  }

  const extracted = extractCommonHostFlags(argv);
  if (!extracted.ok) {
    io.stderr(`${extracted.reason}\n`);
    return USAGE_ERR;
  }
  const { apiKey, aexUrl, debug, rest } = extracted.flags;
  const loginUsage = "usage: aex login --api-key <token> [--aex-url <url>]";
  const unknown = rejectUnknownFlags(io, rest, loginUsage);
  if (unknown) return unknown;
  const positional = rest;
  if (positional.length > 0) {
    io.stderr(`unexpected arguments: ${positional.join(" ")}\n`);
    return USAGE_ERR;
  }
  if (!apiKey) {
    io.stderr(`${loginUsage}\n`);
    return USAGE_ERR;
  }

  const resolvedUrl = aexUrl ?? AEX_DEFAULT_BASE_URL;
  const http = makeHttpClient(io, { apiKey, aexUrl: resolvedUrl, debug, json: false });
  let workspaceId: string | undefined;
  try {
    const me = await operations.whoami(http);
    workspaceId = me.workspaceId;
    if (debug) io.stderr(`[aex] login: whoami ok workspace=${workspaceId ?? "(unknown)"}\n`);
  } catch (err) {
    if (debug) io.stderr("[aex] login: whoami failed — not persisting token\n");
    const described = describeApiError(err);
    return emitJsonError(io, "login_failed", described.message, {
      ...(described.status !== undefined ? { status: described.status } : {}),
      ...(described.remedy ? { remedy: described.remedy } : {})
    });
  }

  await io.configStore.write({ schemaVersion: 1, apiKey, ...(aexUrl ? { aexUrl: resolvedUrl } : {}) });
  if (debug) io.stderr(`[aex] login: persisted to ${io.configStore.location()}\n`);
  io.stdout(
    JSON.stringify({
      ok: true,
      ...(workspaceId ? { workspace: workspaceId } : {}),
      aexUrl: resolvedUrl,
      configPath: io.configStore.location()
    }) + "\n"
  );
  return SUCCESS;
}

export async function executeLogoutCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "logout")) return USAGE_ERR;
  if (!io.configStore) {
    return emitJsonError(io, "config_store_unavailable", "config store unavailable in this environment");
  }
  const extracted = extractCommonHostFlags(argv);
  if (!extracted.ok) {
    io.stderr(`${extracted.reason}\n`);
    return USAGE_ERR;
  }
  const logoutUsage = "usage: aex logout";
  const unknown = rejectUnknownFlags(io, extracted.flags.rest, logoutUsage);
  if (unknown) return unknown;
  if (extracted.flags.rest.length > 0) {
    io.stderr(`unexpected arguments: ${extracted.flags.rest.join(" ")}\n`);
    io.stderr(`${logoutUsage}\n`);
    return USAGE_ERR;
  }
  await io.configStore.clear();
  io.stdout(JSON.stringify({ ok: true, cleared: true, configPath: io.configStore.location() }) + "\n");
  return SUCCESS;
}

export async function executeAuthStatusCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "auth")) return USAGE_ERR;
  if (!io.configStore) {
    return emitJsonError(io, "config_store_unavailable", "config store unavailable in this environment");
  }
  const extracted = extractCommonHostFlags(argv);
  if (!extracted.ok) {
    io.stderr(`${extracted.reason}\n`);
    return USAGE_ERR;
  }
  const authUsage = "usage: aex auth status";
  const unknown = rejectUnknownFlags(io, extracted.flags.rest, authUsage);
  if (unknown) return unknown;
  if (extracted.flags.rest.length > 0) {
    io.stderr(`unexpected arguments: ${extracted.flags.rest.join(" ")}\n`);
    io.stderr(`${authUsage}\n`);
    return USAGE_ERR;
  }
  const stored = await io.configStore.read();
  const hasToken = Boolean(stored?.apiKey);
  // Show only the last 4 chars of the stored token as a non-secret fingerprint.
  const tokenSuffix = hasToken ? stored!.apiKey!.slice(-4) : undefined;
  io.stdout(
    JSON.stringify({
      configPath: io.configStore.location(),
      hasToken,
      ...(tokenSuffix ? { tokenSuffix } : {}),
      ...(stored?.aexUrl ? { aexUrl: stored.aexUrl } : {})
    }) + "\n"
  );
  return SUCCESS;
}
