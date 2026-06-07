/**
 * Installed CLI host-command coverage.
 *
 * This is the blackbox layer for the commands users run after
 * `npm install @aexhq/sdk`: spawn the installed `aex` binary from a clean
 * temp install and point it at a local fake API. Unit tests cover the same
 * verbs through an injected fetch; this file catches packaging, bin wiring,
 * auth header, URL, and public wire-shape drift in the shipped artifact.
 */
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { strToU8, unzipSync } from "fflate";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const IS_WINDOWS = process.platform === "win32";

interface CapturedRequest {
  readonly method: string;
  readonly path: string;
  readonly authorization: string | undefined;
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
      body: parsedBody
    });

    if (req.method === "POST" && url.pathname === "/api/runs") {
      json(res, 200, { id: "run-cli-1", status: "queued", provider: "deepseek", runtime: "managed" });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/runs/run-cli-1") {
      statusPolls += 1;
      json(res, 200, {
        id: "run-cli-1",
        status: statusPolls > 1 ? "succeeded" : "running",
        provider: "deepseek",
        runtime: "managed"
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/runs/run-cli-1/events") {
      json(res, 200, {
        events: [
          { id: "evt-1", type: "RUN_STARTED", data: { phase: "start" } },
          { id: "evt-2", type: "RUN_FINISHED", data: { reason: "complete" } }
        ]
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/runs/run-cli-1/outputs") {
      json(res, 200, {
        outputs: [{ id: "out-1", filename: "report.txt", sizeBytes: 11, contentType: "text/plain" }]
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/runs/run-cli-1/outputs/out-1/download") {
      bytes(res, 200, strToU8("hello world"), "text/plain");
      return;
    }
    if (req.method === "POST" && url.pathname === "/api/runs/run-cli-1/cancel") {
      json(res, 200, {});
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
    binPath = join(install.installDir, "node_modules", ".bin", IS_WINDOWS ? "aex.cmd" : "aex");
  });

  afterAll(async () => {
    await api?.close();
    install?.cleanup();
  });

  it("runs run/status/events/wait/download/cancel through the installed binary", async () => {
    const common = ["--api-token", "tok-installed-cli", "--aex-url", api.baseUrl] as const;

    const run = await runCommand(
      binPath,
      [
        "run",
        "--provider",
        "deepseek",
        "--model",
        "deepseek-chat",
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
    expect(JSON.parse(run.stdout.trim())).toMatchObject({ id: "run-cli-1", status: "queued" });

    const status = await runCommand(binPath, ["status", "run-cli-1", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(status.exitCode, `stdout:\n${status.stdout}\nstderr:\n${status.stderr}`).toBe(0);
    expect(JSON.parse(status.stdout.trim())).toMatchObject({ id: "run-cli-1" });

    const events = await runCommand(binPath, ["events", "run-cli-1", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(events.exitCode, `stdout:\n${events.stdout}\nstderr:\n${events.stderr}`).toBe(0);
    const eventLines = events.stdout.trim().split(/\r?\n/).map((line) => JSON.parse(line) as { id: string });
    expect(eventLines.map((event) => event.id)).toEqual(["evt-1", "evt-2"]);

    const wait = await runCommand(
      binPath,
      ["wait", "run-cli-1", "--interval", "1ms", "--timeout", "2s", ...common],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(wait.exitCode, `stdout:\n${wait.stdout}\nstderr:\n${wait.stderr}`).toBe(0);
    expect(JSON.parse(wait.stdout.trim())).toMatchObject({ id: "run-cli-1", status: "succeeded" });

    const outPath = join(install.installDir, "installed-cli-run.zip");
    const download = await runCommand(binPath, ["download", "run-cli-1", "--out", outPath, ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(download.exitCode, `stdout:\n${download.stdout}\nstderr:\n${download.stderr}`).toBe(0);
    expect(JSON.parse(download.stdout.trim())).toMatchObject({
      runId: "run-cli-1",
      namespace: "all",
      path: outPath
    });
    expect(existsSync(outPath)).toBe(true);
    const entries = unzipSync(new Uint8Array(readFileSync(outPath)));
    expect(Object.keys(entries).sort()).toEqual([
      "events/events.jsonl",
      "manifest.json",
      "metadata/run.json",
      "outputs/report.txt"
    ]);
    expect(new TextDecoder().decode(entries["outputs/report.txt"]!)).toBe("hello world");

    const cancel = await runCommand(binPath, ["cancel", "run-cli-1", ...common], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(cancel.exitCode, `stdout:\n${cancel.stdout}\nstderr:\n${cancel.stderr}`).toBe(0);
    expect(JSON.parse(cancel.stdout.trim())).toEqual({ runId: "run-cli-1", status: "cancel_requested" });

    expect(api.requests.every((request) => request.authorization === "Bearer tok-installed-cli")).toBe(true);
    const methodPaths = api.requests.map((request) => `${request.method} ${request.path}`);
    expect(methodPaths[0]).toBe("POST /api/runs");
    expect(methodPaths).toEqual(
      expect.arrayContaining([
        "GET /api/runs/run-cli-1",
        "GET /api/runs/run-cli-1/events",
        "GET /api/runs/run-cli-1/outputs",
        "GET /api/runs/run-cli-1/outputs/out-1/download",
        "POST /api/runs/run-cli-1/cancel"
      ])
    );
    expect(methodPaths.filter((path) => path === "GET /api/runs/run-cli-1/events").length).toBeGreaterThanOrEqual(2);

    const submit = api.requests[0]!.body as Record<string, unknown>;
    expect(submit.workspaceId).toBeUndefined();
    expect(submit.provider).toBe("deepseek");
    expect(submit.idempotencyKey).toBe("cli-host-installed-shape");
    expect(submit.secrets).toEqual({ apiKey: "sk-deepseek-test" });
    expect(submit.submission).toMatchObject({
      model: "deepseek-chat",
      prompt: ["hello_from_installed_cli"]
    });
  });
});
