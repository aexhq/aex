/**
 * `aex login` / `aex logout` / `aex auth status`.
 *
 * `aex login` (no `--api-key`) runs the DEVICE FLOW: it requests a device code,
 * prints a verification URL + user code for the operator to approve in a browser
 * (using their existing dashboard session), polls for approval, then persists a
 * short-lived ACCOUNT token (`accountToken`) that drives the control-plane verbs
 * (`orgs`/`workspaces`/`keys`). `aex login --api-key <token>` is the
 * non-interactive fallback; it detects the credential KIND by prefix and
 * validates against the matching surface BEFORE writing (a bad token is never
 * persisted):
 *   - `aex_<plane>_…` workspace (data-plane) key → validated via the DATA-plane
 *     `whoami`, stored as `apiKey`.
 *   - `aexu_…` account PAT (control-plane) → validated via the CONTROL-plane
 *     (dashboard-BFF) whoami at the control-plane base URL, stored as
 *     `accountToken`.
 * The token value is never printed by any of these verbs.
 *
 * These are host-only and require `io.configStore` (wired by `cli.ts`); they
 * read no env and are unavailable inside a managed session container.
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO, StoredCliConfig } from "../internal.js";
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

const CONFIG_SCHEMA_VERSION = 1;
/** Prefix of a control-plane account PAT; distinct from a `aex_<plane>_…` workspace key. */
const ACCOUNT_PAT_PREFIX = "aexu_";
/** Fallbacks when the device-code response omits RFC-8628 pacing fields. */
const DEVICE_DEFAULT_INTERVAL_SEC = 5;
const DEVICE_DEFAULT_EXPIRES_SEC = 900;
const DEVICE_SLOW_DOWN_BUMP_SEC = 5;

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, Math.max(0, ms)));

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
  const loginUsage =
    "usage: aex login [--aex-url <url>]  |  aex login --api-key <workspace-key|account-pat> [--aex-url <url>]";
  const unknown = rejectUnknownFlags(io, rest, loginUsage);
  if (unknown) return unknown;
  if (rest.length > 0) {
    io.stderr(`unexpected arguments: ${rest.join(" ")}\n`);
    io.stderr(`${loginUsage}\n`);
    return USAGE_ERR;
  }

  const resolvedUrl = aexUrl ?? AEX_DEFAULT_BASE_URL;
  const persistUrl = aexUrl ? resolvedUrl : undefined;
  const existing = (await io.configStore.read()) ?? {};

  // No --api-key → interactive device flow → account (control-plane) token.
  if (!apiKey) {
    return deviceFlowLogin(io, { baseUrl: resolvedUrl, persistUrl, debug, existing });
  }

  // --api-key → non-interactive fallback. Detect the credential KIND by prefix
  // and validate against the matching surface before persisting anything.
  // An account PAT (`aexu_…`) is a CONTROL-plane credential — not self-describing
  // — so route it to the control-plane whoami and store it as `accountToken`.
  if (apiKey.startsWith(ACCOUNT_PAT_PREFIX)) {
    return accountPatLogin(io, { apiKey, aexUrl, debug, existing });
  }

  // A `aex_<plane>_…` workspace key validates via the DATA-plane whoami and is
  // stored as `apiKey`.
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

  await io.configStore.write({
    ...existing,
    schemaVersion: CONFIG_SCHEMA_VERSION,
    apiKey,
    ...(persistUrl ? { aexUrl: persistUrl } : {})
  });
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

/**
 * Validate + persist an account PAT (`aexu_…`). The PAT is a control-plane
 * credential and is NOT self-describing, so — mirroring
 * `resolveControlPlaneHostFlags` — the base URL comes from the `--aex-url` flag,
 * then the stored url, then the prd default (never key-derived). Validated
 * against the CONTROL-plane (dashboard-BFF) whoami before it is written; a bad
 * PAT exits non-zero and persists nothing. Stored as `accountToken`, preserving
 * any existing workspace `apiKey`.
 */
async function accountPatLogin(
  io: CliIO,
  opts: { readonly apiKey: string; readonly aexUrl: string | null; readonly debug: boolean; readonly existing: StoredCliConfig }
): Promise<CliExitCode> {
  const resolvedUrl = opts.aexUrl ?? opts.existing.aexUrl ?? AEX_DEFAULT_BASE_URL;
  const persistUrl = opts.aexUrl ? resolvedUrl : undefined;
  const http = makeHttpClient(io, { apiKey: opts.apiKey, aexUrl: resolvedUrl, debug: opts.debug, json: false });
  try {
    const me = await operations.accountWhoami(http);
    if (opts.debug) io.stderr(`[aex] login: account whoami ok user=${me.appUserId}\n`);
  } catch (err) {
    if (opts.debug) io.stderr("[aex] login: account whoami failed — not persisting token\n");
    const described = describeApiError(err);
    return emitJsonError(io, "login_failed", described.message, {
      ...(described.status !== undefined ? { status: described.status } : {}),
      ...(described.remedy ? { remedy: described.remedy } : {})
    });
  }

  await io.configStore!.write({
    ...opts.existing,
    schemaVersion: CONFIG_SCHEMA_VERSION,
    accountToken: opts.apiKey,
    ...(persistUrl ? { aexUrl: persistUrl } : {})
  });
  if (opts.debug) io.stderr(`[aex] login: account token persisted to ${io.configStore!.location()}\n`);
  io.stdout(
    JSON.stringify({
      ok: true,
      accountAuthorized: true,
      aexUrl: resolvedUrl,
      configPath: io.configStore!.location()
    }) + "\n"
  );
  return SUCCESS;
}

