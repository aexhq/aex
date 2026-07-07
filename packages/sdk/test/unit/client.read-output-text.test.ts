import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

/** A download response with a streamable body + content-length, like the outputs route. */
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
    return handler(url);
  };
  return new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch });
}

describe("aex.sessions.outputs(id).read", () => {
  it("reads a small file fully (not truncated) by output id", async () => {
    const client = clientFor((url) => {
      if (url.endsWith("/outputs/out-1/download")) return fileResponse("hello world");
      throw new Error(`unexpected ${url}`);
    });
    const result = await client.sessions.outputs("run-1").read({ id: "out-1" });
    expect(result.text).toBe("hello world");
    expect(result.truncated).toBe(false);
    expect(result.totalBytes).toBe(11);
  });

  it("caps at maxBytes and reports truncated using content-length", async () => {
    const big = "x".repeat(1000);
    const client = clientFor((url) => {
      if (url.endsWith("/outputs/out-1/download")) return fileResponse(big);
      throw new Error(`unexpected ${url}`);
    });
    const result = await client.sessions.outputs("run-1").read({ id: "out-1" }, { maxBytes: 10 });
    expect(result.text).toBe("x".repeat(10));
    expect(result.truncated).toBe(true);
    expect(result.totalBytes).toBe(1000);
  });

  it("reports truncated when a no-content-length stream returns one chunk larger than maxBytes", async () => {
    const client = clientFor((url) => {
      if (url.endsWith("/outputs/out-1/download")) return streamedFileResponse(new TextEncoder().encode("x".repeat(25)));
      throw new Error(`unexpected ${url}`);
    });
    const result = await client.sessions.outputs("run-1").read({ id: "out-1" }, { maxBytes: 10 });
    expect(result.text).toBe("x".repeat(10));
    expect(result.truncated).toBe(true);
    expect(result.totalBytes).toBe(25);
  });

  it("resolves a path selector via listOutputs, then downloads by id", async () => {
    const client = clientFor((url) => {
      if (url.endsWith("/api/runs/run-1/outputs")) {
        return json({ outputs: [{ id: "out-9", filename: "report.md" }] });
      }
      if (url.endsWith("/outputs/out-9/download")) return fileResponse("# Report\nbody\n");
      throw new Error(`unexpected ${url}`);
    });
    const result = await client.sessions.outputs("run-1").read({ path: "report.md" });
    expect(result.output.id).toBe("out-9");
    expect(result.text).toContain("# Report");
  });

  it("retries an idempotent read once when the output body stalls", async () => {
    let downloadCalls = 0;
    const client = clientFor((url) => {
      if (url.endsWith("/outputs/out-1/download")) {
        downloadCalls += 1;
        return downloadCalls === 1 ? stalledFileResponse() : fileResponse("after retry");
      }
      throw new Error(`unexpected ${url}`);
    });

    const result = await client.sessions.outputs("run-1").read({ id: "out-1" }, { timeoutMs: 1 });

    expect(result.text).toBe("after retry");
    expect(downloadCalls).toBe(2);
  });

  it("surfaces a structured network timeout after both read attempts stall", async () => {
    let downloadCalls = 0;
    const client = clientFor((url) => {
      if (url.endsWith("/outputs/out-1/download")) {
        downloadCalls += 1;
        return stalledFileResponse();
      }
      throw new Error(`unexpected ${url}`);
    });

    const error = await rejectionOf(client.sessions.outputs("run-1").read({ id: "out-1" }, { timeoutMs: 1 }));
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
      if (url.endsWith("/outputs/out-1/download")) {
        downloadCalls += 1;
        return new Promise<Response>(() => {});
      }
      throw new Error(`unexpected ${url}`);
    });

    const error = await rejectionOf(client.sessions.outputs("run-1").read({ id: "out-1" }, { timeoutMs: 1 }));
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
      if (url.endsWith("/outputs/out-1/download")) return fileResponse("alpha\nBETA\ngamma beta\n");
      throw new Error(`unexpected ${url}`);
    });
    const result = await client.sessions.outputs("run-1").read({ id: "out-1" }, { grep: "beta" });
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
