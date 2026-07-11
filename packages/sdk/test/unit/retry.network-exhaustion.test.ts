/**
 * withRetry: exhausted network-error retries must not rethrow the raw
 * transport error. The thrown error states how many attempts were made and
 * over how many ms, preserves the original rejection on `cause`, and — when
 * the transport already wrapped the failure in an `AexNetworkError` —
 * annotates the existing message instead of double-wrapping.
 */
import { describe, expect, it } from "vitest";
import { AexNetworkError } from "@aexhq/contracts";
import { withRetry } from "../../src/retry.js";

const noSleep = async (): Promise<void> => {};

/** Rejection shaped like undici's: bare TypeError with the code on `cause`. */
function undiciFetchFailed(code = "ECONNREFUSED"): TypeError {
  const cause = Object.assign(new Error(`connect ${code} 127.0.0.1:443`), { code });
  return new TypeError("fetch failed", { cause });
}

async function rejectionOf(promise: Promise<unknown>): Promise<AexNetworkError> {
  return promise.then(
    () => {
      throw new Error("expected the promise to reject");
    },
    (err) => err as AexNetworkError
  );
}

describe("withRetry network-error exhaustion", () => {
  it("wraps a raw fetch rejection with attempts + elapsed after maxAttempts", async () => {
    const raw = undiciFetchFailed();
    let calls = 0;
    let t = 0;
    const wrapped = withRetry(
      async () => {
        calls += 1;
        throw raw;
      },
      { maxAttempts: 3, initialDelayMs: 10 },
      { sleep: noSleep, random: () => 0.5, now: () => (t += 100) }
    );
    const err = await rejectionOf(wrapped("https://api.example.test/api/sessions", {
      method: "POST",
      headers: { "Idempotency-Key": "stable-create" }
    }));
    expect(calls).toBe(3);
    expect(err).toBeInstanceOf(AexNetworkError);
    expect(err.attempts).toBe(3);
    expect(err.method).toBe("POST");
    expect(err.host).toBe("api.example.test");
    expect(err.path).toBe("/api/sessions");
    expect(err.causeCode).toBe("ECONNREFUSED");
    expect(err.cause).toBe(raw);
    expect(err.message).toMatch(/after 3 attempts over \d+ms/);
    expect(err.message).toContain("POST api.example.test/api/sessions failed");
  });

  it("annotates an AexNetworkError from the transport instead of double-wrapping", async () => {
    const raw = undiciFetchFailed("ENOTFOUND");
    const inner = new AexNetworkError({
      method: "GET",
      host: "api.example.test",
      path: "/api/whoami",
      cause: raw
    });
    const wrapped = withRetry(
      async () => {
        throw inner;
      },
      { maxAttempts: 2, initialDelayMs: 1 },
      { sleep: noSleep, random: () => 0, now: Date.now }
    );
    const err = await rejectionOf(wrapped("https://api.example.test/api/whoami"));
    expect(err).toBeInstanceOf(AexNetworkError);
    expect(err.method).toBe("GET");
    expect(err.host).toBe("api.example.test");
    expect(err.path).toBe("/api/whoami");
    expect(err.attempts).toBe(2);
    expect(err.message).toContain("GET api.example.test/api/whoami failed");
    expect(err.message).toMatch(/after 2 attempts over \d+ms/);
    // The chain stays one level deep: the wrapper's cause is the raw
    // rejection, not a nested AexNetworkError.
    expect(err.cause).toBe(raw);
  });

  it("wraps when the elapsed budget is spent before maxAttempts", async () => {
    const raw = undiciFetchFailed();
    const wrapped = withRetry(
      async () => {
        throw raw;
      },
      { maxAttempts: 4, initialDelayMs: 10, maxElapsedMs: 0 },
      { sleep: noSleep, random: () => 1, now: Date.now }
    );
    const err = await rejectionOf(wrapped("https://api.example.test/api/sessions"));
    expect(err).toBeInstanceOf(AexNetworkError);
    expect(err.attempts).toBe(1);
    expect(err.cause).toBe(raw);
    expect(err.message).toMatch(/after 1 attempt over \d+ms/);
  });

  it("still rethrows caller-initiated aborts untouched", async () => {
    const abort = new DOMException("The operation was aborted", "AbortError");
    const wrapped = withRetry(
      async () => {
        throw abort;
      },
      { maxAttempts: 3 },
      { sleep: noSleep, random: () => 0, now: Date.now }
    );
    await expect(wrapped("https://api.example.test/api/sessions")).rejects.toBe(abort);
  });

  it("does not retry a POST network failure without stable idempotency identity", async () => {
    const raw = undiciFetchFailed();
    let calls = 0;
    const wrapped = withRetry(async () => {
      calls += 1;
      throw raw;
    }, { maxAttempts: 4 }, { sleep: noSleep });

    await expect(wrapped("https://api.example.test/api/workspace/files", { method: "POST" })).rejects.toBe(raw);
    expect(calls).toBe(1);
  });

  it("does not retry a POST 503 without stable idempotency identity", async () => {
    let calls = 0;
    const wrapped = withRetry(async () => {
      calls += 1;
      return new Response("unavailable", { status: 503 });
    }, { maxAttempts: 4 }, { sleep: noSleep });

    const response = await wrapped("https://api.example.test/billing/checkout", { method: "POST" });
    expect(response.status).toBe(503);
    expect(calls).toBe(1);
  });

  it("does not replay a DELETE after a lost response without idempotency identity", async () => {
    const lostResponse = undiciFetchFailed("ECONNRESET");
    let calls = 0;
    const wrapped = withRetry(async () => {
      calls += 1;
      if (calls === 1) throw lostResponse;
      return new Response(JSON.stringify({ error: "not found" }), { status: 404 });
    }, { maxAttempts: 4 }, { sleep: noSleep });

    await expect(wrapped("https://api.example.test/api/workspace/files/file-1", { method: "DELETE" }))
      .rejects.toBe(lostResponse);
    expect(calls).toBe(1);
  });

  it("does not retry bare PUT or DELETE transient responses", async () => {
    for (const method of ["PUT", "DELETE"] as const) {
      let calls = 0;
      const wrapped = withRetry(async () => {
        calls += 1;
        return new Response("unavailable", { status: 503 });
      }, { maxAttempts: 4 }, { sleep: noSleep });

      expect((await wrapped("https://api.example.test/resource", { method })).status).toBe(503);
      expect(calls, method).toBe(1);
    }
  });

  it("retries a mutation only when it carries stable idempotency identity", async () => {
    let calls = 0;
    const wrapped = withRetry(async () => {
      calls += 1;
      return new Response("ok", { status: calls === 1 ? 503 : 200 });
    }, { maxAttempts: 2, initialDelayMs: 0 }, { sleep: noSleep, random: () => 0 });

    const response = await wrapped("https://api.example.test/resource", {
      method: "DELETE",
      headers: { "Idempotency-Key": "delete-resource-1" }
    });
    expect(response.status).toBe(200);
    expect(calls).toBe(2);
  });
});
