import { describe, expect, it, vi } from "vitest";
import { Aex } from "@aexhq/sdk";
import { runCli } from "../src/run.js";
import { makeIo } from "./support.js";

const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

describe("aex run --run-timeout floor (T6d — SSoT parser, sync reject)", () => {
  it("rejects a sub-1m --run-timeout synchronously, firing NO network call", async () => {
    const submit = vi.spyOn(Aex.prototype, "submit");
    const cap = makeIo({
      argv: [
        "run",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--anthropic-api-key", "sk-ant-1",
        "--run-timeout", "30s",
        ...COMMON
      ]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    // The SSoT `parseRunTimeout` enforces the [1m, 8h] floor (not a format-only check).
    expect(cap.stderr).toContain("--run-timeout");
    expect(cap.stderr).toMatch(/at least .*1m|60000ms/);
    // No session was created — the reject is fully client-side.
    expect(cap.calls).toHaveLength(0);
    expect(submit).not.toHaveBeenCalled();
  });

  it("accepts a valid --run-timeout above the floor", async () => {
    const submit = vi
      .spyOn(Aex.prototype, "submit")
      .mockResolvedValue({ runId: "r1", session: { record: { id: "s1", status: "running" } } } as never);
    const cap = makeIo({
      argv: [
        "run",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--anthropic-api-key", "sk-ant-1",
        "--run-timeout", "2h",
        ...COMMON
      ]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(submit).toHaveBeenCalledTimes(1);
  });
});
