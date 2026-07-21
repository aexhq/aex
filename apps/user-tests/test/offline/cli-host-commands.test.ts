/**
 * Installed CLI host-command coverage.
 *
 * This is the blackbox layer for the commands users run after
 * `npm i @aexhq/sdk`: spawn the installed `aex` binary from a clean
 * temp install and point it at a local fake API. Unit tests cover the same
 * verbs through an injected fetch; this file catches packaging, bin wiring,
 * auth header, URL, and public wire-shape drift in the shipped artifact.
 */
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { strToU8, unzipSync } from "fflate";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getAexBinPath, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const REPORT_BYTES = strToU8("hello world");
const REPORT_SHA256 = createHash("sha256").update(REPORT_BYTES).digest("hex");

interface CapturedRequest {
  readonly method: string;
  readonly path: string;
  readonly authorization: string | undefined;
  readonly idempotencyKey: string | undefined;
  readonly body: unknown;
}

interface FakeApi {
  readonly baseUrl: string;
  readonly requests: CapturedRequest[];
  readonly close: () => Promise<void>;
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let body = "";
    req.setEncoding("utf8");
    req.on("data", (chunk) => {
      body += chunk;
    });
    req.on("end", () => resolve(body));
    req.on("error", reject);
  });
}

function json(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

function bytes(res: ServerResponse, status: number, body: Uint8Array, contentType = "application/octet-stream"): void {
  res.writeHead(status, { "content-type": contentType });
  res.end(body);
}

async function startFakeApi(): Promise<FakeApi> {
  const requests: CapturedRequest[] = [];
  let statusPolls = 0;

  const server: Server = createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const rawBody = await readBody(req);
    let parsedBody: unknown = rawBody;
    if (rawBody.length > 0) {
      try {
        parsedBody = JSON.parse(rawBody);
      } catch {
        parsedBody = rawBody;
      }
    }
    requests.push({
      method: req.method ?? "GET",
      path: url.pathname + url.search,
      authorization: req.headers.authorization,
      idempotencyKey: typeof req.headers["idempotency-key"] === "string" ? req.headers["idempotency-key"] : undefined,
      body: parsedBody
    });

    // --- session endpoints (run/status/events/wait/cancel speak these) ---
    if (req.method === "POST" && url.pathname === "/api/sessions") {
      json(res, 201, {
        session: {
          id: "session-cli-1",
          status: "idle",
          acceptsMessages: true,
          provider: "deepseek",
          runtimeSize: "0.25cpu-1gb"
        }
      });
      return;
    }
    if (req.method === "POST" && url.pathname === "/api/sessions/session-cli-1/messages") {
      json(res, 202, {
        session: {
          id: "session-cli-1",
          status: "running",
          acceptsMessages: false,
          provider: "deepseek",
          runtimeSize: "0.25cpu-1gb"
        },
        run: { sessionId: "session-cli-1", runId: "run-1", turnSeq: 1, phase: "running" },
        eventCursor: 1
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/sessions/session-cli-1") {
      statusPolls += 1;
      json(res, 200, {
        session: {
          id: "session-cli-1",
          status: statusPolls > 1 ? "idle" : "running",
          acceptsMessages: statusPolls > 1,
          provider: "deepseek",
          runtimeSize: "0.25cpu-1gb"
        }
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/sessions/session-denied") {
      json(res, 403, {
        error: "insufficient_scope",
        message: "the token does not carry sessions:read",
        requestId: "req-packed-cli-error"
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/sessions/session-cli-1/events") {
      json(res, 200, {
        events: [
          {
            specversion: "1.0",
            id: "evt-1",
            source: "runtime",
            type: "RUN_STARTED",
            subject: "session-cli-1",
            threadId: "session-cli-1",
            runId: "run-1",
            time: "2026-07-10T00:00:00.000Z",
            sequence: 0,
            data: { phase: "start" }
          },
          {
            specversion: "1.0",
            id: "evt-2",
            source: "workflow",
            type: "RUN_FINISHED",
            subject: "session-cli-1",
            threadId: "session-cli-1",
            runId: "run-1",
            time: "2026-07-10T00:00:01.000Z",
            sequence: 1,
            data: { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp-1" } }
          }
        ]
      });
      return;
    }
    if (req.method === "POST" && url.pathname === "/api/sessions/session-cli-1/cancel") {
      json(res, 200, { session: { id: "session-cli-1", status: "cancelling", acceptsMessages: false } });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/sessions") {
      json(res, 200, {
        sessions: [{ id: "session-cli-1", status: "idle", acceptsMessages: true, createdAt: "2026-07-02T10:00:00Z", updatedAt: "2026-07-02T10:05:00Z" }]
      });
      return;
    }

    // --- workspace billing + webhook signing secret reads ---
    if (req.method === "GET" && url.pathname === "/api/billing") {
      json(res, 200, {
        balanceUsd: 25,
        monthSpendUsd: 1.5,
        spendCapUsd: 100,
        planKey: "free",
        subscriptionStatus: "none"
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/billing/ledger") {
      json(res, 200, {
        entries: [
          {
            id: "led-1",
            entryType: "top_up",
            amountUsd: 25,
            currency: "USD",
            sessionId: null,
            description: "ci top-up",
            createdBy: "admin:ops@example.test",
            createdAt: "2026-07-01T00:00:00Z"
          }
        ]
      });
      return;
    }
    if (req.method === "POST" && url.pathname === "/api/webhook/signing-secret") {
      json(res, 200, { whsec: "whsec_aW5zdGFsbGVkLWNsaS1zZWNyZXQ=" });
      return;
    }

    // Download assembles the public zip client-side from the session read endpoints.
    if (req.method === "GET" && url.pathname === "/api/sessions/session-cli-1/files") {
      json(res, 200, {
        revision: {
          checkpointId: "cp-1",
          runId: "run-1",
          turnSeq: 1,
          committedAt: "2026-07-10T00:00:00.000Z",
          throughSeq: 2
        },
        files: [{
          id: "out-1",
          checkpointId: "cp-1",
          filename: "report.txt",
          sizeBytes: REPORT_BYTES.byteLength,
          sha256: REPORT_SHA256,
          contentType: "text/plain"
        }]
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/sessions/session-cli-1/files/out-1/download") {
      bytes(res, 200, REPORT_BYTES, "text/plain");
      return;
    }

    json(res, 404, { error: "not_found", path: url.pathname });
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("fake API did not bind to a TCP port");
  }

  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    requests,
    close: () => new Promise((resolve) => server.close(() => resolve()))
  };
}

describe("installed CLI host commands", () => {
  let install: InstallResult;
  let api: FakeApi;
  let binPath: string;

  beforeAll(async () => {
    install = await installAex();
    api = await startFakeApi();
    binPath = getAexBinPath(install.installDir);
  });

  afterAll(async () => {
    await api?.close();
    install?.cleanup();
  });

  it("sessions start/status/events/wait/download/cancel through the installed binary", async () => {
    const common = ["--api-key", "tok-installed-cli", "--aex-url", api.baseUrl] as const;

    const run = await runCommand(
      binPath,
      [
        "start",
        "--provider",
        "deepseek",
        "--model",
        "deepseek-v4-flash",
        "--prompt",
        "hello_from_installed_cli",
        "--deepseek-api-key",
        "sk-deepseek-test",
        "--idempotency-key",
        "cli-host-installed-shape",
        ...common
      ],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(run.exitCode, `stdout:\n${run.stdout}\nstderr:\n${run.stderr}`).toBe(0);
    // `aex start` prints the session record from the accepted first turn.
    expect(JSON.parse(run.stdout.trim())).toEqual({
      id: "session-cli-1",
      status: "running",
      acceptsMessages: false,
      provider: "deepseek",
      runtime: { size: "0.25cpu-1gb" }
    });

    const status = await runCommand(binPath, ["status", "session-cli-1", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(status.exitCode, `stdout:\n${status.stdout}\nstderr:\n${status.stderr}`).toBe(0);
    expect(JSON.parse(status.stdout.trim())).toMatchObject({
      id: "session-cli-1",
      status: "running",
      provider: "deepseek",
      runtime: { size: "0.25cpu-1gb" }
    });

    const events = await runCommand(binPath, ["events", "session-cli-1", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(events.exitCode, `stdout:\n${events.stdout}\nstderr:\n${events.stderr}`).toBe(0);
    const eventLines = events.stdout.trim().split(/\r?\n/).map((line) => JSON.parse(line) as { id: string });
    expect(eventLines.map((event) => event.id)).toEqual(["evt-1", "evt-2"]);

    const wait = await runCommand(
      binPath,
      ["wait", "session-cli-1", "--interval", "1ms", "--timeout", "2s", ...common],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(wait.exitCode, `stdout:\n${wait.stdout}\nstderr:\n${wait.stderr}`).toBe(0);
    expect(JSON.parse(wait.stdout.trim())).toEqual({
      id: "session-cli-1",
      status: "idle",
      acceptsMessages: true,
      provider: "deepseek",
      runtime: { size: "0.25cpu-1gb" }
    });

    const outPath = join(install.installDir, "installed-cli-run.zip");
    const download = await runCommand(binPath, ["download", "session-cli-1", "--out", outPath, ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(download.exitCode, `stdout:\n${download.stdout}\nstderr:\n${download.stderr}`).toBe(0);
    expect(JSON.parse(download.stdout.trim())).toMatchObject({
      sessionId: "session-cli-1",
      namespace: "all",
      path: outPath
    });
    expect(existsSync(outPath)).toBe(true);
    const entries = unzipSync(new Uint8Array(readFileSync(outPath)));
    expect(Object.keys(entries).sort()).toEqual([
      "events/events.jsonl",
      "files/report.txt",
      "manifest.json",
      "metadata/session.json"
    ]);
    expect(new TextDecoder().decode(entries["files/report.txt"]!)).toBe("hello world");

    const cancel = await runCommand(binPath, ["cancel", "session-cli-1", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(cancel.exitCode, `stdout:\n${cancel.stdout}\nstderr:\n${cancel.stderr}`).toBe(0);
    expect(JSON.parse(cancel.stdout.trim())).toEqual({ sessionId: "session-cli-1", status: "cancelling" });

    expect(api.requests.every((request) => request.authorization === "Bearer tok-installed-cli")).toBe(true);
    const methodPaths = api.requests.map((request) => `${request.method} ${request.path}`);
    // `aex start` creates the session, then posts the first turn as a message.
    expect(methodPaths.slice(0, 2)).toEqual(["POST /api/sessions", "POST /api/sessions/session-cli-1/messages"]);
    expect(methodPaths).toEqual(
      expect.arrayContaining([
        "POST /api/sessions",
        "POST /api/sessions/session-cli-1/messages",
        "GET /api/sessions/session-cli-1",
        "GET /api/sessions/session-cli-1/events",
        "POST /api/sessions/session-cli-1/cancel",
        // download assembles the public zip from the session-namespaced read endpoints
        "GET /api/sessions/session-cli-1",
        "GET /api/sessions/session-cli-1/events",
        "GET /api/sessions/session-cli-1/files",
        "GET /api/sessions/session-cli-1/files/out-1/download?checkpointId=cp-1"
      ])
    );

    // submit transport: create carries session config, message carries the
    // first-turn input, and both idempotency keys ride request headers.
    const createReq = api.requests[0]!;
    const messageReq = api.requests[1]!;
    const submit = createReq.body as Record<string, unknown>;
    expect(submit.workspaceId).toBeUndefined();
    expect(submit.provider).toBe("deepseek");
    expect(submit).not.toHaveProperty("region");
    expect(submit).not.toHaveProperty("idempotencyKey");
    expect(submit).not.toHaveProperty("input");
    expect(createReq.idempotencyKey).toBe("cli-host-installed-shape");
    expect(submit.retention).toEqual({ idleTtl: "3m" });
    expect(submit.secrets).toEqual({ apiKeys: { deepseek: "sk-deepseek-test" } });
    expect(submit.submission).toMatchObject({ model: "deepseek-v4-flash" });
    expect(submit.submission).not.toHaveProperty("prompt");
    expect(messageReq.idempotencyKey).toBe("cli-host-installed-shape:message");
    expect(messageReq.body).toEqual({ input: ["hello_from_installed_cli"] });
  });

  it("preserves described API error envelopes in the packed CLI binary", async () => {
    const status = await runCommand(
      binPath,
      ["status", "session-denied", "--api-key", "tok-installed-cli", "--aex-url", api.baseUrl],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );

    expect(status.exitCode).toBe(1);
    expect(status.stdout).toBe("");
    expect(status.stderr).toBe(
      '{"error":"status_failed","message":"insufficient_scope: the token does not carry sessions:read — {\\"requestId\\":\\"req-packed-cli-error\\"}","sessionId":"session-denied","status":403,"remedy":"token lacks permission for this workspace/action"}\n'
    );

    const wait = await runCommand(
      binPath,
      ["wait", "session-denied", "--api-key", "tok-installed-cli", "--aex-url", api.baseUrl],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );

    expect(wait.exitCode).toBe(1);
    expect(wait.stdout).toBe("");
    expect(wait.stderr).toBe(
      '{"error":"wait_failed","message":"insufficient_scope: the token does not carry sessions:read — {\\"requestId\\":\\"req-packed-cli-error\\"}","sessionId":"session-denied","status":403,"remedy":"token lacks permission for this workspace/action"}\n'
    );
  });

  it("reads billing, the webhook signing secret, and the workspace lists through the installed binary", async () => {
    const common = ["--api-key", "tok-installed-cli", "--aex-url", api.baseUrl] as const;

    const billing = await runCommand(binPath, ["billing", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(billing.exitCode, `stdout:\n${billing.stdout}\nstderr:\n${billing.stderr}`).toBe(0);
    expect(billing.stdout).toContain("$25.00");
    expect(billing.stdout).toContain("$1.50");
    expect(billing.stdout).toContain("$100.00");

    const billingJson = await runCommand(binPath, ["billing", "--json", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(billingJson.exitCode, `stdout:\n${billingJson.stdout}\nstderr:\n${billingJson.stderr}`).toBe(0);
    expect(JSON.parse(billingJson.stdout.trim())).toEqual({
      balanceUsd: 25,
      monthSpendUsd: 1.5,
      spendCapUsd: 100,
      planKey: "free",
      subscriptionStatus: "none"
    });

    const ledger = await runCommand(binPath, ["billing", "ledger", "--limit", "10", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(ledger.exitCode, `stdout:\n${ledger.stdout}\nstderr:\n${ledger.stderr}`).toBe(0);
    const ledgerEntries = JSON.parse(ledger.stdout.trim()) as Array<{ id: string; entryType: string }>;
    expect(ledgerEntries.map((entry) => entry.id)).toEqual(["led-1"]);
    expect(ledgerEntries[0]!.entryType).toBe("top_up");

    const secret = await runCommand(binPath, ["webhooks", "secret", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(secret.exitCode, `stdout:\n${secret.stdout}\nstderr:\n${secret.stderr}`).toBe(0);
    // The reveal verb prints the bare whsec string — pipeable straight into a
    // verifier — and never echoes it to stderr.
    expect(secret.stdout.trim()).toBe("whsec_aW5zdGFsbGVkLWNsaS1zZWNyZXQ=");
    expect(secret.stderr).not.toContain("whsec_");

    const rotate = await runCommand(binPath, ["webhooks", "secret", "--rotate", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(rotate.exitCode).toBe(2);
    expect(rotate.stderr).toContain("not supported");

    const sessions = await runCommand(binPath, ["sessions", "--since", "2026-07-01T00:00:00Z", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(sessions.exitCode, `stdout:\n${sessions.stdout}\nstderr:\n${sessions.stderr}`).toBe(0);
    const sessionsPage = JSON.parse(sessions.stdout.trim()) as { sessions: Array<{ id: string }> };
    // The CLI enforces --since client-side (the deployed API ignores the param),
    // so only the July session survives.
    expect(sessionsPage.sessions.map((session) => session.id)).toEqual(["session-cli-1"]);

    const limitedSessions = await runCommand(binPath, ["sessions", "--limit", "5", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(limitedSessions.exitCode, `stdout:\n${limitedSessions.stdout}\nstderr:\n${limitedSessions.stderr}`).toBe(0);
    const limitedSessionsPage = JSON.parse(limitedSessions.stdout.trim()) as { sessions: Array<{ id: string }> };
    expect(limitedSessionsPage.sessions.map((session) => session.id)).toEqual(["session-cli-1"]);

    const methodPaths = api.requests.map((request) => `${request.method} ${request.path}`);
    expect(methodPaths).toEqual(
      expect.arrayContaining([
        "GET /api/billing",
        "GET /api/billing/ledger?limit=10",
        "POST /api/webhook/signing-secret",
        "GET /api/sessions?since=2026-07-01T00%3A00%3A00Z",
        "GET /api/sessions?limit=5"
      ])
    );
  });

  it("gives split and equals value syntax byte-for-byte parity in the packed CLI", async () => {
    const splitArgs = [
      "start",
      "--provider", "deepseek",
      "--model", "deepseek-v4-flash",
      "--prompt", "packed_equals_parity",
      "--deepseek-api-key", "sk-packed-equals-secret",
      "--metadata", "syntax=split",
      "--idempotency-key", "packed-equals-parity",
      "--runtime", "container",
      "--runtime-size", "0.25cpu-1gb",
      "--api-key", "tok-installed-cli",
      "--aex-url", api.baseUrl
    ];
    const equalsArgs = [
      "start",
      "--provider=deepseek",
      "--model=deepseek-v4-flash",
      "--prompt=packed_equals_parity",
      "--deepseek-api-key=sk-packed-equals-secret",
      "--metadata=syntax=split",
      "--idempotency-key=packed-equals-parity",
      "--runtime=container",
      "--runtime-size=0.25cpu-1gb",
      "--api-key=tok-installed-cli",
      `--aex-url=${api.baseUrl}`
    ];

    const beforeSplit = api.requests.length;
    const split = await runCommand(binPath, splitArgs, { cwd: install.installDir, timeoutMs: 30_000 });
    const splitRequests = api.requests.slice(beforeSplit);
    const beforeEquals = api.requests.length;
    const joined = await runCommand(binPath, equalsArgs, { cwd: install.installDir, timeoutMs: 30_000 });
    const joinedRequests = api.requests.slice(beforeEquals);

    expect(joined).toEqual(split);
    expect(joined.exitCode, `stdout:\n${joined.stdout}\nstderr:\n${joined.stderr}`).toBe(0);
    expect(joinedRequests).toEqual(splitRequests);
    expect(joined.stdout).not.toContain("sk-packed-equals-secret");
    expect(joined.stderr).not.toContain("sk-packed-equals-secret");

    const missing = await runCommand(
      binPath,
      ["status", "session-cli-1", "--api-key", "tok-installed-cli", "--aex-url"],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(missing).toMatchObject({ exitCode: 2, stdout: "", stderr: "--aex-url requires a value\n" });
  });

  it("preserves start duplicate precedence and negative ordering in the packed CLI", async () => {
    const before = api.requests.length;
    const duplicate = await runCommand(binPath, [
      "start",
      "--provider=anthropic", "--provider", "deepseek",
      "--model=claude-haiku-4-5", "--model", "deepseek-v4-flash",
      "--prompt=first", "--prompt", "second",
      "--deepseek-api-key=old-secret", "--deepseek-api-key", "new-secret",
      "--metadata=mode=first", "--metadata", "mode=last",
      "--runtime-size=1cpu-4gb", "--runtime-size", "0.25cpu-1gb",
      "--api-key=old-token", "--api-key", "tok-installed-cli",
      `--aex-url=${api.baseUrl}`
    ], { cwd: install.installDir, timeoutMs: 30_000 });
    const requests = api.requests.slice(before);

    expect(duplicate.exitCode, `stdout:\n${duplicate.stdout}\nstderr:\n${duplicate.stderr}`).toBe(0);
    expect(duplicate.stdout).not.toContain("new-secret");
    expect(duplicate.stderr).not.toContain("new-secret");
    expect(requests).toHaveLength(2);
    expect(requests[0]).toMatchObject({
      method: "POST",
      path: "/api/sessions",
      authorization: "Bearer tok-installed-cli",
      body: {
        provider: "deepseek",
        runtimeSize: "0.25cpu-1gb",
        submission: { model: "deepseek-v4-flash", metadata: { mode: "last" } },
        secrets: { apiKeys: { deepseek: "new-secret" } }
      }
    });
    expect(requests[1]).toMatchObject({ body: { input: ["first", "second"] } });

    const negativeBefore = api.requests.length;
    const negative = await runCommand(binPath, [
      "start", "unexpected-position", "--unknown",
      "--api-key", "tok-installed-cli", "--aex-url", api.baseUrl
    ], { cwd: install.installDir, timeoutMs: 30_000 });
    expect(negative).toMatchObject({
      exitCode: 2,
      stdout: "",
      stderr: "aex start --unknown: unknown flag\n"
    });
    expect(api.requests).toHaveLength(negativeBefore);
  });
});
