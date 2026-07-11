/**
 * Streaming multipart upload — part-boundary math, dedup short-circuit, per-part
 * retry/refresh, and abort-on-failure. Driven by mock `http` + mock `fetch` so it
 * sessions fully offline; exercises the real two-pass hash → presign → parts →
 * finalize flow in `uploadAssetMultipart`.
 */
import { describe, expect, it } from "vitest";
import { createHash } from "node:crypto";
import { uploadAssetMultipart, type AssetsHttpClient, type AssetFetch, type ZipStreamDriver } from "../../src/asset-upload.js";

const noDelayRetry = {
  initialDelayMs: 0,
  maxDelayMs: 0,
  random: () => 0,
  sleep: async () => undefined
};

/** A driver emitting a deterministic payload of `size` bytes in `chunk`-sized pushes. */
function driverOf(size: number, chunk = 7000): { drive: ZipStreamDriver; payload: Uint8Array; hashHex: string } {
  const payload = new Uint8Array(size);
  for (let i = 0; i < size; i++) payload[i] = (i * 31 + 5) & 0xff;
  const drive: ZipStreamDriver = async (sink) => {
    for (let off = 0; off < size; off += chunk) {
      await sink(payload.subarray(off, Math.min(off + chunk, size)));
    }
  };
  const hashHex = createHash("sha256").update(payload).digest("hex");
  return { drive, payload, hashHex };
}

interface Recorder {
  presignBodies: unknown[];
  finalizeBodies: unknown[];
  aborts: number;
  refreshes: number;
}

/** Build a mock http whose presign returns a partCount-sized plan derived from the request size. */
function makeHttp(opts: {
  exists?: boolean;
  rec: Recorder;
  key?: string;
  uploadId?: string;
}): AssetsHttpClient {
  const key = opts.key ?? "assets/ws/deadbeef";
  const uploadId = opts.uploadId ?? "mpu-1";
  return {
    async request<T>(path: string, init?: RequestInit): Promise<T> {
      const body = init?.body ? JSON.parse(init.body as string) : {};
      if (path === "/api/assets/presign") {
        opts.rec.presignBodies.push(body);
        if (opts.exists) {
          return { ok: true, exists: true, assetId: `asset_${body.hash.slice(7)}`, contentHash: body.hash, sizeBytes: body.sizeBytes, contentType: body.contentType } as T;
        }
        const partSize = body.partSize as number;
        const partCount = Math.max(1, Math.ceil((body.sizeBytes as number) / partSize));
        const partUrls = Array.from({ length: partCount }, (_, i) => ({ partNumber: i + 1, url: `https://s3.test/${key}?partNumber=${i + 1}` }));
        return { ok: true, exists: false, multipart: { uploadId, key, partSize, partCount, partUrls, expiresInSeconds: 300 } } as T;
      }
      if (path === "/api/assets/mpu/presign-parts") {
        opts.rec.refreshes += 1;
        const nums = body.partNumbers as number[];
        return { partUrls: nums.map((n) => ({ partNumber: n, url: `https://s3.test/${key}?partNumber=${n}&refreshed=1` })) } as T;
      }
      if (path === "/api/assets/finalize") {
        opts.rec.finalizeBodies.push(body);
        return { ok: true, assetId: `asset_${body.hash.slice(7)}`, contentHash: body.hash, sizeBytes: body.sizeBytes, contentType: "application/x-aex-bundle" } as T;
      }
      if (path === "/api/assets/mpu/abort") {
        opts.rec.aborts += 1;
        return { ok: true } as T;
      }
      throw new Error(`unexpected path ${path}`);
    }
  };
}

function partNumberOf(url: string): number {
  return Number(new URL(url).searchParams.get("partNumber"));
}

/** Mock fetch that records PUT part bodies and applies a per-part fault script. */
function makeFetch(opts: {
  bodies: Map<number, Uint8Array>;
  attempts: Map<number, number>;
  fault?: (partNumber: number, attempt: number) => { status: number } | null;
  missingEtag?: (partNumber: number) => boolean;
}): AssetFetch {
  return async (url, init) => {
    const pn = partNumberOf(url);
    const attempt = (opts.attempts.get(pn) ?? 0) + 1;
    opts.attempts.set(pn, attempt);
    const fault = opts.fault?.(pn, attempt) ?? null;
    if (fault) {
      return { ok: false, status: fault.status, text: async () => "err", headers: { get: () => null } };
    }
    opts.bodies.set(pn, init!.body as Uint8Array);
    return {
      ok: true,
      status: 200,
      text: async () => "",
      headers: { get: (h: string) => (h.toLowerCase() === "etag" && !opts.missingEtag?.(pn) ? `"etag-${pn}"` : null) }
    };
  };
}

function reassemble(bodies: Map<number, Uint8Array>): Uint8Array {
  const nums = [...bodies.keys()].sort((a, b) => a - b);
  let total = 0;
  for (const n of nums) total += bodies.get(n)!.length;
  const out = new Uint8Array(total);
  let o = 0;
  for (const n of nums) {
    out.set(bodies.get(n)!, o);
    o += bodies.get(n)!.length;
  }
  return out;
}

