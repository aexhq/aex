import { describe, expect, it } from "vitest";
import { executeCli } from "../src/main.js";
import { makeIo } from "./support.js";
import { canonicalWhoami } from "./canonical-whoami.js";

const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

describe("aex whoami --json + per-verb --help (T6e/T6f)", () => {
  it("`whoami --json` succeeds (JSON), never rejecting the flag", async () => {
    const cap = makeIo({
      argv: ["whoami", "--json", ...COMMON],
      fetchHandler: () =>
        new Response(JSON.stringify(canonicalWhoami("ws-7", ["sessions.write"])), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ workspaceId: "ws-7" });
  });

  it("`whoami --help` prints usage with exit 0 and NO api key required (no network)", async () => {
    const cap = makeIo({ argv: ["whoami", "--help"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("aex whoami");
    expect(cap.stdout).toContain("Usage:");
    expect(cap.calls).toHaveLength(0);
  });

  it("`start --help` prints the session flags with exit 0 and NO api key required", async () => {
    const cap = makeIo({ argv: ["start", "--help"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("aex start");
    expect(cap.stdout).toContain("--skill");
    expect(cap.stdout).toContain("--tool");
    expect(cap.calls).toHaveLength(0);
  });

  it("`files --help` prints the sub-verbs with exit 0", async () => {
    const cap = makeIo({ argv: ["files", "--help"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("aex files");
    expect(cap.stdout).toContain("read");
    expect(cap.stdout).toContain("find");
    expect(cap.calls).toHaveLength(0);
  });
});
