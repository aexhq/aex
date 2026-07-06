/**
 * Asset materialization for the SDK run / session path.
 *
 * Every inline `Skill` / `Tool` / `AgentsMd` / `File` draft is materialized
 * to the hosted API's content-addressable asset store before the session
 * starts, so the wire submission carries only storage-neutral refs.
 *
 * Upload is direct-to-storage (no bytes through the hosted API, so bundle size
 * is bounded by the object store rather than by the API's memory /
 * request-payload limits):
 *
 *   1. POST /assets/presign  → { exists } | { uploadUrl, requiredHeaders }
 *      - `exists:true` is a dedup hit; we're done.
 *      - otherwise the hosted API mints a presigned PUT scoped to the exact
 *        content-addressed key and signs `x-amz-checksum-sha256` so the object
 *        store enforces integrity server-side.
 *   2. PUT the bytes straight to `uploadUrl` with `requiredHeaders` (the signed
 *      checksum). The store rejects a byte mismatch — a 2xx proves bytes == hash.
 *   3. POST /assets/finalize → confirms the object exists (HEAD only).
 *
 */

import { extractErrorCode, redactUrl } from "@aexhq/contracts";
import type { AssetFetch, AssetsHttpClient, UploadedAsset } from "@aexhq/contracts/internal";
import type { ByteSink } from "./canonical-zip.js";

export { uploadAsset } from "@aexhq/contracts/internal";
export type { AssetFetch, AssetsHttpClient, UploadAssetArgs, UploadedAsset } from "@aexhq/contracts/internal";

const DIRECT_UPLOAD_MAX_ATTEMPTS = 3;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

function assetIdFromContentHash(contentHash: string): string {
  const hex = contentHash.startsWith("sha256:") ? contentHash.slice("sha256:".length) : contentHash;
  return `asset_${hex}`;
}

// ===========================================================================
// Multipart streaming upload (large assets — the "kill the in-memory ceiling"
// path). Two-pass over a re-openable, DETERMINISTIC canonical-zip byte stream:
//   pass 1 — stream → running SHA-256 → presign BY HASH (dedup still short-
//            circuits `exists:true` with zero bytes uploaded);
//   pass 2 — re-stream → chunk into parts → S3 multipart UploadPart (bounded
//            concurrency + per-part retry, only the failed part re-sent) →
//            CompleteMultipartUpload (server validates the full-object SHA-256).
// A failure after presign aborts the multipart upload so no orphaned parts bill.
// ===========================================================================

/** Default multipart part size — 16 MiB covers the 100 GiB cap within S3's 10 000-part limit. */
export const DEFAULT_MULTIPART_PART_SIZE = 16 * 1024 * 1024;
/** Default in-flight part concurrency. */
export const DEFAULT_MULTIPART_CONCURRENCY = 4;

/**
 * Drives the canonical-zip framer, pushing every zip byte into `sink` in order.
 * Called TWICE (pass 1 hash, pass 2 upload); the framer is deterministic so both
 * passes emit byte-identical streams.
 */
export type ZipStreamDriver = (sink: ByteSink) => Promise<void>;

export interface UploadAssetStreamArgs {
  readonly http: AssetsHttpClient;
  /** Re-openable, deterministic canonical-zip stream driver (pass-1 hash == pass-2 bytes). */
  readonly drive: ZipStreamDriver;
  readonly contentType?: string;
  readonly fetch?: AssetFetch;
  readonly partSize?: number;
  readonly partConcurrency?: number;
}

interface MultipartPresign {
  readonly uploadId: string;
  readonly key: string;
  readonly partSize: number;
  readonly partCount: number;
  readonly partUrls: ReadonlyArray<{ readonly partNumber: number; readonly url: string }>;
  readonly expiresInSeconds?: number;
}

