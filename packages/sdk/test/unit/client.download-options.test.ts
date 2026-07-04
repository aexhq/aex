import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";

function downloadClient(): Aex {
  const fetch: typeof globalThis.fetch = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (url.endsWith("/api/runs/run-1/outputs/abc/download")) {
      return new Response("hello", { status: 200, headers: { "content-type": "text/plain" } });
    }
    if (url.endsWith("/api/runs/run-1/events")) {
      return new Response(JSON.stringify({ events: [] }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    if (url.endsWith("/api/runs/run-1/outputs")) {
      return new Response(JSON.stringify({ outputs: [] }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    if (url.endsWith("/api/runs/run-1")) {
      return new Response(JSON.stringify({ id: "run-1", status: "succeeded" }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    // Session rehydrate (openSession).
    if (url.endsWith("/api/sessions/run-1")) {
      return new Response(JSON.stringify({ id: "run-1", status: "succeeded" }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    throw new Error(`No fake responder for ${url}`);
  };
  return new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch });
}

describe("SessionHandle download { to } options", () => {
  it("download writes the zip to disk and still returns the bytes", async () => {
    const dir = await mkdtemp(join(tmpdir(), "aex-sdk-download-"));
    try {
      const path = join(dir, "run.zip");
      const session = await downloadClient().openSession("run-1");
      const bytes = await session.download({ to: path });
      const written = await readFile(path);
      expect(bytes.byteLength).toBeGreaterThan(0);
      expect(Array.from(written)).toEqual(Array.from(bytes));
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("downloadOutput writes raw output bytes to disk and still returns them", async () => {
    const dir = await mkdtemp(join(tmpdir(), "aex-sdk-download-"));
    try {
      const path = join(dir, "report.txt");
      const session = await downloadClient().openSession("run-1");
      const bytes = await session.outputs().download({ id: "abc" }, { to: path });
      expect(new TextDecoder().decode(bytes)).toBe("hello");
      expect(await readFile(path, "utf8")).toBe("hello");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
