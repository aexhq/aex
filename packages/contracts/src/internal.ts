import {
  putDirectUploadWithRetry,
  type AssetFetch,
  type AssetUploadRetryOptions
} from "./asset-upload-helper.js";
import { assertArchiveCompressedSize } from "./archive-limits.js";

// Workspace-internal entry point. Re-exports submission building blocks and
// hosts small shared helpers so public packages can avoid hand-mirroring
// behavior. NOT part of the public `@aexhq/contracts` surface; do NOT add this
// to the package `index`. Consumers reach it via the explicit
// `@aexhq/contracts/internal` subpath.
export * from "./connection-ticket.js";
export * from "./continuation-event.js";
export {
  CONTRACT_PARSE_ERROR,
  isContractParseError
} from "./contract-parse-error.js";
export type { ContractParseError } from "./contract-parse-error.js";
export * from "./archive-limits.js";
export * from "./asset-authoring.js";
export * from "./asset-bundle.js";
export * from "./models.js";
export * as operations from "./operations.js";
export * from "./post-hook.js";
export * from "./retry-core.js";
export * from "./runtime-security-profile.js";
export * from "./sdk-secrets.js";
export * from "./session-custody.js";
export * from "./session-retention.js";
export * from "./side-effect-audit.js";
export * from "./stable.js";
export * from "./status.js";
export * from "./submission.js";
export * from "./unknown-field-error.js";
export * from "./workflow-status.js";
export { hasRunTerminalType } from "./event-stream-client.js";
export type { RunTerminalTypeEvent } from "./event-stream-client.js";
export {
  directUploadNetworkError,
  directUploadResponseError,
  isRetryableUploadError,
  isRetryableUploadStatus,
  putDirectUploadWithRetry,
  sanitizeUploadText
} from "./asset-upload-helper.js";
export type {
  AssetFetch,
  AssetUploadResponse,
  AssetUploadRetryOptions,
  AssetUploadSleep
} from "./asset-upload-helper.js";

/**
 * Subset of `HttpClient` needed by the asset uploader. Defined structurally so
 * SDK and CLI tests can supply thin stubs without dragging in the full client.
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
  /** `sha256:<hex>` - the canonical content hash declared on the wire. */
  readonly hash: string;
  readonly contentType?: string;
  readonly fetch?: AssetFetch;
  readonly retry?: AssetUploadRetryOptions;
}

export interface UploadedAsset {
  readonly assetId: string;
  readonly contentHash: string;
  readonly sizeBytes: number;
  readonly contentType: string;
  /** true if identical bytes were already present (dedup hit). */
  readonly exists: boolean;
}

/**
 * Upload `bytes` to the hosted API's content-addressable asset store via the
 * direct-to-storage presign flow shared by SDK and CLI callers.
 */
export async function uploadAsset(args: UploadAssetArgs): Promise<UploadedAsset> {
  assertArchiveCompressedSize(args.bytes.byteLength, "uploadAsset");
  const expected = args.hash.startsWith("sha256:") ? args.hash.slice("sha256:".length) : args.hash;
  const actual = await computeSha256Hex(args.bytes);
  if (actual !== expected) {
    throw new Error(
      `uploadAsset: client-side hash mismatch: computed sha256:${actual} ` +
        `but caller declared ${args.hash}. Aborting to avoid uploading corrupted data.`
    );
  }
  const contentHashHeader = `sha256:${actual}`;
  const contentType = args.contentType ?? "application/zip";

  const presign = await args.http.request<{
    ok: boolean;
    exists: boolean;
    assetId?: string;
    contentHash?: string;
    sizeBytes?: number;
    contentType?: string;
    uploadUrl?: string;
    requiredHeaders?: Record<string, string>;
  }>("/api/assets/presign", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "Idempotency-Key": `asset-presign:${actual}`
    },
    body: JSON.stringify({ hash: contentHashHeader, sizeBytes: args.bytes.byteLength, contentType })
  });

  if (presign.exists) {
    const contentHash = presign.contentHash ?? contentHashHeader;
    return {
      assetId: presign.assetId ?? assetIdFromContentHash(contentHash),
      contentHash,
      sizeBytes: presign.sizeBytes ?? args.bytes.byteLength,
      contentType: presign.contentType ?? contentType,
      exists: true
    };
  }
  if (!presign.uploadUrl) {
    throw new Error("uploadAsset: presign returned no uploadUrl and exists:false");
  }

  const doFetch = args.fetch ?? (globalThis.fetch as unknown as AssetFetch);
  const putHeaders: Record<string, string> = {
    "content-type": contentType,
    ...(presign.requiredHeaders ?? {})
  };
  await putDirectUploadWithRetry(
    doFetch,
    presign.uploadUrl,
    {
      method: "PUT",
      headers: putHeaders,
      body: args.bytes
    },
    args.retry
  );

  const fin = await args.http.request<{
    ok: boolean;
    assetId?: string;
    contentHash?: string;
    sizeBytes?: number;
    contentType?: string;
  }>("/api/assets/finalize", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "Idempotency-Key": `asset-finalize:${actual}`
    },
    body: JSON.stringify({ hash: contentHashHeader, sizeBytes: args.bytes.byteLength })
  });
  const contentHash = fin.contentHash ?? presign.contentHash ?? contentHashHeader;
  return {
    assetId: fin.assetId ?? presign.assetId ?? assetIdFromContentHash(contentHash),
    contentHash,
    sizeBytes: fin.sizeBytes ?? args.bytes.byteLength,
    contentType: fin.contentType ?? presign.contentType ?? contentType,
    exists: false
  };
}

async function computeSha256Hex(bytes: Uint8Array): Promise<string> {
  const subtle = (globalThis as { crypto?: { subtle?: SubtleCrypto } }).crypto?.subtle;
  if (!subtle) {
    throw new Error(
      "uploadAsset: globalThis.crypto.subtle is not available; " +
        "Bun, Node 18+, or a Web-Crypto-capable runtime is required"
    );
  }
  const digest = await subtle.digest("SHA-256", bytes as unknown as BufferSource);
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
