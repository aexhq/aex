/**
 * uploadAsset — the direct-to-storage materialization the SDK submit path uses.
 *
 * Covers the three branches:
 *   - presign returns exists:true  → dedup, no PUT, no finalize
 *   - presign → object storage PUT → finalize  → normal upload (bytes bypass the worker)
 *   - presign 503 presign_unconfigured → fall back to the buffered POST /assets
 */
import { describe, expect, it, vi } from "vitest";
import { uploadAsset, type AssetsHttpClient, type AssetFetch } from "../../src/asset-upload.js";
import { AexApiError } from "../../src/index.js";

const bytes = new TextEncoder().encode("hello skill bundle");
// sha256("hello skill bundle") — precomputed so the client-side check passes.
async function hashOf(b: Uint8Array): Promise<string> {
  const d = await crypto.subtle.digest("SHA-256", b);
  return "sha256:" + Array.from(new Uint8Array(d), (x) => x.toString(16).padStart(2, "0")).join("");
}

describe("uploadAsset (direct-to-storage)", () => {
  it("dedups when presign reports exists:true (no PUT, no finalize)", async () => {
    const hash = await hashOf(bytes);
    const calls: string[] = [];
    const http: AssetsHttpClient = {
      request: vi.fn(async (path: string) => {
        calls.push(path);
        return { ok: true, exists: true, assetId: `asset_x`, contentHash: hash, sizeBytes: bytes.byteLength } as unknown;
      }) as AssetsHttpClient["request"]
    };
    const fetch: AssetFetch = vi.fn(async () => ({ ok: true, status: 200, text: async () => "" }));
    const out = await uploadAsset({ http, bytes, hash, fetch });
    expect(out.exists).toBe(true);
    expect(calls).toEqual(["/assets/presign"]);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("presign → PUT to object storage (with signed checksum header) → finalize", async () => {
    const hash = await hashOf(bytes);
    const hex = hash.slice("sha256:".length);
    const calls: string[] = [];
    const http: AssetsHttpClient = {
      request: vi.fn(async (path: string) => {
        calls.push(path);
        if (path === "/assets/presign") {
          return {
            ok: true,
            exists: false,
            assetId: `asset_${hex}`,
            contentHash: hash,
            uploadUrl: "https://acct.r2.cloudflarestorage.com/bucket/assets/ws/" + hex + "?X-Amz-Signature=sig",
            requiredHeaders: { "x-amz-checksum-sha256": "Y2hlY2tzdW0=" }
          } as unknown;
        }
        return { ok: true, exists: false, assetId: `asset_${hex}`, contentHash: hash, sizeBytes: bytes.byteLength } as unknown;
      }) as AssetsHttpClient["request"]
    };
    let putUrl = "";
    let putHeaders: Record<string, string> = {};
    const fetch: AssetFetch = vi.fn(async (url: string, init?: RequestInit) => {
      putUrl = url;
      putHeaders = (init?.headers ?? {}) as Record<string, string>;
      return { ok: true, status: 200, text: async () => "" };
    });
    const out = await uploadAsset({ http, bytes, hash, fetch });
    expect(out.exists).toBe(false);
    expect(out.assetId).toBe(`asset_${hex}`);
    expect(calls).toEqual(["/assets/presign", "/assets/finalize"]);
    expect(putUrl).toContain("r2.cloudflarestorage.com");
    expect(putHeaders["x-amz-checksum-sha256"]).toBe("Y2hlY2tzdW0=");
  });

  it("falls back to the buffered POST /assets when presign is 503 presign_unconfigured", async () => {
    const hash = await hashOf(bytes);
    const hex = hash.slice("sha256:".length);
    const calls: string[] = [];
    const http: AssetsHttpClient = {
      request: vi.fn(async (path: string) => {
        calls.push(path);
        if (path === "/assets/presign") {
          throw new AexApiError(503, "object storage S3 creds not configured", { ok: false, code: "presign_unconfigured" });
        }
        return { ok: true, exists: false, assetId: `asset_${hex}`, contentHash: hash, sizeBytes: bytes.byteLength } as unknown;
      }) as AssetsHttpClient["request"]
    };
    const fetch: AssetFetch = vi.fn(async () => ({ ok: true, status: 200, text: async () => "" }));
    const out = await uploadAsset({ http, bytes, hash, fetch });
    expect(out.exists).toBe(false);
    expect(out.assetId).toBe(`asset_${hex}`);
    expect(calls).toEqual(["/assets/presign", "/assets"]);
    expect(fetch).not.toHaveBeenCalled(); // no object storage PUT on the buffered path
  });

  it("throws when the object storage PUT fails (e.g. object storage rejected the checksum)", async () => {
    const hash = await hashOf(bytes);
    const hex = hash.slice("sha256:".length);
    const http: AssetsHttpClient = {
      request: vi.fn(async (path: string) => {
        if (path === "/assets/presign") {
          return { ok: true, exists: false, assetId: `asset_${hex}`, contentHash: hash, uploadUrl: "https://acct.r2.cloudflarestorage.com/b/k?X-Amz-Signature=s", requiredHeaders: {} } as unknown;
        }
        return {} as unknown;
      }) as AssetsHttpClient["request"]
    };
    const fetch: AssetFetch = vi.fn(async () => ({ ok: false, status: 400, text: async () => "BadDigest: checksum mismatch" }));
    await expect(uploadAsset({ http, bytes, hash, fetch })).rejects.toThrow(/direct upload PUT failed with status 400/);
  });

  it("rejects a client-side hash mismatch before any network call", async () => {
    const http: AssetsHttpClient = { request: vi.fn() as AssetsHttpClient["request"] };
    await expect(uploadAsset({ http, bytes, hash: `sha256:${"0".repeat(64)}` })).rejects.toThrow(/client-side hash mismatch/);
    expect(http.request).not.toHaveBeenCalled();
  });
});
