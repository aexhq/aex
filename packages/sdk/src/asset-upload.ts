/**
 * Asset materialization for the SDK submit path.
 *
 * Every inline `Skill` / `AgentsMd` / `File` draft is materialized to the
 * hosted API's content-addressable asset endpoint BEFORE the
 * submit round-trip, so the wire submission carries only `kind:"r2"` refs.
 * Uploads are a single content-length-bounded POST to `/assets/upload`;
 * the hosted API scopes the key under the calling workspace and dedupes by
 * sha256, so re-uploading identical bytes is a cheap no-op.
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

export interface UploadAssetToR2Args {
  readonly http: AssetsHttpClient;
  readonly bytes: Uint8Array;
  /** `sha256:<hex>` — the canonical content hash declared on the wire. */
  readonly hash: string;
  readonly contentType?: string;
}

export interface UploadedR2Asset {
  readonly path: string;
  readonly hash: string;
  readonly sizeBytes: number;
  /** true if R2 already had identical bytes under this hash (dedup hit). */
  readonly exists: boolean;
}

/**
 * Upload `bytes` to the hosted API's content-addressable asset endpoint. The
 * hosted API scopes the key under the calling workspace (derived from the
 * API token) so the returned `path` is `assets/<wsId>/<hash>` — safe
 * to embed in a kind:"r2" submission ref.
 *
 * Verifies the advisory hash matches the bytes BEFORE sending so a
 * mismatch fails fast on the client. The hosted API re-verifies on
 * receipt; a mismatch there is a 400 with code "hash_mismatch" that
 * surfaces as an `AntpathApiError`.
 */
export async function uploadAssetToR2(args: UploadAssetToR2Args): Promise<UploadedR2Asset> {
  const expected = args.hash.startsWith("sha256:") ? args.hash.slice("sha256:".length) : args.hash;
  const actual = await computeSha256Hex(args.bytes);
  if (actual !== expected) {
    throw new Error(
      `uploadAssetToR2: client-side hash mismatch — computed sha256:${actual} ` +
        `but caller declared ${args.hash}. Aborting to avoid uploading corrupted data.`
    );
  }
  const body = await args.http.request<{
    ok: boolean;
    exists: boolean;
    path: string;
    hash: string;
    sizeBytes: number;
  }>("/assets/upload", {
    method: "POST",
    headers: {
      "content-type": args.contentType ?? "application/zip",
      "content-length": String(args.bytes.byteLength),
      "x-asset-hash": `sha256:${actual}`
    },
    body: args.bytes
  });
  return {
    path: body.path,
    hash: body.hash,
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
      "uploadAssetToR2: globalThis.crypto.subtle is not available; " +
        "Node 18+ or a Web-Crypto-capable runtime is required"
    );
  }
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  const digest = await subtle.digest("SHA-256", copy.buffer);
  return bufferToHex(digest);
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
