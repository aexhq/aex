/**
 * DX2: `aex tools|runtime-sizes list` — pure reads of the contracts SSoT. No
 * token, no network; human table by default, raw array under `--json`.
 *
 * There is no `models`/`providers` list any more: model ids are open Vercel AI
 * Gateway `creator/model` slugs, not a closed set the CLI can enumerate offline.
 */
import { describe, expect, it } from "bun:test";
import { executeCli } from "../src/main.js";
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

describe("aex models/providers verbs are removed", () => {
  it("treats `aex models` as an unknown subcommand", async () => {
    const cap = makeIo(["models", "list"]);
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.fetchCount()).toBe(0);
  });

  it("treats `aex providers` as an unknown subcommand", async () => {
    const cap = makeIo(["providers", "list"]);
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.fetchCount()).toBe(0);
  });
});

describe("aex tools list", () => {
  it("lists the complete closed builtin set in order and marks every tool default", async () => {
    const cap = makeIo(["tools", "list", "--json"]);
    await executeCli(cap.io);
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
      "git",
      "ls",
      "stat",
      "wc"
    ].map((tool) => ({ tool, default: true })));
    expect(arr.some((entry) => entry.tool === "notebook_edit")).toBe(false);
    expect(cap.fetchCount()).toBe(0);
  });

  it("renders every builtin as default in the human table", async () => {
    const cap = makeIo(["tools", "list"]);
    await executeCli(cap.io);
    expect(cap.out()).toContain("bash");
    expect(cap.out()).toContain("git");
    expect(cap.out()).not.toContain("notebook_edit");
    expect(cap.out()).not.toContain("opt-in");
  });
});

describe("aex runtime-sizes list", () => {
  it("lists presets and marks the default tier", async () => {
    const cap = makeIo(["runtime-sizes", "list", "--json"]);
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    const arr = JSON.parse(cap.out().trim()) as Array<{ size: string; cpus: number; memoryMb: number; default: boolean }>;
    const def = arr.find((e) => e.default);
    expect(def!.size).toBe("0.25cpu-1gb");
    expect(def!.memoryMb).toBe(1024);
  });
});
