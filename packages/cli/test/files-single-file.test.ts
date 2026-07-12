import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import { executeCli } from "../src/main.js";
import { makeIo, type FetchCall } from "./support.js";

const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

const REVISION = {
  checkpointId: "cp-1",
  runId: "run-1",
  turnSeq: 1,
  committedAt: "2026-07-01T00:00:01Z",
  throughSeq: 10
};
const FILE_CONTENTS = "{\"ok\":1}";
const FILES = [{
  id: "o1",
  checkpointId: "cp-1",
  filename: "data.json",
  sizeBytes: 8,
  sha256: createHash("sha256").update(FILE_CONTENTS).digest("hex"),
  contentType: "application/json"
}];

function filesFetch(call: FetchCall): Response {
  const url = new URL(call.url);
  if (url.pathname === "/api/sessions") {
    return json({ sessions: [{ id: "s1", status: "idle", acceptsMessages: true, createdAt: "2026-07-01T00:00:00Z", updatedAt: "2026-07-01T00:00:01Z" }] });
  }
  if (url.pathname === "/api/sessions/s1/files") {
    return json({ revision: REVISION, files: FILES });
  }
  if (url.pathname === "/api/sessions/s1/files/o1/download") {
    return new Response(FILE_CONTENTS, {
      status: 200,
      headers: { "content-type": "application/json", "content-length": "8" }
    });
  }
  if (url.pathname === "/api/sessions/s1/files/o1/link" && call.init.method === "POST") {
    return json({ url: "https://signed.example/o1", expiresInSeconds: 3600 });
  }
  return json({ error: "unexpected", path: url.pathname }, 404);
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

describe("aex files single-file sub-verbs (T6b)", () => {
  it("`files read <id> <path>` resolves and reads the file text", async () => {
    const cap = makeIo({ argv: ["files", "read", "s1", "data.json", ...COMMON], fetchHandler: filesFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toContain("/api/sessions/s1/files/o1/download");
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ text: "{\"ok\":1}", truncated: false });
  });

  it("`files download <id> <path> --out` resolves and writes the bytes", async () => {
    const writes = new Map<string, Uint8Array>();
    const cap = makeIo({ argv: ["files", "download", "s1", "data.json", "--out", "out.json", ...COMMON], writes, fetchHandler: filesFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toContain("/api/sessions/s1/files/o1/download");
    expect([...writes.keys()][0]).toMatch(/out\.json$/);
    expect(new TextDecoder().decode([...writes.values()][0]!)).toBe("{\"ok\":1}");
  });

  it("`files link <id> <path>` resolves and mints the URL", async () => {
    const cap = makeIo({ argv: ["files", "link", "s1", "data.json", ...COMMON], fetchHandler: filesFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toContain("/api/sessions/s1/files/o1/link");
    expect(JSON.parse(cap.stdout.trim()).url).toBe("https://signed.example/o1");
  });

  it("`files find <id> --name` filters session files", async () => {
    const cap = makeIo({ argv: ["files", "find", "s1", "--name", "data", ...COMMON], fetchHandler: filesFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toContain("/api/sessions/s1/files");
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ id: "o1" });
  });

  it("`files <id>` (bare) still lists via the accessor", async () => {
    const cap = makeIo({ argv: ["files", "s1", ...COMMON], fetchHandler: filesFetch });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.map((call) => new URL(call.url).pathname)).toEqual(["/api/sessions/s1/files"]);
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ id: "o1" });
  });
});
