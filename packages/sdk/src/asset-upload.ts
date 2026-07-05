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
import type { ByteSink } from "./canonical-zip.js";

/**
 * Subset of `HttpClient` needed by the asset uploader. Defined as a
 * structural type so tests can supply a thin stub without dragging in
 * the full client.
 */
export interface AssetsHttpClient {
  request<T>(
    path: string,
    init?: RequestInit,
    query?: Record<string, string>
  ): Promise<T>;
}

/** Minimal fetch shape for the direct-to-storage PUT (defaults to global fetch). */
export type AssetFetch = (
  input: string,
  init?: RequestInit
) => Promise<{
  readonly ok: boolean;
  readonly status: number;
  text(): Promise<string>;
  /** Present on real `fetch` Responses; used to read a multipart part's ETag. */
  readonly headers?: { get(name: string): string | null };
}>;

export interface UploadAssetArgs {
  readonly http: AssetsHttpClient;
  readonly bytes: Uint8Array;
  /** `sha256:<hex>` — the canonical content hash declared on the wire. */
  readonly hash: string;
  readonly contentType?: string;
  /** Injected in tests; defaults to `globalThis.fetch` for the direct PUT. */
  readonly fetch?: AssetFetch;
}

export interface UploadedAsset {
  readonly assetId: string;
  readonly contentHash: string;
  readonly sizeBytes: number;
  /** true if identical bytes were already present (dedup hit). */
  readonly exists: boolean;
}

const DIRECT_UPLOAD_MAX_ATTEMPTS = 3;

/**
 * Upload `bytes` to the hosted API's content-addressable asset store via the
 * direct-to-storage presign flow.
 *
 * Verifies the advisory hash matches the bytes BEFORE sending so a mismatch
 * fails fast on the client. Object storage re-verifies via the signed checksum,
 * and the runner re-checks on download.
 */
export async function uploadAsset(args: UploadAssetArgs): Promise<UploadedAsset> {
  const expected = args.hash.startsWith("sha256:") ? args.hash.slice("sha256:".length) : args.hash;
  const actual = await computeSha256Hex(args.bytes);
  if (actual !== expected) {
    throw new Error(
      `uploadAsset: client-side hash mismatch: computed sha256:${actual} ` +
        `but caller declared ${args.hash}. Aborting to avoid uploading corrupted data.`
    );
  }
  const contentHashHeader = `sha256:${actual}`;

  // ---- Step 1: presign (control plane) ----
  // A network failure here surfaces as the transport's AexNetworkError,
  // which already names the method, host, path, and transport code.
  const presign = await args.http.request<{
    ok: boolean;
    exists: boolean;
    assetId?: string;
    contentHash?: string;
    sizeBytes?: number;
    uploadUrl?: string;
    requiredHeaders?: Record<string, string>;
  }>("/assets/presign", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ hash: contentHashHeader, sizeBytes: args.bytes.byteLength })
  });

  // Dedup hit — identical bytes already vaulted under this workspace.
  if (presign.exists) {
    const contentHash = presign.contentHash ?? contentHashHeader;
    return {
      assetId: presign.assetId ?? assetIdFromContentHash(contentHash),
      contentHash,
      sizeBytes: presign.sizeBytes ?? args.bytes.byteLength,
      exists: true
    };
  }
  if (!presign.uploadUrl) {
    throw new Error("uploadAsset: presign returned no uploadUrl and exists:false");
  }

  // ---- Step 2: PUT bytes straight to object storage (data plane, bypasses the hosted API) ----
  const doFetch = args.fetch ?? (globalThis.fetch as unknown as AssetFetch);
  const putHeaders: Record<string, string> = {
    "content-type": args.contentType ?? "application/zip",
    ...(presign.requiredHeaders ?? {})
  };
  await putWithRetry(doFetch, presign.uploadUrl, {
    method: "PUT",
    headers: putHeaders,
    body: args.bytes
  });

  // ---- Step 3: finalize (control plane confirms existence via HEAD) ----
  const fin = await args.http.request<{
    ok: boolean;
    assetId?: string;
    contentHash?: string;
    sizeBytes?: number;
  }>("/assets/finalize", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ hash: contentHashHeader, sizeBytes: args.bytes.byteLength })
  });
  const contentHash = fin.contentHash ?? presign.contentHash ?? contentHashHeader;
  return {
    assetId: fin.assetId ?? presign.assetId ?? assetIdFromContentHash(contentHash),
    contentHash,
    sizeBytes: fin.sizeBytes ?? args.bytes.byteLength,
    exists: false
  };
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/**
 * Compute SHA-256 over `bytes` and return the lowercase hex digest.
 * Uses Web Crypto when available (Bun / Node 18+ / browser); the SDK already
 * requires Web Crypto for `hashSkillBundle`.
 */
async function computeSha256Hex(bytes: Uint8Array): Promise<string> {
  const subtle = (globalThis as { crypto?: { subtle?: SubtleCrypto } }).crypto?.subtle;
  if (!subtle) {
    throw new Error(
      "uploadAsset: globalThis.crypto.subtle is not available; " +
        "Bun, Node 18+, or a Web-Crypto-capable runtime is required"
    );
  }
  // Pass the view directly — Web Crypto accepts an ArrayBufferView and digests
  // exactly its bytes (respecting byteOffset/length). No redundant full-buffer copy.
  const digest = await subtle.digest("SHA-256", bytes as unknown as BufferSource);
  return bufferToHex(digest);
}

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

async function putWithRetry(fetchImpl: AssetFetch, uploadUrl: string, init: RequestInit): Promise<void> {
  for (let attempt = 1; attempt <= DIRECT_UPLOAD_MAX_ATTEMPTS; attempt++) {
    let response: Awaited<ReturnType<AssetFetch>>;
    try {
      response = await fetchImpl(uploadUrl, init);
    } catch (err) {
      if (attempt < DIRECT_UPLOAD_MAX_ATTEMPTS && isRetryableUploadError(err)) {
        continue;
      }
      throw directUploadNetworkError(uploadUrl, err, attempt);
    }

    if (response.ok) return;

    if (attempt < DIRECT_UPLOAD_MAX_ATTEMPTS && isRetryableUploadStatus(response.status)) {
      await response.text().catch(() => "");
      continue;
    }

    const detail = await response.text().catch(() => "");
    throw directUploadResponseError(uploadUrl, response.status, detail, attempt);
  }
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

function bufferToHex(buffer: ArrayBuffer): string {
  const view = new Uint8Array(buffer);
  let out = "";
  for (let i = 0; i < view.length; i++) {
    const byte = view[i] as number;
    out += byte.toString(16).padStart(2, "0");
  }
  return out;
}
