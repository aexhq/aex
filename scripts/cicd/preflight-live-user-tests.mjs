#!/usr/bin/env bun
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const REQUIRED_ENV = ["AEX_API_URL", "AEX_API_KEY", "DEEPSEEK_API_KEY"];
const DEFAULT_ATTEMPTS = 4;
const DEFAULT_TIMEOUT_MS = 30_000;
const DEFAULT_BASE_DELAY_MS = 1_000;
const DEFAULT_MAX_DELAY_MS = 8_000;
const DEFAULT_MIN_MAX_CONCURRENT_SESSIONS = 50;
const DEFAULT_REQUIRED_SCOPES = ["sessions:read", "sessions:write", "files:read"];

class PreflightFatalError extends Error {}

export function retryDelayMs(attempt, options = {}) {
  const baseDelayMs = positiveInt(options.baseDelayMs, DEFAULT_BASE_DELAY_MS);
  const maxDelayMs = positiveInt(options.maxDelayMs, DEFAULT_MAX_DELAY_MS);
  return Math.min(maxDelayMs, baseDelayMs * attempt);
}

export function retryAfterDelayMs(value, attempt, options = {}) {
  const maxDelayMs = positiveInt(options.maxDelayMs, DEFAULT_MAX_DELAY_MS);
  const seconds = Number.parseInt(String(value ?? "").trim(), 10);
  if (!Number.isInteger(seconds) || seconds < 1) return retryDelayMs(attempt, options);
  return Math.min(maxDelayMs, seconds * 1000);
}

export function isRetryableWhoamiStatus(status) {
  return status === 408 || status === 409 || status === 425 || status === 429 || status >= 500;
}

export function fetchFailureCode(error) {
  for (const candidate of causeCandidates(error)) {
    if (typeof candidate.code === "string" && candidate.code.trim() !== "") return candidate.code;
    if (typeof candidate.name === "string" && /AbortError|TimeoutError/.test(candidate.name)) return candidate.name;
    const message = typeof candidate.message === "string" ? candidate.message : "";
    const codeMatch = /\b(E[A-Z0-9_]+|UND_ERR_[A-Z0-9_]+)\b/.exec(message);
    if (codeMatch?.[1]) return codeMatch[1];
  }
  return "unknown";
}

export function isRetryableFetchFailure(error) {
  const code = fetchFailureCode(error);
  return code === "unknown" || new Set([
    "AbortError",
    "ECONNABORTED",
    "ECONNREFUSED",
    "ECONNRESET",
    "EAI_AGAIN",
    "ENETDOWN",
    "ENETRESET",
    "ENETUNREACH",
    "ETIMEDOUT",
    "TimeoutError",
    "UND_ERR_BODY_TIMEOUT",
    "UND_ERR_CONNECT_TIMEOUT",
    "UND_ERR_HEADERS_TIMEOUT",
    "UND_ERR_SOCKET"
  ]).has(code);
}

