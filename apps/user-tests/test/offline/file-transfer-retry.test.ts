/**
 * Blackbox coverage for session-file body transfer resilience through a clean
 * installed package. The test never imports workspace internals: a consumer
 * script imports `@aexhq/sdk`, injects a fake fetch, and observes only the
 * public `sessions.open(sessionId).files` API.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const CHILD_HARNESS = String.raw`
import { ok, strictEqual } from "node:assert/strict";

function requestPath(input) {
  const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  return new URL(raw).pathname;
}

function stalledBodyResponse() {
  return new Response(
    new ReadableStream({
      pull() {
        // Deliberately leave the read pending forever. The SDK must bound it.
      }
    }),
    { status: 200 }
  );
}

function textResponse(value) {
  return new Response(value, {
    status: 200,
    headers: {
      "content-type": "text/plain",
      "content-length": String(new TextEncoder().encode(value).byteLength)
    }
  });
}

function jsonResponse(value) {
  return new Response(JSON.stringify(value), {
    status: 200,
    headers: { "content-type": "application/json" }
  });
}

function makeFetch() {
  const calls = [];
  const bodyAttempts = new Map();
  const fetch = async (input, init = {}) => {
    const path = requestPath(input);
    calls.push({ method: String(init.method || "GET").toUpperCase(), path });
    if (path === "/api/sessions/session-1") {
      return jsonResponse({ session: { id: "session-1", status: "idle", acceptsMessages: true } });
    }
    if (path === "/api/sessions/session-1/files") {
      return jsonResponse({
        revision: {
          checkpointId: "cp-1",
          runId: "run-1",
          turnSeq: 1,
          committedAt: "2026-07-10T00:00:00.000Z",
          throughSeq: 10
        },
        files: [
          { id: "file-read", checkpointId: "cp-1", filename: "read.txt", sizeBytes: 21, sha256: "b6040e352139602607f9f505e2e67c390666f8230e521cdbd5bb011521f03555", contentType: "text/plain" },
          { id: "file-download", checkpointId: "cp-1", filename: "download.txt", sizeBytes: 25, sha256: "2d201c2ed644abd811f85e7d0c723d622b9ab397045ca6f903ddf6d591f4c450", contentType: "text/plain" },
          { id: "file-archive", checkpointId: "cp-1", filename: "archive.txt", sizeBytes: 24, sha256: "e4004a29b896bfa70c355e4179d6eef5dadd58f858b48eda1eec6d78e2f264ba", contentType: "text/plain" },
          { id: "file-timeout", checkpointId: "cp-1", filename: "timeout.txt", sizeBytes: 7, sha256: "f77d1bb58da886e3cbeebbf35a0b3d217b003506792268052c6a730fbc5ec9bc", contentType: "text/plain" }
        ]
      });
    }
    const match = /^\/api\/sessions\/session-1\/files\/([^/]+)\/download$/.exec(path);
    if (match) {
      const id = match[1];
      const next = (bodyAttempts.get(id) || 0) + 1;
      bodyAttempts.set(id, next);
      if (id === "file-timeout") return stalledBodyResponse();
      if (next === 1) return stalledBodyResponse();
      return textResponse(id + " after retry");
    }
    throw new Error("unexpected fake fetch path " + path);
  };
  return { fetch, calls, bodyAttempts };
}
`;

describe("session file transfer retry (installed package)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  async function runChild(script: string, fileName: string, timeoutMs = 120_000): Promise<Record<string, unknown>> {
    const scriptPath = join(install.installDir, fileName);
    writeFileSync(scriptPath, script);
    const child = await runCommand(getBunCommand(), [scriptPath], { cwd: install.installDir, timeoutMs });
    if (child.exitCode !== 0) {
      throw new Error(`${fileName} exited ${child.exitCode}\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
    }
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  }

  it("bounds stalled file bodies, retries idempotent reads/downloads once, and keeps archive downloads usable", async () => {
    const script =
      CHILD_HARNESS +
      String.raw`
const { Aex } = await import("@aexhq/sdk");
const harness = makeFetch();
const client = new Aex({
  apiKey: "tkn",
  baseUrl: "https://example.invalid",
  fetch: harness.fetch,
  retry: false
});
const files = (await client.sessions.open("session-1")).files;

const read = await files.read({ id: "file-read", checkpointId: "cp-1" }, { timeoutMs: 1 });
strictEqual(read.text, "file-read after retry");
strictEqual(read.truncated, false);
strictEqual(harness.bodyAttempts.get("file-read"), 2);

const bytes = await files.download({ id: "file-download", checkpointId: "cp-1" }, { timeoutMs: 1 });
strictEqual(new TextDecoder().decode(bytes), "file-download after retry");
strictEqual(harness.bodyAttempts.get("file-download"), 2);

const archive = await files.download(undefined, { timeoutMs: 1 });
ok(archive.byteLength > 4, "archive download should return bytes");
strictEqual(archive[0], 0x50);
strictEqual(archive[1], 0x4b);
strictEqual(harness.bodyAttempts.get("file-archive"), 2);

let timeoutError = null;
try {
  await files.read({ id: "file-timeout", checkpointId: "cp-1" }, { timeoutMs: 1 });
} catch (err) {
  timeoutError = {
    name: err && err.name,
    code: err && err.code,
    attempts: err && err.attempts,
    causeCode: err && err.causeCode,
    message: err && err.message
  };
}
ok(timeoutError, "persistent stalled body should reject");
strictEqual(timeoutError.code, "NETWORK_ERROR");
strictEqual(timeoutError.attempts, 2);
strictEqual(timeoutError.causeCode, "ETIMEDOUT");
ok(harness.bodyAttempts.get("file-timeout") >= 2);

let validationError = null;
try {
  await files.download({ id: "file-download", checkpointId: "cp-1" }, { timeoutMs: 0 });
} catch (err) {
  validationError = { name: err && err.name, message: err && err.message };
}
ok(validationError, "invalid timeoutMs should reject before a transfer");
ok(/timeoutMs must be a positive finite number/.test(validationError.message));

console.log(JSON.stringify({
  ok: true,
  readAttempts: harness.bodyAttempts.get("file-read"),
  downloadAttempts: harness.bodyAttempts.get("file-download"),
  archiveAttempts: harness.bodyAttempts.get("file-archive"),
  timeoutAttempts: harness.bodyAttempts.get("file-timeout"),
  timeoutError,
  validationError,
  calls: harness.calls
}));
`;
    const result = await runChild(script, "file-transfer-retry.mjs");
    expect(result).toMatchObject({
      ok: true,
      archiveAttempts: 2,
      timeoutError: { code: "NETWORK_ERROR", attempts: 2, causeCode: "ETIMEDOUT" }
    });
    expect(result.readAttempts).toBeGreaterThanOrEqual(2);
    expect(result.downloadAttempts).toBeGreaterThanOrEqual(2);
    expect(result.timeoutAttempts).toBeGreaterThanOrEqual(2);
    expect(String((result.validationError as { message?: unknown }).message ?? "")).toMatch(/timeoutMs must be/);
  });
});
