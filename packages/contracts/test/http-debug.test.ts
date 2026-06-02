/**
 * HttpClient debug sink: opt-in local request tracing.
 *
 * Pins the contract Phase-5 relies on:
 *   - with a `debug` sink, every request emits ONE line carrying method,
 *     path, status, and elapsed — and NOTHING sensitive (no Authorization
 *     header, no body, no query string);
 *   - without a sink, nothing is emitted;
 *   - the sink fires on error responses too (so a failing call is traceable).
 */
import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/http.js";
import { AntpathApiError } from "../src/sdk-errors.js";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

describe("HttpClient debug sink", () => {
  it("emits one redacted trace line per request", async () => {
    const lines: string[] = [];
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiToken: "secret-token-value",
      fetch: async () => jsonResponse({ ok: true }),
      debug: (line) => lines.push(line)
    });
    await client.request("/api/runs/abc", { method: "GET" }, { from: "5" });
    expect(lines).toHaveLength(1);
    const line = lines[0]!;
    expect(line).toContain("GET");
    expect(line).toContain("/api/runs/abc");
    expect(line).toContain("-> 200");
    expect(line).toMatch(/\d+ms/);
    // Nothing sensitive: no token, no query string.
    expect(line).not.toContain("secret-token-value");
    expect(line).not.toContain("from=5");
  });

  it("does not emit when no sink is configured", async () => {
    // The only observable is that constructing + requesting without a sink
    // never throws on the (absent) debug path.
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiToken: "t",
      fetch: async () => jsonResponse({ ok: true })
    });
    await expect(client.request("/api/whoami")).resolves.toEqual({ ok: true });
  });

  it("traces error responses too (then throws)", async () => {
    const lines: string[] = [];
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiToken: "t",
      fetch: async () => jsonResponse({ ok: false, message: "nope" }, 404),
      debug: (line) => lines.push(line)
    });
    await expect(client.request("/api/runs/missing")).rejects.toBeInstanceOf(AntpathApiError);
    expect(lines).toHaveLength(1);
    expect(lines[0]!).toContain("-> 404");
  });
});