/** Stream a large asset to the content store via the two-pass multipart flow. */
export async function uploadAssetMultipart(args: UploadAssetStreamArgs): Promise<UploadedAsset> {
  const partSize = args.partSize ?? DEFAULT_MULTIPART_PART_SIZE;
  const concurrency = Math.max(1, args.partConcurrency ?? DEFAULT_MULTIPART_CONCURRENCY);
  const doFetch = args.fetch ?? (globalThis.fetch as unknown as AssetFetch);

  // ---- Pass 1: stream → running SHA-256 + byte count ----
  const { hashHex, sizeBytes } = await hashAndSizeViaDrive(args.drive);
  const contentHashHeader = `sha256:${hashHex}`;

  // ---- Presign (multipart) — dedup short-circuits before any bytes move ----
  const presign = await args.http.request<{
    ok: boolean;
    exists: boolean;
    assetId?: string;
    contentHash?: string;
    sizeBytes?: number;
    multipart?: MultipartPresign;
  }>("/assets/presign", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ hash: contentHashHeader, sizeBytes, multipart: true, partSize })
  });

  if (presign.exists) {
    const contentHash = presign.contentHash ?? contentHashHeader;
    return {
      assetId: presign.assetId ?? assetIdFromContentHash(contentHash),
      contentHash,
      sizeBytes: presign.sizeBytes ?? sizeBytes,
      exists: true
    };
  }
  const mp = presign.multipart;
  if (!mp || !mp.uploadId || !mp.key || !Array.isArray(mp.partUrls)) {
    throw new Error("uploadAssetMultipart: presign returned no multipart plan and exists:false");
  }

  // ---- Pass 2: re-stream → parts → UploadPart (bounded concurrency + retry) ----
  const urlByPart = new Map<number, string>();
  for (const p of mp.partUrls) urlByPart.set(p.partNumber, p.url);
  const parts: Array<{ partNumber: number; etag: string }> = [];
  const inflight = new Set<Promise<void>>();
  let failed: unknown;

  const refreshPartUrl = async (partNumber: number): Promise<string> => {
    const res = await args.http.request<{ partUrls?: ReadonlyArray<{ partNumber: number; url: string }> }>(
      "/assets/mpu/presign-parts",
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ uploadId: mp.uploadId, key: mp.key, partNumbers: [partNumber] })
      }
    );
    const fresh = res.partUrls?.find((p) => p.partNumber === partNumber)?.url;
    if (!fresh) throw new Error(`uploadAssetMultipart: could not refresh URL for part ${partNumber}`);
    urlByPart.set(partNumber, fresh);
    return fresh;
  };

  const uploadOnePart = async (partNumber: number, bytes: Uint8Array): Promise<void> => {
    let url = urlByPart.get(partNumber);
    if (!url) url = await refreshPartUrl(partNumber);
    const etag = await putPartWithRetry(doFetch, url, bytes, args.contentType, () => refreshPartUrl(partNumber));
    parts.push({ partNumber, etag });
  };

  const submitPart = async (partNumber: number, bytes: Uint8Array): Promise<void> => {
    while (inflight.size >= concurrency) await Promise.race(inflight);
    if (failed !== undefined) throw failed;
    const tracked = uploadOnePart(partNumber, bytes)
      .catch((err) => {
        failed ??= err;
      })
      .finally(() => {
        inflight.delete(tracked);
      });
    inflight.add(tracked);
  };

  try {
    let partNumber = 0;
    const chunker = makePartChunker(partSize, async (bytes) => {
      partNumber += 1;
      await submitPart(partNumber, bytes);
    });
    await args.drive(async (chunk) => {
      await chunker.push(chunk);
      if (failed !== undefined) throw failed;
    });
    await chunker.flush();
    await Promise.all(inflight);
    if (failed !== undefined) throw failed;

    if (partNumber === 0) {
      // Zero-length streams cannot happen for a real zip (EOCD is always emitted),
      // but guard so a bug fails loud rather than completing an empty MPU.
      throw new Error("uploadAssetMultipart: no parts produced from the zip stream");
    }

    parts.sort((a, b) => a.partNumber - b.partNumber);
    const fin = await args.http.request<{
      ok: boolean;
      assetId?: string;
      contentHash?: string;
      sizeBytes?: number;
    }>("/assets/finalize", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ hash: contentHashHeader, sizeBytes, uploadId: mp.uploadId, key: mp.key, parts })
    });
    const contentHash = fin.contentHash ?? contentHashHeader;
    return {
      assetId: fin.assetId ?? assetIdFromContentHash(contentHash),
      contentHash,
      sizeBytes: fin.sizeBytes ?? sizeBytes,
      exists: false
    };
  } catch (err) {
    // Abort so orphaned parts don't bill silently (backed by the S3 lifecycle rule too).
    await args.http
      .request("/assets/mpu/abort", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ uploadId: mp.uploadId, key: mp.key })
      })
      .catch(() => undefined);
    throw err;
  }
}

/** Drive the framer through SHA-256, returning the hex digest + total byte count. Node/Bun only. */
async function hashAndSizeViaDrive(drive: ZipStreamDriver): Promise<{ hashHex: string; sizeBytes: number }> {
  const { createHash } = await import("node:crypto");
  const hash = createHash("sha256");
  let sizeBytes = 0;
  await drive((chunk) => {
    hash.update(chunk);
    sizeBytes += chunk.length;
  });
  return { hashHex: hash.digest("hex"), sizeBytes };
}

/**
 * Repackage an arbitrary-boundary byte stream into fixed `partSize` parts (the
 * last may be smaller), invoking `emit` once per full part. Efficient: front-of-
 * queue splitting, no O(n²) buffer growth.
 */