export async function checkLiveUserTestsPreflight(options = {}) {
  const env = options.env ?? process.env;
  const out = options.out ?? process.stdout;
  const err = options.err ?? process.stderr;
  const fetchImpl = options.fetchImpl ?? globalThis.fetch;
  const sleepFn = options.sleepFn ?? sleep;

  const missing = REQUIRED_ENV.filter((name) => typeof env[name] !== "string" || env[name].trim() === "");
  if (missing.length > 0) {
    throw new Error(`live-user-tests environment is missing required value(s): ${missing.join(", ")}`);
  }

  const apiUrl = parseApiUrl(env.AEX_API_URL, env);
  const attempts = envInt(env, "LIVE_USER_TEST_PREFLIGHT_ATTEMPTS", DEFAULT_ATTEMPTS, 10);
  const timeoutMs = envInt(env, "LIVE_USER_TEST_PREFLIGHT_TIMEOUT_MS", DEFAULT_TIMEOUT_MS, 120_000);
  const baseDelayMs = envInt(env, "LIVE_USER_TEST_PREFLIGHT_RETRY_BASE_MS", DEFAULT_BASE_DELAY_MS, 60_000);
  const maxDelayMs = envInt(env, "LIVE_USER_TEST_PREFLIGHT_RETRY_MAX_MS", DEFAULT_MAX_DELAY_MS, 120_000);
  const minMaxConcurrentSessions = envInt(
    env,
    "LIVE_USER_TEST_MIN_MAX_CONCURRENT_SESSIONS",
    DEFAULT_MIN_MAX_CONCURRENT_SESSIONS,
    10_000
  );
  const maxMaxConcurrentSessions = optionalPositiveInt(env.LIVE_USER_TEST_MAX_MAX_CONCURRENT_SESSIONS, 10_000);
  const requiredScopes = parseRequiredScopes(env.LIVE_USER_TEST_REQUIRED_SCOPES);
  const expectedApiHost = String(env.AEX_EXPECTED_API_HOST ?? "").trim();
  if (expectedApiHost && apiUrl.hostname !== expectedApiHost) {
    throw new PreflightFatalError(
      `live-user-tests AEX_API_URL host=${apiUrl.hostname} does not match expected host=${expectedApiHost}.`
    );
  }
  const url = new URL("/api/whoami", apiUrl);

  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      const res = await fetchWithTimeout(fetchImpl, url, env.AEX_API_KEY, timeoutMs);
      const text = await res.text().catch(() => "");
      const body = parseJsonObject(text);
      const requestId = responseRequestId(res);
      if (!res.ok) {
        const bodyCode = bodyCodeOf(body);
        if (attempt < attempts && isRetryableWhoamiStatus(res.status)) {
          const delayMs = retryAfterDelayMs(res.headers.get("retry-after"), attempt, { baseDelayMs, maxDelayMs });
          err.write(
            `live-user-tests /api/whoami transient HTTP ${res.status} (bodyCode=${bodyCode}, requestId=${requestId}, host=${url.host}) attempt ${attempt}/${attempts}; retrying in ${delayMs}ms\n`
          );
          await sleepFn(delayMs);
          continue;
        }
        throw new PreflightFatalError(
          `live-user-tests /api/whoami preflight failed (status=${res.status}, requestId=${requestId}, bodyCode=${bodyCode}). Check the AEX_API_URL/AEX_API_KEY pairing.`
        );
      }

      const maxConcurrentSessions = body?.limits?.maxConcurrentSessions;
      if (!Number.isInteger(maxConcurrentSessions)) {
        throw new PreflightFatalError(
          `live-user-tests /api/whoami response did not include integer limits.maxConcurrentSessions (requestId=${requestId}).`
        );
      }
      if (maxConcurrentSessions < minMaxConcurrentSessions) {
        throw new PreflightFatalError(
          `live-user-tests workspace maxConcurrentSessions=${maxConcurrentSessions} is below required minimum ${minMaxConcurrentSessions} (requestId=${requestId}).`
        );
      }
      if (maxMaxConcurrentSessions !== null && maxConcurrentSessions > maxMaxConcurrentSessions) {
        throw new PreflightFatalError(
          `live-user-tests workspace maxConcurrentSessions=${maxConcurrentSessions} is above allowed maximum ${maxMaxConcurrentSessions} (requestId=${requestId}).`
        );
      }
      const scopes = new Set(Array.isArray(body?.scopes) ? body.scopes.filter((scope) => typeof scope === "string") : []);
      const missingScopes = requiredScopes.filter((scope) => !scopes.has(scope));
      if (missingScopes.length > 0) {
        throw new PreflightFatalError(
          `live-user-tests /api/whoami token is missing required scope(s): ${missingScopes.join(", ")} (requestId=${requestId}).`
        );
      }
      out.write(
        `live-user-tests /api/whoami preflight passed (status=${res.status}, requestId=${requestId}, maxConcurrentSessions=${maxConcurrentSessions}, requiredScopes=${requiredScopes.join(",")}, attempt=${attempt}/${attempts}).\n`
      );
      return { status: res.status, requestId, maxConcurrentSessions, requiredScopes, attempt, attempts };
    } catch (error) {
      if (error instanceof PreflightFatalError) throw error;
      if (attempt < attempts && isRetryableFetchFailure(error)) {
        const delayMs = retryDelayMs(attempt, { baseDelayMs, maxDelayMs });
        err.write(
          `live-user-tests /api/whoami transient fetch failure (code=${fetchFailureCode(error)}, host=${url.host}) attempt ${attempt}/${attempts}; retrying in ${delayMs}ms\n`
        );
        await sleepFn(delayMs);
        continue;
      }
      throw error;
    }
  }

  throw new Error("live-user-tests /api/whoami preflight retry loop exhausted.");
}

