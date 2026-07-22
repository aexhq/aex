import { describe, expect, it, vi } from "vitest";
import { CANONICAL_SHA256_DIGEST_PATTERN } from "@aexhq/contracts";
import { Aex, File } from "../../src/index.js";

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" }
  });
}

describe("Aex asset retry policy", () => {
  it("retries content-addressed presign with one stable idempotency key", async () => {
    const inputBytes = new TextEncoder().encode("input");
    const presignKeys: Array<string | null> = [];
    const presignHashes: string[] = [];
    let presignAttempts = 0;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      if (url.endsWith("/api/assets/presign")) {
        presignAttempts += 1;
        presignKeys.push(new Headers(init?.headers).get("idempotency-key"));
        const body = JSON.parse(String(init?.body)) as { hash: string };
        presignHashes.push(body.hash);
        if (presignAttempts === 1) throw new TypeError("fetch failed");
        const contentHash = body.hash;
        return json({
          ok: true,
          exists: true,
          assetId: `asset_${contentHash.slice("sha256:".length)}`,
          contentHash,
          sizeBytes: inputBytes.byteLength,
          contentType: "application/zip"
        });
      }
      if (url.endsWith("/api/workspace/files")) return json({ resource: {} });
      throw new Error(`unexpected request: ${init?.method ?? "GET"} ${url}`);
    });
    const client = new Aex({
      apiKey: "tkn",
      baseUrl: "https://api.example.test",
      fetch,
      retry: { maxAttempts: 2, initialDelayMs: 0, maxDelayMs: 0 }
    });
    const file = await File.fromBytes({ name: "input.txt", bytes: inputBytes });

    await expect(client.workspace.files.publish(file)).resolves.toEqual({});
    expect(presignAttempts).toBe(2);
    expect(presignHashes).toHaveLength(2);
    expect(CANONICAL_SHA256_DIGEST_PATTERN.test(presignHashes[0]!)).toBe(true);
    expect(presignHashes[1]).toBe(presignHashes[0]);
    expect(presignKeys).toEqual(presignHashes.map((hash) => `asset-presign:${hash.slice("sha256:".length)}`));
  });

  it("does not retry content-addressed presign when retry is disabled", async () => {
    const presignKeys: Array<string | null> = [];
    const fetch = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      presignKeys.push(new Headers(init?.headers).get("idempotency-key"));
      throw new TypeError("fetch failed");
    });
    const client = new Aex({
      apiKey: "tkn",
      baseUrl: "https://api.example.test",
      fetch,
      retry: false
    });
    const file = await File.fromBytes({ name: "input.txt", bytes: new TextEncoder().encode("input") });

    await expect(client.workspace.files.publish(file)).rejects.toThrow(/presign failed/);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(presignKeys).toHaveLength(1);
    expect(presignKeys[0]).toMatch(/^asset-presign:[0-9a-f]{64}$/);
  });

  it.each([
    [false, 1],
    [{ maxAttempts: 2, initialDelayMs: 0, maxDelayMs: 0 }, 2]
  ] as const)("applies retry=%j to direct object-storage transfers", async (retry, expectedAttempts) => {
    let putAttempts = 0;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      if (url.endsWith("/api/assets/presign")) {
        return json({
          ok: true,
          exists: false,
          uploadUrl: "https://object-storage.example.test/upload?X-Amz-Signature=redacted",
          requiredHeaders: {}
        });
      }
      if (url.startsWith("https://object-storage.example.test/")) {
        putAttempts += 1;
        return new Response("transient", { status: 503 });
      }
      throw new Error(`unexpected request: ${init?.method ?? "GET"} ${url}`);
    });
    const client = new Aex({
      apiKey: "tkn",
      baseUrl: "https://api.example.test",
      fetch,
      retry
    });
    const file = await File.fromBytes({ name: "input.txt", bytes: new TextEncoder().encode("input") });

    await expect(client.workspace.files.publish(file)).rejects.toThrow(/direct upload PUT failed/);
    expect(putAttempts).toBe(expectedAttempts);
  });
});
