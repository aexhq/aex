import { describe, expect, it } from "vitest";
import { strToU8, unzipSync } from "fflate";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve as resolvePath } from "node:path";
import { runCli } from "../src/run.js";
import { parseDuration } from "../src/host/common.js";
import type { CliIO } from "../src/internal.js";

const CWD = "/tmp/cli-test";

/**
 * Make a real temp directory containing a SKILL.md file so the
 * `aex skills upload --file ...` path can read actual bytes.
 * The skills-cmd module reads from `node:fs/promises` directly (not the
 * CliIO abstraction), so virtualizing files via `makeHostIo({files})`
 * does not work for this path. The test is responsible for cleanup.
 */
function makeSkillsTmpDir(skillBody: string): { dir: string; cleanup: () => void } {
  const dir = mkdtempSync(join(tmpdir(), "aex-cli-skills-"));
  writeFileSync(join(dir, "SKILL.md"), skillBody, "utf8");
  return {
    dir,
    cleanup: () => {
      try { rmSync(dir, { recursive: true, force: true }); } catch { /* best-effort */ }
    }
  };
}

/** Compute the absolute path the run-config loader will produce for a
 * given input — keeps tests cross-platform between Windows and POSIX. */
function resolvedFromCwd(p: string): string {
  return resolvePath(CWD, p);
}

interface FetchCall {
  url: string;
  init: RequestInit;
  body: unknown;
}

function makeHostIo(opts: {
  argv: readonly string[];
  fetchHandler?: (call: FetchCall) => Response;
  files?: Record<string, string>;
  writes?: Map<string, Uint8Array>;
}): {
  io: CliIO;
  stdout: string;
  stderr: string;
  exitCode: number | null;
  calls: FetchCall[];
} {
  const state = {
    stdout: "",
    stderr: "",
    exitCode: null as number | null,
    calls: [] as FetchCall[]
  };
  const files = opts.files ?? {};
  const writes = opts.writes ?? new Map<string, Uint8Array>();

  const io: CliIO = {
    argv: ["bun", "/aex/aex", ...opts.argv],
    readFile: async (path) => {
      if (!(path in files)) {
        throw Object.assign(new Error(`ENOENT: ${path}`), { code: "ENOENT" });
      }
      return files[path]!;
    },
    writeFile: async (path, data) => {
      writes.set(path, data);
    },
    cwd: () => CWD,
    fetchImpl: (async (input, init) => {
      const reqInit: RequestInit = init ?? {};
      let body: unknown;
      if (typeof reqInit.body === "string") {
        try {
          body = JSON.parse(reqInit.body);
        } catch {
          body = reqInit.body;
        }
      }
      const url = String(input);
      const call: FetchCall = { url, init: reqInit, body };
      state.calls.push(call);
      const handler = opts.fetchHandler ?? (() => new Response("{}", { status: 200, headers: { "content-type": "application/json" } }));
      return handler(call);
    }) as typeof fetch,
    stdout: (chunk) => {
      state.stdout += chunk;
    },
    stderr: (chunk) => {
      state.stderr += chunk;
    },
    exit: (code) => {
      state.exitCode = code;
    }
  };

  return {
    io,
    get stdout() { return state.stdout; },
    get stderr() { return state.stderr; },
    get exitCode() { return state.exitCode; },
    get calls() { return state.calls; }
  };
}

const COMMON = ["--api-token", "tok-1", "--aex-url", "https://dash.example/"];

