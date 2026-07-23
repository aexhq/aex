import { describe, expect, it, mock } from "bun:test";
import {
  DIRECT_UPLOAD_MAX_ATTEMPTS,
  DIRECT_UPLOAD_MAX_ELAPSED_MS,
  directUploadNetworkError,
  isRetryableUploadError,
  isRetryableUploadStatus,
  putDirectUploadWithRetry,
  sanitizeUploadText,
  type AssetFetch,
  type AssetUploadResponse,
  type AssetUploadRetryOptions
} from "../src/internal.js";

async function rejectionOf(promise: Promise<unknown>): Promise<Error> {
  return promise.then(
    () => {
      throw new Error("expected the promise to reject");
    },
    (err) => err as Error
  );
}

function uploadResponse(
  status: number,
  headers: Record<string, string> = {},
  body = ""
): AssetUploadResponse {
  const lower = new Map(Object.entries(headers).map(([name, value]) => [name.toLowerCase(), value]));
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name: string) => lower.get(name.toLowerCase()) ?? null },
    text: async () => body
  };
}

function deterministicRetryOptions(args: {
  readonly random?: number;
  readonly now?: number;
  readonly initialDelayMs?: number;
  readonly maxDelayMs?: number;
  readonly maxElapsedMs?: number;
} = {}): { readonly options: AssetUploadRetryOptions; readonly slept: number[] } {
  let clock = args.now ?? 0;
  const slept: number[] = [];
  return {
    slept,
    options: {
      initialDelayMs: args.initialDelayMs ?? 100,
      maxDelayMs: args.maxDelayMs ?? 1000,
      maxElapsedMs: args.maxElapsedMs ?? 10_000,
      random: () => args.random ?? 1,
      now: () => clock,
      sleep: async (ms: number) => {
        slept.push(ms);
        clock += ms;
      }
    }
  };
}

