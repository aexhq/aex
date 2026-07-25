import { describe, expect, it } from "bun:test";
import { strToU8, unzipSync } from "fflate";
import { createHash } from "node:crypto";
import { resolve as resolvePath } from "node:path";
import { executeCli } from "../src/main.js";
import { parseDuration, takeOptionFlag } from "../src/host/common.js";
import type { CliIO } from "../src/internal.js";
import { canonicalWhoami } from "./canonical-whoami.js";

const CWD = "/tmp/cli-test";
const EMPTY_SHA256 = createHash("sha256").update("").digest("hex");

/** Compute the absolute path the session-config loader will produce for a
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

const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

function undiciFetchFailed(code = "UND_ERR_CONNECT_TIMEOUT"): TypeError {
  return new TypeError("fetch failed", { cause: Object.assign(new Error("Connect Timeout Error"), { code }) });
}

describe("aex whoami", () => {
  it("calls GET /api/whoami without a workspace query and prints the body", async () => {
    const cap = makeHostIo({
      argv: ["whoami", "--api-key", "tok-1", "--aex-url", "https://dash.example/"],
      fetchHandler: () =>
        new Response(JSON.stringify(canonicalWhoami("ws-7", ["sessions.write"])), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/whoami");
    expect(cap.calls[0]!.init.method ?? "GET").toBe("GET");
    const printed = JSON.parse(cap.stdout.trim()) as { principalType: string; workspaceId: string };
    expect(printed.principalType).toBe("api_key");
    expect(printed.workspaceId).toBe("ws-7");
  });

  it("rejects when --api-key is missing", async () => {
    const cap = makeHostIo({ argv: ["whoami", "--aex-url", "https://dash.example/"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--api-key");
  });

  it("defaults --aex-url to https://api.aex.dev when omitted", async () => {
    const cap = makeHostIo({
      argv: ["whoami", "--api-key", "tok-1"],
      fetchHandler: () =>
        new Response(JSON.stringify(canonicalWhoami("ws-9")), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.url).toBe("https://api.aex.dev/api/whoami");
  });
});

describe("aex status", () => {
  it("does not send a workspaceId query parameter (derived from token server-side)", async () => {
    const cap = makeHostIo({
      argv: ["status", "session-42", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ session: { id: "session-42", status: "idle", acceptsMessages: true } }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.url).toContain("/api/sessions/session-42");
    expect(cap.calls[0]!.url).not.toContain("workspaceId=");
    expect(JSON.parse(cap.stdout)).toEqual({ id: "session-42", status: "idle", acceptsMessages: true });
  });

  it("does not accept a --workspace flag (workspace is derived from the token)", async () => {
    const cap = makeHostIo({
      argv: ["status", "session-1", "--workspace", "ws-1", "--api-key", "tok", "--aex-url", "https://x"]
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("unknown");
  });

  it("rejects unknown flags before making a network call", async () => {
    const cap = makeHostIo({
      argv: ["status", "session-1", "--typo-flag", ...COMMON],
      fetchHandler: () => {
        throw new Error("status should not fetch after an unknown flag");
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.calls).toHaveLength(0);
    expect(cap.stderr).toContain("unknown flag: --typo-flag");
    expect(cap.stderr).toContain("usage: aex status");
  });
});

describe("aex deliveries", () => {
  it("GETs the webhook-deliveries endpoint and prints the array as JSON", async () => {
    const rows = [
      {
        id: "wd-1",
        runId: "run-1",
        turnSeq: 1,
        eventType: "run.finished",
        status: "delivered",
        attemptCount: 1,
        lastStatusCode: 200,
        createdAt: "2026-06-21T00:00:00.000Z"
      }
    ];
    const cap = makeHostIo({
      argv: ["deliveries", "session-42", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ deliveries: rows }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/sessions/session-42/webhook-deliveries");
    expect(cap.calls[0]!.init.method ?? "GET").toBe("GET");
    expect(JSON.parse(cap.stdout)).toEqual(rows);
  });

  it("requires exactly one session-id positional", async () => {
    const cap = makeHostIo({ argv: ["deliveries", ...COMMON] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("usage: aex deliveries");
  });
});

describe("aex events", () => {
  it("lists events as NDJSON", async () => {
    const cap = makeHostIo({
      argv: ["events", "session-9", ...COMMON],
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
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const lines = cap.stdout.trim().split("\n");
    expect(lines).toHaveLength(2);
    expect(JSON.parse(lines[0]!)).toEqual({ id: "e1", type: "agent.message" });
    expect(JSON.parse(lines[1]!)).toEqual({ id: "e2", type: "session.status_running" });
  });

  it("--follow polls /events and emits NDJSON until terminal (never opens an SSE stream)", async () => {
    const cap = makeHostIo({
      argv: ["events", "session-poll", "--follow", ...COMMON],
      fetchHandler: (call) => {
        if (call.url.endsWith("/events/stream")) {
          throw new Error("SSE endpoint must not be touched");
        }
        if (call.url.endsWith("/events")) {
          return new Response(
            JSON.stringify({
              events: [
                { id: "p1", type: "TEXT_MESSAGE_CONTENT" },
                { id: "p2", type: "RUN_FINISHED", data: { outcome: "succeeded" } }
              ]
            }),
            { status: 200, headers: { "content-type": "application/json" } }
          );
        }
        throw new Error(`unexpected URL ${call.url}`);
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout.trim()).toContain("p1");
    expect(cap.calls.some((c) => c.url.endsWith("/events/stream"))).toBe(false);
  });

  it("--follow stops when RUN_FINISHED carries outcome=timed_out", async () => {
    const cap = makeHostIo({
      argv: ["events", "session-timeout", "--follow", ...COMMON],
      fetchHandler: (call) => {
        if (call.url.endsWith("/events")) {
          return new Response(
            JSON.stringify({
              events: [
                { id: "t1", type: "TEXT_MESSAGE_CONTENT" },
                { id: "t2", type: "RUN_FINISHED", data: { outcome: "timed_out" } }
              ]
            }),
            { status: 200, headers: { "content-type": "application/json" } }
          );
        }
        throw new Error(`unexpected URL ${call.url}`);
      }
    });
    await executeCli(cap.io);
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

describe("takeOptionFlag", () => {
  it("distinguishes an absent option from a present option missing its value", () => {
    expect(takeOptionFlag(["session-1"], "--timeout")).toEqual({
      value: undefined,
      remaining: ["session-1"],
      error: null
    });
    expect(takeOptionFlag(["session-1", "--timeout"], "--timeout")).toEqual({
      value: undefined,
      remaining: ["session-1"],
      error: "--timeout requires a value"
    });
  });
});

describe("aex wait", () => {
  it("polls GET /api/sessions/{id} until non-progressing, prints the final session, and exits 0 on idle", async () => {
    let polls = 0;
    const cap = makeHostIo({
      argv: ["wait", "session-w", "--interval", "1ms", ...COMMON],
      fetchHandler: () => {
        polls++;
        const status = polls < 3 ? "running" : "idle";
        return new Response(JSON.stringify({ session: { id: "session-w", status, acceptsMessages: status === "idle" } }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(polls).toBe(3);
    expect(cap.calls[0]!.url).toContain("/api/sessions/session-w");
    const printed = JSON.parse(cap.stdout.trim()) as { id: string; status: string };
    expect(printed).toMatchObject({ id: "session-w", status: "idle" });
  });

  it("exits 1 (RUNTIME_ERR) when the session stops progressing with error", async () => {
    const cap = makeHostIo({
      argv: ["wait", "session-f", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ session: { id: "session-f", status: "error", acceptsMessages: true } }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(1);
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ status: "error" });
  });

  it("retains status and remedy when a session poll fails", async () => {
    const cap = makeHostIo({
      argv: ["wait", "session-denied", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({
          error: "insufficient_scope",
          message: "the token does not carry the required scope",
          requiredScope: "sessions:read"
        }), {
          status: 403,
          headers: { "content-type": "application/json" }
        })
    });

    await executeCli(cap.io);

    expect(cap.exitCode).toBe(1);
    expect(cap.calls).toHaveLength(1);
    expect(cap.stdout).toBe("");
    const error = JSON.parse(cap.stderr) as Record<string, unknown>;
    expect(error).toMatchObject({
      error: "wait_failed",
      sessionId: "session-denied",
      status: 403,
      remedy: "token lacks permission for this workspace/action"
    });
    expect(error.message).toContain('"requiredScope":"sessions:read"');
  });

  it("exits 3 (TIMEOUT_ERR) with a JSON error when --timeout elapses before parked", async () => {
    const cap = makeHostIo({
      argv: ["wait", "session-slow", "--timeout", "0ms", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ session: { id: "session-slow", status: "running", acceptsMessages: false } }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(3);
    const err = JSON.parse(cap.stderr.trim()) as { error: string; sessionId: string; lastStatus: string };
    expect(err.error).toBe("wait_timeout");
    expect(err.sessionId).toBe("session-slow");
    expect(err.lastStatus).toBe("running");
  });

  it("rejects a malformed --timeout with USAGE_ERR", async () => {
    const cap = makeHostIo({ argv: ["wait", "session-x", "--timeout", "soon", ...COMMON] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--timeout");
  });

  it("rejects a trailing --timeout instead of silently disabling the deadline", async () => {
    const cap = makeHostIo({ argv: ["wait", "session-x", ...COMMON, "--timeout"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--timeout requires a value");
    expect(cap.calls).toHaveLength(0);
  });

  it("requires exactly one session-id positional", async () => {
    const cap = makeHostIo({ argv: ["wait", ...COMMON] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("usage: aex wait");
  });
});

describe("aex events --follow --timeout", () => {
  it("exits 3 with a JSON error when the follow deadline elapses before terminal", async () => {
    const cap = makeHostIo({
      argv: ["events", "session-ev", "--follow", "--timeout", "0ms", ...COMMON],
      fetchHandler: (call) => {
        if (call.url.endsWith("/events")) {
          return new Response(JSON.stringify({ events: [] }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        return new Response(JSON.stringify({ session: { id: "session-ev", status: "running", acceptsMessages: false } }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(3);
    expect(JSON.parse(cap.stderr.trim())).toMatchObject({ error: "events_follow_timeout", sessionId: "session-ev" });
  });
});

describe("aex files", () => {
  it("lists files as NDJSON", async () => {
    const cap = makeHostIo({
      argv: ["files", "session-9", ...COMMON],
      fetchHandler: () =>
        new Response(
          JSON.stringify({
            revision: {
              checkpointId: "cp-1",
              runId: "run-1",
              turnSeq: 1,
              committedAt: "2026-07-01T00:00:00Z",
              throughSeq: 10
            },
            files: [{
              id: "o1",
              checkpointId: "cp-1",
              filename: "report.md",
              sizeBytes: 0,
              sha256: EMPTY_SHA256
            }]
          }),
          { status: 200, headers: { "content-type": "application/json" } }
        )
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ id: "o1", filename: "report.md" });
  });
});

describe("aex cancel + delete", () => {
  it("cancel POSTs and prints the result", async () => {
    const cap = makeHostIo({
      argv: ["cancel", "session-x", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ session: { id: "session-x", status: "cancelling", acceptsMessages: false } }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.init.method).toBe("POST");
    expect(cap.calls[0]!.url).toContain("/api/sessions/session-x/cancel");
    expect(JSON.parse(cap.stdout)).toEqual({ sessionId: "session-x", status: "cancelling" });
  });

  it("delete DELETEs and prints the result", async () => {
    const cap = makeHostIo({
      argv: ["delete", "session-x", ...COMMON],
      // The delete route answers with both counters on every call; a stub that
      // omits them models a response the server does not send.
      fetchHandler: () => new Response(JSON.stringify({
        session: { id: "session-x", status: "deleted", acceptsMessages: false },
        purgedSessionFileObjects: 0,
        cleanupComplete: true
      }), { status: 200, headers: { "content-type": "application/json" } })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.init.method).toBe("DELETE");
    expect(cap.calls[0]!.url).toContain("/api/sessions/session-x");
    expect(JSON.parse(cap.stdout)).toEqual({ sessionId: "session-x", deleted: true });
  });

  it("delete-asset DELETEs a normalized workspace asset id and prints the result", async () => {
    const hex = "a".repeat(64);
    const cap = makeHostIo({
      argv: ["delete-asset", `sha256:${hex}`, ...COMMON],
      fetchHandler: () => new Response(null, { status: 204 })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe(`https://dash.example/api/assets/asset_${hex}`);
    expect(cap.calls[0]!.init.method).toBe("DELETE");
    expect(JSON.parse(cap.stdout.trim())).toEqual({ hash: `sha256:${hex}`, deleted: true });
  });

  it("delete-asset rejects missing hashes without calling the API", async () => {
    const cap = makeHostIo({
      argv: ["delete-asset", ...COMMON]
    });
    await executeCli(cap.io);
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
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(1);
    expect(cap.calls[0]!.url).toBe(`https://dash.example/api/assets/asset_${hex}`);
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
      message: "asset_not_found: asset not found",
      hash: hex,
      status: 404,
      remedy: "no such run/resource — verify the id"
    });
  });
});

describe("aex download", () => {
  // Route the reads the download verbs fan out to: getSession + listEvents +
  // listSessionFiles + per-file /download.
  const json = (body: unknown) =>
    new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  const wholeSessionHandler =
    (sessionId: string) =>
    ({ url }: { url: string }): Response => {
      const pathname = new URL(url).pathname;
      if (pathname.endsWith(`/api/sessions/${sessionId}/events`)) return json({ events: [{ seq: 0, kind: "runtime_start" }] });
      if (pathname.endsWith(`/api/sessions/${sessionId}/files`)) {
        return json({
          revision: {
            checkpointId: "cp-1",
            runId: "run-1",
            turnSeq: 1,
            committedAt: "2026-07-01T00:00:00Z",
            throughSeq: 10
          },
          files: [{
            id: "o1",
            checkpointId: "cp-1",
            filename: "report.txt",
            sizeBytes: 5,
            sha256: createHash("sha256").update("hello").digest("hex"),
            contentType: "text/plain"
          }]
        });
      }
      if (pathname.endsWith(`/api/sessions/${sessionId}/files/o1/download`)) {
        return new Response(strToU8("hello").buffer, { status: 200, headers: { "content-type": "text/plain" } });
      }
      return json({ session: { id: sessionId, status: "idle", acceptsMessages: true } });
    };

  it("assembles the public whole-run zip client-side and writes it to --out", async () => {
    const writes = new Map<string, Uint8Array>();
    const cap = makeHostIo({
      argv: ["download", "session-1", "--out", "session-1.zip", ...COMMON],
      writes,
      fetchHandler: wholeSessionHandler("session-1")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    // getSession + events + files + one per-file download.
    expect(cap.calls).toHaveLength(4);

    const writtenKey = [...writes.keys()][0]!;
    expect(writtenKey).toMatch(/session-1\.zip$/);
    const entries = unzipSync(writes.get(writtenKey)!);
    expect(Object.keys(entries).sort()).toEqual([
      "events/events.jsonl",
      "files/report.txt",
      "manifest.json",
      "metadata/session.json"
    ]);
    expect(new TextDecoder().decode(entries["files/report.txt"]!)).toBe("hello");
    expect(JSON.parse(new TextDecoder().decode(entries["metadata/session.json"]!)).id).toBe("session-1");

    const printed = JSON.parse(cap.stdout.trim()) as { sessionId: string; namespace: string; bytes: number };
    expect(printed.sessionId).toBe("session-1");
    expect(printed.namespace).toBe("all");
    expect(printed.bytes).toBe(writes.get(writtenKey)!.byteLength);
  });

  it("defaults the output path to aex-session-<session-id>.zip when --out is omitted", async () => {
    const writes = new Map<string, Uint8Array>();
    const cap = makeHostIo({
      argv: ["download", "session-2", ...COMMON],
      writes,
      fetchHandler: wholeSessionHandler("session-2")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const writtenKey = [...writes.keys()][0]!;
    expect(writtenKey).toMatch(/aex-session-session-2\.zip$/);
  });

  it("--only files zips just the deliverables (no logs, no metadata/events)", async () => {
    const writes = new Map<string, Uint8Array>();
    const cap = makeHostIo({
      argv: ["download", "session-1", "--only", "files", ...COMMON],
      writes,
      fetchHandler: wholeSessionHandler("session-1")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const writtenKey = [...writes.keys()][0]!;
    expect(writtenKey).toMatch(/aex-session-session-1-files\.zip$/);
    const entries = unzipSync(writes.get(writtenKey)!);
    expect(Object.keys(entries).sort()).toEqual(["manifest.json", "report.txt"]);
    expect(new TextDecoder().decode(entries["report.txt"]!)).toBe("hello");
    expect((JSON.parse(cap.stdout.trim()) as { namespace: string }).namespace).toBe("files");
  });

  it("rejects --only logs with a usage error", async () => {
    const cap = makeHostIo({
      argv: ["download", "session-1", "--only", "logs", ...COMMON],
      fetchHandler: wholeSessionHandler("session-1")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--only must be one of");
    expect(cap.calls).toHaveLength(0);
  });

  it("--only metadata reads the session record and manifest", async () => {
    const writes = new Map<string, Uint8Array>();
    const cap = makeHostIo({
      argv: ["download", "session-1", "--only", "metadata", ...COMMON],
      writes,
      fetchHandler: wholeSessionHandler("session-1")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/sessions/session-1");
    const entries = unzipSync(writes.get([...writes.keys()][0]!)!);
    expect(Object.keys(entries).sort()).toEqual(["manifest.json", "session.json"]);
    expect(JSON.parse(new TextDecoder().decode(entries["manifest.json"]!))).toMatchObject({
      sessionId: "session-1",
      namespace: "metadata",
      files: [{ path: "session.json", role: "session_metadata", status: "present" }],
      errors: []
    });
  });

  it("retries transient transport failures for idempotent event download reads", async () => {
    const writes = new Map<string, Uint8Array>();
    let eventReads = 0;
    const cap = makeHostIo({
      argv: ["download", "session-retry", "--only", "events", "--out", "retry.zip", ...COMMON],
      writes,
      fetchHandler: (call) => {
        if (call.url.endsWith("/api/sessions/session-retry/events")) {
          eventReads += 1;
          if (eventReads === 1) throw undiciFetchFailed();
          return json({ events: [{ id: "e1", type: "RUN_STARTED" }] });
        }
        throw new Error(`unexpected URL ${call.url}`);
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(eventReads).toBe(2);
    const entries = unzipSync(writes.get([...writes.keys()][0]!)!);
    expect(Object.keys(entries).sort()).toEqual(["events.jsonl", "manifest.json"]);
  });

  it("rejects an unknown --only namespace with a usage error", async () => {
    const cap = makeHostIo({
      argv: ["download", "session-1", "--only", "bogus", ...COMMON],
      fetchHandler: wholeSessionHandler("session-1")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--only must be one of");
    expect(cap.calls).toHaveLength(0);
  });

  it("rejects Object.prototype names as --only namespaces", async () => {
    const cap = makeHostIo({
      argv: ["download", "session-1", "--only", "toString", ...COMMON],
      fetchHandler: wholeSessionHandler("session-1")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--only must be one of");
    expect(cap.calls).toHaveLength(0);
  });
});

// `aex start` uses the shared submit transport: create a session, then post the
// first turn to the session messages endpoint. The CLI prints the accepted
// session record from the message response.
function sessionStartHandler(sessionId: string, status = "running"): (call: FetchCall) => Response {
  const json = (body: unknown, statusCode: number): Response =>
    new Response(JSON.stringify(body), { status: statusCode, headers: { "content-type": "application/json" } });
  return (call) => {
    if (call.url.endsWith("/api/sessions")) {
      return json({ session: { id: sessionId, status: "idle", acceptsMessages: true } }, 201);
    }
    if (call.url.endsWith(`/api/sessions/${sessionId}/messages`)) {
      return json({
        session: { id: sessionId, status, acceptsMessages: status !== "running" },
        run: { sessionId, runId: "run-1", turnSeq: 1, phase: "running" },
        eventCursor: 1
      }, 202);
    }
    return json({}, 200);
  };
}

describe("aex start", () => {
  it("opens a session from --config, sends the prompt as the first turn, prints the session record", async () => {
    const runConfig = {
      model: "anthropic/claude-haiku-4-5",
      system: "be helpful",
      prompt: ["hi"],
      mcpServers: [
        {
          name: "github",
          url: "https://example.com/mcp",
          headers: { Authorization: "Bearer t-from-config" }
        }
      ]
    };
    const cap = makeHostIo({
      argv: [
        "start",
        "--config",
        "/abs/session.json",
        "--idempotency-key",
        "idem-deterministic",
        ...COMMON
      ],
      files: { [resolvedFromCwd("/abs/session.json")]: JSON.stringify(runConfig) },
      fetchHandler: sessionStartHandler("sess-1")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(2);
    const create = cap.calls[0]!;
    const message = cap.calls[1]!;
    expect(create.url).toBe("https://dash.example/api/sessions");
    expect(create.init.method).toBe("POST");
    expect(message.url).toBe("https://dash.example/api/sessions/sess-1/messages");
    expect(message.init.method).toBe("POST");
    // idempotency is header-carried on create (not in the body).
    expect((create.init.headers as Record<string, string>)["Idempotency-Key"]).toBe("idem-deterministic");
    expect((message.init.headers as Record<string, string>)["Idempotency-Key"]).toBe("idem-deterministic:message");
    const createBody = create.body as Record<string, unknown>;
    expect(createBody.workspaceId).toBeUndefined();
    expect("idempotencyKey" in createBody).toBe(false);
    expect("input" in createBody).toBe(false);
    expect("postHook" in createBody).toBe(false);
    expect(createBody.retention).toEqual({ idleTtl: "3m" });
    const submission = createBody.submission as Record<string, unknown>;
    expect(submission.model).toBe("anthropic/claude-haiku-4-5");
    // the prompt is NOT part of the create submission — it rides the create `input`.
    expect("prompt" in submission).toBe(false);
    expect(submission.assets).toEqual({ files: [], skills: [], tools: [], instructions: [] });
    expect(submission.builtinTools).toBe("default");
    expect(submission.mcpServers).toEqual([
      { name: "github", url: "https://example.com/mcp" }
    ]);
    const secrets = createBody.secrets as Record<string, unknown>;
    // Managed keys: no provider apiKeys ride the wire.
    expect(secrets).not.toHaveProperty("apiKeys");
    expect(secrets.mcpServers).toEqual([
      {
        name: "github",
        url: "https://example.com/mcp",
        headers: { Authorization: "Bearer t-from-config" }
      }
    ]);
    // the prompt rides the first-turn message call, not the create body.
    expect(message.body).toEqual({ input: ["hi"] });
    const printed = JSON.parse(cap.stdout.trim()) as { id: string; status: string };
    expect(printed).toMatchObject({ id: "sess-1", status: "running" });
  });

  it("opens a session from --model/--prompt/--mcp/--mcp-auth flags", async () => {
    const cap = makeHostIo({
      argv: [
        "start",
        "--model",
        "anthropic/claude-haiku-4-5",
        "--prompt",
        "hello",
        "--mcp",
        "github=https://example.com/mcp",
        "--mcp-auth",
        "github=Authorization:Bearer t",
        "--idempotency-key",
        "idem-flat",
        ...COMMON
      ],
      fetchHandler: sessionStartHandler("sess-flat")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const body = cap.calls[0]!.body as Record<string, unknown>;
    const submission = body.submission as Record<string, unknown>;
    expect(submission.assets).toEqual({ files: [], skills: [], tools: [], instructions: [] });
    expect(submission.builtinTools).toBe("default");
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
    // the prompt rides the first-turn message call, not the create body.
    expect("input" in body).toBe(false);
    expect(cap.calls[1]!.body).toEqual({ input: ["hello"] });
  });

  it("retries a transient create-session transport failure with the same idempotency key", async () => {
    let creates = 0;
    const cap = makeHostIo({
      argv: [
        "start",
        "--model",
        "anthropic/claude-haiku-4-5",
        "--prompt",
        "hello",
        "--idempotency-key",
        "idem-retry",
        ...COMMON
      ],
      fetchHandler: (call) => {
        if (call.url.endsWith("/api/sessions")) {
          creates += 1;
          if (creates === 1) throw undiciFetchFailed();
          return new Response(JSON.stringify({ session: { id: "sess-retry", status: "idle", acceptsMessages: true } }), {
            status: 201,
            headers: { "content-type": "application/json" }
          });
        }
        if (call.url.endsWith("/api/sessions/sess-retry/messages")) {
          return new Response(
            JSON.stringify({
              session: { id: "sess-retry", status: "running", acceptsMessages: false },
              run: { sessionId: "sess-retry", runId: "run-1", turnSeq: 1, phase: "running" },
              eventCursor: 1
            }),
            { status: 202, headers: { "content-type": "application/json" } }
          );
        }
        throw new Error(`unexpected URL ${call.url}`);
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(creates).toBe(2);
    expect(cap.calls).toHaveLength(3);
    const firstCreate = cap.calls[0]!;
    const secondCreate = cap.calls[1]!;
    expect(firstCreate.url).toBe("https://dash.example/api/sessions");
    expect(secondCreate.url).toBe("https://dash.example/api/sessions");
    expect((firstCreate.init.headers as Record<string, string>)["Idempotency-Key"]).toBe("idem-retry");
    expect((secondCreate.init.headers as Record<string, string>)["Idempotency-Key"]).toBe("idem-retry");
    expect(secondCreate.body).toEqual(firstCreate.body);
    expect(cap.calls[2]!.url).toBe("https://dash.example/api/sessions/sess-retry/messages");
  });

  it("opens a DeepSeek session from its gateway slug with no provider selector or key", async () => {
    const cap = makeHostIo({
      argv: [
        "start",
        "--model",
        "deepseek/deepseek-v4-flash",
        "--prompt",
        "hello",
        "--idempotency-key",
        "idem-ds",
        ...COMMON
      ],
      fetchHandler: sessionStartHandler("sess-ds")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const body = cap.calls[0]!.body as Record<string, unknown>;
    expect(body).not.toHaveProperty("provider");
    expect(body.secrets).toEqual({});
    expect((body.submission as Record<string, unknown>).model).toBe("deepseek/deepseek-v4-flash");
  });

  it("threads --webhook into the request body as webhook.url", async () => {
    const cap = makeHostIo({
      argv: [
        "start",
        "--model",
        "anthropic/claude-haiku-4-5",
        "--prompt",
        "hello",
        "--webhook",
        "https://hooks.example.com/aex",
        "--idempotency-key",
        "idem-webhook",
        ...COMMON
      ],
      fetchHandler: sessionStartHandler("sess-webhook")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const body = cap.calls[0]!.body as Record<string, unknown>;
    expect(body.webhook).toEqual({ url: "https://hooks.example.com/aex" });
  });

  it("opens a session from a model slug alone (managed keys — no provider key)", async () => {
    const cap = makeHostIo({
      argv: ["start", "--model", "anthropic/claude-haiku-4-5", "--prompt", "p", ...COMMON],
      fetchHandler: sessionStartHandler("sess-managed")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const body = cap.calls[0]!.body as Record<string, unknown>;
    expect(body.secrets).toEqual({});
  });

  it("rejects removed proxy endpoint flags", async () => {
    const endpoint = {
      name: "stripe",
      baseUrl: "https://api.stripe.com",
      authShape: { type: "bearer" },
      allowMethods: ["GET"],
      allowPathPrefixes: ["/v1"]
    };
    const cap = makeHostIo({
      argv: [
        "start",
        "--model",
        "anthropic/claude-haiku-4-5",
        "--prompt",
        "x",
        "--proxy-endpoint",
        JSON.stringify(endpoint),
        "--proxy-auth",
        "stripe=bearer:sk_test",
        ...COMMON
      ],
      fetchHandler: sessionStartHandler("sess-proxy")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("aex start --proxy-endpoint: is no longer supported");
    expect(cap.calls).toHaveLength(0);
  });

  it("rejects removed proxy auth flags even without a proxy endpoint", async () => {
    const cap = makeHostIo({
      argv: [
        "start",
        "--model",
        "anthropic/claude-haiku-4-5",
        "--prompt",
        "x",
        "--proxy-auth",
        "stripe=basic:u:p",
        ...COMMON
      ]
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("aex start --proxy-auth: is no longer supported");
  });

  it("rejects --mcp-auth that does not match a declared --mcp", async () => {
    const cap = makeHostIo({
      argv: [
        "start",
        "--model",
        "anthropic/claude-haiku-4-5",
        "--prompt",
        "x",
        "--mcp",
        "github=https://example.com/mcp",
        "--mcp-auth",
        "gitlab=Authorization:Bearer t",
        ...COMMON
      ]
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("aex start --mcp-auth: gitlab");
  });

  it("merges multiple --mcp-auth flags for the same server (does not collapse)", async () => {
    const cap = makeHostIo({
      argv: [
        "start",
        "--model",
        "anthropic/claude-haiku-4-5",
        "--prompt",
        "x",
        "--mcp",
        "github=https://example.com/mcp",
        "--mcp-auth",
        "github=Authorization:Bearer t",
        "--mcp-auth",
        "github=X-Trace-Id:abc-123",
        ...COMMON
      ],
      fetchHandler: sessionStartHandler("sess-merge")
    });
    await executeCli(cap.io);
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
        "start",
        "--model",
        "anthropic/claude-haiku-4-5",
        "--prompt",
        "x",
        "--mcp",
        "github=https://example.com/mcp",
        "--mcp-auth",
        "github=Authorization:Bearer t1",
        "--mcp-auth",
        "github=Authorization:Bearer t2",
        ...COMMON
      ]
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("duplicate header");
  });

  it("rejects positional arguments (no session-config positional)", async () => {
    const cap = makeHostIo({
      argv: ["start", "/some/session.json", ...COMMON]
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("no positional arguments");
  });

  it("treats @@literal as a literal '@literal' on --prompt", async () => {
    const cap = makeHostIo({
      argv: [
        "start",
        "--model",
        "anthropic/claude-haiku-4-5",
        "--prompt",
        "@@alice please look at this",
        ...COMMON
      ],
      fetchHandler: sessionStartHandler("sess-esc")
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    // the escaped literal rides the first-turn message call's `input`.
    const body = cap.calls[0]!.body as Record<string, unknown>;
    expect("input" in body).toBe(false);
    expect(cap.calls[1]!.body).toEqual({ input: ["@alice please look at this"] });
  });
});
