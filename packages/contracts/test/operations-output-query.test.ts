import { describe, expect, it } from "vitest";
import { HttpClient, RunStateError, operations, type Output } from "../src/index.js";

const BASE = "https://api.test";

interface RecordedCall {
  readonly path: string;
  readonly method: string;
  readonly body?: string;
  readonly authorization: string | null;
}

const outputs: readonly Output[] = [
  { id: "json", filename: "outputs/reports/summary.json", contentType: "application/json", sizeBytes: 10 },
  { id: "txt", filename: "reports/notes.txt", contentType: "text/plain; charset=utf-8", sizeBytes: 20 },
  { id: "deep", filename: "reports/nested/frame.png", contentType: "image/png", sizeBytes: 30 },
  { id: "pdf", filename: "docs/spec.pdf", contentType: "application/octet-stream", sizeBytes: 40 },
  { id: "zip", filename: "bundle.zip", contentType: "application/zip", sizeBytes: 50 },
  { id: "video", filename: "media/clip.mp4", sizeBytes: 60 }
];

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

function clientFor(routes: Record<string, (init: RequestInit | undefined) => Response>) {
  const calls: RecordedCall[] = [];
  const fetchImpl = async (input: string | URL | Request, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const parsed = new URL(url);
    const path = `${parsed.pathname}${parsed.search}`;
    const method = (init?.method ?? "GET").toString();
    calls.push({
      path,
      method,
      ...(typeof init?.body === "string" ? { body: init.body } : {}),
      authorization: new Headers(init?.headers).get("authorization")
    });
    return routes[path]?.(init) ?? json({ ok: false, code: "not_found" });
  };
  return { http: new HttpClient({ apiToken: "tok", baseUrl: BASE, fetch: fetchImpl }), calls };
}

describe("operations output discovery", () => {
  it("filters by normalized path, basename, directory, extension, content type, and high-level type", async () => {
    const { http } = clientFor({
      "/api/runs/run-1/outputs": () => json({ outputs })
    });

    await expect(operations.listOutputs(http, "run-1", { path: "/outputs/reports/summary.json" }))
      .resolves.toEqual([outputs[0]]);
    await expect(operations.listOutputs(http, "run-1", { filename: /^notes\.txt$/ }))
      .resolves.toEqual([outputs[1]]);
    await expect(operations.listOutputs(http, "run-1", { dir: "reports", recursive: false }))
      .resolves.toEqual([outputs[0], outputs[1]]);
    await expect(operations.listOutputs(http, "run-1", { dir: "reports" }))
      .resolves.toEqual([outputs[0], outputs[1], outputs[2]]);
    await expect(operations.listOutputs(http, "run-1", { extension: ".json" }))
      .resolves.toEqual([outputs[0]]);
    await expect(operations.listOutputs(http, "run-1", { contentType: "image/*" }))
      .resolves.toEqual([outputs[2]]);
    await expect(operations.listOutputs(http, "run-1", { type: "video" }))
      .resolves.toEqual([outputs[5]]);
    await expect(operations.listOutputs(http, "run-1", { type: "pdf" }))
      .resolves.toEqual([]);
  });

  it("classifies from content type first and extension second", () => {
    expect(operations.classifyOutput({ filename: "data.json", contentType: "application/octet-stream" })).toBe("binary");
    expect(operations.classifyOutput({ filename: "spec.pdf" })).toBe("pdf");
    expect(operations.classifyOutput({ filename: "clip.mp4" })).toBe("video");
    expect(operations.classifyOutput({ filename: "bundle.tar.gz" })).toBe("archive");
  });

  it("returns null for no match and throws RunStateError for ambiguous single-output lookup", async () => {
    const { http } = clientFor({
      "/api/runs/run-1/outputs": () =>
        json({ outputs: [{ id: "a", filename: "a/report.txt" }, { id: "b", filename: "b/report.txt" }] })
    });

    await expect(operations.findOutput(http, "run-1", { filename: "missing.txt" })).resolves.toBeNull();
    await expect(operations.findOutput(http, "run-1", { extension: "txt" })).rejects.toBeInstanceOf(RunStateError);
  });
});

describe("operations output links", () => {
  it("resolves a query, posts normalized TTL seconds, and returns resolved output metadata", async () => {
    const { http, calls } = clientFor({
      "/api/runs/run-1/outputs": () => json({ outputs }),
      "/api/runs/run-1/outputs/txt/link": () =>
        json({ url: "https://storage.example/direct.txt", expiresAt: "2026-06-18T12:00:00.000Z" })
    });

    const link = await operations.outputLink(http, "run-1", { filename: "notes.txt" }, { expiresIn: "15m" });

    expect(calls.map((call) => [call.method, call.path])).toEqual([
      ["GET", "/api/runs/run-1/outputs"],
      ["POST", "/api/runs/run-1/outputs/txt/link"]
    ]);
    expect(JSON.parse(calls[1]!.body!)).toEqual({ expiresInSeconds: 900 });
    expect(link).toMatchObject({
      url: "https://storage.example/direct.txt",
      expiresInSeconds: 900,
      output: { id: "txt", filename: "reports/notes.txt" }
    });
  });

  it("keeps createOutputLink as an id-only compatibility path without listing outputs first", async () => {
    const { http, calls } = clientFor({
      "/api/runs/run-1/outputs/txt/link": () => json({ url: "https://storage.example/direct.txt" })
    });

    const link = await operations.createOutputLink(http, "run-1", "txt");

    expect(calls.map((call) => [call.method, call.path])).toEqual([["POST", "/api/runs/run-1/outputs/txt/link"]]);
    expect(JSON.parse(calls[0]!.body!)).toEqual({ expiresInSeconds: 3600 });
    expect(link.expiresInSeconds).toBe(3600);
    expect(link.output).toEqual({ id: "txt" });
  });

  it("posts event archive link requests with the same TTL body", async () => {
    const { http, calls } = clientFor({
      "/api/runs/run-1/events/link": () => json({ url: "https://storage.example/events.jsonl" })
    });

    const link = await operations.eventArchiveLink(http, "run-1", { expiresIn: "1d" });

    expect(calls.map((call) => [call.method, call.path])).toEqual([["POST", "/api/runs/run-1/events/link"]]);
    expect(JSON.parse(calls[0]!.body!)).toEqual({ expiresInSeconds: 86400 });
    expect(link.expiresInSeconds).toBe(86400);
  });
});
