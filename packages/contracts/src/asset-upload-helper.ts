import { extractErrorCode, redactUrl } from "./sdk-errors.js";
import {
  abortableSleep,
  computeRetryDelayMs,
  parseRetryAfterMs,
  type RetryBackoffConfig
} from "./retry-core.js";

export interface AssetUploadResponse {
  readonly ok: boolean;
  readonly status: number;
  text(): Promise<string>;
  readonly headers?: { get(name: string): string | null };
}

/** Minimal fetch shape for direct-to-storage PUT requests. */
export type AssetFetch = (input: string, init?: RequestInit) => Promise<AssetUploadResponse>;

export const DIRECT_UPLOAD_MAX_ATTEMPTS = 3;
export const DIRECT_UPLOAD_INITIAL_DELAY_MS = 500;
export const DIRECT_UPLOAD_MAX_DELAY_MS = 5_000;
export const DIRECT_UPLOAD_MAX_ELAPSED_MS = 30_000;

export type AssetUploadSleep = (ms: number, signal?: AbortSignal) => Promise<void>;

export interface AssetUploadRetryOptions {
  readonly maxAttempts?: number;
  readonly initialDelayMs?: number;
  readonly maxDelayMs?: number;
  readonly maxElapsedMs?: number;
  readonly sleep?: AssetUploadSleep;
  readonly random?: () => number;
  readonly now?: () => number;
}

export interface ResolvedAssetUploadRetryConfig extends RetryBackoffConfig {
  readonly maxAttempts: number;
  readonly maxElapsedMs: number;
}

export async function putDirectUploadWithRetry(
  fetchImpl: AssetFetch,
  uploadUrl: string,
  init: RequestInit,
  options: AssetUploadRetryOptions = {}
): Promise<void> {
  const config = resolveAssetUploadRetryConfig(options);
  const sleep = options.sleep ?? abortableSleep;
  const random = options.random ?? Math.random;
  const now = options.now ?? Date.now;
  const startedAt = now();

  for (let attempt = 1; attempt <= config.maxAttempts; attempt++) {
    let response: Awaited<ReturnType<AssetFetch>>;
    try {
      response = await fetchImpl(uploadUrl, init);
    } catch (err) {
      if (attempt < config.maxAttempts && isRetryableUploadError(err)) {
        const delay = directUploadRetryDelayMs(config, attempt, random, undefined);
        if (!withinDirectUploadRetryBudget(config, startedAt, delay, now)) {
          throw directUploadNetworkError(uploadUrl, err, attempt);
        }
        await sleep(delay, init.signal ?? undefined);
        continue;
      }
      throw directUploadNetworkError(uploadUrl, err, attempt);
    }

    if (response.ok) return;

    const retryAfterMs = parseRetryAfterMs(response.headers?.get("retry-after"), now());
    if (attempt < config.maxAttempts && isRetryableUploadStatus(response.status)) {
      const delay = directUploadRetryDelayMs(config, attempt, random, retryAfterMs);
      if (!withinDirectUploadRetryBudget(config, startedAt, delay, now)) {
        const detail = await response.text().catch(() => "");
        throw directUploadResponseError(uploadUrl, response.status, detail, attempt);
      }
      await response.text().catch(() => "");
      await sleep(delay, init.signal ?? undefined);
      continue;
    }

    const detail = await response.text().catch(() => "");
    throw directUploadResponseError(uploadUrl, response.status, detail, attempt);
  }
}

export function resolveAssetUploadRetryConfig(options: AssetUploadRetryOptions = {}): ResolvedAssetUploadRetryConfig {
  const maxAttempts = Math.max(1, Math.floor(options.maxAttempts ?? DIRECT_UPLOAD_MAX_ATTEMPTS));
  const initialDelayMs = Math.max(0, options.initialDelayMs ?? DIRECT_UPLOAD_INITIAL_DELAY_MS);
  const maxDelayMs = Math.max(initialDelayMs, options.maxDelayMs ?? DIRECT_UPLOAD_MAX_DELAY_MS);
  const maxElapsedMs = Math.max(0, options.maxElapsedMs ?? DIRECT_UPLOAD_MAX_ELAPSED_MS);
  return { maxAttempts, initialDelayMs, maxDelayMs, maxElapsedMs };
}

export function directUploadRetryDelayMs(
  config: RetryBackoffConfig,
  attempt: number,
  random: () => number,
  retryAfterMs: number | undefined
): number {
  return computeRetryDelayMs(config, attempt, random, retryAfterMs);
}

export function withinDirectUploadRetryBudget(
  config: Pick<ResolvedAssetUploadRetryConfig, "maxElapsedMs">,
  startedAt: number,
  delayMs: number,
  now: () => number
): boolean {
  return now() - startedAt + delayMs <= config.maxElapsedMs;
}

export function isRetryableUploadStatus(status: number): boolean {
  return status === 408 || status === 425 || status === 429 || (status >= 500 && status <= 599);
}

export function isRetryableUploadError(err: unknown): boolean {
  if (isNamedError(err, "AbortError")) return false;
  return true;
}

export function directUploadNetworkError(uploadUrl: string, err: unknown, attempts: number): Error {
  const safeUrl = redactUrl(uploadUrl);
  const code = extractErrorCode(err);
  const detail = sanitizeUploadText(errorMessage(err)).slice(0, 500);
  return new Error(
    `uploadAsset: direct upload PUT failed for ${safeUrl} after ${attemptsLabel(attempts)}` +
      (code ? ` (${code})` : "") +
      (detail ? `: ${detail}` : "")
  );
}

export function directUploadResponseError(uploadUrl: string, status: number, detail: string, attempts: number): Error {
  const safeUrl = redactUrl(uploadUrl);
  const safeDetail = sanitizeUploadText(detail).slice(0, 500);
  return new Error(
    `uploadAsset: direct upload PUT failed for ${safeUrl} with status ${status}` +
      (attempts > 1 ? ` after ${attemptsLabel(attempts)}` : "") +
      (safeDetail ? `: ${safeDetail}` : "")
  );
}

export function sanitizeUploadText(text: string): string {
  return text
    .replace(/https?:\/\/[^\s<>"'`]+/g, (raw) => redactUrlPreservingTrailingPunctuation(raw))
    .replace(
      /\b(?:X-Amz-(?:Algorithm|Credential|Date|Expires|Security-Token|Signature|SignedHeaders)|AWSAccessKeyId|Signature|Credential|Security-Token|AccessKeyId|SecretAccessKey|SessionToken)=([^&\s<>"'`]+)/gi,
      "[redacted]"
    )
    .replace(/\bAKIA[0-9A-Z]{8,}\b/g, "[redacted]");
}

function attemptsLabel(attempts: number): string {
  return attempts === 1 ? "1 attempt" : `${attempts} attempts`;
}

function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message || err.name;
  if (typeof err === "string") return err;
  return String(err);
}

function isNamedError(err: unknown, name: string): boolean {
  return stringProperty(err, "name") === name;
}

function stringProperty(value: unknown, key: string): string | undefined {
  if (!value || typeof value !== "object") return undefined;
  const prop = (value as Record<string, unknown>)[key];
  return typeof prop === "string" && prop.length > 0 ? prop : undefined;
}

function redactUrlPreservingTrailingPunctuation(raw: string): string {
  const trailing = raw.match(/[),.;:!?]+$/)?.[0] ?? "";
  const candidate = trailing ? raw.slice(0, -trailing.length) : raw;
  return `${redactUrl(candidate)}${trailing}`;
}
