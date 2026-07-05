/**
 * BLACKBOX — the terminal-boundary / await-settle class (WS1 H4·T1, WS3 T2·T3·T4).
 *
 * Every assertion here is what a customer sees through the public `aex.run()` /
 * `session.send().done()` surface against a realistic platform. The findings this
 * pins closed:
 *   H4/T1  a cancelled or timed-out run must NOT read as a clean idle/success.
 *   T1     a clean one-shot reports the OUTCOME `succeeded`, never a bare `idle`,
 *          while the resumable session RECORD keeps its `idle` lifecycle status.
 *   T2/T3  cost is ALWAYS present at the settled boundary — even a $0 turn.
 *   T4     usage is ALWAYS present, derived from the server's provider usage.
 *   parity done() == run(): the same settled shape from both surfaces.
 */
import { describe, expect, it } from "vitest";
import type { SessionRunOptions } from "../../../src/index.js";
import { FakePlatform } from "./fake-platform.js";

const ONE_SHOT: SessionRunOptions = { model: "claude-haiku-4-5", message: "do the thing", apiKeys: { anthropic: "sk-ant" } };

describe("blackbox: await-settle terminal boundary", () => {
  it("a clean one-shot reports outcome 'succeeded' with cost + usage always present", async () => {
    const platform = new FakePlatform();
    const result = await platform.run(ONE_SHOT, {
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
  });

  it("a $0 turn still settles with costUsd:0 and empty usage — no hang, never undefined", async () => {
    const platform = new FakePlatform();
    const result = await platform.run(ONE_SHOT, { text: "cheap", outcome: "succeeded", costUsd: 0 });
    expect(result.ok).toBe(true);
    expect(result.costUsd).toBe(0);
    expect(result.usage).toEqual({});
  });

  it("a CANCELLED run reports outcome 'cancelled' + ok:false — NOT a clean idle", async () => {
    const platform = new FakePlatform();
    const result = await platform.run(ONE_SHOT, { outcome: "cancelled", costUsd: 0.004 });
    expect(result.status).toBe("cancelled");
    expect(result.ok).toBe(false);
    // Byte-different from a succeeded run — the H4 core.
    expect(result.status).not.toBe("succeeded");
    expect(result.status).not.toBe("idle");
  });

  it("a wall-clock TIMEOUT reports the typed outcome 'timed_out' + ok:false", async () => {
    const platform = new FakePlatform();
    const result = await platform.run(ONE_SHOT, { outcome: "timed_out", costUsd: 0.004 });
    expect(result.status).toBe("timed_out");
    expect(result.ok).toBe(false);
  });

  it("a FAILED run reports outcome 'failed' + the terminal error text", async () => {
    const platform = new FakePlatform();
    const result = await platform.run(ONE_SHOT, { outcome: "failed", errorMessage: "invalid provider api key" });
    expect(result.ok).toBe(false);
    expect(result.status).toBe("failed");
    expect(result.error).toBe("invalid provider api key");
  });

  it("done() and run() return the SAME settled shape for the same terminal", async () => {
    const run = new FakePlatform();
    const viaRun = await run.run(ONE_SHOT, { text: "hi", costUsd: 0.002, usage: { inputTokens: 4, outputTokens: 1 } });

    const send = new FakePlatform();
    const handle = await send.aex.openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
    const viaDone = (await send.send(handle, "hi", {
      text: "hi",
      costUsd: 0.002,
      usage: { inputTokens: 4, outputTokens: 1 }
    })) as { ok: boolean; status: string; costUsd: number; usage: unknown };

    // The done() turn result carries the same settled fields run() reshapes from.
    expect(viaDone.ok).toBe(viaRun.ok);
    expect(viaDone.status).toBe(viaRun.status);
    expect(viaDone.costUsd).toBe(viaRun.costUsd);
    expect(viaDone.usage).toEqual(viaRun.usage);
  });

  it("each turn of a reused session carries its OWN outcome + cost (no bleed)", async () => {
    const platform = new FakePlatform();
    const handle = await platform.aex.openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });

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

  it("run() AWAITS the settle commit — a lagged settle still yields cost + outcome, and it polled", async () => {
    const platform = new FakePlatform();
    // settleLag: the park fires with the record UNSETTLED; only a later poll
    // returns the settled record. A caller that returned at the park (the pre-fix
    // behavior) would read no cost/outcome — so this proves run() awaits settle.
    const result = await platform.run(ONE_SHOT, {
      text: "lagged",
      costUsd: 0.005,
      usage: { inputTokens: 3 },
      settleLag: true
    });
    expect(result.ok).toBe(true);
    expect(result.status).toBe("succeeded");
    expect(result.costUsd).toBe(0.005);
    expect(result.usage.inputTokens).toBe(3);
    // It polled GET /api/sessions/:id until the record settled (never one-and-done).
    const settlePolls = platform.requests.filter((r) => /^GET \/api\/sessions\/[^/?]+$/.test(r)).length;
    expect(settlePolls).toBeGreaterThanOrEqual(2);
  });
});
