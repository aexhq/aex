import { describe, expect, it } from "bun:test";
import { executeCli } from "../src/main.js";
import type { CliIO } from "../src/internal.js";

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
    argv: ["bun", "/aex/aex", ...opts.argv],
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

describe("aex --help", () => {
  it("prints host-mode usage and exits 0", async () => {
    const cap = makeIo({ argv: ["--help"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("aex start");
    expect(cap.stdout).toContain("aex whoami");
    expect(cap.stdout).toContain("aex tail");
    expect(cap.stdout).toContain("aex inspect");
    expect(cap.stdout).toContain("Usage:");
  });

  it("advertises the workspace read verbs (billing, webhooks secret, sessions, sessions)", async () => {
    const cap = makeIo({ argv: ["--help"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("aex billing [--json]");
    expect(cap.stdout).toContain("aex billing ledger [--limit N]");
    expect(cap.stdout).toContain("aex webhooks secret");
    expect(cap.stdout).toContain("aex sessions [--limit N] [--since ISO]");
    expect(cap.stdout).toContain("aex sessions [--limit N]");
    // The signing-secret verb is reveal-only; help must not advertise rotation.
    expect(cap.stdout).not.toContain("--rotate");
  });

  it("does not advertise removed launch flags or commands", async () => {
    const cap = makeIo({ argv: ["--help"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).not.toContain("--workspace");
    expect(cap.stdout).not.toContain("proxy");
    expect(cap.stdout).not.toContain("--proxy-endpoint");
    expect(cap.stdout).not.toContain("aex debug");
    expect(cap.stdout).not.toContain("--settle");
    expect(cap.stdout).not.toContain("starttime-sizes");
    expect(cap.stdout).not.toContain("session.finished");
    expect(cap.stdout).toContain("aex runtime-sizes list");
    expect(cap.stdout).toContain("run.finished/run.error");
  });

  it("advertises --aex-url as optional with the api.aex.dev default", async () => {
    const cap = makeIo({ argv: ["--help"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("https://api.aex.dev");
    expect(cap.stdout).toContain("https://dev-api.aex.dev");
    expect(cap.stdout).toContain("optional after `aex login`");
  });
});

describe("aex start managed gateway model (SDK parity)", () => {
  it("submits a gateway model slug with no provider selector and no key", async () => {
    // Managed keys: the customer names a `creator/model` slug and the platform
    // routes it — no provider inference, no per-provider key.
    const cap = makeIo({
      argv: [
        "start",
        "--model", "deepseek/deepseek-v4-flash",
        "--prompt", "hi",
        "--api-key", "tok",
        "--aex-url", "https://api.test"
      ],
      fetchHandler: async () =>
        new Response(JSON.stringify({ error: "boom" }), { status: 500, headers: { "content-type": "application/json" } })
    });
    await executeCli(cap.io);
    expect(cap.fetchCalls.length).toBeGreaterThan(0);
    const body = JSON.parse(String(cap.fetchCalls[0]!.init?.body ?? "{}")) as Record<string, unknown>;
    expect(body).not.toHaveProperty("provider");
    const submission = body["submission"] as Record<string, unknown> | undefined;
    expect(submission?.["model"]).toBe("deepseek/deepseek-v4-flash");
    expect(submission).not.toHaveProperty("provider");
  });

  it("rejects a bare (non-slug) model id at the boundary", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "not-a-slug",
        "--prompt", "hi",
        "--api-key", "tok",
        "--aex-url", "https://api.test"
      ]
    });
    await executeCli(cap.io);
    expect(cap.exitCode).not.toBe(0);
    expect(cap.stderr).toContain("--model");
    expect(cap.fetchCalls.length).toBe(0);
  });
});

describe("removed commands", () => {
  it("treats the old proxy verb as an unknown subcommand", async () => {
    const cap = makeIo({ argv: ["proxy", "--help"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("unknown subcommand: proxy");
  });

  it("treats the private debug verb as an unknown subcommand", async () => {
    const cap = makeIo({ argv: ["debug", "session-1"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("unknown subcommand: debug");
  });

  it("exits 2 on unknown subcommand", async () => {
    const cap = makeIo({ argv: ["snorlax"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("unknown subcommand");
  });
});
