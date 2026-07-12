import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";

function listedFile(id: string, contents: string, filename = "result.txt") {
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

function pullTrackedFileResponse(chunks: readonly Uint8Array[]) {
  let index = 0;
  let pulls = 0;
  let cancelled = false;
  const response = new Response(
    new ReadableStream<Uint8Array>({
      pull(controller) {
        pulls += 1;
        const chunk = chunks[index++];
        if (chunk === undefined) controller.close();
        else controller.enqueue(chunk);
      },
      cancel() {
        cancelled = true;
      }
    }, { highWaterMark: 0 }),
    { status: 200 }
  );
  return {
    response,
    pulls: () => pulls,
    cancelled: () => cancelled
  };
}

function clientFor(
  handler: (url: string) => Response | Promise<Response>,
  files: readonly ReturnType<typeof listedFile>[]
): { readonly client: Aex; readonly calls: string[] } {
  const calls: string[] = [];
  const fetch: typeof globalThis.fetch = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    calls.push(url);
    if (url.endsWith("/api/sessions/session-1")) {
      return json({ session: { id: "session-1", status: "idle", acceptsMessages: true } });
    }
    if (new URL(url).pathname === "/api/sessions/session-1/files") {
      return json(checkpointSnapshot(files));
    }
    return handler(url);
  };
  return { client: new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch }), calls };
}

describe("session.files.read", () => {
  it("reads a small file fully (not truncated) by file id", async () => {
    const { client, calls } = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) return fileResponse("hello world");
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-1", "hello world")]);
    const session = await client.sessions.open("session-1");
    calls.length = 0;
    const result = await session.files.read({ id: "out-1", checkpointId: "cp-1" });
    expect(result.text).toBe("hello world");
    expect(result.truncated).toBe(false);
    expect(result.totalBytes).toBe(11);
    expect(calls).toEqual([
      "https://example.test/api/sessions/session-1/files?checkpointId=cp-1",
      "https://example.test/api/sessions/session-1/files/out-1/download?checkpointId=cp-1"
    ]);
  });

  it("caps at maxBytes and reports truncated using content-length", async () => {
    const big = "x".repeat(1000);
    const { client } = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) return fileResponse(big);
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-1", big)]);
    const result = await (await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" }, { maxBytes: 10 });
    expect(result.text).toBe("x".repeat(10));
    expect(result.truncated).toBe(true);
    expect(result.totalBytes).toBe(1000);
  });

  it("reports truncated when a no-content-length stream returns one chunk larger than maxBytes", async () => {
    const contents = "x".repeat(25);
    const { client } = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) return streamedFileResponse(new TextEncoder().encode("x".repeat(25)));
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-1", contents)]);
    const result = await (await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" }, { maxBytes: 10 });
    expect(result.text).toBe("x".repeat(10));
    expect(result.truncated).toBe(true);
    expect(result.totalBytes).toBe(25);
  });

  it("retains only the capped prefix and cancels without pulling or peeking again", async () => {
    const first = new TextEncoder().encode("x".repeat(25));
    const second = new TextEncoder().encode("y".repeat(25));
    const tracked = pullTrackedFileResponse([first, second]);
    const { client } = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) return tracked.response;
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-1", `${"x".repeat(25)}${"y".repeat(25)}`)]);

    const result = await (await client.sessions.open("session-1")).files.read(
      { id: "out-1", checkpointId: "cp-1" },
      { maxBytes: 10 }
    );

    expect(result).toMatchObject({ text: "x".repeat(10), truncated: true, totalBytes: 50 });
    expect(tracked.pulls()).toBe(1);
    expect(tracked.cancelled()).toBe(true);
  });

  it("rejects a capped response whose content-length contradicts committed metadata", async () => {
    let downloadCalls = 0;
    const contents = "x".repeat(100);
    const { client } = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) {
        downloadCalls += 1;
        return fileResponse(contents, 99);
      }
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-1", contents)]);

    await expect(
      (await client.sessions.open("session-1")).files.read(
        { id: "out-1", checkpointId: "cp-1" },
        { maxBytes: 10 }
      )
    ).rejects.toThrow(/integrity/i);
    expect(downloadCalls).toBe(1);
  });

  it("verifies SHA-256 when a read consumes the whole file", async () => {
    let downloadCalls = 0;
    const { client } = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) {
        downloadCalls += 1;
        return fileResponse("jello world");
      }
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-1", "hello world")]);

    await expect(
      (await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" })
    ).rejects.toThrow(/integrity/i);
    expect(downloadCalls).toBe(1);
  });

  it("resolves a path selector via listSessionFiles, then downloads by id", async () => {
    const contents = "# Report\nbody\n";
    const { client } = clientFor((url) => {
      if (url.includes("/files/out-9/download?checkpointId=cp-1")) return fileResponse("# Report\nbody\n");
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-9", contents, "report.md")]);
    const result = await (await client.sessions.open("session-1")).files.read({ path: "report.md" });
    expect(result.file.id).toBe("out-9");
    expect(result.text).toContain("# Report");
  });

  it("retries an idempotent read once when the file body stalls", async () => {
    let downloadCalls = 0;
    const { client } = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) {
        downloadCalls += 1;
        return downloadCalls === 1 ? stalledFileResponse() : fileResponse("after retry");
      }
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-1", "after retry")]);

    const result = await (await client.sessions.open("session-1")).files.read({ id: "out-1", checkpointId: "cp-1" }, { timeoutMs: 1 });

    expect(result.text).toBe("after retry");
    expect(downloadCalls).toBe(2);
  });

  it("surfaces a structured network timeout after both read attempts stall", async () => {
    let downloadCalls = 0;
    const { client } = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) {
        downloadCalls += 1;
        return stalledFileResponse();
      }
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-1", "expected")]);

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
    const { client } = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) {
        downloadCalls += 1;
        return new Promise<Response>(() => {});
      }
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-1", "expected")]);

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
    const contents = "alpha\nBETA\ngamma beta\n";
    const { client } = clientFor((url) => {
      if (url.includes("/files/out-1/download?checkpointId=cp-1")) return fileResponse("alpha\nBETA\ngamma beta\n");
      throw new Error(`unexpected ${url}`);
    }, [listedFile("out-1", contents)]);
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
