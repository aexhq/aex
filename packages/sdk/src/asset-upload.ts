/**
 * Asset materialization for the SDK run / session path.
 *
 * Every inline skill-tool / `Tool` / `AgentsMd` / `File` draft is materialized
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
export type AssetFetch = (input: string, init?: RequestInit) => Promise<{ readonly ok: boolean; readonly status: number; text(): Promise<string> }>;

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
  let presign:
    | {
        ok: boolean;
        exists: boolean;
        assetId?: string;
        contentHash?: string;
        sizeBytes?: number;
        uploadUrl?: string;
        requiredHeaders?: Record<string, string>;
      }
    | undefined;
  try {
    presign = await args.http.request<NonNullable<typeof presign>>("/assets/presign", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ hash: contentHashHeader, sizeBytes: args.bytes.byteLength })
    });
  } catch (err) {
    throw err;
  }

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
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  const digest = await subtle.digest("SHA-256", copy.buffer);
  return bufferToHex(digest);
}

function assetIdFromContentHash(contentHash: string): string {
  const hex = contentHash.startsWith("sha256:") ? contentHash.slice("sha256:".length) : contentHash;
  return `asset_${hex}`;
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

function extractErrorCode(err: unknown): string | undefined {
  const code = stringProperty(err, "code");
  if (code) return code;
  const cause = objectProperty(err, "cause");
  const causeCode = stringProperty(cause, "code");
  if (causeCode) return causeCode;
  const match = /\bE[A-Z0-9_]+\b/.exec(errorMessage(err));
  return match?.[0];
}

function isNamedError(err: unknown, name: string): boolean {
  return stringProperty(err, "name") === name;
}

function objectProperty(value: unknown, key: string): Record<string, unknown> | undefined {
  if (!value || typeof value !== "object") return undefined;
  const prop = (value as Record<string, unknown>)[key];
  return prop && typeof prop === "object" ? (prop as Record<string, unknown>) : undefined;
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

function redactUrl(url: string): string {
  try {
    const parsed = new URL(url);
    const auth = parsed.username || parsed.password ? "[redacted]@" : "";
    const query = parsed.search ? "?[redacted]" : "";
    return `${parsed.protocol}//${auth}${parsed.host}${parsed.pathname}${query}`;
  } catch {
    const withoutAuth = url.replace(/\/\/[^/?#\s]+@/, "//[redacted]@");
    const queryStart = withoutAuth.indexOf("?");
    return queryStart === -1 ? withoutAuth : `${withoutAuth.slice(0, queryStart)}?[redacted]`;
  }
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
