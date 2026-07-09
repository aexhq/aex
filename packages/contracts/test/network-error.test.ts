/**
 * AexNetworkError: transport-level failures must carry request context.
 *
 * Pins the fix for the "bare `TypeError: fetch failed`" report: a fetch
 * rejection out of `HttpClient.request` / `HttpClient.download` is wrapped
 * into an `AexNetworkError` naming the method, redacted host+path, and the
 * transport code undici hides on `err.cause.code` — with the original error
 * preserved on `cause`. Also pins:
 *   - `AexError` forwarding `cause` (previously dropped);
 *   - a named, redacted error for an invalid `baseUrl` (previously a bare
 *     `TypeError: Invalid URL`).
 */
import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/http.js";
import { AexError, AexNetworkError } from "../src/sdk-errors.js";

/** Rejection shaped like undici's: bare TypeError with the code on `cause`. */
function undiciFetchFailed(code = "ECONNREFUSED", detail = `connect ${code} 127.0.0.1:443`): TypeError {
  const cause = Object.assign(new Error(detail), { code });
  return new TypeError("fetch failed", { cause });
}

async function rejectionOf<T = AexNetworkError>(promise: Promise<unknown>): Promise<T> {
  return promise.then(
    () => {
      throw new Error("expected the promise to reject");
    },
    (err) => err as T
  );
}

describe("AexError cause forwarding", () => {
  it("forwards options.cause onto the Error cause chain", () => {
    const cause = new Error("socket hang up");
    const err = new AexError("API_ERROR", "boom", undefined, { cause });
    expect(err.cause).toBe(cause);
  });

  it("keeps the existing three-argument signature working", () => {
    const err = new AexError("API_ERROR", "boom", { a: 1 });
    expect(err.cause).toBeUndefined();
    expect(err.details).toEqual({ a: 1 });
  });
});