describe("uploadAssetMultipart — part boundaries", () => {
  const P = 1024;
  for (const size of [1, P - 1, P, P + 1, 2 * P, 3 * P + 17]) {
    it(`size=${size} splits into the right parts and reassembles to the payload`, async () => {
      const rec: Recorder = { presignBodies: [], finalizeBodies: [], aborts: 0, refreshes: 0 };
      const http = makeHttp({ rec });
      const bodies = new Map<number, Uint8Array>();
      const fetch = makeFetch({ bodies, attempts: new Map() });
      const { drive, payload, hashHex } = driverOf(size);

      const result = await uploadAssetMultipart({
        http,
        drive,
        fetch,
        contentType: "application/x-aex-bundle",
        partSize: P,
        partConcurrency: 3
      });

      expect(result.exists).toBe(false);
      expect(result.contentHash).toBe(`sha256:${hashHex}`);
      expect(result.contentType).toBe("application/x-aex-bundle");
      expect(rec.presignBodies[0]).toMatchObject({ contentType: "application/x-aex-bundle", multipart: true });
      // Correct part count + last-part remainder.
      const expectedParts = Math.max(1, Math.ceil(size / P));
      expect(bodies.size).toBe(expectedParts);
      // Reassembled parts == the exact payload the framer produced.
      expect([...reassemble(bodies)]).toEqual([...payload]);
      // finalize carried ascending part numbers + etags.
      const fin = rec.finalizeBodies[0] as { parts: Array<{ partNumber: number; etag: string }>; uploadId: string };
      expect(fin.parts.map((p) => p.partNumber)).toEqual(Array.from({ length: expectedParts }, (_, i) => i + 1));
      expect(fin.parts.every((p) => p.etag === `etag-${p.partNumber}`)).toBe(true);
      expect(rec.aborts).toBe(0);
    });
  }
});

describe("uploadAssetMultipart — dedup + retry + abort", () => {
  it("short-circuits on exists:true (zero parts, no finalize)", async () => {
    const rec: Recorder = { presignBodies: [], finalizeBodies: [], aborts: 0, refreshes: 0 };
    const http = makeHttp({ rec, exists: true });
    const bodies = new Map<number, Uint8Array>();
    const fetch = makeFetch({ bodies, attempts: new Map() });
    const { drive, hashHex } = driverOf(50_000);
    const result = await uploadAssetMultipart({ http, drive, fetch, partSize: 1024 });
    expect(result.exists).toBe(true);
    expect(result.contentHash).toBe(`sha256:${hashHex}`);
    expect(bodies.size).toBe(0);
    expect(rec.finalizeBodies.length).toBe(0);
  });

  it("retries only the failed part on a transient 503, then completes", async () => {
    const rec: Recorder = { presignBodies: [], finalizeBodies: [], aborts: 0, refreshes: 0 };
    const http = makeHttp({ rec });
    const bodies = new Map<number, Uint8Array>();
    const attempts = new Map<number, number>();
    const fetch = makeFetch({ bodies, attempts, fault: (pn, attempt) => (pn === 2 && attempt === 1 ? { status: 503 } : null) });
    const { drive, payload } = driverOf(3 * 1024); // 3 parts
    await uploadAssetMultipart({ http, drive, fetch, partSize: 1024, partConcurrency: 1, retry: noDelayRetry });
    expect(attempts.get(2)).toBe(2); // part 2 retried once
    expect(attempts.get(1)).toBe(1);
    expect([...reassemble(bodies)]).toEqual([...payload]);
    expect(rec.aborts).toBe(0);
  });

  it("refreshes the part URL on a 403 expiry, then retries", async () => {
    const rec: Recorder = { presignBodies: [], finalizeBodies: [], aborts: 0, refreshes: 0 };
    const http = makeHttp({ rec });
    const bodies = new Map<number, Uint8Array>();
    const attempts = new Map<number, number>();
    const fetch = makeFetch({ bodies, attempts, fault: (pn, attempt) => (pn === 1 && attempt === 1 ? { status: 403 } : null) });
    const { drive } = driverOf(2 * 1024);
    await uploadAssetMultipart({ http, drive, fetch, partSize: 1024, partConcurrency: 1 });
    expect(rec.refreshes).toBe(1);
    expect(attempts.get(1)).toBe(2);
    expect(rec.aborts).toBe(0);
  });

  it("aborts the multipart upload on a non-retryable part failure", async () => {
    const rec: Recorder = { presignBodies: [], finalizeBodies: [], aborts: 0, refreshes: 0 };
    const http = makeHttp({ rec });
    const bodies = new Map<number, Uint8Array>();
    const attempts = new Map<number, number>();
    const fetch = makeFetch({ bodies, attempts, fault: () => ({ status: 400 }) }); // always 400 (non-retryable)
    const { drive } = driverOf(2 * 1024);
    await expect(uploadAssetMultipart({ http, drive, fetch, partSize: 1024, partConcurrency: 2 })).rejects.toThrow();
    expect(rec.aborts).toBe(1);
    expect(rec.finalizeBodies.length).toBe(0);
  });

  it("aborts before finalize when object storage omits the part ETag", async () => {
    const rec: Recorder = { presignBodies: [], finalizeBodies: [], aborts: 0, refreshes: 0 };
    const http = makeHttp({ rec });
    const bodies = new Map<number, Uint8Array>();
    const attempts = new Map<number, number>();
    const fetch = makeFetch({ bodies, attempts, missingEtag: (pn) => pn === 1 });
    const { drive } = driverOf(1024);

    await expect(uploadAssetMultipart({ http, drive, fetch, partSize: 1024, partConcurrency: 1 })).rejects.toThrow(
      /part 1 PUT succeeded without an ETag header/
    );

    expect(attempts.get(1)).toBe(1);
    expect(rec.aborts).toBe(1);
    expect(rec.finalizeBodies.length).toBe(0);
  });
});