describe("asset upload internal helpers", () => {
  it("classifies retryable upload statuses exactly", () => {
    for (const status of [408, 425, 429, 500, 502, 599]) {
      expect(isRetryableUploadStatus(status), `status ${status}`).toBe(true);
    }
    for (const status of [400, 401, 403, 404, 409, 422, 499, 600]) {
      expect(isRetryableUploadStatus(status), `status ${status}`).toBe(false);
    }
  });

  it("retries network failures except AbortError", () => {
    const aborted = new Error("aborted");
    aborted.name = "AbortError";

    expect(isRetryableUploadError(new TypeError("fetch failed"))).toBe(true);
    expect(isRetryableUploadError(Object.assign(new TypeError("socket reset"), { code: "ECONNRESET" }))).toBe(true);
    expect(isRetryableUploadError(aborted)).toBe(false);
  });

  it("uses a burst-tolerant default retry budget for direct uploads", () => {
    expect(DIRECT_UPLOAD_MAX_ATTEMPTS).toBe(5);
    expect(DIRECT_UPLOAD_MAX_ELAPSED_MS).toBe(60_000);
  });

  it("redacts signed URLs, credential query params, and access-key-shaped text", () => {
    const signedUrl =
      "https://AKIATESTKEY1234:very-secret@storage.example.test/b/k" +
      "?X-Amz-Credential=credential&X-Amz-Security-Token=session-token&X-Amz-Signature=signature";
    const out = sanitizeUploadText(
      `PUT failed for ${signedUrl}); raw AWSAccessKeyId=access-key Signature=signature AKIATESTKEY1234`
    );

    expect(out).toContain("https://[redacted]@storage.example.test/b/k?[redacted])");
    expect(out).not.toMatch(
      /X-Amz|Credential=credential|Security-Token|Signature=signature|AWSAccessKeyId|AKIATESTKEY1234|very-secret|session-token/
    );
  });

  it("redacts network error messages and preserves attempt count/code", () => {
    const uploadUrl =
      "https://AKIATESTKEY1234:very-secret@storage.example.test/b/k" +
      "?X-Amz-Credential=credential&X-Amz-Security-Token=session-token&X-Amz-Signature=signature";
    const raw = Object.assign(new TypeError(`fetch failed for ${uploadUrl}: ECONNRESET`), { code: "ECONNRESET" });

    const message = directUploadNetworkError(uploadUrl, raw, 3).message;

    expect(message).toContain("after 3 attempts");
    expect(message).toContain("ECONNRESET");
    expect(message).toContain("https://[redacted]@storage.example.test/b/k?[redacted]");
    expect(message).not.toMatch(/X-Amz|Credential=credential|Security-Token|Signature=signature|AKIATESTKEY1234|very-secret/);
  });

  it("does not retry permanent direct-upload 4xx responses", async () => {
    const uploadUrl = "https://storage.example.test/b/k?X-Amz-Signature=signature";
    const { options, slept } = deterministicRetryOptions();
    const fetch: AssetFetch = mock(async () => ({
      ok: false,
      status: 403,
      text: async () => `Forbidden for ${uploadUrl}`
    }));

    const err = await rejectionOf(putDirectUploadWithRetry(fetch, uploadUrl, { method: "PUT" }, options));

    expect(fetch).toHaveBeenCalledTimes(1);
    expect(slept).toEqual([]);
    expect(err.message).toContain("status 403");
    expect(err.message).toContain("https://storage.example.test/b/k?[redacted]");
    expect(err.message).not.toContain("after 2 attempts");
    expect(err.message).not.toMatch(/X-Amz-Signature|Signature=signature/);
  });

  it("honors Retry-After seconds and HTTP dates before retrying direct uploads", async () => {
    const uploadUrl = "https://storage.example.test/b/k?X-Amz-Signature=signature";
    let secondsCall = 0;
    const secondsFetch: AssetFetch = mock(async () => {
      secondsCall += 1;
      return secondsCall === 1 ? uploadResponse(429, { "retry-after": "2" }, "slow") : uploadResponse(200);
    });
    const seconds = deterministicRetryOptions({ random: 0, initialDelayMs: 100 });

    await putDirectUploadWithRetry(secondsFetch, uploadUrl, { method: "PUT" }, seconds.options);

    expect(secondsFetch).toHaveBeenCalledTimes(2);
    expect(seconds.slept).toEqual([2000]);

    const now = Date.UTC(2026, 0, 1, 0, 0, 0);
    const retryAt = new Date(now + 3_000).toUTCString();
    let dateCall = 0;
    const dateFetch: AssetFetch = mock(async () => {
      dateCall += 1;
      return dateCall === 1 ? uploadResponse(503, { "Retry-After": retryAt }, "unavailable") : uploadResponse(200);
    });
    const date = deterministicRetryOptions({ random: 0, now, initialDelayMs: 100 });

    await putDirectUploadWithRetry(dateFetch, uploadUrl, { method: "PUT" }, date.options);

    expect(dateFetch).toHaveBeenCalledTimes(2);
    expect(date.slept).toEqual([3000]);
  });

  it("does not retry AbortError from direct-upload fetch", async () => {
    const uploadUrl = "https://storage.example.test/b/k?X-Amz-Signature=signature";
    const abort = new Error("caller aborted");
    abort.name = "AbortError";
    const fetch: AssetFetch = mock(async () => {
      throw abort;
    });
    const { options, slept } = deterministicRetryOptions();

    const err = await rejectionOf(putDirectUploadWithRetry(fetch, uploadUrl, { method: "PUT" }, options));

    expect(fetch).toHaveBeenCalledTimes(1);
    expect(slept).toEqual([]);
    expect(err.message).toContain("after 1 attempt");
  });

  it("uses the injected sleep with the computed full-jitter delay", async () => {
    const uploadUrl = "https://storage.example.test/b/k?X-Amz-Signature=signature";
    let call = 0;
    const fetch: AssetFetch = mock(async () => {
      call += 1;
      return call === 1 ? uploadResponse(500, {}, "temporary") : uploadResponse(200);
    });
    const { options, slept } = deterministicRetryOptions({ random: 0.25, initialDelayMs: 100, maxDelayMs: 100 });

    await putDirectUploadWithRetry(fetch, uploadUrl, { method: "PUT" }, options);

    expect(fetch).toHaveBeenCalledTimes(2);
    expect(slept).toEqual([25]);
  });

  it("stops retrying when the next direct-upload delay would exceed the elapsed budget", async () => {
    const uploadUrl = "https://storage.example.test/b/k?X-Amz-Signature=signature";
    const fetch: AssetFetch = mock(async () => uploadResponse(503, {}, "still unavailable"));
    const { options, slept } = deterministicRetryOptions({
      random: 1,
      initialDelayMs: 100,
      maxDelayMs: 100,
      maxElapsedMs: 50
    });

    const err = await rejectionOf(putDirectUploadWithRetry(fetch, uploadUrl, { method: "PUT" }, options));

    expect(fetch).toHaveBeenCalledTimes(1);
    expect(slept).toEqual([]);
    expect(err.message).toContain("status 503");
    expect(err.message).not.toContain("after 2 attempts");
  });
});