describe("HttpClient network failures", () => {
  it("wraps request() fetch rejections into AexNetworkError with method/host/path/code", async () => {
    const raw = undiciFetchFailed();
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "secret-token-value",
      fetch: async () => {
        throw raw;
      }
    });
    const err = await rejectionOf(
      client.request("/assets/presign", { method: "POST", body: "{}" }, { sig: "s3cr3t-query" })
    );
    expect(err).toBeInstanceOf(AexNetworkError);
    expect(err).toBeInstanceOf(AexError);
    expect(err.code).toBe("NETWORK_ERROR");
    expect(err.method).toBe("POST");
    expect(err.host).toBe("api.example.test");
    expect(err.path).toBe("/assets/presign");
    expect(err.causeCode).toBe("ECONNREFUSED");
    expect(err.cause).toBe(raw);
    expect(err.message).toContain("POST api.example.test/assets/presign failed");
    expect(err.message).toContain("ECONNREFUSED");
    // Nothing sensitive: no token, no query string.
    expect(err.message).not.toContain("secret-token-value");
    expect(err.message).not.toContain("s3cr3t-query");
  });

  it("retries transient GET request transport failures when enabled", async () => {
    const raw = undiciFetchFailed("UND_ERR_CONNECT_TIMEOUT", "Connect Timeout Error");
    const debug: string[] = [];
    let calls = 0;
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "t",
      retryTransientGets: {
        maxAttempts: 3,
        baseDelayMs: 0,
        sleep: async () => {}
      },
      debug: (line) => debug.push(line),
      fetch: async () => {
        calls += 1;
        if (calls === 1) throw raw;
        return new Response(JSON.stringify({ id: "session-1", status: "succeeded" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });
    await expect(client.request("/api/sessions/session-1")).resolves.toMatchObject({ id: "session-1" });
    expect(calls).toBe(2);
    expect(debug.join("\n")).toContain("transient UND_ERR_CONNECT_TIMEOUT");
  });

  it("retries transient POST failures when an Idempotency-Key makes the write safe", async () => {
    const raw = undiciFetchFailed("UND_ERR_CONNECT_TIMEOUT", "Connect Timeout Error");
    const debug: string[] = [];
    const seenKeys: string[] = [];
    let calls = 0;
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "t",
      retryTransientGets: {
        maxAttempts: 3,
        baseDelayMs: 0,
        sleep: async () => {}
      },
      debug: (line) => debug.push(line),
      fetch: async (_url, init) => {
        calls += 1;
        seenKeys.push(new Headers(init?.headers).get("idempotency-key") ?? "");
        if (calls === 1) throw raw;
        return new Response(JSON.stringify({ id: "sess-1", status: "idle" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });
    await expect(
      client.request("/api/sessions", {
        method: "POST",
        headers: { "Idempotency-Key": "create-1" },
        body: "{}"
      })
    ).resolves.toMatchObject({ id: "sess-1" });
    expect(calls).toBe(2);
    expect(seenKeys).toEqual(["create-1", "create-1"]);
    expect(debug.join("\n")).toContain("POST /api/sessions transient UND_ERR_CONNECT_TIMEOUT");
  });

  it("does not retry transient POST request failures without an idempotency key", async () => {
    const raw = undiciFetchFailed("UND_ERR_CONNECT_TIMEOUT", "Connect Timeout Error");
    let calls = 0;
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "t",
      retryTransientGets: true,
      fetch: async () => {
        calls += 1;
        throw raw;
      }
    });
    const err = await rejectionOf(client.request("/api/sessions", { method: "POST", body: "{}" }));
    expect(calls).toBe(1);
    expect(err).toBeInstanceOf(AexNetworkError);
    expect(err.method).toBe("POST");
    expect(err.causeCode).toBe("UND_ERR_CONNECT_TIMEOUT");
  });

  it("reports exhausted transient GET attempts on the final network error", async () => {
    const raw = undiciFetchFailed("ECONNRESET", "socket hang up");
    let calls = 0;
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "t",
      retryTransientGets: {
        maxAttempts: 2,
        baseDelayMs: 0,
        sleep: async () => {}
      },
      fetch: async () => {
        calls += 1;
        throw raw;
      }
    });
    const err = await rejectionOf(client.request("/api/sessions/session-1"));
    expect(calls).toBe(2);
    expect(err).toBeInstanceOf(AexNetworkError);
    expect(err.method).toBe("GET");
    expect(err.causeCode).toBe("ECONNRESET");
    expect(err.attempts).toBe(2);
    expect(err.message).toContain("after 2 attempts");
  });

  it("wraps download() fetch rejections the same way", async () => {
    const raw = undiciFetchFailed("ENOTFOUND", "getaddrinfo ENOTFOUND api.example.test");
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "t",
      fetch: async () => {
        throw raw;
      }
    });
    const err = await rejectionOf(client.download("/api/sessions/ses_1/archive"));
    expect(err).toBeInstanceOf(AexNetworkError);
    expect(err.method).toBe("GET");
    expect(err.host).toBe("api.example.test");
    expect(err.path).toBe("/api/sessions/ses_1/archive");
    expect(err.causeCode).toBe("ENOTFOUND");
    expect(err.cause).toBe(raw);
    expect(err.message).toContain("GET api.example.test/api/sessions/ses_1/archive failed");
    expect(err.message).toContain("ENOTFOUND");
  });

  it("rethrows caller-initiated aborts untouched", async () => {
    const abort = new DOMException("The operation was aborted", "AbortError");
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "t",
      fetch: async () => {
        throw abort;
      }
    });
    const err = await rejectionOf<unknown>(client.request("/api/whoami"));
    expect(err).toBe(abort);
  });
});

describe("HttpClient baseUrl validation", () => {
  it("names the invalid baseUrl instead of throwing a bare TypeError", () => {
    expect(
      () =>
        new HttpClient({
          baseUrl: "not a url",
          apiKey: "t",
          fetch: async () => new Response("{}")
        })
    ).toThrow(/baseUrl.*not a url/);
  });

  it("redacts credentials embedded in an invalid baseUrl", () => {
    let thrown: Error | undefined;
    try {
      new HttpClient({
        baseUrl: "https://user:hunter2secret@bad host.example",
        apiKey: "t",
        fetch: async () => new Response("{}")
      });
    } catch (err) {
      thrown = err as Error;
    }
    expect(thrown).toBeDefined();
    expect(thrown!.message).toContain("baseUrl");
    expect(thrown!.message).not.toContain("hunter2secret");
  });
});