async function fetchWithTimeout(fetchImpl, url, token, timeoutMs) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(new Error("AEX_LIVE_PREFLIGHT_TIMEOUT")), timeoutMs);
  timeout.unref?.();
  try {
    return await fetchImpl(url, {
      headers: { authorization: `Bearer ${token}` },
      signal: controller.signal
    });
  } finally {
    clearTimeout(timeout);
  }
}

function parseJsonObject(text) {
  try {
    const parsed = text ? JSON.parse(text) : {};
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function bodyCodeOf(body) {
  return typeof body?.error === "string" ? body.error : typeof body?.code === "string" ? body.code : "(none)";
}

function responseRequestId(res) {
  return (
    res.headers.get("x-amzn-requestid") ??
    res.headers.get("x-request-id") ??
    res.headers.get("apigw-requestid") ??
    "unknown"
  );
}

function envInt(env, name, fallback, max) {
  return positiveInt(env[name], fallback, max);
}

function optionalPositiveInt(value, max) {
  const raw = String(value ?? "").trim();
  if (!raw) return null;
  return positiveInt(raw, 0, max);
}

function parseRequiredScopes(value) {
  const raw = String(value ?? "").trim();
  if (!raw) return DEFAULT_REQUIRED_SCOPES;
  return raw
    .split(",")
    .map((scope) => scope.trim())
    .filter((scope) => scope.length > 0);
}

function positiveInt(value, fallback, max = Number.POSITIVE_INFINITY) {
  const parsed = Number.parseInt(String(value ?? ""), 10);
  if (!Number.isFinite(parsed) || parsed < 1) return fallback;
  return Math.min(parsed, max);
}

function parseApiUrl(value, env) {
  let parsed;
  try {
    parsed = new URL(String(value).trim());
  } catch {
    throw new PreflightFatalError("live-user-tests AEX_API_URL must be an absolute URL.");
  }
  if (parsed.protocol !== "https:") {
    throw new PreflightFatalError(`live-user-tests AEX_API_URL must use https (host=${parsed.host}).`);
  }
  if (String(env.LIVE_USER_TEST_ALLOW_PRIVATE_API_URL ?? "").trim().toLowerCase() !== "true" && isPrivateHost(parsed.hostname)) {
    throw new PreflightFatalError(`live-user-tests AEX_API_URL host=${parsed.hostname} is not a public live endpoint.`);
  }
  parsed.pathname = parsed.pathname.replace(/\/+$/, "");
  parsed.search = "";
  parsed.hash = "";
  return parsed;
}

function isPrivateHost(hostname) {
  const host = hostname.toLowerCase();
  if (host === "localhost" || host.endsWith(".localhost") || host.endsWith(".local")) return true;
  if (host === "::1" || host === "[::1]") return true;
  const ipv4 = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/);
  if (!ipv4) return false;
  const [a, b] = ipv4.slice(1, 3).map((part) => Number.parseInt(part, 10));
  return a === 10 || a === 127 || (a === 169 && b === 254) || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168);
}

function causeCandidates(error) {
  const out = [];
  const seen = new Set();
  const visit = (value) => {
    if (value === null || typeof value !== "object" || seen.has(value)) return;
    seen.add(value);
    out.push(value);
    visit(value.cause);
    if (Array.isArray(value.errors)) {
      for (const inner of value.errors) visit(inner);
    }
  };
  visit(error);
  return out;
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  checkLiveUserTestsPreflight().catch((error) => {
    process.stderr.write(`::error::${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  });
}
