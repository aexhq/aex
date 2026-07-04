/**
 * `renderEnvelope` — one-line projections. Focus: the `aex.session.idle`
 * custom must surface a non-default park reason (e.g. `cancel_requested`),
 * otherwise a cancelled turn renders identically to a completed one and
 * `aex cancel` looks like a no-op in `aex tail` / `aex inspect`.
 */
import { describe, expect, it } from "vitest";
import type { AexEvent } from "@aexhq/contracts";
import { renderEnvelope } from "../src/host/stream-render.js";

const custom = (data: Record<string, unknown>, message?: string): AexEvent =>
  ({
    specversion: "1.0",
    id: "run-x:6",
    source: "runtime",
    type: "CUSTOM",
    subject: "run-x",
    time: new Date(6).toISOString(),
    sequence: 6,
    data: data as AexEvent["data"],
    ...(message !== undefined ? { message } : {})
  }) as AexEvent;

describe("renderEnvelope CUSTOM aex.session.idle", () => {
  it("appends a non-default park reason", () => {
    const line = renderEnvelope(
      custom(
        { name: "aex.session.idle", value: { state: "idle", reason: "cancel_requested", turnSeq: 1 } },
        "session idle"
      )
    );
    expect(line).toBe("[aex] session idle (cancel_requested)");
  });

  it("stays terse for a normal completion", () => {
    const line = renderEnvelope(
      custom({ name: "aex.session.idle", value: { state: "idle", reason: "completed", turnSeq: 1 } }, "session idle")
    );
    expect(line).toBe("[aex] session idle");
  });

  it("tolerates a missing reason", () => {
    const line = renderEnvelope(custom({ name: "aex.session.idle", value: { state: "idle" } }, "session idle"));
    expect(line).toBe("[aex] session idle");
  });
});
