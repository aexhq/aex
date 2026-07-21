/**
 * DX1: the stored-config fallback in `resolveCommonHostFlags`, exercised through
 * a real host verb (`whoami`). A token persisted by `aex login` is honored when
 * `--api-key` is absent; an explicit flag overrides it; `--aex-url` precedence
 * is flag > stored > default; and with neither flag nor stored token the verb
 * fails with the actionable "run `aex login`" message.
 */
import { describe, expect, it } from "vitest";
import { executeCli } from "../src/main.js";
import { AEX_INDEX_PATH, type CliIO, type StoredCliConfig } from "../src/internal.js";
import { canonicalWhoami } from "./canonical-whoami.js";

function makeIo(opts: {
  argv: readonly string[];
  stored?: StoredCliConfig | null;
  files?: Record<string, string>;
  fetchHandler?: (url: string, init?: RequestInit) => Response;
}): {
  io: CliIO;
  out: () => string;
  err: () => string;
  exit: () => number | null;
  calls: Array<{ url: string; init?: RequestInit }>;
  writes: StoredCliConfig[];
  cleared: number;
} {
  const state = { stdout: "", stderr: "", exit: null as number | null };
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const writes: StoredCliConfig[] = [];
  let cleared = 0;
  let stored = opts.stored ?? null;
  const files = opts.files ?? {};
  const io: CliIO = {
    argv: ["bun", "/aex/aex", ...opts.argv],
    readFile: async (path) => {
      if (!(path in files)) throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
      return files[path]!;
    },
    writeFile: async () => {
      throw new Error("writeFile not configured");
    },
    cwd: () => "/tmp",
    fetchImpl: async (url, init) => {
      calls.push({ url: String(url), ...(init ? { init } : {}) });
      if (opts.fetchHandler) return opts.fetchHandler(String(url), init);
      return new Response(JSON.stringify(canonicalWhoami("ws-1")), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    },
    stdout: (c) => {
      state.stdout += c;
    },
    stderr: (c) => {
      state.stderr += c;
    },
    exit: (code) => {
      state.exit = code;
    },
    configStore: {
      location: () => "/home/u/.config/aex/config.json",
      read: async () => stored,
      write: async (config) => {
        writes.push(config);
        stored = config;
      },
      clear: async () => {
        cleared++;
        stored = null;
      }
    }
  };
  return {
    io,
    out: () => state.stdout,
    err: () => state.stderr,
    exit: () => state.exit,
    calls,
    writes,
    get cleared() {
      return cleared;
    }
  };
}

describe("resolveCommonHostFlags — stored-config fallback (DX1)", () => {
  it("uses the stored token + url when --api-key is absent", async () => {
    const cap = makeIo({
      argv: ["whoami"],
      stored: { schemaVersion: 1, apiKey: "stored-tok", aexUrl: "https://stored.example" }
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe("https://stored.example/api/whoami");
    expect(new Headers(cap.calls[0]!.init?.headers).get("authorization")).toBe("Bearer stored-tok");
  });

  it("lets an explicit --api-key override the stored token", async () => {
    const cap = makeIo({
      argv: ["whoami", "--api-key", "flag-tok", "--aex-url", "https://flag.example"],
      stored: { schemaVersion: 1, apiKey: "stored-tok", aexUrl: "https://stored.example" }
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls[0]!.url).toBe("https://flag.example/api/whoami");
    expect(new Headers(cap.calls[0]!.init?.headers).get("authorization")).toBe("Bearer flag-tok");
  });

  it("applies --aex-url precedence flag > stored > default", async () => {
    // flag url present, stored url present → flag wins
    const cap = makeIo({
      argv: ["whoami", "--aex-url", "https://flag.example"],
      stored: { apiKey: "stored-tok", aexUrl: "https://stored.example" }
    });
    await executeCli(cap.io);
    expect(cap.calls[0]!.url).toBe("https://flag.example/api/whoami");

    // no flag url, stored url absent → default base url
    const cap2 = makeIo({ argv: ["whoami"], stored: { apiKey: "stored-tok" } });
    await executeCli(cap2.io);
    expect(cap2.calls[0]!.url).toBe("https://api.aex.dev/api/whoami");
  });

  it("fails with the actionable login hint when neither flag nor stored token exist", async () => {
    const cap = makeIo({ argv: ["whoami"], stored: null });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("run `aex login`");
    expect(cap.calls).toHaveLength(0);
  });

  it("emits a non-secret auth-source line under --debug (token never printed)", async () => {
    const cap = makeIo({
      argv: ["whoami", "--debug"],
      stored: { apiKey: "super-secret-token", aexUrl: "https://stored.example" }
    });
    await executeCli(cap.io);
    expect(cap.err()).toContain("[aex] auth: stored token (/home/u/.config/aex/config.json)");
    expect(cap.err()).not.toContain("super-secret-token");
  });

  it("uses stored credentials across representative read and mutation handlers", async () => {
    const cases = [
      {
        argv: ["status", "session-read"],
        suffix: "/api/sessions/session-read",
        response: { session: { id: "session-read", status: "idle", acceptsMessages: true } }
      },
      {
        argv: ["cancel", "session-mutate"],
        suffix: "/api/sessions/session-mutate/cancel",
        response: { session: { id: "session-mutate", status: "cancelling", acceptsMessages: false } }
      }
    ] as const;

    for (const scenario of cases) {
      const cap = makeIo({
        argv: scenario.argv,
        stored: { schemaVersion: 1, apiKey: "stored-tok", aexUrl: "https://stored.example" },
        fetchHandler: () => new Response(JSON.stringify(scenario.response), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
      });
      await executeCli(cap.io);
      expect(cap.exit(), scenario.argv[0]).toBe(0);
      expect(cap.calls, scenario.argv[0]).toHaveLength(1);
      expect(cap.calls[0]!.url).toBe(`https://stored.example${scenario.suffix}`);
      expect(new Headers(cap.calls[0]!.init?.headers).get("authorization")).toBe("Bearer stored-tok");
    }
  });

  it("uses stored credentials for session creation and first-turn submission", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "claude-haiku-4-5",
        "--prompt", "auth regression",
        "--anthropic-api-key", "provider-secret"
      ],
      stored: { schemaVersion: 1, apiKey: "stored-tok", aexUrl: "https://stored.example" },
      fetchHandler: (url) => url.endsWith("/api/sessions")
        ? new Response(JSON.stringify({
            session: { id: "session-submit", status: "idle", acceptsMessages: true }
          }), { status: 201, headers: { "content-type": "application/json" } })
        : new Response(JSON.stringify({
            session: { id: "session-submit", status: "running", acceptsMessages: false },
            run: { sessionId: "session-submit", runId: "run-1", turnSeq: 1, phase: "running" },
            eventCursor: 1
          }), { status: 202, headers: { "content-type": "application/json" } })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls.map((call) => call.url)).toEqual([
      "https://stored.example/api/sessions",
      "https://stored.example/api/sessions/session-submit/messages"
    ]);
    for (const call of cap.calls) {
      expect(new Headers(call.init?.headers).get("authorization")).toBe("Bearer stored-tok");
    }
    expect(cap.out()).not.toContain("stored-tok");
    expect(cap.err()).not.toContain("stored-tok");
    expect(cap.out()).not.toContain("provider-secret");
    expect(cap.err()).not.toContain("provider-secret");
  });

  it("still refuses host verbs inside a managed session container", async () => {
    const cap = makeIo({
      argv: ["whoami"],
      stored: { apiKey: "stored-tok" },
      files: { [AEX_INDEX_PATH]: "{}" }
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("cannot execute inside a managed session container");
    expect(cap.calls).toHaveLength(0);
  });
});
