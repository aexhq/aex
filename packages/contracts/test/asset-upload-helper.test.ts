import { describe, expect, it, vi } from "vitest";
import {
  directUploadNetworkError,
  isRetryableUploadError,
  isRetryableUploadStatus,
  putDirectUploadWithRetry,
  sanitizeUploadText,
  type AssetFetch
} from "../src/internal.js";

async function rejectionOf(promise: Promise<unknown>): Promise<Error> {
  return promise.then(
    () => {
      throw new Error("expected the promise to reject");
    },
    (err) => err as Error
  );
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
    const fetch: AssetFetch = vi.fn(async () => ({
      ok: false,
      status: 403,
      text: async () => `Forbidden for ${uploadUrl}`
    }));

    const err = await rejectionOf(putDirectUploadWithRetry(fetch, uploadUrl, { method: "PUT" }));

    expect(fetch).toHaveBeenCalledTimes(1);
    expect(err.message).toContain("status 403");
    expect(err.message).toContain("https://storage.example.test/b/k?[redacted]");
    expect(err.message).not.toContain("after 2 attempts");
    expect(err.message).not.toMatch(/X-Amz-Signature|Signature=signature/);
  });
});