interface DeviceCodeResponse {
  readonly device_code: string;
  readonly user_code: string;
  readonly verification_uri: string;
  readonly verification_uri_complete?: string;
  readonly expires_in?: number;
  readonly interval?: number;
}

interface DeviceTokenResponse {
  readonly access_token?: string;
  readonly error?: string;
  readonly message?: string;
}

async function deviceFlowLogin(
  io: CliIO,
  opts: { readonly baseUrl: string; readonly persistUrl: string | undefined; readonly debug: boolean; readonly existing: StoredCliConfig }
): Promise<CliExitCode> {
  let start: DeviceCodeResponse;
  try {
    start = await postDeviceJson<DeviceCodeResponse>(io, opts.baseUrl, "/api/device/code", {});
  } catch (err) {
    return emitJsonError(io, "device_code_failed", (err as Error).message ?? "device authorization request failed");
  }
  if (!start.device_code || !start.user_code || !start.verification_uri) {
    return emitJsonError(io, "device_code_failed", "device authorization response is missing device_code/user_code/verification_uri");
  }

  // Human instructions go to STDERR so stdout stays a clean JSON result.
  io.stderr(`To sign in, open: ${start.verification_uri_complete ?? start.verification_uri}\n`);
  io.stderr(`Enter code: ${start.user_code}\n`);
  io.stderr("Waiting for approval…\n");

  let intervalSec = Math.max(1, start.interval ?? DEVICE_DEFAULT_INTERVAL_SEC);
  const deadline = Date.now() + Math.max(1, start.expires_in ?? DEVICE_DEFAULT_EXPIRES_SEC) * 1000;

  while (Date.now() < deadline) {
    let poll: DeviceTokenResponse;
    try {
      poll = await postDeviceJson<DeviceTokenResponse>(io, opts.baseUrl, "/api/device/token", {
        device_code: start.device_code
      });
    } catch (err) {
      return emitJsonError(io, "device_token_failed", (err as Error).message ?? "device token poll failed");
    }

    if (typeof poll.access_token === "string" && poll.access_token.length > 0) {
      await io.configStore!.write({
        ...opts.existing,
        schemaVersion: CONFIG_SCHEMA_VERSION,
        accountToken: poll.access_token,
        ...(opts.persistUrl ? { aexUrl: opts.persistUrl } : {})
      });
      if (opts.debug) io.stderr(`[aex] login: account token persisted to ${io.configStore!.location()}\n`);
      io.stdout(
        JSON.stringify({
          ok: true,
          accountAuthorized: true,
          aexUrl: opts.baseUrl,
          configPath: io.configStore!.location()
        }) + "\n"
      );
      return SUCCESS;
    }

    switch (poll.error) {
      case "authorization_pending":
        break;
      case "slow_down":
        intervalSec += DEVICE_SLOW_DOWN_BUMP_SEC;
        break;
      case "access_denied":
        return emitJsonError(io, "login_denied", "authorization was denied in the browser");
      case "expired_token":
        return emitJsonError(io, "login_expired", "the device code expired before it was approved — run `aex login` again");
      default:
        if (poll.error) {
          return emitJsonError(io, "login_failed", poll.message ?? poll.error);
        }
        // No token and no recognized error: keep polling until the deadline.
        break;
    }
    await sleep(intervalSec * 1000);
  }
  return emitJsonError(io, "login_timeout", "timed out waiting for device approval — run `aex login` again");
}

/** Unauthenticated JSON POST for the device-flow bootstrap (no bearer header). */
async function postDeviceJson<T>(io: CliIO, baseUrl: string, path: string, body: unknown): Promise<T> {
  const url = `${baseUrl.replace(/\/$/, "")}${path}`;
  const response = await io.fetchImpl(url, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json" },
    body: JSON.stringify(body)
  });
  const text = await response.text();
  let parsed: unknown = {};
  if (text.length > 0) {
    try {
      parsed = JSON.parse(text) as unknown;
    } catch {
      parsed = { raw: text };
    }
  }
  // The device-token endpoint returns 400 with `{ error: "authorization_pending" }`
  // during polling — that is a NORMAL state, not a transport failure, so a body
  // carrying an `error`/`access_token` is returned to the caller regardless of
  // status. A status error with no usable body throws.
  if (!response.ok && !(parsed && typeof parsed === "object" && ("error" in parsed || "access_token" in parsed))) {
    throw new Error(`${path} failed with HTTP ${response.status}`);
  }
  return parsed as T;
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
  const workspaceKey = stored?.apiKey;
  const accountToken = stored?.accountToken;
  // Report EITHER credential kind. The workspace (data-plane) key is preferred
  // for the fingerprint; the account PAT is the fallback so a device-flow /
  // PAT-only login still reports a stored credential (not `hasToken:false`).
  const active = workspaceKey ?? accountToken;
  const hasToken = Boolean(active);
  // Show only the last 4 chars of the stored token as a non-secret fingerprint.
  const tokenSuffix = active ? active.slice(-4) : undefined;
  const credential = workspaceKey ? "workspace" : accountToken ? "account" : undefined;
  io.stdout(
    JSON.stringify({
      configPath: io.configStore.location(),
      hasToken,
      ...(tokenSuffix ? { tokenSuffix } : {}),
      ...(credential ? { credential } : {}),
      ...(stored?.aexUrl ? { aexUrl: stored.aexUrl } : {})
    }) + "\n"
  );
  return SUCCESS;
}
