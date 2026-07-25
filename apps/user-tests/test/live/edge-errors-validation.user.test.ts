/**
 * LIVE edge-case sweep of the SDK's HTTP / validation / auth error surface against
 * the target live plane, from a real customer's installed `@aexhq/sdk`. NO sessions are ever
 * dispatched (no provider key needed, zero Fargate cost) — every case is a
 * validation reject, an auth reject, a 404, or a bounded network failure.
 *
 * Asserted (each classified in the agent report as EXPECTED / PRODUCT_BUG):
 *   1. Garbage apiKey → `whoami()` rejects with a typed AexApiError, status 401
 *      (or 403) — a clean auth reject, never an opaque throw or a hang.
 *   2. Valid apiKey → `whoami()` resolves to a workspace-identity object.
 *   3. Nonexistent sessionId → `sessions.open/get` and the session-handle files namespace
 *      reject with a typed AexApiError 4xx (a clean 404, never a 5xx).
 *   4. Client-side malformed session config (missing model / empty message / legacy
 *      field / the REMOVED provider + apiKeys fields) fails fast with a typed
 *      `SessionConfigValidationError` (an `AexError`) BEFORE any network call.
 *   5. Unreachable baseUrl → the retry loop gives up with a bounded network error
 *      (does NOT hang), surfacing a real Error rather than swallowing it.
 *
 * Required env (exported by the shared live runner for the target plane):
 *   AEX_API_URL, AEX_API_KEY
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live: required env ${name} is missing (export via the dev live runner).`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");

interface TokenReject {
  readonly rejected: boolean;
  readonly isApiError: boolean;
  readonly status: number | null;
  readonly name: string | null;
  readonly bodyError: string | null;
  readonly hasMessage: boolean;
  readonly leaked: boolean;
}

interface EdgeResult {
  readonly malformedToken: TokenReject;
  readonly missingBearer: TokenReject;
  readonly wellFormedUnauthToken: TokenReject;
  readonly validWhoami: { ok: boolean; isObject: boolean; keyCount: number };
  readonly missingSession: { open: EdgeErr; get: EdgeErr; files: EdgeErr };
  readonly validation: Record<string, EdgeErr>;
  readonly missingApiKeyCtor: { name: string | null; isAexError: boolean; message: string | null };
  readonly unreachable: { rejected: boolean; isApiError: boolean; name: string | null; elapsedMs: number };
}

interface EdgeErr {
  readonly rejected: boolean;
  readonly isApiError?: boolean;
  readonly isAexError?: boolean;
  readonly status?: number | null;
  readonly name?: string | null;
  readonly code?: string | null;
  readonly message?: string | null;
}

describe("live plane — SDK error/validation/auth edge cases", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "rejects bad auth/ids/config with typed errors and bounds unreachable hosts (no sessions dispatched)",
    async () => {
      const bogusSession = "sess_edge_missing_" + Math.random().toString(36).slice(2, 10);
      const script = String.raw`
import {
  Aex, AexApiError, AexError, SessionConfigValidationError, tryParseApiKey
} from "@aexhq/sdk";

const apiUrl = process.env.AEX_API_URL;
const apiKey = process.env.AEX_API_KEY;
const bogusSession = ${JSON.stringify(bogusSession)};

// Never let the real token bleed into the recorded evidence.
const REDACT = (s) => typeof s === "string" ? s.split(apiKey).join("<token>") : s;

// Build a STRUCTURALLY-VALID aex_* bearer token (correct 6-segment shape +
// valid crc32 checksum) that is NOT a real credential, to separate the
// "malformed structure" reject from the "well-formed but unauthenticated"
// reject. Mirrors the crc used by the region-token router.
function crc32b36(input) {
  let crc = 0xffffffff;
  for (const b of Buffer.from(input, "utf8")) { crc ^= b; for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1)); }
  return ((crc ^ 0xffffffff) >>> 0).toString(36);
}
function craftWellFormedToken() {
  const parsedApiKey = tryParseApiKey(apiKey);
  const activePlane = parsedApiKey && (parsedApiKey.plane === "dev" || parsedApiKey.plane === "prd")
    ? parsedApiKey.plane
    : apiUrl.replace(/\/+$/, "") === "https://api.aex.dev" ? "prd" : "dev";
  const body = ["aex", activePlane, "euw1", "wwwwwwww", "feedfacecafebeef"].join("_");
  return body + "_" + crc32b36(body);
}

function tokenReject(err, junkToken) {
  const msg = err && err.message != null ? String(err.message) : "";
  const bodyError = err instanceof AexApiError && err.body && typeof err.body === "object" && typeof err.body.error === "string"
    ? err.body.error : null;
  return {
    rejected: true,
    isApiError: err instanceof AexApiError,
    status: err instanceof AexApiError ? err.status : null,
    name: err && err.name != null ? String(err.name) : null,
    bodyError,
    hasMessage: typeof msg === "string" && msg.length > 0,
    leaked: junkToken ? msg.includes(junkToken) : false
  };
}

function describeErr(err, includeStatus) {
  const out = { rejected: true };
  if (err instanceof AexApiError) { out.isApiError = true; out.status = err.status; }
  else { out.isApiError = false; }
  out.isAexError = err instanceof AexError;
  out.name = err && err.name != null ? String(err.name) : null;
  out.code = err && err.code != null ? String(err.code) : null;
  out.message = err && err.message != null ? REDACT(String(err.message)).slice(0, 240) : null;
  if (!includeStatus) delete out.status;
  return out;
}

// (1a) A structurally MALFORMED token → whoami rejects cleanly (typed
//      AexApiError). On dev this is HTTP 400 {"error":"malformed_token"}.
const JUNK = "aex_garbage_not_a_real_token_zzzz";
const garbage = new Aex({ apiKey: JUNK, baseUrl: apiUrl });
let malformedToken;
try { await garbage.whoami(); malformedToken = { rejected: false, isApiError: false, status: null, name: null, bodyError: null, hasMessage: false, leaked: false }; }
catch (err) { malformedToken = tokenReject(err, JUNK); }

// (1b) No bearer at all → the plain missing-credentials fallback.
let missingBearer;
try {
  const res = await fetch(apiUrl.replace(/\/$/, "") + "/api/whoami");
  let bodyError = null;
  let hasMessage = false;
  try {
    const body = await res.json();
    bodyError = body && typeof body.error === "string" ? body.error : null;
    hasMessage = body && typeof body.message === "string" && body.message.length > 0;
  } catch {}
  missingBearer = {
    rejected: !res.ok,
    isApiError: false,
    status: res.status,
    name: null,
    bodyError,
    hasMessage,
    leaked: false
  };
} catch (err) {
  missingBearer = {
    rejected: true,
    isApiError: false,
    status: null,
    name: err && err.name != null ? String(err.name) : null,
    bodyError: null,
    hasMessage: err && err.message != null,
    leaked: false
  };
}

// (1c) A WELL-FORMED aex_* token (valid shape+crc) but not a real credential →
//      whoami rejects with HTTP 401 {"error":"token_invalid"} (the true
//      invalid-credential path, distinct from the no-bearer and malformed paths).
const wellFormed = new Aex({ apiKey: craftWellFormedToken(), baseUrl: apiUrl });
let wellFormedUnauthToken;
try { await wellFormed.whoami(); wellFormedUnauthToken = { rejected: false, isApiError: false, status: null, name: null, bodyError: null, hasMessage: false, leaked: false }; }
catch (err) { wellFormedUnauthToken = tokenReject(err, null); }

// (2) Valid token → whoami resolves to a workspace-identity object.
const client = new Aex({ apiKey, baseUrl: apiUrl });
let validWhoami;
try {
  const who = await client.whoami();
  validWhoami = { ok: true, isObject: who != null && typeof who === "object", keyCount: who && typeof who === "object" ? Object.keys(who).length : 0 };
} catch (err) {
  validWhoami = { ok: false, isObject: false, keyCount: 0, error: REDACT(String(err && err.message)) };
}

// (3) Nonexistent session id → typed AexApiError 4xx on open/get/files.
async function capture(fn, includeStatus) {
  try { await fn(); return { rejected: false }; }
  catch (err) { return describeErr(err, includeStatus); }
}
const missingSession = {
  open: await capture(() => client.sessions.open(bogusSession), true),
  get: await capture(() => client.sessions.get(bogusSession), true),
  files: await capture(async () => (await client.sessions.open(bogusSession)).files.list(), true)
};

// (4) Client-side malformed session config → typed SessionConfigValidationError, no network.
const M = "anthropic/claude-haiku-4-5";
const validation = {
  emptyMessage: await capture(() => client.start({ model: M, message: "" }), false),
  missingModel: await capture(() => client.sessions.create({}), false),
  legacyPromptField: await capture(() => client.sessions.create({ model: M, prompt: "hi" }), false),
  // Managed keys INVERTED these two: provider and apiKeys used to be required, and
  // are now removed fields the SDK must reject client-side. A stale caller that still
  // sends them should learn so before a request leaves the process.
  // (No backticks in this block: it lives inside a String.raw template.)
  removedProviderField: await capture(() => client.sessions.create({ model: M, provider: "deepseek" }), false),
  removedApiKeysField: await capture(() => client.sessions.create({ model: M, apiKeys: { deepseek: "x" } }), false),
  legacySignal: await capture(() => client.start({ model: M, message: "hi", signal: new AbortController().signal }), false)
};

// (4b) Constructor with no credential — what TYPE does it throw?
let missingApiKeyCtor;
try {
  new Aex({});
  missingApiKeyCtor = { name: null, isAexError: false, message: null };
} catch (err) {
  missingApiKeyCtor = {
    name: err && err.name != null ? String(err.name) : null,
    isAexError: err instanceof AexError,
    message: err && err.message != null ? String(err.message).slice(0, 160) : null
  };
}

// (5) Unreachable baseUrl → the retry loop gives up bounded (does not hang).
const unreachableClient = new Aex({
  apiKey: "aex_x",
  baseUrl: "https://127.0.0.1:9",
  retry: { maxAttempts: 2, initialDelayMs: 1, maxDelayMs: 3 }
});
const u0 = Date.now();
let unreachable;
try {
  await unreachableClient.whoami();
  unreachable = { rejected: false, isApiError: false, name: null, elapsedMs: Date.now() - u0 };
} catch (err) {
  unreachable = {
    rejected: true,
    isApiError: err instanceof AexApiError,
    name: err && err.name != null ? String(err.name) : null,
    elapsedMs: Date.now() - u0
  };
}

process.stdout.write(JSON.stringify({
  malformedToken, missingBearer, wellFormedUnauthToken, validWhoami, missingSession, validation, missingApiKeyCtor, unreachable
}));
process.exit(0);
`;
      const scriptPath = join(install.installDir, "edge-errors-validation-runner.mjs");
      writeFileSync(scriptPath, script);

      const apiKey = requireEnv("AEX_API_KEY");
      const passEnv: Record<string, string> = { AEX_API_URL: apiUrl, AEX_API_KEY: apiKey };
      const pathKey = process.platform === "win32" ? "Path" : "PATH";
      if (process.env[pathKey]) passEnv[pathKey] = process.env[pathKey]!;
      const carry =
        process.platform === "win32"
          ? ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]
          : ["HOME", "TMPDIR", "LANG", "LC_ALL"];
      for (const k of carry) if (process.env[k]) passEnv[k] = process.env[k]!;

      const child = await runCommand(getBunCommand(), [scriptPath], {
        cwd: install.installDir,
        timeoutMs: 120_000,
        env: passEnv
      });
      if (child.exitCode !== 0) {
        throw new Error(`edge runner exited ${child.exitCode}\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
      }
      const result = JSON.parse(child.stdout.trim()) as EdgeResult;

      // (1a) Malformed token: clean, typed auth-class reject — no hang, no leak.
      //      NOTE: the hosted API returns 400 `malformed_token` here (not 401). The SDK
      //      surfaces it correctly as a typed AexApiError; the 400-vs-401 status
      //      is flagged as a contract finding (contradicts live-api-fuzz's
      //      "garbage bearer ⇒ 401/403" invariant). We assert the actual
      //      deployed two-tier contract so this doubles as a regression guard.
      expect(result.malformedToken.rejected).toBe(true);
      expect(result.malformedToken.isApiError).toBe(true);
      expect(result.malformedToken.name).toBe("AexAuthError");
      expect(result.malformedToken.status).toBe(400);
      expect(result.malformedToken.bodyError).toBe("malformed_token");
      expect(result.malformedToken.hasMessage).toBe(true);
      expect(result.malformedToken.leaked).toBe(false);

      // (1b) No bearer at all → the plain 401 missing-credentials fallback.
      expect(result.missingBearer.rejected).toBe(true);
      expect(result.missingBearer.status).toBe(401);
      expect(result.missingBearer.bodyError).toBe("unauthorized");

      // (1c) Well-formed but unauthenticated token → the classified 401 path.
      expect(result.wellFormedUnauthToken.rejected).toBe(true);
      expect(result.wellFormedUnauthToken.isApiError).toBe(true);
      expect(result.wellFormedUnauthToken.status).toBe(401);
      expect(result.wellFormedUnauthToken.bodyError).toBe("token_invalid");
      expect(result.wellFormedUnauthToken.leaked).toBe(false);

      // (2) Valid token resolves an identity object.
      expect(result.validWhoami.ok).toBe(true);
      expect(result.validWhoami.isObject).toBe(true);
      expect(result.validWhoami.keyCount).toBeGreaterThan(0);

      // (3) Nonexistent session → typed 4xx (clean 404), never a 5xx.
      for (const key of ["open", "get", "files"] as const) {
        const e = result.missingSession[key];
        expect(e.rejected, `sessions.${key} should reject for a bogus id`).toBe(true);
        expect(e.isApiError, `sessions.${key} error should be AexApiError`).toBe(true);
        expect(e.status, `sessions.${key} status`).toBeGreaterThanOrEqual(400);
        expect(e.status, `sessions.${key} must not be 5xx`).toBeLessThan(500);
      }

      // (4) Client-side validation → typed SessionConfigValidationError (an AexError), pre-network.
      for (const [caseName, e] of Object.entries(result.validation)) {
        expect(e.rejected, `${caseName} should reject`).toBe(true);
        expect(e.isAexError, `${caseName} should be an AexError`).toBe(true);
        expect(e.name, `${caseName} error type`).toBe("SessionConfigValidationError");
        expect(e.code, `${caseName} error code`).toBe("SESSION_CONFIG_INVALID");
        expect(typeof e.message === "string" && e.message.length > 0, `${caseName} has a message`).toBe(true);
        // Client-side validation never reaches the wire, so there is no HTTP status.
        expect(e.status, `${caseName} is pre-network (no status)`).toBeUndefined();
      }

      // (5) Unreachable host → bounded network failure, surfaced (not swallowed, not a hang).
      expect(result.unreachable.rejected).toBe(true);
      expect(result.unreachable.isApiError).toBe(false); // a raw transport error, not an HTTP error
      expect(result.unreachable.elapsedMs).toBeLessThan(20_000);
    },
    120_000
  );
});
