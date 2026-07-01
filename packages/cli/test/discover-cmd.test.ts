/**
 * DX2: `aex models|providers|tools|runtime-sizes list` — pure reads of the
 * contracts SSoT. No token, no network; human table by default, raw array under
 * `--json`.
 */
import { describe, expect, it } from "vitest";
import { runCli } from "../src/run.js";
import type { CliIO } from "../src/internal.js";

function makeIo(argv: readonly string[]): {
  io: CliIO;
  out: () => string;
  exit: () => number | null;
  fetchCount: () => number;
} {
  const state = { stdout: "", exit: null as number | null, fetchCount: 0 };
  const io: CliIO = {
    argv: ["bun", "/aex/aex", ...argv],
    readFile: async () => {
      throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
    },
    writeFile: async () => {},
    cwd: () => "/tmp",
    fetchImpl: async () => {
      state.fetchCount++;
      return new Response("{}", { status: 200 });
    },
    stdout: (c) => {
      state.stdout += c;
    },
    stderr: () => {},
    exit: (code) => {
      state.exit = code;
    }
  };
  return {
    io,
    out: () => state.stdout,
    exit: () => state.exit,
    fetchCount: () => state.fetchCount
  };
}

describe("aex models list", () => {
  it("prints a human table with a known model + its default provider", async () => {
    const cap = makeIo(["models", "list"]);
    await runCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.out()).toContain("MODEL");
    expect(cap.out()).toContain("claude-sonnet-4-6");
    expect(cap.out()).toContain("anthropic");
    expect(cap.fetchCount()).toBe(0);
  });

  it("emits a JSON array under --json with the expected shape", async () => {
    const cap = makeIo(["models", "list", "--json"]);
    await runCli(cap.io);
    expect(cap.exit()).toBe(0);
    const arr = JSON.parse(cap.out().trim()) as Array<{ model: string; defaultProvider: string | null; providers: string[] }>;
    const haiku = arr.find((e) => e.model === "claude-haiku-4-5");
    expect(haiku).toBeDefined();
    expect(haiku!.defaultProvider).toBe("anthropic");
    expect(haiku!.providers).toContain("anthropic");
  });

  it("works without the explicit `list` subcommand", async () => {
    const cap = makeIo(["models"]);
    await runCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.out()).toContain("claude-haiku-4-5");
  });
});

describe("aex providers list", () => {
  it("lists providers with their display name + models", async () => {
    const cap = makeIo(["providers", "list", "--json"]);
    await runCli(cap.io);
    expect(cap.exit()).toBe(0);
    const arr = JSON.parse(cap.out().trim()) as Array<{ provider: string; displayName: string; models: string[] }>;
    const anthropic = arr.find((e) => e.provider === "anthropic");
    expect(anthropic!.displayName).toBe("Anthropic");
    expect(anthropic!.models.length).toBeGreaterThan(0);
  });
});

describe("aex tools list", () => {
  it("lists the complete closed builtin set in order and marks every tool default", async () => {
    const cap = makeIo(["tools", "list", "--json"]);
    await runCli(cap.io);
    expect(cap.exit()).toBe(0);
    const arr = JSON.parse(cap.out().trim()) as Array<{ tool: string; default: boolean }>;
    expect(arr).toEqual([
      "bash",
      "read_file",
      "write_file",
      "edit_file",
      "grep",
      "glob",
      "head",
      "tail",
      "todo_write",
      "subagent",
      "subagent_result",
      "web_fetch",
      "web_search",
      "bash_output",
      "bash_kill",
      "code_execution",
      "wait",
      "git"
    ].map((tool) => ({ tool, default: true })));
    expect(arr.some((entry) => entry.tool === "notebook_edit")).toBe(false);
    expect(cap.fetchCount()).toBe(0);
  });

  it("renders every builtin as default in the human table", async () => {
    const cap = makeIo(["tools", "list"]);
    await runCli(cap.io);
    expect(cap.out()).toContain("bash");
    expect(cap.out()).toContain("git");
    expect(cap.out()).not.toContain("notebook_edit");
    expect(cap.out()).not.toContain("opt-in");
  });
});

describe("aex runtime-sizes list", () => {
  it("lists presets and marks the default tier", async () => {
    const cap = makeIo(["runtime-sizes", "list", "--json"]);
    await runCli(cap.io);
    expect(cap.exit()).toBe(0);
    const arr = JSON.parse(cap.out().trim()) as Array<{ size: string; cpus: number; memoryMb: number; default: boolean }>;
    const def = arr.find((e) => e.default);
    expect(def!.size).toBe("shared-0.25x-1gb");
    expect(def!.memoryMb).toBe(1024);
  });
});
