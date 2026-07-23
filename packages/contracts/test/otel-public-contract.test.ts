import { describe, expect, it } from "bun:test";
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

function projectedAttributes(output: unknown, signal: OtlpSignal): ReadonlyArray<Record<string, unknown>> {
  const body = output as {
    readonly resourceSpans?: ReadonlyArray<{
      readonly scopeSpans: ReadonlyArray<{
        readonly spans: ReadonlyArray<{ readonly attributes: ReadonlyArray<Record<string, unknown>> }>;
      }>;
    }>;
    readonly resourceLogs?: ReadonlyArray<{
      readonly scopeLogs: ReadonlyArray<{
        readonly logRecords: ReadonlyArray<{ readonly attributes: ReadonlyArray<Record<string, unknown>> }>;
      }>;
    }>;
  };
  return signal === "traces"
    ? body.resourceSpans?.[0]?.scopeSpans[0]?.spans[0]?.attributes ?? []
    : body.resourceLogs?.[0]?.scopeLogs[0]?.logRecords[0]?.attributes ?? [];
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

  it("projects an external log stream onto the standard log.iostream attribute", () => {
    const output = projector()([
      envelope({
        channel: "log",
        type: "LOG",
        level: "info",
        message: "tool stdout",
        data: {
          level: "info",
          message: "tool stdout",
          fields: { stream: "stdout" }
        }
      })
    ], { signal: "logs" });
    const attributes = projectedAttributes(output, "logs");

    expect(attributes).toContainEqual({
      key: "log.iostream",
      value: { stringValue: "stdout" }
    });
    expect(attributes.map((attribute) => attribute.key)).not.toContain("stream");
  });

  it("projects a public tool exit code onto the AEX-owned namespaced attribute", () => {
    const output = projector()([
      envelope({
        source: "agent",
        type: "TOOL_CALL_RESULT",
        data: {
          id: "4:tool-1",
          content: "ok",
          exitCode: 23
        }
      })
    ], { signal: "traces" });
    const attributes = projectedAttributes(output, "traces");

    expect(attributes).toContainEqual({
      key: "aex.tool.exit_code",
      value: { intValue: "23" }
    });
    expect(attributes.map((attribute) => attribute.key)).not.toContain("exitCode");
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
