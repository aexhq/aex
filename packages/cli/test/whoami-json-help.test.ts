import { describe, expect, it } from "vitest";
import { runCli } from "../src/run.js";
import { makeIo } from "./support.js";

const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

describe("aex whoami --json + per-verb --help (T6e/T6f)", () => {
  it("`whoami --json` succeeds (JSON), never rejecting the flag", async () => {
    const cap = makeIo({
      argv: ["whoami", "--json", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify({ workspaceId: "ws-7", scopes: ["runs.write"] }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ workspaceId: "ws-7" });
  });

  it("`whoami --help` prints usage with exit 0 and NO api key required (no network)", async () => {
    const cap = makeIo({ argv: ["whoami", "--help"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("aex whoami");
    expect(cap.stdout).toContain("Usage:");
    expect(cap.calls).toHaveLength(0);
  });

  it("`run --help` prints the run flags with exit 0 and NO api key required", async () => {
    const cap = makeIo({ argv: ["run", "--help"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("aex run");
    expect(cap.stdout).toContain("--skill");
    expect(cap.stdout).toContain("--tool");
    expect(cap.calls).toHaveLength(0);
  });

  it("`outputs --help` prints the sub-verbs with exit 0", async () => {
    const cap = makeIo({ argv: ["outputs", "--help"] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("aex outputs");
    expect(cap.stdout).toContain("read");
    expect(cap.stdout).toContain("search");
    expect(cap.calls).toHaveLength(0);
  });
});
