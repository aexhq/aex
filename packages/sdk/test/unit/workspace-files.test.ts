import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";

import { ROUTES, WorkspaceFiles, type ExecuteOptions, type ResourceExecutor, type RouteId } from "../../src/index.js";

interface Call {
  readonly route: RouteId;
  readonly bindings: Readonly<Record<string, string>>;
  readonly options: ExecuteOptions;
}

class FixtureExecutor implements ResourceExecutor {
  readonly calls: Call[] = [];
  readonly answers = new Map<RouteId, unknown>();

  async execute<T>(
    route: RouteId,
    bindings: Readonly<Record<string, string>> = {},
    options: ExecuteOptions = {},
  ): Promise<T> {
    this.calls.push({ route, bindings, options });
    return this.answers.get(route) as T;
  }


  async *stream<T>(): AsyncIterable<T> {
    throw new Error("this file helper fixture has no stream routes");
  }
}

describe("latest-only workspace-file helpers", () => {
  test("the public file/upload surface is exactly seven current-value operations", () => {
    const routes = (Object.keys(ROUTES) as RouteId[])
      .filter((route) => route.startsWith("registry_files_") || route.startsWith("upload_"))
      .sort();
    expect(routes).toEqual([
      "registry_files_delete",
      "registry_files_download_create",
      "registry_files_get",
      "registry_files_list",
      "registry_files_put",
      "upload_complete",
      "upload_create",
    ]);
  });

  test("inline arbitrary bytes use base64 and exact SHA-256", async () => {
    const executor = new FixtureExecutor();
    executor.answers.set("registry_files_put", ready("asset", Uint8Array.from([0, 255, 1])));
    const files = new WorkspaceFiles(executor);
    await files.put("asset", { type: "bytes", bytes: Uint8Array.from([0, 255, 1]) });

    expect(executor.calls).toHaveLength(1);
    const body = executor.calls[0]?.options.body as {
      content: { type: string; encoding: string; data: string; sha256: string };
    };
    expect(body.content).toEqual({
      type: "inline",
      encoding: "base64",
      data: "AP8B",
      sha256: digest(Uint8Array.from([0, 255, 1])),
    });
  });

  test("URL admission is passed as a pending latest overwrite", async () => {
    const executor = new FixtureExecutor();
    executor.answers.set("registry_files_put", { name: "remote", state: "pending" });
    const files = new WorkspaceFiles(executor);
    const result = await files.put("remote", { type: "url", url: "https://example.test/file.pdf" }, {
      mediaType: "application/pdf",
    });
    expect(result.state).toBe("pending");
    expect(executor.calls[0]?.options.body).toMatchObject({
      content: { type: "url", url: "https://example.test/file.pdf" },
      mediaType: "application/pdf",
    });
  });

  test("direct upload replays only signed headers and completes exact parts", async () => {
    const executor = new FixtureExecutor();
    const bytes = Uint8Array.from({ length: 5 * 1024 * 1024 + 5 }, (_, index) => index % 251);
    executor.answers.set("upload_create", {
      upload: { id: "upl_1", partCount: 2, partSizeBytes: String(5 * 1024 * 1024) },
      grants: [
        { partNumber: 1, url: "https://s3.test/1", headers: [{ name: "x-amz-checksum-sha256", value: "one" }] },
        { partNumber: 2, url: "https://s3.test/2", headers: [{ name: "x-amz-checksum-sha256", value: "two" }] },
      ],
    });
    executor.answers.set("upload_complete", ready("binary", bytes));
    const uploaded: Uint8Array[] = [];
    const headers: string[] = [];
    const fetchLike = (async (_url: string | URL | Request, init?: RequestInit) => {
      uploaded.push(new Uint8Array(await new Response(init?.body).arrayBuffer()));
      headers.push(new Headers(init?.headers).get("x-amz-checksum-sha256") ?? "");
      return new Response(null, { status: 200, headers: { etag: `etag-${uploaded.length}` } });
    }) as typeof fetch;

    const files = new WorkspaceFiles(executor, fetchLike);
    await files.upload("binary", bytes);
    expect(uploaded.map(({ byteLength }) => byteLength)).toEqual([5 * 1024 * 1024, 5]);
    expect(uploaded[0]?.slice(0, 3)).toEqual(Uint8Array.from([0, 1, 2]));
    expect(headers).toEqual(["one", "two"]);
    expect(executor.calls.map(({ route }) => route)).toEqual(["upload_create", "upload_complete"]);
    expect(executor.calls[0]?.options.body).toMatchObject({
      name: "binary",
      sizeBytes: String(bytes.byteLength),
      parts: [
        { partNumber: 1, sizeBytes: String(5 * 1024 * 1024), sha256: digest(bytes.subarray(0, 5 * 1024 * 1024)) },
        { partNumber: 2, sizeBytes: "5", sha256: digest(bytes.subarray(5 * 1024 * 1024)) },
      ],
    });
    expect(executor.calls[1]?.options.body).toEqual({
      parts: [
        { partNumber: 1, etag: "etag-1" },
        { partNumber: 2, etag: "etag-2" },
      ],
    });
  });

  test("download refuses corrupt bytes", async () => {
    const executor = new FixtureExecutor();
    executor.answers.set("registry_files_download_create", {
      url: "https://s3.test/object",
      sha256: digest(Uint8Array.from([1, 2, 3])),
      sizeBytes: "3",
    });
    const fetchLike = (async () => new Response(Uint8Array.from([1, 2, 4]))) as unknown as typeof fetch;
    const files = new WorkspaceFiles(executor, fetchLike);
    await expect(files.download("asset")).rejects.toThrow("exact length/hash verification");
  });
});

function ready(name: string, bytes: Uint8Array): unknown {
  return {
    name,
    state: "ready",
    value: {
      content: { sha256: digest(bytes), sizeBytes: String(bytes.byteLength) },
      mediaType: "application/octet-stream",
      mode: "0644",
    },
  };
}

function digest(bytes: Uint8Array): string {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}
