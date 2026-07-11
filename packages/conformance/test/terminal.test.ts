import { describe, expect, it } from "vitest";
import { expectTerminalEvent } from "../src/terminal.js";

const finished = {
  type: "RUN_FINISHED",
  data: {
    outcome: "succeeded",
    checkpoint: { checkpointId: "cp_1" },
    costUsd: 0.01,
    providerUsage: []
  }
};

describe("expectTerminalEvent", () => {
  it("returns a checkpoint-consistent RUN_FINISHED", () => {
    const terminal = expectTerminalEvent(
      [{ type: "RUN_STARTED", data: {} }, finished],
      { outcome: "succeeded" }
    );
    expect(terminal).toBe(finished);
  });

  it("requires per-run billing on RUN_ERROR", () => {
    expect(() => expectTerminalEvent([
      { type: "RUN_ERROR", data: { outcome: "failed", failureClass: "setup_failed" } }
    ], { outcome: "failed", failureClass: "setup_failed" })).toThrow(/costUsd/);
    expect(() => expectTerminalEvent([
      {
        type: "RUN_ERROR",
        data: { outcome: "failed", failureClass: "setup_failed", costUsd: 0, providerUsage: [] }
      }
    ], { outcome: "failed", failureClass: "setup_failed" })).not.toThrow();
  });

  it("rejects missing or duplicate terminals", () => {
    expect(() => expectTerminalEvent([{ type: "RUN_STARTED", data: {} }], { outcome: "succeeded" }))
      .toThrow(/no terminal/);
    expect(() => expectTerminalEvent([finished, finished], { outcome: "succeeded" }))
      .toThrow(/2 terminal events/);
  });

  it("rejects outcome mismatches", () => {
    expect(() => expectTerminalEvent([finished], { outcome: "cancelled" }))
      .toThrow(/outcome="succeeded" but expected "cancelled"/);
  });

  it("enforces terminal kind and outcome pairing", () => {
    expect(() => expectTerminalEvent([{
      type: "RUN_ERROR",
      data: { outcome: "cancelled", costUsd: 0, providerUsage: [] }
    }], { outcome: "cancelled" })).toThrow(/RUN_ERROR must carry outcome="failed"/);
    expect(() => expectTerminalEvent([{
      type: "RUN_FINISHED",
      data: {
        outcome: "failed",
        checkpoint: { checkpointId: "cp_1" },
        costUsd: 0,
        providerUsage: []
      }
    }], { outcome: "failed" })).toThrow(/must use RUN_ERROR/);
  });

  it("requires checkpoint, cost, and usage on RUN_FINISHED", () => {
    expect(() => expectTerminalEvent([
      { type: "RUN_FINISHED", data: { outcome: "succeeded", costUsd: 0, providerUsage: [] } }
    ], { outcome: "succeeded" })).toThrow(/committed checkpoint/);
    expect(() => expectTerminalEvent([
      { type: "RUN_FINISHED", data: { outcome: "succeeded", checkpoint: { checkpointId: "cp_1" }, providerUsage: [] } }
    ], { outcome: "succeeded" })).toThrow(/costUsd/);
    expect(() => expectTerminalEvent([
      { type: "RUN_FINISHED", data: { outcome: "succeeded", checkpoint: { checkpointId: "cp_1" }, costUsd: 0 } }
    ], { outcome: "succeeded" })).toThrow(/providerUsage/);
  });

  it("interpolates context into failures", () => {
    expect(() => expectTerminalEvent([finished], {
      outcome: "failed",
      context: "deepseek-managed"
    })).toThrow(/\[deepseek-managed\]/);
  });
});
