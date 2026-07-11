import { describe, expect, it, vi } from "vitest";
import { Aex, File } from "../../src/index.js";

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" }
  });
}

describe("Aex asset retry policy", () => {
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
