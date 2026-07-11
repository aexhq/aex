import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

/** A download response with a streamable body + content-length, like the files route. */
function fileResponse(text: string, contentLength = text.length): Response {
  return new Response(text, { status: 200, headers: { "content-length": String(contentLength) } });
}

function streamedFileResponse(bytes: Uint8Array): Response {
  return new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(bytes);
        controller.close();
      }
    }),
    { status: 200 }
  );
}

function stalledFileResponse(): Response {
  return new Response(
    new ReadableStream<Uint8Array>({
      pull() {
        // Intentionally never enqueue: simulates a response body that connected
        // but then stopped delivering bytes.
      }
    }),
    { status: 200 }
  );
}

function clientFor(handler: (url: string) => Response | Promise<Response>): Aex {
  const fetch: typeof globalThis.fetch = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (url.endsWith("/api/sessions/session-1")) {
      return json({ session: { id: "session-1", status: "idle", acceptsMessages: true } });
    }
    return handler(url);
  };
  return new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch });
}

describe("session.files.read", () => {
  it("reads a small file fully (not truncated) by file id", async () => {
    const client = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) return fileResponse("hello world");
      throw new Error(`unexpected ${url}`);
    });
    const result = await (await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" });
    expect(result.text).toBe("hello world");
    expect(result.truncated).toBe(false);
    expect(result.totalBytes).toBe(11);
  });

  it("caps at maxBytes and reports truncated using content-length", async () => {
    const big = "x".repeat(1000);
    const client = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) return fileResponse(big);
      throw new Error(`unexpected ${url}`);
    });
    const result = await (await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" }, { maxBytes: 10 });
    expect(result.text).toBe("x".repeat(10));
    expect(result.truncated).toBe(true);
    expect(result.totalBytes).toBe(1000);
  });

  it("reports truncated when a no-content-length stream returns one chunk larger than maxBytes", async () => {
    const client = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) return streamedFileResponse(new TextEncoder().encode("x".repeat(25)));
      throw new Error(`unexpected ${url}`);
    });
    const result = await (await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" }, { maxBytes: 10 });
    expect(result.text).toBe("x".repeat(10));
    expect(result.truncated).toBe(true);
    expect(result.totalBytes).toBe(25);
  });

  it("resolves a path selector via listSessionFiles, then downloads by id", async () => {
    const client = clientFor((url) => {
      if (url.endsWith("/api/sessions/session-1/files")) {
        return json({
          revision: { checkpointId: "cp-1", runId: "run-1", turnSeq: 1, committedAt: "2026-07-10T00:00:00Z", throughSeq: 9 },
          files: [{ id: "out-9", checkpointId: "cp-1", filename: "report.md" }]
        });
      }
      if (url.includes("/files/out-9/download?checkpointId=cp-1")) return fileResponse("# Report\nbody\n");
      throw new Error(`unexpected ${url}`);
    });
    const result = await (await client.sessions.open("session-1")).files.read({ path: "report.md" });
    expect(result.file.id).toBe("out-9");
    expect(result.text).toContain("# Report");
  });

  it("retries an idempotent read once when the file body stalls", async () => {
    let downloadCalls = 0;
    const client = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) {
        downloadCalls += 1;
        return downloadCalls === 1 ? stalledFileResponse() : fileResponse("after retry");
      }
      throw new Error(`unexpected ${url}`);
    });

    const result = await (await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" }, { timeoutMs: 1 });

    expect(result.text).toBe("after retry");
    expect(downloadCalls).toBe(2);
  });

  it("surfaces a structured network timeout after both read attempts stall", async () => {
    let downloadCalls = 0;
    const client = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) {
        downloadCalls += 1;
        return stalledFileResponse();
      }
      throw new Error(`unexpected ${url}`);
    });

    const error = await rejectionOf((await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" }, { timeoutMs: 1 }));
    expect(error).toMatchObject({
      code: "NETWORK_ERROR",
      attempts: 2,
      causeCode: "ETIMEDOUT"
    });
    expect((error as Error).message).toContain("phase=body-read");
    expect(downloadCalls).toBe(2);
  });

  it("identifies download-open timeouts before a response body exists", async () => {
    let downloadCalls = 0;
    const client = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) {
        downloadCalls += 1;
        return new Promise<Response>(() => {});
      }
      throw new Error(`unexpected ${url}`);
    });

    const error = await rejectionOf((await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" }, { timeoutMs: 1 }));
    expect(error).toMatchObject({
      code: "NETWORK_ERROR",
      attempts: 2,
      causeCode: "ETIMEDOUT"
    });
    expect((error as Error).message).toContain("phase=download-open");
    expect(downloadCalls).toBe(2);
  });

  it("grep keeps only matching lines of the capped text", async () => {
    const client = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) return fileResponse("alpha\nBETA\ngamma beta\n");
      throw new Error(`unexpected ${url}`);
    });
    const result = await (await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" }, { grep: "beta" });
    expect(result.text).toBe("BETA\ngamma beta");
  });
});

async function rejectionOf(promise: Promise<unknown>): Promise<unknown> {
  try {
    await promise;
  } catch (error) {
    return error;
  }
  throw new Error("expected promise to reject");
}
