import { describe, expect, it, vi } from "vitest";
import { Aex, type SessionRunOptions } from "@aexhq/sdk";
import { runCli } from "../src/run.js";
import { makeIo } from "./support.js";

const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

describe("aex run model policy (T6c/T6g — SDK arbitrates, no RUN_MODELS gate)", () => {
  it("accepts a forward-compat unknown model when --provider is explicit", async () => {
    const submit = vi
      .spyOn(Aex.prototype, "submit")
      .mockResolvedValue({ runId: "r1", session: { record: { id: "s1", status: "running" } } } as never);

    const cap = makeIo({
      argv: [
        "run",
        "--model", "future-model-x",
        "--provider", "deepseek",
        "--deepseek-api-key", "sk-ds-1",
        "--prompt", "hi",
        ...COMMON
      ]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(submit).toHaveBeenCalledTimes(1);
    const options = submit.mock.calls[0]![0] as SessionRunOptions;
    expect(options.model).toBe("future-model-x");
    expect(options.provider).toBe("deepseek");
  });

  it("emits a shared did-you-mean hint for a typo'd known model (no --provider)", async () => {
    const submit = vi.spyOn(Aex.prototype, "submit");
    const cap = makeIo({
      argv: [
        "run",
        "--model", "deepseek-v4-flsh",
        "--deepseek-api-key", "sk-ds-1",
        "--prompt", "hi",
        ...COMMON
      ]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain('did you mean "deepseek-v4-flash"');
    expect(submit).not.toHaveBeenCalled();
  });
});
