/**
 * Asset materialization for the SDK submit path.
 *
 * Every inline `Skill` / `AgentsMd` / `File` draft is materialized to the
 * hosted API's content-addressable asset store before the submit round-trip,
 * so the wire submission carries only storage-neutral `kind:"asset"` refs.
 *
 * Upload is direct-to-storage (no bytes through the hosted API, so bundle size
 * is bounded by the object store rather than by the API's memory /
 * request-payload limits):
 *
 *   1. POST /assets/presign  → { exists } | { uploadUrl, requiredHeaders }
 *      - `exists:true` is a dedup hit; we're done.
 *      - otherwise the Worker mints a presigned PUT scoped to the exact
 *        content-addressed key and signs `x-amz-checksum-sha256` so the object
 *        store enforces integrity server-side.
 *   2. PUT the bytes straight to `uploadUrl` with `requiredHeaders` (the signed
 *      checksum). The store rejects a byte mismatch — a 2xx proves bytes == hash.
 *   3. POST /assets/finalize → confirms the object exists (HEAD only).
 *
 * Fallback: when the hosted API has no object-store upload credentials it
 * answers presign with 503 `presign_unconfigured`; we POST the bytes to the
 * buffered `/assets`
 * path (small bundles only). The runner re-verifies the hash on download in
 * every case.
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

/**
 * Upload `bytes` to the hosted API's content-addressable asset store via the
 * direct-to-storage presign flow, falling back to the buffered `/assets` POST
 * when the hosted API has no object-store upload credentials.
 *
 * Verifies the advisory hash matches the bytes BEFORE sending so a mismatch
 * fails fast on the client. The store (or the buffered endpoint) re-verifies, and the
 * runner re-checks on download.
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
    // 503 presign_unconfigured → fall back to the buffered upload path.
    if (isPresignUnconfigured(err)) {
      return uploadAssetBuffered(args, actual);
    }
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
  const putRes = await doFetch(presign.uploadUrl, {
    method: "PUT",
    headers: putHeaders,
    body: args.bytes
  });
  if (!putRes.ok) {
    const detail = await putRes.text().catch(() => "");
    throw new Error(
      `uploadAsset: direct upload PUT failed with status ${putRes.status}` +
        (detail ? `: ${detail.slice(0, 500)}` : "")
    );
  }

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

/** Detect the 503 `presign_unconfigured` rejection, regardless of error class. */
function isPresignUnconfigured(err: unknown): boolean {
  if (!err || typeof err !== "object") return false;
  const e = err as { status?: unknown; details?: unknown; code?: unknown };
  if (e.status !== 503) return false;
  const detailCode = (e.details as { code?: unknown } | undefined)?.code;
  return e.code === "presign_unconfigured" || detailCode === "presign_unconfigured";
}

/**
 * Fallback: POST the bytes to the buffered `/assets` endpoint. Used only when
 * the hosted API has no object-store upload credentials (presign 503). Subject to the API's
 * payload limit, so suitable for small bundles only.
 */
async function uploadAssetBuffered(args: UploadAssetArgs, actualHex: string): Promise<UploadedAsset> {
  const body = await args.http.request<{
    ok: boolean;
    assetId?: string;
    exists: boolean;
    path?: string;
    hash?: string;
    contentHash?: string;
    sizeBytes: number;
  }>("/assets", {
    method: "POST",
    headers: {
      "content-type": args.contentType ?? "application/zip",
      "content-length": String(args.bytes.byteLength),
      "x-asset-hash": `sha256:${actualHex}`
    },
    body: args.bytes
  });
  const contentHash = body.contentHash ?? body.hash ?? `sha256:${actualHex}`;
  return {
    assetId: body.assetId ?? assetIdFromContentHash(contentHash),
    contentHash,
    sizeBytes: body.sizeBytes,
    exists: body.exists
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

function bufferToHex(buffer: ArrayBuffer): string {
  const view = new Uint8Array(buffer);
  let out = "";
  for (let i = 0; i < view.length; i++) {
    const byte = view[i] as number;
    out += byte.toString(16).padStart(2, "0");
  }
  return out;
}
