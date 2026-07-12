/**
 * uploadAsset — the direct-to-storage materialization the SDK submit path uses.
 *
 * Covers the three branches:
 *   - presign returns exists:true  → dedup, no PUT, no finalize
 *   - presign → object storage PUT → finalize  → normal upload (bytes bypass the hosted API)
 *   - presign errors fail without a buffered `/api/assets` retry
 */
import { describe, expect, it, vi } from "vitest";
import { ASSET_ARCHIVE_LIMITS, HttpClient } from "@aexhq/contracts";
import { uploadAsset, type AssetsHttpClient, type AssetFetch } from "../../src/asset-upload.js";
import { AexApiError, AexNetworkError } from "../../src/index.js";

const bytes = new TextEncoder().encode("hello skill bundle");
const noDelayRetry = {
  initialDelayMs: 0,
  maxDelayMs: 0,
  random: () => 0,
  sleep: async () => undefined
};
// sha256("hello skill bundle") — precomputed so the client-side check passes.
async function hashOf(b: Uint8Array): Promise<string> {
  const d = await crypto.subtle.digest("SHA-256", b);
  return "sha256:" + Array.from(new Uint8Array(d), (x) => x.toString(16).padStart(2, "0")).join("");
}

describe("uploadAsset (direct-to-storage)", () => {
  it("rejects an oversized runtime archive before hashing or presign", async () => {
    const http: AssetsHttpClient = { request: vi.fn() as AssetsHttpClient["request"] };
    const oversized = { byteLength: ASSET_ARCHIVE_LIMITS.maxCompressedBytes + 1 } as Uint8Array;

    await expect(uploadAsset({ http, bytes: oversized, hash: `sha256:${"0".repeat(64)}` }))
      .rejects.toThrow(/64 MiB compressed limit/);
    expect(http.request).not.toHaveBeenCalled();
  });

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
    expect(calls).toEqual(["/api/assets/presign"]);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("presign → PUT to object storage (with signed checksum header) → finalize", async () => {
    const hash = await hashOf(bytes);
    const hex = hash.slice("sha256:".length);
    const calls: string[] = [];
    let presignBody: Record<string, unknown> | undefined;
    const http: AssetsHttpClient = {
      request: vi.fn(async (path: string, init?: RequestInit) => {
        calls.push(path);
        if (path === "/api/assets/presign") {
          presignBody = JSON.parse(String(init?.body)) as Record<string, unknown>;
          return {
            ok: true,
            exists: false,
            assetId: `asset_${hex}`,
            contentHash: hash,
            uploadUrl: "https://object-storage.example.test/bucket/assets/ws/" + hex + "?X-Amz-Signature=sig",
            requiredHeaders: { "x-amz-checksum-sha256": "Y2hlY2tzdW0=" }
          } as unknown;
        }
        return { ok: true, exists: false, assetId: `asset_${hex}`, contentHash: hash, sizeBytes: bytes.byteLength, contentType: "application/x-aex-bundle" } as unknown;
      }) as AssetsHttpClient["request"]
    };
    let putUrl = "";
    let putMethod = "";
    let putBody: BodyInit | null | undefined;
    let putHeaders: Record<string, string> = {};
    const fetch: AssetFetch = vi.fn(async (url: string, init?: RequestInit) => {
      putUrl = url;
      putMethod = init?.method ?? "";
      putBody = init?.body;
      putHeaders = (init?.headers ?? {}) as Record<string, string>;
      return { ok: true, status: 200, text: async () => "" };
    });
    const out = await uploadAsset({ http, bytes, hash, contentType: "application/x-aex-bundle", fetch });
    expect(out.exists).toBe(false);
    expect(out.assetId).toBe(`asset_${hex}`);
    expect(out.contentType).toBe("application/x-aex-bundle");
    expect(presignBody).toMatchObject({ contentType: "application/x-aex-bundle" });
    expect(calls).toEqual(["/api/assets/presign", "/api/assets/finalize"]);
    expect(putUrl).toBe("https://object-storage.example.test/bucket/assets/ws/" + hex + "?X-Amz-Signature=sig");
    expect(putMethod).toBe("PUT");
    expect(Array.from(putBody as Uint8Array)).toEqual(Array.from(bytes));
    expect(putHeaders["x-amz-checksum-sha256"]).toBe("Y2hlY2tzdW0=");
    expect(putHeaders["content-type"]).toBe("application/x-aex-bundle");
  });

  it("does not fall back to buffered POST /api/assets when presign is unavailable", async () => {
    const hash = await hashOf(bytes);
    const calls: string[] = [];
    const http: AssetsHttpClient = {
      request: vi.fn(async (path: string) => {
        calls.push(path);
        if (path === "/api/assets/presign") {
          throw new AexApiError(503, "object storage S3 creds not configured", { ok: false, code: "presign_unconfigured" });
        }
        throw new Error(`unexpected request: ${path}`);
      }) as AssetsHttpClient["request"]
    };
    const fetch: AssetFetch = vi.fn(async () => ({ ok: true, status: 200, text: async () => "" }));
    await expect(uploadAsset({ http, bytes, hash, fetch })).rejects.toThrow(/object storage S3 creds not configured/);
    expect(calls).toEqual(["/api/assets/presign"]);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("surfaces network context when the presign request cannot reach the API", async () => {
    const hash = await hashOf(bytes);
    const raw = new TypeError("fetch failed", {
      cause: Object.assign(new Error("connect ECONNREFUSED 127.0.0.1:443"), { code: "ECONNREFUSED" })
    });
    // Integration through the real transport: uploadAsset → HttpClient →
    // rejecting fetch, exactly the unreachable-API shape from the field report.
    const http = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "tok",
      fetch: async () => {
        throw raw;
      }
    });
    const put: AssetFetch = vi.fn(async () => ({ ok: true, status: 200, text: async () => "" }));
    let thrown: unknown;
    try {
      await uploadAsset({ http, bytes, hash, fetch: put });
    } catch (err) {
      thrown = err;
    }
    expect(thrown).toBeInstanceOf(AexNetworkError);
    const err = thrown as AexNetworkError;
    expect(err.message).toContain("POST api.example.test/api/assets/presign failed");
    expect(err.message).toContain("ECONNREFUSED");
    expect(err.cause).toBe(raw);
    expect(put).not.toHaveBeenCalled();
  });

  it("throws when the object storage PUT fails (e.g. object storage rejected the checksum)", async () => {
    const hash = await hashOf(bytes);
    const hex = hash.slice("sha256:".length);
    const uploadUrl =
      "https://acct.object-storage.example.test/b/k?X-Amz-Security-Token=token&X-Amz-Signature=s";
    const http: AssetsHttpClient = {
      request: vi.fn(async (path: string) => {
        if (path === "/api/assets/presign") {
          return { ok: true, exists: false, assetId: `asset_${hex}`, contentHash: hash, uploadUrl, requiredHeaders: {} } as unknown;
        }
        return {} as unknown;
      }) as AssetsHttpClient["request"]
    };
    const fetch: AssetFetch = vi.fn(async () => ({ ok: false, status: 400, text: async () => "BadDigest: checksum mismatch" }));
    let thrown: unknown;
    try {
      await uploadAsset({ http, bytes, hash, fetch });
    } catch (err) {
      thrown = err;
    }

    expect(fetch).toHaveBeenCalledTimes(1);
    expect(thrown).toBeInstanceOf(Error);
    expect(String((thrown as Error).message)).toMatch(
      /direct upload PUT failed for https:\/\/acct\.object-storage\.example\.test\/b\/k\?\[redacted\] with status 400/
    );
    expect(String((thrown as Error).message)).not.toMatch(/X-Amz-Security-Token|X-Amz-Signature|token&|Signature=s/);
  });

  it("retries transient object storage 429/5xx responses and then finalizes", async () => {
    const hash = await hashOf(bytes);
    const hex = hash.slice("sha256:".length);
    const calls: string[] = [];
    const http: AssetsHttpClient = {
      request: vi.fn(async (path: string) => {
        calls.push(path);
        if (path === "/api/assets/presign") {
          return {
            ok: true,
            exists: false,
            assetId: `asset_${hex}`,
            contentHash: hash,
            uploadUrl: `https://acct.object-storage.example.test/b/${hex}?X-Amz-Signature=s`,
            requiredHeaders: {}
          } as unknown;
        }
        return { ok: true, exists: false, assetId: `asset_${hex}`, contentHash: hash, sizeBytes: bytes.byteLength } as unknown;
      }) as AssetsHttpClient["request"]
    };
    const fetch: AssetFetch = vi
      .fn()
      .mockResolvedValueOnce({ ok: false, status: 429, text: async () => "SlowDown" })
      .mockResolvedValueOnce({ ok: false, status: 500, text: async () => "InternalError" })
      .mockResolvedValueOnce({ ok: true, status: 200, text: async () => "" });

    const out = await uploadAsset({ http, bytes, hash, fetch, retry: noDelayRetry });

    expect(out.exists).toBe(false);
    expect(fetch).toHaveBeenCalledTimes(3);
    expect(calls).toEqual(["/api/assets/presign", "/api/assets/finalize"]);
  });

  it("retries transient object storage network errors and redacts the signed URL when exhausted", async () => {
    const hash = await hashOf(bytes);
    const hex = hash.slice("sha256:".length);
    const uploadUrl =
      `https://AKIA_TEST:secret@acct.object-storage.example.test/b/${hex}` +
      "?X-Amz-Credential=credential&X-Amz-Security-Token=token&X-Amz-Signature=signature";
    const http: AssetsHttpClient = {
      request: vi.fn(async (path: string) => {
        if (path === "/api/assets/presign") {
          return { ok: true, exists: false, assetId: `asset_${hex}`, contentHash: hash, uploadUrl, requiredHeaders: {} } as unknown;
        }
        throw new Error(`unexpected request: ${path}`);
      }) as AssetsHttpClient["request"]
    };
    const fetchErr = Object.assign(new TypeError(`fetch failed for ${uploadUrl}: ECONNRESET`), { code: "ECONNRESET" });
    const fetch: AssetFetch = vi.fn(async () => {
      throw fetchErr;
    });

    let thrown: unknown;
    try {
      await uploadAsset({ http, bytes, hash, fetch, retry: noDelayRetry });
    } catch (err) {
      thrown = err;
    }

    expect(thrown).toBeInstanceOf(Error);
    expect(fetch).toHaveBeenCalledTimes(5);
    expect(String((thrown as Error).message)).toContain(
      `https://[redacted]@acct.object-storage.example.test/b/${hex}?[redacted]`
    );
    expect(String((thrown as Error).message)).toContain("ECONNRESET");
    expect(String((thrown as Error).message)).toContain("after 5 attempts");
    expect(String((thrown as Error).message)).not.toMatch(/X-Amz|Credential=credential|Security-Token|Signature|AKIA_TEST|secret|token/);
  });

  it("does not retry permanent object storage 4xx responses", async () => {
    const hash = await hashOf(bytes);
    const hex = hash.slice("sha256:".length);
    const http: AssetsHttpClient = {
      request: vi.fn(async (path: string) => {
        if (path === "/api/assets/presign") {
          return {
            ok: true,
            exists: false,
            assetId: `asset_${hex}`,
            contentHash: hash,
            uploadUrl: "https://acct.object-storage.example.test/b/k?X-Amz-Signature=s",
            requiredHeaders: {}
          } as unknown;
        }
        throw new Error(`unexpected request: ${path}`);
      }) as AssetsHttpClient["request"]
    };
    const fetch: AssetFetch = vi.fn(async () => ({
      ok: false,
      status: 403,
      text: async () => "Forbidden for https://acct.object-storage.example.test/b/k?X-Amz-Signature=s"
    }));

    let thrown: unknown;
    try {
      await uploadAsset({ http, bytes, hash, fetch });
    } catch (err) {
      thrown = err;
    }

    expect(thrown).toBeInstanceOf(Error);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(String((thrown as Error).message)).toContain("status 403");
    expect(String((thrown as Error).message)).toContain("https://acct.object-storage.example.test/b/k?[redacted]");
    expect(String((thrown as Error).message)).not.toMatch(/X-Amz-Signature|Signature=s/);
  });

  it("rejects a client-side hash mismatch before any network call", async () => {
    const http: AssetsHttpClient = { request: vi.fn() as AssetsHttpClient["request"] };
    await expect(uploadAsset({ http, bytes, hash: `sha256:${"0".repeat(64)}` })).rejects.toThrow(/client-side hash mismatch/);
    expect(http.request).not.toHaveBeenCalled();
  });
});
