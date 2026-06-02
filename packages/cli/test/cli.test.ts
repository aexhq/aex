import { describe, expect, it, vi } from "vitest";
import { PROXY_PROTOCOL_VERSION } from "@antpath/contracts";
import { runCli } from "../src/run.js";
import { ANTPATH_INDEX_PATH, ANTPATH_RUN_TOKEN_PATH, type CliIO } from "../src/internal.js";

interface IoCapture {
  io: CliIO;
  stdout: string;
  stderr: string;
  exitCode: number | null;
  fetchCalls: Array<{ url: string; init: RequestInit | undefined }>;
}

function makeIo(opts: {
  argv: readonly string[];
  files?: Record<string, string>;
  fetchHandler?: (url: string, init?: RequestInit) => Promise<Response>;
} = { argv: [] }): IoCapture {
  const files: Record<string, string> = opts.files ?? {};
  const cap: IoCapture = {
    stdout: "",
    stderr: "",
    exitCode: null,
    fetchCalls: [],
    io: undefined as unknown as CliIO
  };
  const io: CliIO = {
    argv: ["node", "/antpath/antpath", ...opts.argv],
    readFile: async (path) => {
      if (!(path in files)) throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
      return files[path]!;
    },
    writeFile: async () => {
      throw new Error("writeFile not configured for this test");
    },
    cwd: () => "/tmp/test-cwd",
    fetchImpl: async (url, init) => {
      cap.fetchCalls.push({ url: String(url), init });
      if (!opts.fetchHandler) {
        return new Response("{}", { status: 200, headers: { "content-type": "application/json" } });
      }
      return opts.fetchHandler(String(url), init);
    },
    stdout: (chunk) => {
      cap.stdout += chunk;
    },
    stderr: (chunk) => {
      cap.stderr += chunk;
    },
    exit: (code) => {
      cap.exitCode = code;
    }
  };
  cap.io = io;
  return cap;
}

function manifestJson(opts: {
  endpoints?: Array<{ name: string; allowMethods?: string[]; allowPathPrefixes?: string[] }>;
  proxyBaseUrl?: string | null;
}): string {
  return JSON.stringify({
    protocolVersion: PROXY_PROTOCOL_VERSION,
    runId: "run-1",
    proxyBaseUrl: opts.proxyBaseUrl === undefined ? "https://dash.example.com/api/runs/run-1/proxy" : opts.proxyBaseUrl,
    endpoints: (opts.endpoints ?? []).map((e) => ({
      name: e.name,
      baseUrl: "https://upstream.example.com",
      authShape: { type: "bearer" },
      allowMethods: e.allowMethods ?? ["GET"],
      allowPathPrefixes: e.allowPathPrefixes ?? ["/"],
      allowHeaders: [],
      responseMode: "headers_only",
      maxRequestBytes: 65536,
      maxResponseBytes: 65536,
      timeoutMs: 10000,
      perCallBudget: 60,
      responseByteBudget: 1048576
    }))
  });
}

