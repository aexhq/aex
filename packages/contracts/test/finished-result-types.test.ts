/**
 * WS3 class-killer: the unified finished-result type has non-optional
 * costUsd/usage/terminal-status, so a path that forgets to populate them fails
 * typecheck. Plus the UsageSummary projector from providerUsage.
 */
import { describe, expect, it } from "vitest";
import { usageFromProviderUsage, type TurnResult, type UsageSummary, type SessionCostProviderUsage } from "../src/index.js";

describe("TurnResult non-optional terminal fields (WS3)", () => {
  it("[compile-time] TurnResult requires costUsd, usage, and a terminal status", () => {
    const ok: TurnResult = { status: "succeeded", ok: true, costUsd: 0, usage: {} };
    expect(ok.costUsd).toBe(0);
    // @ts-expect-error — omitting costUsd/usage/status fails to typecheck
    const missing: TurnResult = { ok: true };
    void missing;
  });

  it("[compile-time] status must be a terminal OUTCOME, not a lifecycle status", () => {
    // @ts-expect-error — 'idle' is a lifecycle status, not a SessionTerminalOutcome
    const bad: TurnResult = { status: "idle", ok: true, costUsd: 0, usage: {} };
    void bad;
  });
});

describe("usageFromProviderUsage projector (WS3)", () => {
  it("sums each token field across provider entries", () => {
    const providerUsage: SessionCostProviderUsage[] = [
      { provider: "deepseek", inputTokens: 10, outputTokens: 5, totalTokens: 15 },
      { provider: "deepseek", inputTokens: 3, outputTokens: 2, totalTokens: 5 }
    ];
    const usage: UsageSummary = usageFromProviderUsage(providerUsage);
    expect(usage).toEqual({ inputTokens: 13, outputTokens: 7, totalTokens: 20 });
  });

  it("keeps a field absent when no entry carried it", () => {
    expect(usageFromProviderUsage([{ provider: "deepseek", inputTokens: 4 }])).toEqual({ inputTokens: 4 });
  });

  it("returns an empty summary for absent/empty providerUsage", () => {
    expect(usageFromProviderUsage(undefined)).toEqual({});
    expect(usageFromProviderUsage([])).toEqual({});
  });
});
