import { extractErrorCode, redactUrl } from "./sdk-errors.js";
import { isRetryableHttpStatus, tryParseRetryAfterMs } from "./retry-core.js";
import {
  nextHttpRetryDelayMs,
  resolveHttpRetryDeps,
  resolveHttpRetryPolicy,
  type HttpRetryDeps,
  type HttpRetryOptions
} from "./http.js";

export interface AssetUploadResponse {
  readonly ok: boolean;
  readonly status: number;
  text(): Promise<string>;
  readonly headers?: { get(name: string): string | null };
}

/** Minimal fetch shape for direct-to-storage PUT requests. */
export type AssetFetch = (input: string, init?: RequestInit) => Promise<AssetUploadResponse>;

export type AssetUploadSleep = NonNullable<HttpRetryDeps["sleep"]>;

/**
 * The shared {@link HttpRetryOptions} plus the injectable clock/RNG/sleep the
 * upload loops need. Direct-to-storage PUTs use the SAME policy numbers as the
 * API transport — there is no third set of defaults.
 */
export interface AssetUploadRetryOptions extends HttpRetryOptions, HttpRetryDeps {}

export async function putDirectUploadWithRetry(
  fetchImpl: AssetFetch,
  uploadUrl: string,
  init: RequestInit,
  options: AssetUploadRetryOptions = {}
): Promise<void> {
  const policy = resolveHttpRetryPolicy(options);
  const deps = resolveHttpRetryDeps(options);
  const startedAtMs = deps.now();

  for (let attempt = 1; attempt <= policy.maxAttempts; attempt++) {
    let response: Awaited<ReturnType<AssetFetch>>;
    try {
      response = await fetchImpl(uploadUrl, init);
    } catch (err) {
      const delayMs = isRetryableUploadError(err)
        ? nextHttpRetryDelayMs({ policy, attempt, startedAtMs, deps })
        : undefined;
      if (delayMs === undefined) throw directUploadNetworkError(uploadUrl, err, attempt);
      await deps.sleep(delayMs, init.signal ?? undefined);
      continue;
    }

    if (response.ok) return;

    const retryAfterMs = tryParseRetryAfterMs(response.headers?.get("retry-after"), deps.now());
    const delayMs = isRetryableUploadStatus(response.status)
      ? nextHttpRetryDelayMs({ policy, attempt, startedAtMs, deps, retryAfterMs })
      : undefined;
    if (delayMs !== undefined) {
      await response.text().catch(() => "");
      await deps.sleep(delayMs, init.signal ?? undefined);
      continue;
    }

    const detail = await response.text().catch(() => "");
    throw directUploadResponseError(uploadUrl, response.status, detail, attempt);
  }
}

/**
 * Object-store PUT retry set: the shared HTTP set from `retry-core.ts`, widened
 * by the two statuses only a storage endpoint emits (408 Request Timeout, 425
 * Too Early) plus the remaining 5xx an S3-compatible endpoint can return.
 */
export function isRetryableUploadStatus(status: number): boolean {
  return isRetryableHttpStatus(status) || status === 408 || status === 425 || (status >= 500 && status <= 599);
}

/** Every transport rejection except a caller-initiated abort is transient for a PUT. */
export function isRetryableUploadError(err: unknown): boolean {
  return (err as { readonly name?: unknown } | null | undefined)?.name !== "AbortError";
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

function redactUrlPreservingTrailingPunctuation(raw: string): string {
  const trailing = raw.match(/[),.;:!?]+$/)?.[0] ?? "";
  const candidate = trailing ? raw.slice(0, -trailing.length) : raw;
  return `${redactUrl(candidate)}${trailing}`;
}
