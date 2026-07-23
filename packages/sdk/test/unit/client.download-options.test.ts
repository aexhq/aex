import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex } from "../../src/index.js";

function listedFile(id: string, contents: string, filename = "report.txt") {
  return {
    id,
    checkpointId: "cp-1",
    filename,
    sizeBytes: new TextEncoder().encode(contents).byteLength,
    sha256: createHash("sha256").update(contents).digest("hex")
  };
}

function checkpointSnapshot(files: readonly ReturnType<typeof listedFile>[]) {
  return {
    revision: {
      checkpointId: "cp-1",
      runId: "run-1",
      turnSeq: 1,
      committedAt: "2026-07-10T00:00:00Z",
      throughSeq: 9
    },
    files
  };
}

function downloadClient(): Aex {
  const fetch: FetchLike = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (url.endsWith("/api/sessions/session-1/files/abc/download?checkpointId=cp-1")) {
      return new Response("hello", { status: 200, headers: { "content-type": "text/plain" } });
    }
    if (url.endsWith("/api/sessions/session-1/events")) {
      return new Response(JSON.stringify({ events: [] }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    if (new URL(url).pathname === "/api/sessions/session-1/files") {
      const files = new URL(url).searchParams.get("checkpointId") === null
        ? []
        : [listedFile("abc", "hello")];
      return new Response(JSON.stringify(checkpointSnapshot(files)), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    if (url.endsWith("/api/sessions/session-1")) {
      return new Response(JSON.stringify({ session: { id: "session-1", status: "idle", acceptsMessages: true } }), {
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
      const session = await downloadClient().sessions.open("session-1");
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
      const session = await downloadClient().sessions.open("session-1");
      const bytes = await session.files.download({ id: "abc", checkpointId: "cp-1" }, { to: path });
      expect(new TextDecoder().decode(bytes)).toBe("hello");
      expect(await readFile(path, "utf8")).toBe("hello");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("downloadSessionFile retries a stalled selected-file body once", async () => {
    let downloadCalls = 0;
    const fetch: FetchLike = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      if (url.endsWith("/api/sessions/session-1")) {
        return new Response(JSON.stringify({ session: { id: "session-1", status: "idle", acceptsMessages: true } }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      if (url.endsWith("/api/sessions/session-1/files/abc/download?checkpointId=cp-1")) {
        downloadCalls += 1;
        return downloadCalls === 1 ? stalledFileResponse() : new Response("hello", { status: 200 });
      }
      if (url.endsWith("/api/sessions/session-1/files?checkpointId=cp-1")) {
        return Response.json(checkpointSnapshot([listedFile("abc", "hello")]));
      }
      throw new Error(`No fake responder for ${url}`);
    };
    const session = await new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch }).sessions.open("session-1");

    const bytes = await session.files.download({ id: "abc", checkpointId: "cp-1" }, { timeoutMs: 1 });

    expect(new TextDecoder().decode(bytes)).toBe("hello");
    expect(downloadCalls).toBe(2);
  });

  it("verifies a selected download once and does not retry an integrity mismatch", async () => {
    let downloadCalls = 0;
    const fetch: FetchLike = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      if (url.endsWith("/api/sessions/session-1")) {
        return Response.json({ session: { id: "session-1", status: "idle", acceptsMessages: true } });
      }
      if (url.endsWith("/api/sessions/session-1/files?checkpointId=cp-1")) {
        return Response.json(checkpointSnapshot([listedFile("abc", "expected")]));
      }
      if (url.endsWith("/api/sessions/session-1/files/abc/download?checkpointId=cp-1")) {
        downloadCalls += 1;
        return new Response("tampered", { status: 200 });
      }
      throw new Error(`No fake responder for ${url}`);
    };
    const session = await new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch }).sessions.open("session-1");

    await expect(
      session.files.download({ id: "abc", checkpointId: "cp-1" })
    ).rejects.toThrow(/integrity/i);
    expect(downloadCalls).toBe(1);
  });
});
