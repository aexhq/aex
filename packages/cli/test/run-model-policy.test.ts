import { describe, expect, it } from "vitest";
import { runCli } from "../src/run.js";
import { makeIo } from "./support.js";

const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

describe("aex run model policy (T6c/T6g — SDK arbitrates, no RUN_MODELS gate)", () => {
  it("accepts a forward-compat unknown model when --provider is explicit", async () => {
    const cap = makeIo({
      argv: [
        "run",
        "--model", "future-model-x",
        "--provider", "deepseek",
        "--deepseek-api-key", "sk-ds-1",
        "--prompt", "hi",
        ...COMMON
      ],
      fetchHandler: () =>
        new Response(JSON.stringify({ id: "s1", status: "running" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        })
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
    const body = cap.calls[0]!.body as { provider?: string; submission?: { model?: string } };
    expect(body.provider).toBe("deepseek");
    expect(body.submission?.model).toBe("future-model-x");
  });

  it("emits a shared did-you-mean hint for a typo'd known model (no --provider)", async () => {
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
    expect(cap.calls).toHaveLength(0);
  });
});
