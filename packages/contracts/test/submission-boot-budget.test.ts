import { describe, expect, it } from "vitest";
import { parseRunSubmissionRequest } from "../src/index.js";
// `sessionBudgetLimits` is the platform-facing boot helper (it rides the
// `export * from "./submission.js"` re-export, like parseRunLimits). Pin it
// through the `@aexhq/contracts/internal` subpath the private runtime consumes.
import { sessionBudgetLimits, type RunLimits } from "../src/internal.js";

function baseRequest() {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider: "anthropic" as const,
    submission: {
      model: "claude-haiku-4-5",
      prompt: ["hello"],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: { apiKeys: { anthropic: "sk-anthropic-test" } }
  };
}

// A deliberately small per-run cap so the assertions read as a "kill this run
// almost immediately" budget. End-to-end enforcement/termination lives in the
// private container supervisor + session planner (which read
// `sessionConfig.limits.budgetUsd`); this suite proves the contract flow —
// wire `limits.maxSpendUsd` → boot `budgetUsd` — that the live kill depends on.
const SMALL_CAP_USD = 0.05;

describe("sessionBudgetLimits (wire maxSpendUsd → boot budgetUsd)", () => {
  it("maps a set spend cap to the boot budgetUsd field", () => {
    const limits: RunLimits = { maxSpendUsd: SMALL_CAP_USD };
    expect(sessionBudgetLimits(limits)).toEqual({ budgetUsd: SMALL_CAP_USD });
  });

  it("preserves a fractional USD amount verbatim (no rounding)", () => {
    expect(sessionBudgetLimits({ maxSpendUsd: 2.5 })).toEqual({ budgetUsd: 2.5 });
  });

  it("returns an empty (spread-safe) fragment when no cap is set", () => {
    expect(sessionBudgetLimits(undefined)).toEqual({});
    expect(sessionBudgetLimits({})).toEqual({});
    expect(sessionBudgetLimits({ maxConcurrentChildRuns: 4 })).toEqual({});
  });

  it("ignores the sibling lineage dials — only the spend cap becomes budgetUsd", () => {
    expect(
      sessionBudgetLimits({ maxConcurrentChildRuns: 2, maxSubagentDepth: 3, maxSpendUsd: SMALL_CAP_USD })
    ).toEqual({ budgetUsd: SMALL_CAP_USD });
  });

  it("names the boot field exactly `budgetUsd` (the field the session planner reads)", () => {
    expect(Object.keys(sessionBudgetLimits({ maxSpendUsd: SMALL_CAP_USD }))).toEqual(["budgetUsd"]);
  });
});

describe("spend cap carries through the full submission → boot flow", () => {
  it("a submitted overrides.maxSpendUsd survives parse and becomes boot budgetUsd", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      limits: { maxSpendUsd: SMALL_CAP_USD }
    });
    // 1. The parser surfaces the cap on the validated request (unchanged behaviour).
    expect(parsed.limits).toEqual({ maxSpendUsd: SMALL_CAP_USD });
    // 2. The boot session-config limits fragment names it `budgetUsd` — this is
    //    the value the private supervisor/planner enforce the run against. Before
    //    this wiring the cap was dropped at boot build and the advertised cap did
    //    nothing.
    expect(sessionBudgetLimits(parsed.limits)).toEqual({ budgetUsd: SMALL_CAP_USD });
  });

  it("no cap submitted ⇒ no boot budget (run stays unbounded per-run)", () => {
    const parsed = parseRunSubmissionRequest(baseRequest());
    expect(parsed.limits).toBeUndefined();
    expect(sessionBudgetLimits(parsed.limits)).toEqual({});
  });
});
