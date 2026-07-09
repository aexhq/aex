import { describe, expect, it } from "vitest";
import { executeCli } from "../src/main.js";
import { makeIo, type FetchCall } from "./support.js";

const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

const OUTPUTS = [{ id: "o1", filename: "data.json", sizeBytes: 8, contentType: "application/json" }];

function outputsFetch(call: FetchCall): Response {
  const url = new URL(call.url);
  if (url.pathname === "/api/sessions") {
    return json({ sessions: [{ id: "s1", status: "idle", createdAt: "2026-07-01T00:00:00Z", updatedAt: "2026-07-01T00:00:01Z" }] });
  }
  if (url.pathname === "/api/sessions/s1/outputs") {
    return json({ outputs: OUTPUTS });
  }
  if (url.pathname === "/api/sessions/s1/outputs") {
    return json({ outputs: OUTPUTS });
  }
  if (url.pathname === "/api/sessions/s1/outputs/o1/download") {
    return new Response("{\"ok\":1}", {
      status: 200,
      headers: { "content-type": "application/json", "content-length": "8" }
    });
  }
  if (url.pathname === "/api/sessions/s1/outputs/o1/link" && call.init.method === "POST") {
    return json({ url: "https://signed.example/o1", expiresInSeconds: 3600 });
  }
  return json({ error: "unexpected", path: url.pathname }, 404);
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

describe("aex outputs single-file sub-verbs (T6b)", () => {
  it("`outputs read <id> <path>` resolves and reads the output text", async () => {
    const cap = makeIo({ argv: ["outputs", "read", "s1", "data.json", ...COMMON], fetchHandler: outputsFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toContain("/api/sessions/s1/outputs/o1/download");
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ text: "{\"ok\":1}", truncated: false });
  });

  it("`outputs download <id> <path> --out` resolves and writes the bytes", async () => {
    const writes = new Map<string, Uint8Array>();
    const cap = makeIo({ argv: ["outputs", "download", "s1", "data.json", "--out", "out.json", ...COMMON], writes, fetchHandler: outputsFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toContain("/api/sessions/s1/outputs/o1/download");
    expect([...writes.keys()][0]).toMatch(/out\.json$/);
    expect(new TextDecoder().decode([...writes.values()][0]!)).toBe("{\"ok\":1}");
  });

  it("`outputs link <id> <path>` resolves and mints the URL", async () => {
    const cap = makeIo({ argv: ["outputs", "link", "s1", "data.json", ...COMMON], fetchHandler: outputsFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toContain("/api/sessions/s1/outputs/o1/link");
    expect(JSON.parse(cap.stdout.trim()).url).toBe("https://signed.example/o1");
  });

  it("`outputs find <id> --name` filters session outputs", async () => {
    const cap = makeIo({ argv: ["outputs", "find", "s1", "--name", "data", ...COMMON], fetchHandler: outputsFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toContain("/api/sessions/s1/outputs");
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ id: "o1" });
  });

  it("`outputs search --query` searches recent session output metadata", async () => {
    const cap = makeIo({ argv: ["outputs", "search", "--query", "data", ...COMMON], fetchHandler: outputsFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toEqual(["/api/sessions", "/api/sessions/s1/outputs"]);
    expect(JSON.parse(cap.stdout.trim()).hits).toEqual([
      expect.objectContaining({ sessionId: "s1", outputId: "o1", filename: "data.json" })
    ]);
  });

  it("`outputs <id>` (bare) still lists via the accessor", async () => {
    const cap = makeIo({ argv: ["outputs", "s1", ...COMMON], fetchHandler: outputsFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toEqual(["/api/sessions/s1/outputs"]);
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ id: "o1" });
  });
});