describe("aex whoami", () => {
  it("calls GET /api/whoami without a workspace query and prints the body", async () => {
    const cap = makeHostIo({
      argv: ["whoami", "--api-token", "tok-1", "--aex-url", "https://dash.example/"],
      fetchHandler: () =>
        new Response(JSON.stringify({ principalType: "api_token", workspaceId: "ws-7", scopes: ["runs.write"] }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/whoami");
    expect(cap.calls[0]!.init.method ?? "GET").toBe("GET");
    const printed = JSON.parse(cap.stdout.trim()) as { principalType: string; workspaceId: string };
    expect(printed.principalType).toBe("api_token");
    expect(printed.workspaceId).toBe("ws-7");
  });

  it("rejects when --api-token is missing", async () => {
    const cap = makeHostIo({ argv: ["whoami", "--aex-url", "https://dash.example/"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--api-token");
  });

  it("defaults --aex-url to https://api.aex.dev when omitted", async () => {
    const cap = makeHostIo({
      argv: ["whoami", "--api-token", "tok-1"],
      fetchHandler: () =>
        new Response(JSON.stringify({ principalType: "api_token", workspaceId: "ws-9", scopes: [] }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.url).toBe("https://api.aex.dev/api/whoami");
  });
});

describe("aex status", () => {
  it("does not send a workspaceId query parameter (derived from token server-side)", async () => {
    const cap = makeHostIo({
      argv: ["status", "run-42", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "run-42", status: "succeeded" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.url).toContain("/api/runs/run-42");
    expect(cap.calls[0]!.url).not.toContain("workspaceId=");
    expect(JSON.parse(cap.stdout)).toEqual({ id: "run-42", status: "succeeded" });
  });

  it("does not accept a --workspace flag (workspace is derived from the token)", async () => {
    const cap = makeHostIo({
      argv: ["status", "run-1", "--workspace", "ws-1", "--api-token", "tok", "--aex-url", "https://x"]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("unknown");
  });
});

describe("aex deliveries", () => {
  it("GETs the webhook-deliveries endpoint and prints the array as JSON", async () => {
    const rows = [
      {
        id: "wd-1",
        eventType: "run.finished",
        status: "delivered",
        attemptCount: 1,
        lastStatusCode: 200,
        createdAt: "2026-06-21T00:00:00.000Z"
      }
    ];
    const cap = makeHostIo({
      argv: ["deliveries", "run-42", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ deliveries: rows }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/runs/run-42/webhook-deliveries");
    expect(cap.calls[0]!.init.method ?? "GET").toBe("GET");
    expect(JSON.parse(cap.stdout)).toEqual(rows);
  });

  it("requires exactly one run-id positional", async () => {
    const cap = makeHostIo({ argv: ["deliveries", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("usage: aex deliveries");
  });
});

describe("aex events", () => {
  it("lists events as NDJSON", async () => {
    const cap = makeHostIo({
      argv: ["events", "run-9", ...COMMON],
      fetchHandler: () =>
        new Response(
          JSON.stringify({
            events: [
              { id: "e1", type: "agent.message" },
              { id: "e2", type: "session.status_running" }
            ]
          }),
          { status: 200, headers: { "content-type": "application/json" } }
        )
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const lines = cap.stdout.trim().split("\n");
    expect(lines).toHaveLength(2);
    expect(JSON.parse(lines[0]!)).toEqual({ id: "e1", type: "agent.message" });
    expect(JSON.parse(lines[1]!)).toEqual({ id: "e2", type: "session.status_running" });
  });

  it("--follow polls /events and emits NDJSON until terminal (never opens an SSE stream)", async () => {
    const cap = makeHostIo({
      argv: ["events", "run-poll", "--follow", ...COMMON],
      fetchHandler: (call) => {
        if (call.url.endsWith("/events/stream")) {
          throw new Error("SSE endpoint must not be touched");
        }
        if (call.url.endsWith("/events")) {
          return new Response(
            JSON.stringify({ events: [{ id: "p1", type: "TEXT_MESSAGE_CONTENT" }] }),
            { status: 200, headers: { "content-type": "application/json" } }
          );
        }
        // GET run record — return terminal so the loop exits.
        return new Response(JSON.stringify({ id: "run-poll", status: "succeeded" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout.trim()).toContain("p1");
    expect(cap.calls.some((c) => c.url.endsWith("/events/stream"))).toBe(false);
  });

  it("--follow stops on a timed_out run instead of hanging", async () => {
    // Regression: `timed_out` is a terminal status. A prior hardcoded set
    // omitted it, so the polling loop would never exit for a timed-out run.
    const cap = makeHostIo({
      argv: ["events", "run-timeout", "--follow", ...COMMON],
      fetchHandler: (call) => {
        if (call.url.endsWith("/events")) {
          return new Response(
            JSON.stringify({ events: [{ id: "t1", type: "TEXT_MESSAGE_CONTENT" }] }),
            { status: 200, headers: { "content-type": "application/json" } }
          );
        }
        // GET run record — terminal `timed_out` must exit the loop.
        return new Response(JSON.stringify({ id: "run-timeout", status: "timed_out" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout.trim()).toContain("t1");
  });
});

describe("parseDuration", () => {
  it("parses unit suffixes and bare ms integers", () => {
    expect(parseDuration("500ms").ms).toBe(500);
    expect(parseDuration("30s").ms).toBe(30_000);
    expect(parseDuration("8m").ms).toBe(480_000);
    expect(parseDuration("1h").ms).toBe(3_600_000);
    expect(parseDuration("2000").ms).toBe(2000);
    expect(parseDuration("1.5s").ms).toBe(1500);
  });

  it("rejects malformed and negative input without coercing to 0", () => {
    expect(parseDuration("soon").error).toBeTruthy();
    expect(parseDuration("5min").error).toBeTruthy();
    expect(parseDuration("-5s").error).toBeTruthy();
    expect(parseDuration("").error).toBeTruthy();
    expect(parseDuration("soon").ms).toBeNull();
  });
});

describe("aex wait", () => {
  it("polls GET /runs/{id} until terminal, prints the final run, exits 0 on succeeded", async () => {
    let polls = 0;
    const cap = makeHostIo({
      argv: ["wait", "run-w", "--interval", "1ms", ...COMMON],
      fetchHandler: () => {
        polls++;
        const status = polls < 3 ? "running" : "succeeded";
        return new Response(JSON.stringify({ id: "run-w", status }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(polls).toBe(3);
    const printed = JSON.parse(cap.stdout.trim()) as { id: string; status: string };
    expect(printed).toMatchObject({ id: "run-w", status: "succeeded" });
  });

  it("exits 1 (RUNTIME_ERR) when the run reaches a non-succeeded terminal status", async () => {
    const cap = makeHostIo({
      argv: ["wait", "run-f", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "run-f", status: "failed" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(1);
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ status: "failed" });
  });

  it("exits 3 (TIMEOUT_ERR) with a JSON error when --timeout elapses before terminal", async () => {
    const cap = makeHostIo({
      argv: ["wait", "run-slow", "--timeout", "0ms", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "run-slow", status: "queued" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(3);
    const err = JSON.parse(cap.stderr.trim()) as { error: string; runId: string; lastStatus: string };
    expect(err.error).toBe("wait_timeout");
    expect(err.runId).toBe("run-slow");
    expect(err.lastStatus).toBe("queued");
  });

  it("rejects a malformed --timeout with USAGE_ERR", async () => {
    const cap = makeHostIo({ argv: ["wait", "run-x", "--timeout", "soon", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--timeout");
  });

  it("requires exactly one run-id positional", async () => {
    const cap = makeHostIo({ argv: ["wait", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("usage: aex wait");
  });
});

describe("aex events --follow --timeout", () => {
  it("exits 3 with a JSON error when the follow deadline elapses before terminal", async () => {
    const cap = makeHostIo({
      argv: ["events", "run-ev", "--follow", "--timeout", "0ms", ...COMMON],
      fetchHandler: (call) => {
        if (call.url.endsWith("/events")) {
          return new Response(JSON.stringify({ events: [] }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        return new Response(JSON.stringify({ id: "run-ev", status: "running" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(3);
    expect(JSON.parse(cap.stderr.trim())).toMatchObject({ error: "events_follow_timeout", runId: "run-ev" });
  });
});

describe("aex outputs", () => {
  it("lists outputs as NDJSON", async () => {
    const cap = makeHostIo({
      argv: ["outputs", "run-9", ...COMMON],
      fetchHandler: () =>
        new Response(
          JSON.stringify({ outputs: [{ id: "o1", filename: "report.md" }] }),
          { status: 200, headers: { "content-type": "application/json" } }
        )
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ id: "o1", filename: "report.md" });
  });
});

describe("aex cancel + delete", () => {
  it("cancel POSTs and prints the result", async () => {
    const cap = makeHostIo({
      argv: ["cancel", "run-x", ...COMMON],
      fetchHandler: () => new Response("{}", { status: 200, headers: { "content-type": "application/json" } })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.init.method).toBe("POST");
    expect(cap.calls[0]!.url).toContain("/api/runs/run-x/cancel");
    expect(JSON.parse(cap.stdout)).toEqual({ runId: "run-x", status: "cancel_requested" });
  });

  it("delete DELETEs and prints the result", async () => {
    const cap = makeHostIo({
      argv: ["delete", "run-x", ...COMMON],
      fetchHandler: () => new Response("{}", { status: 200, headers: { "content-type": "application/json" } })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.init.method).toBe("DELETE");
    expect(JSON.parse(cap.stdout)).toEqual({ runId: "run-x", deleted: true });
  });

  it("delete-asset DELETEs a normalized workspace asset id and prints the result", async () => {
    const hex = "a".repeat(64);
    const cap = makeHostIo({
      argv: ["delete-asset", `sha256:${hex}`, ...COMMON],
      fetchHandler: () => new Response(null, { status: 204 })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe(`https://dash.example/assets/asset_${hex}`);
    expect(cap.calls[0]!.init.method).toBe("DELETE");
    expect(JSON.parse(cap.stdout.trim())).toEqual({ hash: `sha256:${hex}`, deleted: true });
  });

  it("delete-asset rejects missing hashes without calling the API", async () => {
    const cap = makeHostIo({
      argv: ["delete-asset", ...COMMON]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("usage: aex delete-asset");
    expect(cap.calls).toHaveLength(0);
  });

  it("delete-asset emits a structured error with the requested hash on API failure", async () => {
    const hex = "c".repeat(64);
    const cap = makeHostIo({
      argv: ["delete-asset", hex, ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ error: "asset_not_found", message: "asset not found" }), {
          status: 404,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(1);
    expect(cap.calls[0]!.url).toBe(`https://dash.example/assets/asset_${hex}`);
    const err = JSON.parse(cap.stderr.trim()) as {
      error: string;
      message: string;
      hash: string;
      status?: number;
      remedy?: string;
    };
    // DX4: the envelope is now enriched with the HTTP status + an actionable
    // remedy keyed on it (the message stays the API's own error string).
    expect(err).toEqual({
      error: "delete_asset_failed",
      message: "asset_not_found",
      hash: hex,
      status: 404,
      remedy: "no such run/resource — verify the id"
    });
  });
});

describe("aex download", () => {
  // Route the reads the download verbs fan out to: getRun + listEvents +
  // listOutputs + per-output /download.
  const json = (body: unknown) =>
    new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  const wholeRunHandler =
    (runId: string) =>
    ({ url }: { url: string }): Response => {
      if (url.endsWith(`/api/runs/${runId}/events`)) return json({ events: [{ seq: 0, kind: "runtime_start" }] });
      if (url.endsWith(`/api/runs/${runId}/outputs`)) {
        return json({ outputs: [{ id: "o1", filename: "report.txt", sizeBytes: 5, contentType: "text/plain" }] });
      }
      if (url.endsWith(`/api/runs/${runId}/outputs/o1/download`)) {
        return new Response(strToU8("hello").buffer, { status: 200, headers: { "content-type": "text/plain" } });
      }
      return json({ id: runId, status: "succeeded" });
    };

  it("assembles the public whole-run zip client-side and writes it to --out", async () => {
    const writes = new Map<string, Uint8Array>();
    const cap = makeHostIo({
      argv: ["download", "run-1", "--out", "run-1.zip", ...COMMON],
      writes,
      fetchHandler: wholeRunHandler("run-1")
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    // getRun + events + outputs + one per-output download.
    expect(cap.calls).toHaveLength(4);

    const writtenKey = [...writes.keys()][0]!;
    expect(writtenKey).toMatch(/run-1\.zip$/);
    const entries = unzipSync(writes.get(writtenKey)!);
    expect(Object.keys(entries).sort()).toEqual([
      "events/events.jsonl",
      "manifest.json",
      "metadata/run.json",
      "outputs/report.txt"
    ]);
    expect(new TextDecoder().decode(entries["outputs/report.txt"]!)).toBe("hello");
    expect(JSON.parse(new TextDecoder().decode(entries["metadata/run.json"]!)).id).toBe("run-1");

    const printed = JSON.parse(cap.stdout.trim()) as { runId: string; namespace: string; bytes: number };
    expect(printed.runId).toBe("run-1");
    expect(printed.namespace).toBe("all");
    expect(printed.bytes).toBe(writes.get(writtenKey)!.byteLength);
  });

  it("defaults the output path to aex-run-<run-id>.zip when --out is omitted", async () => {
    const writes = new Map<string, Uint8Array>();
    const cap = makeHostIo({
      argv: ["download", "run-2", ...COMMON],
      writes,
      fetchHandler: wholeRunHandler("run-2")
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const writtenKey = [...writes.keys()][0]!;
    expect(writtenKey).toMatch(/aex-run-run-2\.zip$/);
  });

  it("--only outputs zips just the deliverables (no logs, no metadata/events)", async () => {
    const writes = new Map<string, Uint8Array>();
    const cap = makeHostIo({
      argv: ["download", "run-1", "--only", "outputs", ...COMMON],
      writes,
      fetchHandler: wholeRunHandler("run-1")
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const writtenKey = [...writes.keys()][0]!;
    expect(writtenKey).toMatch(/aex-run-run-1-outputs\.zip$/);
    const entries = unzipSync(writes.get(writtenKey)!);
    expect(Object.keys(entries).sort()).toEqual(["manifest.json", "report.txt"]);
    expect(new TextDecoder().decode(entries["report.txt"]!)).toBe("hello");
    expect((JSON.parse(cap.stdout.trim()) as { namespace: string }).namespace).toBe("outputs");
  });

  it("rejects --only logs with a usage error", async () => {
    const cap = makeHostIo({
      argv: ["download", "run-1", "--only", "logs", ...COMMON],
      fetchHandler: wholeRunHandler("run-1")
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--only must be one of");
    expect(cap.calls).toHaveLength(0);
  });

  it("--only metadata reads only the run record", async () => {
    const writes = new Map<string, Uint8Array>();
    const cap = makeHostIo({
      argv: ["download", "run-1", "--only", "metadata", ...COMMON],
      writes,
      fetchHandler: wholeRunHandler("run-1")
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/runs/run-1");
    const entries = unzipSync(writes.get([...writes.keys()][0]!)!);
    expect(Object.keys(entries)).toEqual(["run.json"]);
  });

  it("rejects an unknown --only namespace with a usage error", async () => {
    const cap = makeHostIo({
      argv: ["download", "run-1", "--only", "bogus", ...COMMON],
      fetchHandler: wholeRunHandler("run-1")
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--only must be one of");
    expect(cap.calls).toHaveLength(0);
  });
});

describe("aex run", () => {
  it("submits a run config loaded from --config and prints the run record", async () => {
    const runConfig = {
      model: "claude-haiku-4-5",
      system: "be helpful",
      prompt: ["hi"],
      skills: [{ kind: "asset", assetId: "asset_pdf", name: "pdf" }],
      mcpServers: [
        {
          name: "github",
          url: "https://example.com/mcp",
          headers: { Authorization: "Bearer t-from-config" }
        }
      ],
      postHook: {
        command: "bun test",
        timeout: "2m",
        maxTurns: 2,
        maxChars: 2048
      }
    };
    const cap = makeHostIo({
      argv: [
        "run",
        "--config",
        "/abs/run.json",
        "--anthropic-api-key",
        "sk-ant-1",
        "--idempotency-key",
        "idem-deterministic",
        ...COMMON
      ],
      files: { [resolvedFromCwd("/abs/run.json")]: JSON.stringify(runConfig) },
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "run-new", status: "queued" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/runs");
    expect(cap.calls[0]!.init.method).toBe("POST");
    const body = cap.calls[0]!.body as Record<string, unknown>;
    expect(body.workspaceId).toBeUndefined();
    expect(body.idempotencyKey).toBe("idem-deterministic");
    expect(body.postHook).toEqual({
      command: "bun test",
      timeout: "2m",
      maxTurns: 2,
      maxChars: 2048
    });
    const submission = body.submission as Record<string, unknown>;
    expect(submission.model).toBe("claude-haiku-4-5");
    expect(submission.prompt).toEqual(["hi"]);
    expect(submission.skills).toEqual([
      { kind: "asset", assetId: "asset_pdf", name: "pdf" }
    ]);
    expect(submission.mcpServers).toEqual([
      { name: "github", url: "https://example.com/mcp" }
    ]);
    const secrets = body.secrets as Record<string, unknown>;
    expect(secrets.apiKeys).toEqual({ anthropic: "sk-ant-1" });
    expect(secrets.mcpServers).toEqual([
      {
        name: "github",
        url: "https://example.com/mcp",
        headers: { Authorization: "Bearer t-from-config" }
      }
    ]);
    const printed = JSON.parse(cap.stdout.trim()) as { id: string; status: string };
    expect(printed).toMatchObject({ id: "run-new", status: "queued" });
  });

  it("submits a run request built from --model/--prompt/--mcp/--mcp-auth flags", async () => {
    const cap = makeHostIo({
      argv: [
        "run",
        "--model",
        "claude-haiku-4-5",
        "--prompt",
        "hello",
        "--mcp",
        "github=https://example.com/mcp",
        "--mcp-auth",
        "github=Authorization:Bearer t",
        "--anthropic-api-key",
        "sk-ant-2",
        "--idempotency-key",
        "idem-flat",
        ...COMMON
      ],
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "r-flat", status: "queued" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const body = cap.calls[0]!.body as Record<string, unknown>;
    const submission = body.submission as Record<string, unknown>;
    expect(submission.skills).toEqual([]);
    expect(submission.mcpServers).toEqual([
      { name: "github", url: "https://example.com/mcp" }
    ]);
    const secrets = body.secrets as Record<string, unknown>;
    expect(secrets.mcpServers).toEqual([
      {
        name: "github",
        url: "https://example.com/mcp",
        headers: { Authorization: "Bearer t" }
      }
    ]);
  });

  it("submits DeepSeek runs with --provider deepseek and --deepseek-api-key", async () => {
    const cap = makeHostIo({
      argv: [
        "run",
        "--provider",
        "deepseek",
        "--model",
        "deepseek-chat",
        "--prompt",
        "hello",
        "--deepseek-api-key",
        "sk-ds-1",
        "--idempotency-key",
        "idem-ds",
        ...COMMON
      ],
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "r-deepseek", status: "queued" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const body = cap.calls[0]!.body as Record<string, unknown>;
    expect(body.provider).toBe("deepseek");
    expect(body.secrets).toEqual({ apiKeys: { deepseek: "sk-ds-1" } });
  });

  it("threads --webhook into the request body as webhook.url", async () => {
    const cap = makeHostIo({
      argv: [
        "run",
        "--model",
        "claude-haiku-4-5",
        "--prompt",
        "hello",
        "--webhook",
        "https://hooks.example.com/aex",
        "--anthropic-api-key",
        "sk-ant-1",
        "--idempotency-key",
        "idem-webhook",
        ...COMMON
      ],
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "r-webhook", status: "queued" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const body = cap.calls[0]!.body as Record<string, unknown>;
    expect(body.webhook).toEqual({ url: "https://hooks.example.com/aex" });
  });

  it("rejects when --anthropic-api-key is missing", async () => {
    const cap = makeHostIo({
      argv: ["run", "--model", "claude-haiku-4-5", "--prompt", "p", ...COMMON]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--anthropic-api-key");
  });

  it("parses --proxy-auth and validates it against --proxy-endpoint", async () => {
    const endpoint = {
      name: "stripe",
      baseUrl: "https://api.stripe.com",
      authShape: { type: "bearer" },
      allowMethods: ["GET"],
      allowPathPrefixes: ["/v1"]
    };
    const cap = makeHostIo({
      argv: [
        "run",
        "--model",
        "claude-haiku-4-5",
        "--prompt",
        "x",
        "--anthropic-api-key",
        "sk-ant-1",
        "--proxy-endpoint",
        JSON.stringify(endpoint),
        "--proxy-auth",
        "stripe=bearer:sk_test",
        ...COMMON
      ],
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "r1", status: "queued" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const body = cap.calls[0]!.body as Record<string, unknown>;
    expect((body.proxyEndpoints as Array<{ name: string }>)[0]!.name).toBe("stripe");
    const secrets = body.secrets as Record<string, unknown>;
    const auth = secrets.proxyEndpointAuth as Array<{ name: string; value: { type: string; token: string } }>;
    expect(auth[0]!.name).toBe("stripe");
    expect(auth[0]!.value).toEqual({ type: "bearer", token: "sk_test" });
  });

  it("rejects when --proxy-endpoint shape disagrees with --proxy-auth shape", async () => {
    const endpoint = {
      name: "stripe",
      baseUrl: "https://api.stripe.com",
      authShape: { type: "bearer" },
      allowMethods: ["GET"],
      allowPathPrefixes: ["/v1"]
    };
    const cap = makeHostIo({
      argv: [
        "run",
        "--model",
        "claude-haiku-4-5",
        "--prompt",
        "x",
        "--anthropic-api-key",
        "sk-ant-1",
        "--proxy-endpoint",
        JSON.stringify(endpoint),
        "--proxy-auth",
        "stripe=basic:u:p",
        ...COMMON
      ]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("proxy auth validation failed");
  });

  it("rejects --mcp-auth that does not match a declared --mcp", async () => {
    const cap = makeHostIo({
      argv: [
        "run",
        "--model",
        "claude-haiku-4-5",
        "--prompt",
        "x",
        "--mcp",
        "github=https://example.com/mcp",
        "--mcp-auth",
        "gitlab=Authorization:Bearer t",
        "--anthropic-api-key",
        "sk-ant-1",
        ...COMMON
      ]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--mcp-auth gitlab");
  });

  it("merges multiple --mcp-auth flags for the same server (does not collapse)", async () => {
    const cap = makeHostIo({
      argv: [
        "run",
        "--model",
        "claude-haiku-4-5",
        "--prompt",
        "x",
        "--mcp",
        "github=https://example.com/mcp",
        "--mcp-auth",
        "github=Authorization:Bearer t",
        "--mcp-auth",
        "github=X-Trace-Id:abc-123",
        "--anthropic-api-key",
        "sk-ant-1",
        ...COMMON
      ],
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "r-merge", status: "queued" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const body = cap.calls[0]!.body as Record<string, unknown>;
    const secrets = body.secrets as Record<string, unknown>;
    const mcpSecrets = secrets.mcpServers as Array<{ name: string; headers: Record<string, string> }>;
    expect(mcpSecrets).toHaveLength(1);
    expect(mcpSecrets[0]!.name).toBe("github");
    expect(mcpSecrets[0]!.headers).toEqual({
      Authorization: "Bearer t",
      "X-Trace-Id": "abc-123"
    });
  });

  it("rejects duplicate --mcp-auth header names for the same server", async () => {
    const cap = makeHostIo({
      argv: [
        "run",
        "--model",
        "claude-haiku-4-5",
        "--prompt",
        "x",
        "--mcp",
        "github=https://example.com/mcp",
        "--mcp-auth",
        "github=Authorization:Bearer t1",
        "--mcp-auth",
        "github=Authorization:Bearer t2",
        "--anthropic-api-key",
        "sk-ant-1",
        ...COMMON
      ]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("duplicate header");
  });

  it("rejects positional arguments (no run-config positional)", async () => {
    const cap = makeHostIo({
      argv: ["run", "/some/run.json", "--anthropic-api-key", "x", ...COMMON]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("no positional arguments");
  });

  it("treats @@literal as a literal '@literal' on --prompt", async () => {
    const cap = makeHostIo({
      argv: [
        "run",
        "--model",
        "claude-haiku-4-5",
        "--prompt",
        "@@alice please look at this",
        "--anthropic-api-key",
        "sk-ant-1",
        ...COMMON
      ],
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "r-esc", status: "queued" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const body = cap.calls[0]!.body as Record<string, unknown>;
    const submission = body.submission as Record<string, unknown>;
    expect(submission.prompt).toEqual(["@alice please look at this"]);
  });
});

describe("aex skills", () => {
  it("lists skills as NDJSON", async () => {
    const cap = makeHostIo({
      argv: ["skills", "list", ...COMMON],
      fetchHandler: () =>
        new Response(
          JSON.stringify({
            skills: [
              { id: "skl_a", name: "alpha", hash: "h1", state: "ready" },
              { id: "skl_b", name: "beta", hash: "h2", state: "ready" }
            ]
          }),
          { status: 200, headers: { "content-type": "application/json" } }
        )
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/skills");
    expect(cap.calls[0]!.init.method ?? "GET").toBe("GET");
    const lines = cap.stdout.trim().split("\n");
    expect(lines).toHaveLength(2);
    expect(JSON.parse(lines[0]!)).toMatchObject({ id: "skl_a" });
    expect(JSON.parse(lines[1]!)).toMatchObject({ id: "skl_b" });
  });

  it("fetches a single skill by id", async () => {
    const cap = makeHostIo({
      argv: ["skills", "get", "skl_x", ...COMMON],
      fetchHandler: () =>
        new Response(
          JSON.stringify({ skill: { id: "skl_x", name: "x", hash: "h", state: "ready" } }),
          { status: 200, headers: { "content-type": "application/json" } }
        )
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/skills/skl_x");
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ id: "skl_x" });
  });

  it("DELETEs a skill", async () => {
    const cap = makeHostIo({
      argv: ["skills", "delete", "skl_y_long_id", ...COMMON],
      fetchHandler: () => new Response(null, { status: 204 })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/skills/skl_y_long_id");
    expect(cap.calls[0]!.init.method).toBe("DELETE");
    expect(JSON.parse(cap.stdout.trim())).toEqual({ skillId: "skl_y_long_id", deleted: true });
  });

  it("requires --name on upload", async () => {
    const cap = makeHostIo({
      argv: ["skills", "upload", "--file", "SKILL.md", ...COMMON]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--name is required");
  });

  it("rejects upload without --from-path or --file", async () => {
    const cap = makeHostIo({
      argv: ["skills", "upload", "--name", "x", ...COMMON]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--from-path");
  });

  it("rejects upload combining --from-path and --file", async () => {
    const cap = makeHostIo({
      argv: [
        "skills",
        "upload",
        "--name",
        "x",
        "--from-path",
        "./d",
        "--file",
        "SKILL.md",
        ...COMMON
      ]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--from-path");
  });

  it("rejects unknown skills verb", async () => {
    const cap = makeHostIo({ argv: ["skills", "frobnicate", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("unknown skills verb");
  });

  it("rejects unknown flags on skills list", async () => {
    const cap = makeHostIo({ argv: ["skills", "list", "--typo", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("unknown flag: --typo");
  });

  it("upload --file: runs the direct-to-storage flow (presign → object storage PUT → finalize) and prints the skill record", async () => {
    // The CLI catalog upload now goes direct-to-storage: the bytes never transit
    // the hosted API. Assert the three-step wire shape (presign → PUT → finalize)
    // and that the signed checksum header rides the object storage PUT.
    const tmp = makeSkillsTmpDir(
      "---\nname: rules-cli\ndescription: cli upload skill\n---\n# rules-cli\n"
    );
    const cap = makeHostIo({
      argv: [
        "skills",
        "upload",
        "--name",
        "rules-cli",
        "--file",
        join(tmp.dir, "SKILL.md"),
        ...COMMON
      ],
      fetchHandler: (call) => {
        if (call.url.endsWith("/api/skills/presign")) {
          const reqBody = JSON.parse(call.init.body as string) as { name: string; hash: string; sizeBytes: number };
          if (reqBody.name !== "rules-cli" || !/^sha256:[0-9a-f]{64}$/.test(reqBody.hash)) {
            return new Response(JSON.stringify({ error: { message: "bad presign body" } }), { status: 400, headers: { "content-type": "application/json" } });
          }
          return new Response(
            JSON.stringify({
              ok: true,
              skillId: "skl_cli_42",
              uploadUrl: "https://object-storage.example.test/bucket/assets/ws/hash?X-Amz-Signature=sig",
              requiredHeaders: { "x-amz-checksum-sha256": "Y2hlY2tzdW0=" },
              expiresInSeconds: 300
            }),
            { status: 201, headers: { "content-type": "application/json" } }
          );
        }
        if (call.url.includes("object-storage.example.test")) {
          return new Response("", { status: 200 }); // object storage accepts the direct PUT
        }
        if (call.url.endsWith("/api/skills/skl_cli_42/finalize")) {
          return new Response(
            JSON.stringify({
              skill: { id: "skl_cli_42", name: "rules-cli", state: "ready", hash: "sha256:" + "a".repeat(64), fileCount: 1 }
            }),
            { status: 200, headers: { "content-type": "application/json" } }
          );
        }
        return new Response(JSON.stringify({ error: { message: `unexpected url ${call.url}` } }), { status: 400, headers: { "content-type": "application/json" } });
      }
    });
    try {
      await runCli(cap.io);
      expect(cap.stderr).toBe("");
      expect(cap.exitCode).toBe(0);
      const urls = cap.calls.map((c) => c.url);
      expect(urls).toContain("https://dash.example/api/skills/presign");
      expect(urls.some((u) => u.includes("object-storage.example.test"))).toBe(true);
      expect(urls).toContain("https://dash.example/api/skills/skl_cli_42/finalize");
      const storagePut = cap.calls.find((c) => c.url.includes("object-storage.example.test"))!;
      expect(storagePut.init.method).toBe("PUT");
      expect((storagePut.init.headers as Record<string, string>)["x-amz-checksum-sha256"]).toBe("Y2hlY2tzdW0=");
      const printed = JSON.parse(cap.stdout.trim()) as { id: string; name: string };
      expect(printed.id).toBe("skl_cli_42");
      expect(printed.name).toBe("rules-cli");
    } finally {
      tmp.cleanup();
    }
  });

  it("upload: treats presign_unconfigured as terminal and does not POST a bundle to the API", async () => {
    const tmp = makeSkillsTmpDir(
      "---\nname: rules-direct\ndescription: cli upload skill\n---\n# rules-direct\n"
    );
    const cap = makeHostIo({
      argv: ["skills", "upload", "--name", "rules-direct", "--file", join(tmp.dir, "SKILL.md"), ...COMMON],
      fetchHandler: (call) => {
        if (call.url.endsWith("/api/skills/presign")) {
          return new Response(JSON.stringify({ ok: false, code: "presign_unconfigured", message: "object storage S3 creds not configured" }), { status: 503, headers: { "content-type": "application/json" } });
        }
        return new Response(JSON.stringify({ error: { message: `unexpected url ${call.url}` } }), { status: 400, headers: { "content-type": "application/json" } });
      }
    });
    try {
      await runCli(cap.io);
      expect(cap.exitCode).toBe(1);
      const urls = cap.calls.map((c) => c.url);
      expect(urls).toEqual(["https://dash.example/api/skills/presign"]);
      expect(urls.some((u) => u.includes("object-storage.example.test"))).toBe(false);
      const errJson = JSON.parse(cap.stderr.trim()) as { error: string; message: string; status?: number };
      expect(errJson.error).toBe("skill_upload_failed");
      expect(errJson.status).toBe(503);
      expect(errJson.message).toContain("object storage S3 creds not configured");
    } finally {
      tmp.cleanup();
    }
  });

  it("upload: propagates the server's verbose error message (so 'Bucket not found' reaches the user)", async () => {
    // Customer regression coverage (Bug 1, the user-visible half):
    // when object storage returns a 400 with a body like
    // `{"statusCode":"404","error":"Bucket not found"}`, the API
    // wraps it into a 500 carrying that body verbatim. The CLI must
    // forward the verbose detail to the user — otherwise the operator
    // sees only "skill_upload_failed: upload failed" and cannot
    // diagnose the missing-bucket cause.
    const tmp = makeSkillsTmpDir(
      "---\nname: broken\ndescription: broken upload\n---\n# broken\n"
    );
    const cap = makeHostIo({
      argv: [
        "skills",
        "upload",
        "--name",
        "broken",
        "--file",
        join(tmp.dir, "SKILL.md"),
        ...COMMON
      ],
      fetchHandler: () =>
        new Response(
          JSON.stringify({
            error:
              "Object storage upload failed with HTTP 400: " +
              '{"statusCode":"404","error":"Bucket not found","message":"Bucket not found"}'
          }),
          { status: 500, headers: { "content-type": "application/json" } }
        )
    });
    try {
      await runCli(cap.io);
      expect(cap.exitCode).toBe(1);
      // The CLI emits JSON errors on stderr; the verbose message must
      // travel through unmodified.
      const errLine = cap.stderr.trim();
      const errJson = JSON.parse(errLine) as { error: string; message: string };
      expect(errJson.error).toBe("skill_upload_failed");
      expect(errJson.message).toMatch(/Bucket not found/i);
    } finally {
      tmp.cleanup();
    }
  });
});
