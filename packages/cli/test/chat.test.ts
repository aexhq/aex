/**
 * `aex chat` — offline-validatable surface (arg validation + container refusal).
 * The Claude loop itself is live-only; the corpus scoping it relies on is covered
 * by the SDK corpus-tools unit tests.
 */
import { describe, expect, it } from "vitest";
import { runCli } from "../src/run.js";
import { AEX_INDEX_PATH, type CliIO } from "../src/internal.js";

function makeIo(opts: { argv: readonly string[]; files?: Record<string, string> }): {
  io: CliIO;
  err: () => string;
  exit: () => number | null;
  fetchCount: () => number;
} {
  const state = { stderr: "", exit: null as number | null, fetchCount: 0 };
  const files = opts.files ?? {};
  const io: CliIO = {
    argv: ["bun", "/aex/aex", ...opts.argv],
    readFile: async (path) => {
      if (!(path in files)) throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
      return files[path]!;
    },
    writeFile: async () => {},
    cwd: () => "/tmp",
    fetchImpl: async () => {
      state.fetchCount++;
      return new Response("{}", { status: 200 });
    },
    stdout: () => {},
    stderr: (c) => {
      state.stderr += c;
    },
    exit: (code) => {
      state.exit = code;
    }
  };
  return { io, err: () => state.stderr, exit: () => state.exit, fetchCount: () => state.fetchCount };
}

const TOKEN = ["--api-token", "tok", "--aex-url", "https://dash.example"];

describe("aex chat (offline validation)", () => {
  it("requires at least one --session", async () => {
    const cap = makeIo({ argv: ["chat", ...TOKEN, "--anthropic-api-key", "sk", "--prompt", "hi"] });
    await runCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("requires at least one --session");
    expect(cap.fetchCount()).toBe(0);
  });

  it("requires --anthropic-api-key", async () => {
    const cap = makeIo({ argv: ["chat", ...TOKEN, "--session", "sess-1", "--prompt", "hi"] });
    await runCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("--anthropic-api-key is required");
  });

  it("requires --prompt", async () => {
    const cap = makeIo({ argv: ["chat", ...TOKEN, "--session", "sess-1", "--anthropic-api-key", "sk"] });
    await runCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("--prompt");
  });

  it("requires an API token (no stored config)", async () => {
    const cap = makeIo({ argv: ["chat", "--session", "sess-1", "--anthropic-api-key", "sk", "--prompt", "hi"] });
    await runCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("run `aex login`");
  });

  it("refuses to run inside a managed run container", async () => {
    const cap = makeIo({
      argv: ["chat", ...TOKEN, "--session", "sess-1", "--anthropic-api-key", "sk", "--prompt", "hi"],
      files: { [AEX_INDEX_PATH]: "{}" }
    });
    await runCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("cannot run inside a managed run container");
  });
});