describe("antpath --help", () => {
  it("prints host-mode usage and exits 0 without a manifest", async () => {
    const cap = makeIo({ argv: ["--help"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("antpath run");
    expect(cap.stdout).toContain("antpath whoami");
    expect(cap.stdout).toContain("Usage:");
  });

  it("does not advertise --workspace anywhere in host help", async () => {
    // The workspace is derived 1:1 from the API token; --workspace is rejected
    // at parse time. The help text must not contradict that contract.
    const cap = makeIo({ argv: ["--help"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).not.toContain("--workspace");
  });

  it("advertises --antpath-url as optional with the api.antpath.ai default", async () => {
    const cap = makeIo({ argv: ["--help"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("https://api.antpath.ai");
  });

  it("lists declared endpoints when manifest is present", async () => {
    const cap = makeIo({
      argv: [],
      files: {
        [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe" }, { name: "internal" }] })
      }
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("stripe");
    expect(cap.stdout).toContain("internal");
  });

  it("notes when no proxy endpoints were declared", async () => {
    const cap = makeIo({
      argv: ["--help"],
      files: { [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [], proxyBaseUrl: null }) }
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("no proxy endpoints");
  });
});

describe("antpath proxy — argument validation", () => {
  it("exits 2 on unknown subcommand", async () => {
    const cap = makeIo({ argv: ["snorlax"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("unknown subcommand");
  });

  it("exits 2 with missing endpoint-name", async () => {
    const cap = makeIo({ argv: ["proxy"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("missing endpoint-name");
  });

  it("exits 2 on unknown flag", async () => {
    const cap = makeIo({ argv: ["proxy", "--bogus", "x", "stripe"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toMatch(/unknown flag/);
  });

  it("exits 2 on invalid --response-mode", async () => {
    const cap = makeIo({ argv: ["proxy", "stripe", "--response-mode", "everything"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--response-mode");
  });

  it("rejects --header without =", async () => {
    const cap = makeIo({ argv: ["proxy", "stripe", "--header", "boom"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("KEY=VALUE");
  });
});

describe("antpath proxy — IO contract", () => {
  it("fails when the manifest is missing", async () => {
    const cap = makeIo({ argv: ["proxy", "stripe"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(1);
    expect(cap.stderr).toContain("manifest not mounted");
  });

  it("fails when the run has no proxyBaseUrl declared", async () => {
    const cap = makeIo({
      argv: ["proxy", "stripe"],
      files: { [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [], proxyBaseUrl: null }) }
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(1);
    const body = JSON.parse(cap.stderr.trim());
    expect(body.error).toBe("endpoint_not_found");
  });

  it("fails when the run-token file is missing", async () => {
    const cap = makeIo({
      argv: ["proxy", "stripe"],
      files: { [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe" }] }) }
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(1);
    const body = JSON.parse(cap.stderr.trim());
    expect(body.error).toBe("unauthorized");
  });
});

describe("antpath proxy — successful call", () => {
  it("sends the protocol headers and writes the response envelope to stdout", async () => {
    const upstreamBody = {
      endpointName: "stripe",
      upstreamStatus: 200,
      upstreamHeaders: { "content-type": "application/json" },
      effectiveResponseMode: "headers_only",
      modeClamped: false,
      remainingCalls: 59,
      remainingResponseBytes: 1000000
    };
    const cap = makeIo({
      argv: ["proxy", "stripe", "--method", "POST", "--path", "/v1/refunds"],
      files: {
        [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe", allowMethods: ["POST"] }] }),
        [ANTPATH_RUN_TOKEN_PATH]: "bearer-xyz"
      },
      fetchHandler: async () =>
        new Response(JSON.stringify(upstreamBody), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.fetchCalls).toHaveLength(1);
    const call = cap.fetchCalls[0]!;
    expect(call.url).toBe("https://dash.example.com/api/runs/run-1/proxy/stripe");
    const headers = new Headers((call.init as RequestInit).headers);
    expect(headers.get("authorization")).toBe("Bearer bearer-xyz");
    expect(headers.get("x-antpath-proxy-protocol")).toBe(PROXY_PROTOCOL_VERSION);
    expect(headers.get("x-antpath-method")).toBe("POST");
    expect(headers.get("x-antpath-path")).toBe("/v1/refunds");
    const body = JSON.parse(cap.stdout.trim());
    expect(body.upstreamStatus).toBe(200);
  });

  it("forwards --header K=V via the X-Antpath-Headers JSON record", async () => {
    let captured: Headers | null = null;
    const cap = makeIo({
      argv: ["proxy", "stripe", "--header", "accept=application/json"],
      files: {
        [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe" }] }),
        [ANTPATH_RUN_TOKEN_PATH]: "tok"
      },
      fetchHandler: async (_url, init) => {
        captured = new Headers((init as RequestInit).headers);
        return new Response("{}", { status: 200, headers: { "content-type": "application/json" } });
      }
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const json = JSON.parse(captured!.get("x-antpath-headers")!);
    expect(json.accept).toBe("application/json");
  });

  it("propagates --query as the X-Antpath-Query header", async () => {
    let captured: Headers | null = null;
    const cap = makeIo({
      argv: ["proxy", "stripe", "--query", '{"limit":"10"}'],
      files: {
        [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe" }] }),
        [ANTPATH_RUN_TOKEN_PATH]: "tok"
      },
      fetchHandler: async (_url, init) => {
        captured = new Headers((init as RequestInit).headers);
        return new Response("{}", { status: 200 });
      }
    });
    await runCli(cap.io);
    expect(captured!.get("x-antpath-query")).toBe('{"limit":"10"}');
  });

  it("forwards --data inline as the request body", async () => {
    let bodyCaptured: string | null = null;
    const cap = makeIo({
      argv: ["proxy", "stripe", "--method", "POST", "--data", "hello"],
      files: {
        [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe", allowMethods: ["POST"] }] }),
        [ANTPATH_RUN_TOKEN_PATH]: "tok"
      },
      fetchHandler: async (_url, init) => {
        const b = (init as RequestInit).body;
        bodyCaptured = b ? Buffer.from(b as Uint8Array).toString("utf8") : null;
        return new Response("{}", { status: 200 });
      }
    });
    await runCli(cap.io);
    expect(bodyCaptured).toBe("hello");
  });
});

describe("antpath proxy — error envelope", () => {
  it("exits 1 with a stable error body on a 4xx from the BFF", async () => {
    const cap = makeIo({
      argv: ["proxy", "stripe", "--method", "GET", "--path", "/x"],
      files: {
        [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe" }] }),
        [ANTPATH_RUN_TOKEN_PATH]: "tok"
      },
      fetchHandler: async () =>
        new Response(JSON.stringify({ error: "policy_denied", message: "[redacted]" }), {
          status: 403,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(1);
    const body = JSON.parse(cap.stderr.trim());
    expect(body.error).toBe("policy_denied");
  });

  it("exits 1 with unauthorized on a 401 from the BFF", async () => {
    const cap = makeIo({
      argv: ["proxy", "stripe"],
      files: {
        [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe" }] }),
        [ANTPATH_RUN_TOKEN_PATH]: "tok"
      },
      fetchHandler: async () =>
        new Response(JSON.stringify({ error: "unauthorized", message: "[redacted]" }), { status: 401 })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(1);
    const body = JSON.parse(cap.stderr.trim());
    expect(body.error).toBe("unauthorized");
  });

  // Locks the agent-visible CLI contract: every BFF-returned error code
  // exits non-zero with a structured stderr envelope that matches the
  // BFF's `error` field exactly. The agent reads stderr — drift here
  // means the agent has to learn a second error-code vocabulary.
  const errorTable: ReadonlyArray<{
    name: string;
    httpStatus: number;
    errorCode: string;
  }> = [
    { name: "bad_request", httpStatus: 400, errorCode: "bad_request" },
    { name: "endpoint_not_found", httpStatus: 404, errorCode: "endpoint_not_found" },
    { name: "rate_limited", httpStatus: 429, errorCode: "rate_limited" },
    { name: "budget_exceeded", httpStatus: 429, errorCode: "budget_exceeded" },
    { name: "upstream_error", httpStatus: 502, errorCode: "upstream_error" },
    { name: "upstream_timeout", httpStatus: 504, errorCode: "upstream_timeout" },
    { name: "exceeded_cap", httpStatus: 502, errorCode: "exceeded_cap" },
    { name: "internal_error", httpStatus: 500, errorCode: "internal_error" },
    { name: "ssrf_denied", httpStatus: 403, errorCode: "ssrf_denied" },
    { name: "unsupported_protocol", httpStatus: 426, errorCode: "unsupported_protocol" }
  ];

  for (const tc of errorTable) {
    it(`exits 1 and surfaces ${tc.errorCode} on HTTP ${tc.httpStatus}`, async () => {
      const cap = makeIo({
        argv: ["proxy", "stripe"],
        files: {
          [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe" }] }),
          [ANTPATH_RUN_TOKEN_PATH]: "tok"
        },
        fetchHandler: async () =>
          new Response(JSON.stringify({ error: tc.errorCode, message: "[redacted]" }), {
            status: tc.httpStatus,
            headers: { "content-type": "application/json" }
          })
      });
      await runCli(cap.io);
      expect(cap.exitCode).toBe(1);
      const body = JSON.parse(cap.stderr.trim());
      expect(body.error).toBe(tc.errorCode);
      // The agent must never see upstream-derived strings — the BFF's
      // sanitization rule mandates `message: "[redacted]"`.
      expect(body.message).toBe("[redacted]");
    });
  }

  it("falls back to a stable error envelope when the BFF body is not JSON", async () => {
    const cap = makeIo({
      argv: ["proxy", "stripe"],
      files: {
        [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe" }] }),
        [ANTPATH_RUN_TOKEN_PATH]: "tok"
      },
      fetchHandler: async () =>
        new Response("not json at all", { status: 502, headers: { "content-type": "text/plain" } })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(1);
    // CLI must produce a structured stderr line even when the BFF
    // misbehaves. The contract: the FIRST line of stderr is parseable
    // JSON with an `error` field.
    const envelopeLine = cap.stderr.split("\n").find((l) => l.startsWith("{"));
    expect(envelopeLine, `expected a JSON envelope line in stderr; got:\n${cap.stderr}`).toBeDefined();
    const body = JSON.parse(envelopeLine!);
    expect(typeof body.error).toBe("string");
  });

  it(
    "reproduces the customer's canonical-host redirect regression: HTTP 307 + 'Redirecting...' body, " +
      "and the new diagnostic message exposes HTTP status + body snippet",
    async () => {
      // This mirrors the response shape captured when a proxy public base
      // URL pointed at a host that canonicalized to another host:
      //
      //   HTTP/2 307
      //   cache-control: public, max-age=0, must-revalidate
      //   content-type: text/plain
      //   location: https://www.antpath.ai/api/runs/<runId>/proxy/<name>
      //   server: edge
      //
      //   Redirecting...
      //
      // The bug: in-container CLI uses `redirect: "manual"` (SSRF
      // guard) and parses "Redirecting...\n" as JSON, surfacing
      // "proxy returned non-JSON response" with no actionable detail.
      // Fix: the error message now embeds HTTP status + first 200
      // chars of the body so triage doesn't require another live run.
      const cap = makeIo({
        argv: ["proxy", "stripe"],
        files: {
          [ANTPATH_INDEX_PATH]: manifestJson({ endpoints: [{ name: "stripe" }] }),
          [ANTPATH_RUN_TOKEN_PATH]: "tok"
        },
        fetchHandler: async () =>
          new Response("Redirecting...\n", {
            status: 307,
            headers: {
              "content-type": "text/plain",
              "cache-control": "public, max-age=0, must-revalidate",
              location: "https://www.dashboard.example.com/api/runs/run-1/proxy/stripe",
              server: "edge"
            }
          })
      });
      await runCli(cap.io);
      expect(cap.exitCode).toBe(1);
      const envelopeLine = cap.stderr.split("\n").find((l) => l.startsWith("{"));
      expect(envelopeLine, `expected JSON envelope line in stderr; got:\n${cap.stderr}`).toBeDefined();
      const body = JSON.parse(envelopeLine!) as { error: string; message: string };
      expect(body.error).toBe("internal_error");
      // Load-bearing assertions — these would have FAILED against the
      // pre-fix CLI which only emitted a generic
      // "proxy returned non-JSON response" with no detail. Triage
      // signal is in the message itself.
      expect(body.message).toMatch(/HTTP 307/);
      expect(body.message).toMatch(/Redirecting/);
    }
  );
});

describe("antpath proxy --help", () => {
  it("renders proxy help and exits 0", async () => {
    const cap = makeIo({ argv: ["proxy", "--help"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("--method");
    expect(cap.stdout).toContain("--response-mode");
  });
});