function makePartChunker(
  partSize: number,
  emit: (bytes: Uint8Array) => Promise<void>
): { push: (chunk: Uint8Array) => Promise<void>; flush: () => Promise<void> } {
  const chunks: Uint8Array[] = [];
  let buffered = 0;
  const emitPart = async (size: number): Promise<void> => {
    const part = new Uint8Array(size);
    let off = 0;
    while (off < size) {
      const head = chunks[0]!;
      const take = Math.min(head.length, size - off);
      part.set(head.subarray(0, take), off);
      off += take;
      if (take === head.length) chunks.shift();
      else chunks[0] = head.subarray(take);
    }
    buffered -= size;
    await emit(part);
  };
  return {
    push: async (chunk: Uint8Array): Promise<void> => {
      if (chunk.length === 0) return;
      chunks.push(chunk);
      buffered += chunk.length;
      while (buffered >= partSize) await emitPart(partSize);
    },
    flush: async (): Promise<void> => {
      if (buffered > 0) await emitPart(buffered);
    }
  };
}

/** PUT one part with bounded retry; refreshes the URL once on a 403 (expiry). Returns the ETag. */
async function putPartWithRetry(
  fetchImpl: AssetFetch,
  url: string,
  bytes: Uint8Array,
  contentType: string | undefined,
  refreshUrl: () => Promise<string>
): Promise<string> {
  let currentUrl = url;
  let refreshed = false;
  for (let attempt = 1; attempt <= DIRECT_UPLOAD_MAX_ATTEMPTS; attempt++) {
    let response: Awaited<ReturnType<AssetFetch>>;
    try {
      response = await fetchImpl(currentUrl, {
        method: "PUT",
        headers: { "content-type": contentType ?? "application/zip" },
        body: bytes as unknown as BodyInit
      });
    } catch (err) {
      if (attempt < DIRECT_UPLOAD_MAX_ATTEMPTS && isRetryableUploadError(err)) continue;
      throw directUploadNetworkError(currentUrl, err, attempt);
    }
    if (response.ok) {
      const etag = response.headers?.get?.("etag") ?? response.headers?.get?.("ETag") ?? "";
      return etag.replace(/"/g, "");
    }
    // A 403 mid-upload is a presign-expiry; refresh the URL once and retry.
    if (response.status === 403 && !refreshed) {
      refreshed = true;
      await response.text().catch(() => "");
      currentUrl = await refreshUrl();
      continue;
    }
    if (attempt < DIRECT_UPLOAD_MAX_ATTEMPTS && isRetryableUploadStatus(response.status)) {
      await response.text().catch(() => "");
      continue;
    }
    const detail = await response.text().catch(() => "");
    throw directUploadResponseError(currentUrl, response.status, detail, attempt);
  }
  throw directUploadResponseError(currentUrl, 0, "exhausted retries", DIRECT_UPLOAD_MAX_ATTEMPTS);
}

function isRetryableUploadStatus(status: number): boolean {
  return status === 408 || status === 425 || status === 429 || (status >= 500 && status <= 599);
}

function isRetryableUploadError(err: unknown): boolean {
  if (isNamedError(err, "AbortError")) return false;
  return true;
}

function directUploadNetworkError(uploadUrl: string, err: unknown, attempts: number): Error {
  const safeUrl = redactUrl(uploadUrl);
  const code = extractErrorCode(err);
  const detail = sanitizeUploadText(errorMessage(err)).slice(0, 500);
  return new Error(
    `uploadAsset: direct upload PUT failed for ${safeUrl} after ${attemptsLabel(attempts)}` +
      (code ? ` (${code})` : "") +
      (detail ? `: ${detail}` : "")
  );
}

function directUploadResponseError(uploadUrl: string, status: number, detail: string, attempts: number): Error {
  const safeUrl = redactUrl(uploadUrl);
  const safeDetail = sanitizeUploadText(detail).slice(0, 500);
  return new Error(
    `uploadAsset: direct upload PUT failed for ${safeUrl} with status ${status}` +
      (attempts > 1 ? ` after ${attemptsLabel(attempts)}` : "") +
      (safeDetail ? `: ${safeDetail}` : "")
  );
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

function sanitizeUploadText(text: string): string {
  return text
    .replace(/https?:\/\/[^\s<>"'`]+/g, (raw) => redactUrlPreservingTrailingPunctuation(raw))
    .replace(
      /\b(?:X-Amz-(?:Algorithm|Credential|Date|Expires|Security-Token|Signature|SignedHeaders)|AWSAccessKeyId|Signature|Credential|Security-Token|AccessKeyId|SecretAccessKey|SessionToken)=([^&\s<>"'`]+)/gi,
      "[redacted]"
    )
    .replace(/\bAKIA[0-9A-Z]{8,}\b/g, "[redacted]");
}

function redactUrlPreservingTrailingPunctuation(raw: string): string {
  const trailing = raw.match(/[),.;:!?]+$/)?.[0] ?? "";
  const candidate = trailing ? raw.slice(0, -trailing.length) : raw;
  return `${redactUrl(candidate)}${trailing}`;
}
