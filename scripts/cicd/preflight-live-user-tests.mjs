#!/usr/bin/env bun
import { appendFileSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const REQUIRED_ENV = ["AEX_API_URL", "AEX_API_KEY"];
const DEFAULT_ATTEMPTS = 4;
const DEFAULT_TIMEOUT_MS = 30_000;
const DEFAULT_BASE_DELAY_MS = 1_000;
const DEFAULT_MAX_DELAY_MS = 8_000;

class PreflightFatalError extends Error {}

export function retryDelayMs(attempt, options = {}) {
  const base = positiveInt(options.baseDelayMs, DEFAULT_BASE_DELAY_MS);
  const maximum = positiveInt(options.maxDelayMs, DEFAULT_MAX_DELAY_MS);
  return Math.min(maximum, base * attempt);
}

export function retryAfterDelayMs(value, attempt, options = {}) {
  const maximum = positiveInt(options.maxDelayMs, DEFAULT_MAX_DELAY_MS);
  const seconds = Number.parseInt(String(value ?? "").trim(), 10);
  return Number.isInteger(seconds) && seconds >= 1
    ? Math.min(maximum, seconds * 1_000)
    : retryDelayMs(attempt, options);
}

export function isRetryablePreflightStatus(status) {
  return status === 408 || status === 409 || status === 425 || status === 429 || status >= 500;
}

export function fetchFailureCode(error) {
  for (const candidate of causeCandidates(error)) {
    if (typeof candidate.code === "string" && candidate.code.trim()) return candidate.code;
    if (typeof candidate.name === "string" && /AbortError|TimeoutError/.test(candidate.name)) return candidate.name;
    const match = /\b(E[A-Z0-9_]+|UND_ERR_[A-Z0-9_]+)\b/.exec(String(candidate.message ?? ""));
    if (match?.[1]) return match[1];
  }
  return "unknown";
}

export function isRetryableFetchFailure(error) {
  return new Set([
    "unknown",
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
  ]).has(fetchFailureCode(error));
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
  const expectedHost = String(env.AEX_EXPECTED_API_HOST ?? "").trim();
  if (expectedHost && apiUrl.hostname !== expectedHost) {
    throw new PreflightFatalError(
      `live-user-tests AEX_API_URL host=${apiUrl.hostname} does not match expected host=${expectedHost}.`
    );
  }
  const attempts = envInt(env, "LIVE_USER_TEST_PREFLIGHT_ATTEMPTS", DEFAULT_ATTEMPTS, 10);
  const timeoutMs = envInt(env, "LIVE_USER_TEST_PREFLIGHT_TIMEOUT_MS", DEFAULT_TIMEOUT_MS, 120_000);
  const baseDelayMs = envInt(env, "LIVE_USER_TEST_PREFLIGHT_RETRY_BASE_MS", DEFAULT_BASE_DELAY_MS, 60_000);
  const maxDelayMs = envInt(env, "LIVE_USER_TEST_PREFLIGHT_RETRY_MAX_MS", DEFAULT_MAX_DELAY_MS, 120_000);
  const url = new URL("/api/sessions?limit=1", apiUrl);

  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      const response = await fetchWithTimeout(fetchImpl, url, env.AEX_API_KEY, timeoutMs);
      const requestId = responseRequestId(response);
      const body = parseJsonObject(await response.text().catch(() => ""));
      if (!response.ok) {
        if (attempt < attempts && isRetryablePreflightStatus(response.status)) {
          const delay = retryAfterDelayMs(response.headers.get("retry-after"), attempt, {
            baseDelayMs,
            maxDelayMs
          });
          err.write(
            `live-user-tests /api/sessions transient HTTP ${response.status} ` +
              `(requestId=${requestId}, host=${url.host}) attempt ${attempt}/${attempts}; retrying in ${delay}ms\n`
          );
          await sleepFn(delay);
          continue;
        }
        throw new PreflightFatalError(
          `live-user-tests /api/sessions preflight failed ` +
            `(status=${response.status}, requestId=${requestId}, bodyCode=${bodyCodeOf(body)}).`
        );
      }
      if (!Array.isArray(body.items)) {
        throw new PreflightFatalError(
          `live-user-tests /api/sessions response did not contain an items array (requestId=${requestId}).`
        );
      }
      out.write(
        `live-user-tests /api/sessions preflight passed ` +
          `(status=${response.status}, requestId=${requestId}, attempt=${attempt}/${attempts}).\n`
      );
      return { status: response.status, requestId, attempt, attempts };
    } catch (error) {
      if (error instanceof PreflightFatalError) throw error;
      if (attempt < attempts && isRetryableFetchFailure(error)) {
        const delay = retryDelayMs(attempt, { baseDelayMs, maxDelayMs });
        err.write(
          `live-user-tests /api/sessions transient fetch failure ` +
            `(code=${fetchFailureCode(error)}, host=${url.host}) attempt ${attempt}/${attempts}; retrying in ${delay}ms\n`
        );
        await sleepFn(delay);
        continue;
      }
      throw error;
    }
  }
  throw new Error("live-user-tests /api/sessions preflight retry loop exhausted.");
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
    return parsed !== null && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function bodyCodeOf(body) {
  return typeof body.error === "string"
    ? body.error
    : typeof body.code === "string"
      ? body.code
      : "(none)";
}

function responseRequestId(response) {
  return response.headers.get("x-amzn-requestid") ??
    response.headers.get("x-request-id") ??
    response.headers.get("apigw-requestid") ??
    "unknown";
}

function positiveInt(value, fallback, maximum = Number.POSITIVE_INFINITY) {
  const parsed = Number.parseInt(String(value ?? ""), 10);
  return Number.isFinite(parsed) && parsed >= 1 ? Math.min(parsed, maximum) : fallback;
}

function envInt(env, name, fallback, maximum) {
  return positiveInt(env[name], fallback, maximum);
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
  if (
    String(env.LIVE_USER_TEST_ALLOW_PRIVATE_API_URL ?? "").trim().toLowerCase() !== "true" &&
    isPrivateHost(parsed.hostname)
  ) {
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
  const [first, second] = ipv4.slice(1, 3).map((part) => Number.parseInt(part, 10));
  return first === 10 ||
    first === 127 ||
    (first === 169 && second === 254) ||
    (first === 172 && second >= 16 && second <= 31) ||
    (first === 192 && second === 168);
}

function causeCandidates(error) {
  const out = [];
  const seen = new Set();
  const visit = (value) => {
    if (value === null || typeof value !== "object" || seen.has(value)) return;
    seen.add(value);
    out.push(value);
    visit(value.cause);
    if (Array.isArray(value.errors)) value.errors.forEach(visit);
  };
  visit(error);
  return out;
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  checkLiveUserTestsPreflight()
    .then(() => {
      const githubOutput = String(process.env.GITHUB_OUTPUT ?? "").trim();
      if (githubOutput) appendFileSync(githubOutput, "preflight_status=passed\n");
    })
    .catch((error) => {
      process.stderr.write(`::error::${error instanceof Error ? error.message : String(error)}\n`);
      process.exit(1);
    });
}
