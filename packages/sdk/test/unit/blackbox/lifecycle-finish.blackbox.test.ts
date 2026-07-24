/**
 * BLACKBOX - the committed RUN terminal boundary.
 *
 * Every assertion here is what a customer sees through the public `aex.start()` /
 * `session.messages.send().finished()` surface against a realistic platform. The findings this
 * pins closed:
 *   H4/T1  a cancelled or timed-out session must NOT read as a clean idle/success.
 *   T1     a clean one-shot reports the OUTCOME `succeeded`, never a bare `idle`,
 *          while the resumable session RECORD keeps its `idle` lifecycle status.
 *   T2/T3  cost is ALWAYS present at the RUN terminal — even a $0 run.
 *   T4     usage is ALWAYS present, derived from the server's provider usage.
 *   parity finished() == start(): the same finished shape from both surfaces.
 */
import { describe, expect, it } from "bun:test";
import type { SessionStartOptions } from "../../../src/index.js";
import { FakePlatform } from "./fake-platform.js";

const ONE_SHOT: SessionStartOptions = { model: "anthropic/claude-haiku-4-5", message: "do the thing" };

describe("blackbox: committed run terminal boundary", () => {
  it("a clean one-shot reports outcome 'succeeded' with cost + usage always present", async () => {
    const platform = new FakePlatform();
    const result = await platform.start(ONE_SHOT, {
      text: "all done",
      outcome: "succeeded",
      costUsd: 0.0123,
      usage: { inputTokens: 10, outputTokens: 5, totalTokens: 15 }
    });

    expect(result.ok).toBe(true);
    // The RESULT status is the terminal OUTCOME, never a bare lifecycle `idle`...
    expect(result.status).toBe("succeeded");
    // ...while the resumable session RECORD keeps its `idle` lifecycle status.
    expect(result.session?.status).toBe("idle");
    expect(result.text).toBe("all done");
    expect(result.costUsd).toBe(0.0123);
    expect(result.usage).toEqual({ inputTokens: 10, outputTokens: 5, totalTokens: 15 });
    expect(result.error).toBeUndefined();
    expect(result.checkpoint?.checkpointId).toBe("cp-1");
  });

  it("a $0 run finishes with costUsd:0 and empty usage", async () => {
    const platform = new FakePlatform();
    const result = await platform.start(ONE_SHOT, { text: "cheap", outcome: "succeeded", costUsd: 0 });
    expect(result.ok).toBe(true);
    expect(result.costUsd).toBe(0);
    expect(result.usage).toEqual({});
  });

  it("a CANCELLED run reports outcome 'cancelled' + ok:false — NOT a clean idle", async () => {
    const platform = new FakePlatform();
    const result = await platform.start(ONE_SHOT, { outcome: "cancelled", costUsd: 0.004 });
    expect(result.status).toBe("cancelled");
    expect(result.ok).toBe(false);
    // Byte-different from a succeeded run — the H4 core.
    expect(result.status).not.toBe("succeeded");
    expect(result.status).not.toBe("idle");
  });

  it("a wall-clock TIMEOUT reports the typed outcome 'timed_out' + ok:false", async () => {
    const platform = new FakePlatform();
    const result = await platform.start(ONE_SHOT, { outcome: "timed_out", costUsd: 0.004 });
    expect(result.status).toBe("timed_out");
    expect(result.ok).toBe(false);
  });

  it("a FAILED run reports outcome 'failed' + the terminal error text", async () => {
    const platform = new FakePlatform();
    const result = await platform.start(ONE_SHOT, { outcome: "failed", errorMessage: "invalid provider api key" });
    expect(result.ok).toBe(false);
    expect(result.status).toBe("failed");
    expect(result.error).toBe("invalid provider api key");
    expect(result.checkpoint).toBeUndefined();
    expect(result.files).toEqual([]);
    expect(result.costUsd).toBe(0);
    expect(result.usage).toEqual({});
  });

  it("finished() and start() return the same committed shape", async () => {
    const run = new FakePlatform();
    const viaRun = await run.start(ONE_SHOT, { text: "hi", costUsd: 0.002, usage: { inputTokens: 4, outputTokens: 1 } });

    const send = new FakePlatform();
    const handle = await send.aex.sessions.create({ model: "anthropic/claude-haiku-4-5" });
    const viaFinished = (await send.send(handle, "hi", {
      text: "hi",
      costUsd: 0.002,
      usage: { inputTokens: 4, outputTokens: 1 }
    })) as { ok: boolean; status: string; costUsd: number; usage: unknown };

    expect(viaFinished.ok).toBe(viaRun.ok);
    expect(viaFinished.status).toBe(viaRun.status);
    expect(viaFinished.costUsd).toBe(viaRun.costUsd);
    expect(viaFinished.usage).toEqual(viaRun.usage);
  });

  it("each turn of a reused session carries its OWN outcome + cost (no bleed)", async () => {
    const platform = new FakePlatform();
    const handle = await platform.aex.sessions.create({ model: "anthropic/claude-haiku-4-5" });

    const t1 = (await platform.send(handle, "turn one", { text: "one", costUsd: 0.01, outcome: "succeeded" })) as {
      ok: boolean;
      status: string;
      costUsd: number;
    };
    expect(t1.ok).toBe(true);
    expect(t1.status).toBe("succeeded");
    expect(t1.costUsd).toBe(0.01);

    const t2 = (await platform.send(handle, "turn two", { outcome: "cancelled", costUsd: 0.02 })) as {
      ok: boolean;
      status: string;
      costUsd: number;
    };
    expect(t2.ok).toBe(false);
    expect(t2.status).toBe("cancelled");
    expect(t2.costUsd).toBe(0.02);
  });

  for (const held of ["suspended", "awaiting_approval"] as const) {
    it(`reports ${held} as an interrupted run while preserving thread lifecycle`, async () => {
      const platform = new FakePlatform();
      const result = await platform.start(ONE_SHOT, { outcome: "interrupted", sessionStatus: held, costUsd: 0 });
      expect(result.status).toBe("interrupted");
      expect(result.session?.status).toBe(held);
      expect(result.ok).toBe(false);
      expect(result.checkpoint?.checkpointId).toBe("cp-1");
    });
  }
});
