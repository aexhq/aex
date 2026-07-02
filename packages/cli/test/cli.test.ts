import { describe, expect, it } from "vitest";
import { runCli } from "../src/run.js";
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
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("aex run");
    expect(cap.stdout).toContain("aex whoami");
    expect(cap.stdout).toContain("aex tail");
    expect(cap.stdout).toContain("aex inspect");
    expect(cap.stdout).toContain("Usage:");
  });

  it("does not advertise removed launch flags or commands", async () => {
    const cap = makeIo({ argv: ["--help"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).not.toContain("--workspace");
    expect(cap.stdout).not.toContain("proxy");
    expect(cap.stdout).not.toContain("--proxy-endpoint");
  });

  it("advertises --aex-url as optional with the api.aex.dev default", async () => {
    const cap = makeIo({ argv: ["--help"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("https://api.aex.dev");
  });
});

describe("removed commands", () => {
  it("treats the old proxy verb as an unknown subcommand", async () => {
    const cap = makeIo({ argv: ["proxy", "--help"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("unknown subcommand: proxy");
  });

  it("exits 2 on unknown subcommand", async () => {
    const cap = makeIo({ argv: ["snorlax"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("unknown subcommand");
  });
});
