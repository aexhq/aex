import { describe, expect, it } from "vitest";
import * as contracts from "../src/index.js";
import type { AexEvent } from "../src/index.js";

type OtlpSignal = "traces" | "logs";
type PublicProjector = (
  events: readonly AexEvent[],
  options: { readonly signal: OtlpSignal }
) => unknown;

const TRACE_ID = "0123456789abcdef0123456789abcdef";
const SPAN_ID = "0123456789abcdef";
const PARENT_SPAN_ID = "fedcba9876543210";

function projector(): PublicProjector {
  const candidate = (contracts as unknown as { readonly toOTLP?: unknown }).toOTLP;
  expect(candidate, "@aexhq/contracts must publicly export toOTLP").toBeTypeOf("function");
  if (typeof candidate !== "function") throw new TypeError("toOTLP is not implemented");
  return candidate as PublicProjector;
}

function envelope(overrides: Partial<AexEvent> = {}): AexEvent {
  return {
    specversion: "1.0",
    id: "evt-7",
    source: "runtime",
    type: "CUSTOM",
    subject: "ses_public",
    threadId: "ses_public",
    runId: "ses_public:turn:3",
    time: "2026-07-22T12:00:00.000Z",
    sequence: 7,
    data: { name: "aex.step", value: { turnSeq: 3 } },
    ...overrides,
    // These are additive public envelope fields. Keep the cast local until the
    // production contract lands; this red test still exercises their wire use.
    traceId: TRACE_ID,
    spanId: SPAN_ID,
    parentSpanId: PARENT_SPAN_ID
  } as AexEvent;
}

describe("public OTLP/HTTP JSON projection", () => {
  it("returns a standards-pure traces request body and preserves public trace identity", () => {
    const output = projector()([envelope()], { signal: "traces" });
    const record = output as Record<string, unknown>;

    expect(Object.keys(record)).toEqual(["resourceSpans"]);
    expect(Array.isArray(record.resourceSpans)).toBe(true);
    expect(record).not.toHaveProperty("nextCursor");
    expect(record).not.toHaveProperty("events");
    expect(record).not.toHaveProperty("data");

    const json = JSON.stringify(output);
    expect(json).toContain(TRACE_ID);
    expect(json).toContain(SPAN_ID);
    expect(json).toContain(PARENT_SPAN_ID);
    expect(json).toContain("aex.visibility");
    expect(json).toContain("external");
  });

  it("returns a standards-pure logs request body with OTLP AnyValue fields", () => {
    const output = projector()([
      envelope({
        channel: "log",
        type: "LOG",
        level: "warn",
        message: "tool output was truncated",
        data: { level: "warn", message: "tool output was truncated", fields: { callId: "4:tool-1" } }
      })
    ], { signal: "logs" });
    const record = output as Record<string, unknown>;

    expect(Object.keys(record)).toEqual(["resourceLogs"]);
    expect(Array.isArray(record.resourceLogs)).toBe(true);
    expect(record).not.toHaveProperty("nextCursor");
    const json = JSON.stringify(output);
    expect(json).toContain("tool output was truncated");
    expect(json).toContain("WARN");
    expect(json).toContain(TRACE_ID);
    expect(json).toContain(SPAN_ID);
  });

  it("fails closed over public inputs instead of exposing internal or secret-shaped fields", () => {
    const output = projector()([
      envelope({
        data: {
          name: "aex.step",
          value: {
            turnSeq: 3,
            apiKey: "sk-secret-canary-should-never-escape",
            prompt: "private-prompt-canary",
            providerDeployment: "private-deployment-canary",
            callbackUrl: "https://private.example/callback?token=secret"
          }
        }
      })
    ], { signal: "traces" });

    const json = JSON.stringify(output);
    for (const forbidden of [
      "sk-secret-canary-should-never-escape",
      "private-prompt-canary",
      "private-deployment-canary",
      "private.example",
      "apiKey",
      "providerDeployment",
      "callbackUrl"
    ]) {
      expect(json).not.toContain(forbidden);
    }
  });
});
