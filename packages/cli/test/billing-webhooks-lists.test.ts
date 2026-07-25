/**
 * Unit coverage for the workspace read verbs added for launch:
 *   - `aex billing` / `aex billing ledger` (GET /api/billing, /api/billing/ledger)
 *   - `aex webhooks secret` (POST /api/webhook/signing-secret)
 *   - `aex sessions` / `aex sessions` (GET /api/sessions, /api/sessions)
 *
 * Same injected-IO style as host.test.ts: fake fetch, captured stdout/stderr.
 */
import { describe, expect, it } from "bun:test";
import { executeCli } from "../src/main.js";
import type { CliIO } from "../src/internal.js";

interface FetchCall {
  url: string;
  init: RequestInit;
}

function makeHostIo(opts: {
  argv: readonly string[];
  fetchHandler?: (call: FetchCall) => Response;
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

  const io: CliIO = {
    argv: ["bun", "/aex/aex", ...opts.argv],
    readFile: async (path) => {
      throw Object.assign(new Error(`ENOENT: ${path}`), { code: "ENOENT" });
    },
    writeFile: async () => {
      throw new Error("writeFile not configured for this test");
    },
    cwd: () => "/tmp/cli-test",
    fetchImpl: (async (input, init) => {
      const call: FetchCall = { url: String(input), init: init ?? {} };
      state.calls.push(call);
      const handler =
        opts.fetchHandler ??
        (() => new Response("{}", { status: 200, headers: { "content-type": "application/json" } }));
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

const BILLING_SUMMARY = {
  balanceUsd: 12.5,
  monthSpendUsd: 0.42,
  spendCapUsd: 50,
  planKey: "free",
  subscriptionStatus: "none"
};

describe("aex billing", () => {
  it("GETs /api/billing and prints a human-readable summary", async () => {
    const cap = makeHostIo({
      argv: ["billing", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify(BILLING_SUMMARY), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/billing");
    expect(cap.calls[0]!.init.method ?? "GET").toBe("GET");
    expect(cap.stdout).toContain("$12.50");
    expect(cap.stdout).toContain("$0.42");
    expect(cap.stdout).toContain("$50.00");
    expect(cap.stdout).toContain("free");
  });

  it("prints the raw wire body with --json (additive fields included)", async () => {
    const withExtra = { ...BILLING_SUMMARY, paymentMethodStatus: "active", accountType: "team" };
    const cap = makeHostIo({
      argv: ["billing", "--json", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify(withExtra), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(JSON.parse(cap.stdout)).toEqual(withExtra);
  });

  it("emits a structured error envelope on an API failure", async () => {
    const cap = makeHostIo({
      argv: ["billing", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ error: "insufficient_scope" }), {
          status: 403,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(1);
    const err = JSON.parse(cap.stderr) as { error: string; status: number };
    expect(err.error).toBe("billing_failed");
    expect(err.status).toBe(403);
  });

  it("rejects unexpected arguments", async () => {
    const cap = makeHostIo({ argv: ["billing", "extra", ...COMMON] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("usage: aex billing");
  });
});

describe("aex billing ledger", () => {
  it("GETs /api/billing/ledger with the limit param and prints the entries as JSON", async () => {
    const entries = [
      {
        id: "led-1",
        entryType: "top_up",
        amountUsd: 10,
        currency: "USD",
        sessionId: null,
        description: "admin top-up",
        createdBy: "admin:ops@example.test",
        createdAt: "2026-07-01T00:00:00Z"
      }
    ];
    const cap = makeHostIo({
      argv: ["billing", "ledger", "--limit", "5", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ entries }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const url = new URL(cap.calls[0]!.url);
    expect(url.pathname).toBe("/api/billing/ledger");
    expect(url.searchParams.get("limit")).toBe("5");
    expect(JSON.parse(cap.stdout)).toEqual(entries);
  });

  it("rejects a non-integer --limit", async () => {
    const cap = makeHostIo({ argv: ["billing", "ledger", "--limit", "many", ...COMMON] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--limit");
    expect(cap.calls).toHaveLength(0);
  });
});

describe("aex billing portal", () => {
  it("POSTs /api/billing/portal and prints the hosted URL", async () => {
    const cap = makeHostIo({
      argv: [
        "billing",
        "portal",
        "--return-url",
        "https://aex.dev/billing",
        "--idempotency-key",
        "portal-key",
        ...COMMON
      ],
      fetchHandler: () =>
        new Response(JSON.stringify({ url: "https://billing.stripe.test/session" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toBe("https://billing.stripe.test/session\n");
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/billing/portal");
    expect(cap.calls[0]!.init.method).toBe("POST");
    expect(JSON.parse(String(cap.calls[0]!.init.body))).toEqual({ returnUrl: "https://aex.dev/billing" });
    expect(new Headers(cap.calls[0]!.init.headers).get("idempotency-key")).toBe("portal-key");
  });

  it("supports --json", async () => {
    const cap = makeHostIo({
      argv: ["billing", "portal", "--json", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ url: "https://billing.stripe.test/session" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(JSON.parse(cap.stdout)).toEqual({ url: "https://billing.stripe.test/session" });
  });
});

describe("aex webhooks secret", () => {
  it("POSTs /api/webhook/signing-secret and prints ONLY the whsec reveal", async () => {
    const cap = makeHostIo({
      argv: ["webhooks", "secret", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ whsec: "whsec_dGVzdC1zZWNyZXQ=" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe("https://dash.example/api/webhook/signing-secret");
    expect(cap.calls[0]!.init.method).toBe("POST");
    // The command IS the explicit reveal request — stdout is the bare secret.
    expect(cap.stdout).toBe("whsec_dGVzdC1zZWNyZXQ=\n");
    // The secret never leaks onto stderr.
    expect(cap.stderr).not.toContain("whsec_");
  });

  it("keeps the secret out of --debug traces", async () => {
    const cap = makeHostIo({
      argv: ["webhooks", "secret", "--debug", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ whsec: "whsec_c3VwZXItc2VjcmV0" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stderr).not.toContain("whsec_");
    expect(cap.stderr).not.toContain("c3VwZXItc2VjcmV0");
  });

  it("rejects --rotate with an actionable message (the hosted API does not rotate)", async () => {
    const cap = makeHostIo({ argv: ["webhooks", "secret", "--rotate", ...COMMON] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--rotate");
    expect(cap.stderr).toContain("not supported");
    expect(cap.calls).toHaveLength(0);
  });

  it("requires the `secret` subcommand", async () => {
    const cap = makeHostIo({ argv: ["webhooks", ...COMMON] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("usage: aex webhooks secret");
  });
});

describe("aex sessions", () => {
  const PAGE = {
    sessions: [
      { id: "session-new", status: "idle", acceptsMessages: true, createdAt: "2026-07-02T10:00:00Z", updatedAt: "2026-07-02T10:05:00Z", costUsd: 0.01 },
      { id: "session-old", status: "error", acceptsMessages: true, createdAt: "2026-06-01T00:00:00Z", updatedAt: "2026-06-01T00:01:00Z" }
    ],
    nextCursor: "cursor-2"
  };

  it("forwards limit + since and prints the server page unchanged", async () => {
    const cap = makeHostIo({
      argv: ["sessions", "--limit", "2", "--since", "2026-07-01T00:00:00Z", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify(PAGE), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const url = new URL(cap.calls[0]!.url);
    expect(url.pathname).toBe("/api/sessions");
    expect(url.searchParams.get("limit")).toBe("2");
    expect(url.searchParams.get("since")).toBe("2026-07-01T00:00:00Z");
    const printed = JSON.parse(cap.stdout) as { sessions: Array<{ id: string }>; nextCursor?: string };
    expect(printed.sessions.map((r) => r.id)).toEqual(["session-new", "session-old"]);
    expect(printed.nextCursor).toBe("cursor-2");
  });

  it("prints the page unfiltered without --since", async () => {
    const cap = makeHostIo({
      argv: ["sessions", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify(PAGE), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const url = new URL(cap.calls[0]!.url);
    expect(url.search).toBe("");
    expect(JSON.parse(cap.stdout)).toEqual(PAGE);
  });

  it("rejects an unparseable --since", async () => {
    const cap = makeHostIo({ argv: ["sessions", "--since", "yesterdayish", ...COMMON] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--since");
    expect(cap.calls).toHaveLength(0);
  });
});

describe("aex sessions", () => {
  it("GETs /api/sessions with the limit param and prints the page as JSON", async () => {
    const page = {
      sessions: [{ id: "sess-1", status: "idle", acceptsMessages: true, createdAt: "t", updatedAt: "t" }]
    };
    const cap = makeHostIo({
      argv: ["sessions", "--limit", "10", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify(page), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    const url = new URL(cap.calls[0]!.url);
    expect(url.pathname).toBe("/api/sessions");
    expect(url.searchParams.get("limit")).toBe("10");
    expect(JSON.parse(cap.stdout)).toEqual(page);
  });

  it("rejects unexpected arguments", async () => {
    const cap = makeHostIo({ argv: ["sessions", "sess-1", ...COMMON] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("usage: aex sessions");
  });
});
