/** `renderEnvelope` one-line projections for the canonical run lifecycle. */
import { describe, expect, it } from "vitest";
import type { AexEvent } from "@aexhq/contracts";
import { renderEnvelope } from "../src/host/stream-render.js";

function event(type: AexEvent["type"], data: Record<string, unknown>, message?: string): AexEvent {
  return {
    specversion: "1.0",
    id: "session-x:6",
    source: "runtime",
    type,
    subject: "session-x",
    threadId: "session-x",
    runId: "run-1",
    time: new Date(6).toISOString(),
    sequence: 6,
    data: data as AexEvent["data"],
    ...(message !== undefined ? { message } : {})
  };
}

describe("renderEnvelope run terminals", () => {
  it("renders successful completion from RUN_FINISHED", () => {
    expect(renderEnvelope(event("RUN_FINISHED", { outcome: "succeeded" }))).toBe("✓ run finished");
  });

  it("renders a non-success outcome from RUN_FINISHED", () => {
    expect(renderEnvelope(event("RUN_FINISHED", { outcome: "cancelled" }))).toBe("✓ run finished (cancelled)");
  });

  it("renders RUN_ERROR with its public failure message", () => {
    expect(renderEnvelope(event("RUN_ERROR", { outcome: "failed" }, "provider unavailable"))).toBe(
      "✗ run error: provider unavailable"
    );
  });

  it("does not treat CUSTOM events as run completion", () => {
    expect(renderEnvelope(event("CUSTOM", { name: "aex.notification" }, "runtime notice"))).toBe(
      "[aex] runtime notice"
    );
  });
});
