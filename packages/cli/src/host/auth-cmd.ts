/**
 * `aex login` / `aex logout` / `aex auth status` (DX1).
 *
 * Persist the API token + default `--aex-url` to a 0600 config file so a dev
 * stops re-passing `--api-token` on every command. `login` validates the token
 * against `whoami` BEFORE writing (a bad token is never persisted). The token
 * value is never printed by any of these verbs.
 *
 * These are host-only and require `io.configStore` (wired by `cli.ts`); they
 * read no env and are unavailable inside a managed run container.
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
  refuseInsideManagedRun
} from "./common.js";
import { AEX_DEFAULT_BASE_URL } from "@aexhq/contracts";

export async function runLoginCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "login")) return USAGE_ERR;
  if (!io.configStore) {
    return emitJsonError(io, "config_store_unavailable", "config store unavailable in this environment");
  }

  const extracted = extractCommonHostFlags(argv);
  if (!extracted.ok) {
    io.stderr(`${extracted.reason}\n`);
    return USAGE_ERR;
  }
  const { apiToken, aexUrl, debug, rest } = extracted.flags;
  const positional = rest.filter((a) => !a.startsWith("--"));
  if (positional.length > 0) {
    io.stderr(`unexpected arguments: ${positional.join(" ")}\n`);
    return USAGE_ERR;
  }
  if (!apiToken) {
    io.stderr("usage: aex login --api-token <token> [--aex-url <url>]\n");
    return USAGE_ERR;
  }

  const resolvedUrl = aexUrl ?? AEX_DEFAULT_BASE_URL;
  const http = makeHttpClient(io, { apiToken, aexUrl: resolvedUrl, debug });
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

  await io.configStore.write({ schemaVersion: 1, apiToken, ...(aexUrl ? { aexUrl: resolvedUrl } : {}) });
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

export async function runLogoutCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "logout")) return USAGE_ERR;
  if (!io.configStore) {
    return emitJsonError(io, "config_store_unavailable", "config store unavailable in this environment");
  }
  const extracted = extractCommonHostFlags(argv);
  if (!extracted.ok) {
    io.stderr(`${extracted.reason}\n`);
    return USAGE_ERR;
  }
  await io.configStore.clear();
  io.stdout(JSON.stringify({ ok: true, cleared: true, configPath: io.configStore.location() }) + "\n");
  return SUCCESS;
}

export async function runAuthStatusCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "auth")) return USAGE_ERR;
  if (!io.configStore) {
    return emitJsonError(io, "config_store_unavailable", "config store unavailable in this environment");
  }
  const extracted = extractCommonHostFlags(argv);
  if (!extracted.ok) {
    io.stderr(`${extracted.reason}\n`);
    return USAGE_ERR;
  }
  const stored = await io.configStore.read();
  const hasToken = Boolean(stored?.apiToken);
  // Show only the last 4 chars of the stored token as a non-secret fingerprint.
  const tokenSuffix = hasToken ? stored!.apiToken!.slice(-4) : undefined;
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
