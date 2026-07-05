/**
 * BLACKBOX — batch + fire-and-forget submit (WS10 T14 / T14b).
 *
 * The findings this pins closed:
 *   T14   `aex.batch()` returns EVERY item's settled result plus an HONEST
 *         cost/usage rollup — a failed item lands in `failed[]`, never a silent
 *         `$0` success, and no item's cost is dropped from the total.
 *   T14b  `aex.submit()` is a distinct NON-blocking verb: it resolves the runId
 *         WITHOUT awaiting settle (the honest counterpart to await-settle run()).
 */
import { describe, expect, it } from "vitest";
import type { SessionRunOptions } from "../../../src/index.js";
import { FakePlatform } from "./fake-platform.js";

const item = (message: string): SessionRunOptions => ({
  model: "claude-haiku-4-5",
  message,
  apiKeys: { anthropic: "sk-ant" }
});

describe("blackbox: batch rollup honesty", () => {
  it("returns every item's outcome + cost and a real rollup — failures are not silent $0 successes", async () => {
    const platform = new FakePlatform();
    const result = await platform.batch([item("a"), item("b"), item("c")], [
      { outcome: "succeeded", costUsd: 0.01, usage: { inputTokens: 10, outputTokens: 2 } },
      { outcome: "failed", errorMessage: "provider exploded", costUsd: 0.002 },
      { outcome: "cancelled", costUsd: 0.003 }
    ]);

    expect(result.results.length).toBe(3);
    // Every item carries its OWN settled fields.
    for (const r of result.results) {
      expect(typeof r.runId).toBe("string");
      expect(typeof r.costUsd).toBe("number");
    }
    const byOutcome = Object.fromEntries(result.results.map((r) => [r.status, r]));
    expect(byOutcome.succeeded?.ok).toBe(true);
    expect(byOutcome.failed?.ok).toBe(false);
    expect(byOutcome.cancelled?.ok).toBe(false);

    // The rollup is HONEST: every cost is summed, ok/failed partitioned.
    expect(result.totalCostUsd).toBeCloseTo(0.015, 10);
    expect(result.okCount).toBe(1);
    expect(result.failed.map((f) => f.status).sort()).toEqual(["cancelled", "failed"]);
    // A failed item is never a silent $0 success — it kept its cost AND is in failed[].
    expect(byOutcome.failed?.costUsd).toBe(0.002);
  });
});

describe("blackbox: fire-and-forget submit", () => {
  it("resolves the runId WITHOUT awaiting settle (no turn stream, no settle poll)", async () => {
    const platform = new FakePlatform();
    const submitted = await platform.aex.submit(item("kick it off"));

    expect(typeof submitted.runId).toBe("string");
    expect(submitted.session).toBeDefined();
    // Non-blocking: only the create was issued — no settle GET, no event ticket.
    expect(platform.requests.filter((r) => r.startsWith("POST /api/sessions")).length).toBe(1);
    expect(platform.requests.some((r) => r.includes("/events/ticket"))).toBe(false);
    expect(platform.requests.some((r) => r.startsWith("GET /api/sessions/"))).toBe(false);
  });
});
