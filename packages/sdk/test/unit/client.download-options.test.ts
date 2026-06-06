import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { AgentExecutor } from "../../src/index.js";

function downloadClient(): AgentExecutor {
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
    throw new Error(`No fake responder for ${url}`);
  };
  return new AgentExecutor({ apiToken: "tkn", baseUrl: "https://example.test", fetch });
}

describe("AgentExecutor download { to } options", () => {
  it("download writes the zip to disk and still returns the bytes", async () => {
    const dir = await mkdtemp(join(tmpdir(), "aex-sdk-download-"));
    try {
      const path = join(dir, "run.zip");
      const client = downloadClient();
      const bytes = await client.download("run-1", { to: path });
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
      const client = downloadClient();
      const bytes = await client.downloadOutput("run-1", { id: "abc" }, { to: path });
      expect(new TextDecoder().decode(bytes)).toBe("hello");
      expect(await readFile(path, "utf8")).toBe("hello");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
