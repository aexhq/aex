import { describe, expect, it } from "vitest";
import { executeFilesSyncCmd } from "../src/files-sync.js";
import type { CliIO, SessionFilesSyncFileEntry } from "../src/internal.js";

/** Build a minimal `CliIO` for `files sync` test cases. */
function makeIo(opts: {
  walk?: (root: string) => Promise<readonly SessionFilesSyncFileEntry[] | null>;
  inContainer?: boolean;
}): {
  io: CliIO;
  stdout: string;
  stderr: string;
} {
  const state = { stdout: "", stderr: "" };
  const io: CliIO = {
    argv: ["bun", "/aex/aex", "files", "sync"],
    readFile: async (path) => {
      if (path === "/mnt/session/uploads/aex/index.json" && opts.inContainer) return "{}";
      const err = Object.assign(new Error(`ENOENT: ${path}`), { code: "ENOENT" });
      throw err;
    },
    writeFile: async () => {
      throw new Error("not used");
    },
    fetchImpl: (async () => new Response("{}")) as typeof fetch,
    cwd: () => "/tmp",
    stdout: (chunk) => {
      state.stdout += chunk;
    },
    stderr: (chunk) => {
      state.stderr += chunk;
    },
    exit: () => {},
    ...(opts.walk ? { walkDirectory: opts.walk } : {})
  };
  return {
    io,
    get stdout() {
      return state.stdout;
    },
    get stderr() {
      return state.stderr;
    }
  };
}

describe("aex files sync (internal)", () => {
  it("refuses to run on the host (no /aex/index.json)", async () => {
    const cap = makeIo({ inContainer: false, walk: async () => [] });
    const exit = await executeFilesSyncCmd(cap.io, ["/workspace/files"]);
    expect(exit.code).not.toBe(0);
    expect(cap.stderr).toMatch(/in-container/i);
    expect(cap.stdout).toBe("");
  });

  it("rejects empty dir list", async () => {
    const cap = makeIo({ inContainer: true });
    const exit = await executeFilesSyncCmd(cap.io, []);
    expect(exit.code).not.toBe(0);
    expect(cap.stderr).toMatch(/usage/i);
  });

  it("emits one JSON line per discovered file plus a summary line", async () => {
    const cap = makeIo({
      inContainer: true,
      walk: async (root) => {
        if (root === "/workspace/files") {
          return [
            { path: "/workspace/files/a.txt", sizeBytes: 12 },
            { path: "/workspace/files/b.bin", sizeBytes: 99 }
          ];
        }
        return [];
      }
    });
    const exit = await executeFilesSyncCmd(cap.io, ["/workspace/files", "/workspace/state"]);
    expect(exit.code).toBe(0);
    const lines = cap.stdout.trim().split("\n");
    expect(lines).toHaveLength(3);
    const parsed = lines.map((line) => JSON.parse(line) as Record<string, unknown>);
    expect(parsed[0]).toMatchObject({ dir: "/workspace/files", path: "/workspace/files/a.txt", sizeBytes: 12 });
    expect(parsed[1]).toMatchObject({ dir: "/workspace/files", path: "/workspace/files/b.bin", sizeBytes: 99 });
    expect(parsed[2]).toMatchObject({ summary: { dirs: 2, files: 2, missing: 0 } });
  });

  it("reports missing dirs via stderr and continues", async () => {
    const cap = makeIo({
      inContainer: true,
      walk: async (root) => (root === "/workspace/state" ? null : [])
    });
    const exit = await executeFilesSyncCmd(cap.io, ["/workspace/files", "/workspace/state"]);
    expect(exit.code).toBe(0);
    expect(cap.stderr).toContain(`"missing_or_unreadable"`);
    expect(cap.stderr).toContain(`"/workspace/state"`);
    const summary = cap.stdout.trim().split("\n").pop()!;
    expect(JSON.parse(summary)).toMatchObject({ summary: { dirs: 2, files: 0, missing: 1 } });
  });

  it("rejects non-absolute output dirs as a defence against platform mis-instruction", async () => {
    const cap = makeIo({ inContainer: true, walk: async () => [] });
    const exit = await executeFilesSyncCmd(cap.io, ["workspace/files"]);
    expect(exit.code).toBe(0);
    expect(cap.stderr).toContain(`"non_absolute_path"`);
    const summary = cap.stdout.trim().split("\n").pop()!;
    expect(JSON.parse(summary)).toMatchObject({ summary: { dirs: 1, files: 0, missing: 1 } });
  });
});
