import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";

function downloadClient(): Aex {
  const fetch: typeof globalThis.fetch = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (url.endsWith("/api/sessions/session-1/files/abc/download")) {
      return new Response("hello", { status: 200, headers: { "content-type": "text/plain" } });
    }
    if (url.endsWith("/api/sessions/session-1/events")) {
      return new Response(JSON.stringify({ events: [] }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    if (url.endsWith("/api/sessions/session-1/files")) {
      return new Response(JSON.stringify({ files: [] }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    if (url.endsWith("/api/sessions/session-1")) {
      return new Response(JSON.stringify({ id: "session-1", status: "succeeded" }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    // Session rehydrate (openSession).
    if (url.endsWith("/api/sessions/session-1")) {
      return new Response(JSON.stringify({ id: "session-1", status: "succeeded" }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    throw new Error(`No fake responder for ${url}`);
  };
  return new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch });
}

function stalledFileResponse(): Response {
  return new Response(
    new ReadableStream<Uint8Array>({
      pull() {
        // Simulates a body stream that opened but stopped delivering bytes.
      }
    }),
    { status: 200 }
  );
}

describe("SessionHandle download { to } options", () => {
  it("download writes the zip to disk and still returns the bytes", async () => {
    const dir = await mkdtemp(join(tmpdir(), "aex-sdk-download-"));
    try {
      const path = join(dir, "run.zip");
      const session = await downloadClient().openSession("session-1");
      const bytes = await session.download({ to: path });
      const written = await readFile(path);
      expect(bytes.byteLength).toBeGreaterThan(0);
      expect(Array.from(written)).toEqual(Array.from(bytes));
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("downloadSessionFile writes raw file bytes to disk and still returns them", async () => {
    const dir = await mkdtemp(join(tmpdir(), "aex-sdk-download-"));
    try {
      const path = join(dir, "report.txt");
      const session = await downloadClient().openSession("session-1");
      const bytes = await session.files().download({ id: "abc" }, { to: path });
      expect(new TextDecoder().decode(bytes)).toBe("hello");
      expect(await readFile(path, "utf8")).toBe("hello");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("downloadSessionFile retries a stalled selected-file body once", async () => {
    let downloadCalls = 0;
    const fetch: typeof globalThis.fetch = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      if (url.endsWith("/api/sessions/session-1")) {
        return new Response(JSON.stringify({ id: "session-1", status: "succeeded" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      if (url.endsWith("/api/sessions/session-1/files/abc/download")) {
        downloadCalls += 1;
        return downloadCalls === 1 ? stalledFileResponse() : new Response("hello", { status: 200 });
      }
      throw new Error(`No fake responder for ${url}`);
    };
    const session = await new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch }).openSession("session-1");

    const bytes = await session.files().download({ id: "abc" }, { timeoutMs: 1 });

    expect(new TextDecoder().decode(bytes)).toBe("hello");
    expect(downloadCalls).toBe(2);
  });
});
