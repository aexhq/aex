/**
 * Asset materialization for the SDK submit path.
 *
 * Every inline `Skill` / `AgentsMd` / `File` draft is materialized to the
 * hosted API's content-addressable asset endpoint before the submit
 * round-trip, so the wire submission carries only storage-neutral
 * `kind:"asset"` refs. Uploads are a single content-length-bounded POST to
 * `/assets`; the hosted API scopes storage under the calling workspace and
 * dedupes by sha256.
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

export interface UploadAssetArgs {
  readonly http: AssetsHttpClient;
  readonly bytes: Uint8Array;
  /** `sha256:<hex>` — the canonical content hash declared on the wire. */
  readonly hash: string;
  readonly contentType?: string;
}

export interface UploadedAsset {
  readonly assetId: string;
  readonly contentHash: string;
  readonly sizeBytes: number;
  /** true if identical bytes were already present (dedup hit). */
  readonly exists: boolean;
}

/**
 * Upload `bytes` to the hosted API's content-addressable asset endpoint.
 *
 * Verifies the advisory hash matches the bytes BEFORE sending so a
 * mismatch fails fast on the client. The hosted API re-verifies on
 * receipt; a mismatch there is a 400 with code "hash_mismatch" that
 * surfaces as an `AntpathApiError`.
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
      "x-asset-hash": `sha256:${actual}`
    },
    body: args.bytes
  });
  const contentHash = body.contentHash ?? body.hash ?? `sha256:${actual}`;
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
 * Uses Web Crypto when available (Node 18+ / browser); the SDK already
 * requires Web Crypto for `hashSkillBundle`.
 */
async function computeSha256Hex(bytes: Uint8Array): Promise<string> {
  const subtle = (globalThis as { crypto?: { subtle?: SubtleCrypto } }).crypto?.subtle;
  if (!subtle) {
    throw new Error(
      "uploadAsset: globalThis.crypto.subtle is not available; " +
        "Node 18+ or a Web-Crypto-capable runtime is required"
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
