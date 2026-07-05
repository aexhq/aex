/**
 * BLACKBOX — schema-decode structured output (WS8 / T8).
 *
 * The finding: a run submitted with a `json_schema` `responseFormat` must return a
 * TYPED decode outcome — `{ kind:'decoded', value }` on a conforming output or
 * `{ kind:'refused', reason }` on a violation — never an untyped, possibly
 * hallucinated object silently passed through. Both branches asserted through the
 * public `aex.run<T>()` surface.
 */
import { describe, expect, it } from "vitest";
import { FakePlatform } from "./fake-platform.js";

interface Sentiment {
  readonly label: string;
  readonly score: number;
}

const RESPONSE_FORMAT = { kind: "json_schema" as const, schema: { type: "object" } };

describe("blackbox: structured-output decode", () => {
  it("returns a typed decoded value for a conforming output", async () => {
    const platform = new FakePlatform();
    const result = await platform.run<Sentiment>(
      {
        model: "claude-haiku-4-5",
        message: "classify sentiment",
        apiKeys: { anthropic: "sk-ant" },
        responseFormat: RESPONSE_FORMAT
      },
      {
        custom: [{ type: "CUSTOM", data: { name: "aex.result.decoded", value: { label: "positive", score: 0.92 } } }],
        outcome: "succeeded",
        costUsd: 0.01
      }
    );

    expect(result.ok).toBe(true);
    expect(result.outcome).toEqual({ kind: "decoded", value: { label: "positive", score: 0.92 } });
    // The decoded value is typed T — a compile-time contract, exercised at runtime.
    const decoded = result.outcome?.kind === "decoded" ? result.outcome.value : undefined;
    expect(decoded?.label).toBe("positive");
    // responseFormat is LOAD-BEARING: the SDK forwarded it on the create request
    // (a regression that drops it would leave the run undecoded server-side).
    const create = platform.requestLog.find((r) => r.method === "POST" && r.path.endsWith("/api/sessions"));
    expect(JSON.stringify(create?.body)).toContain("json_schema");
  });

  it("returns a TYPED refusal (not a garbage object) when the output violates the schema", async () => {
    const platform = new FakePlatform();
    const result = await platform.run<Sentiment>(
      {
        model: "claude-haiku-4-5",
        message: "classify sentiment",
        apiKeys: { anthropic: "sk-ant" },
        responseFormat: RESPONSE_FORMAT
      },
      {
        custom: [
          {
            type: "CUSTOM",
            data: { name: "aex.result.refused", value: { reason: "schema_violation", detail: "score missing" } }
          }
        ],
        outcome: "succeeded",
        costUsd: 0.01
      }
    );

    expect(result.outcome).toEqual({ kind: "refused", reason: "schema_violation", detail: "score missing" });
  });

  it("SANITIZES an out-of-vocabulary refusal reason to the safe 'refused' (never leaks it raw)", async () => {
    const platform = new FakePlatform();
    const result = await platform.run<Sentiment>(
      {
        model: "claude-haiku-4-5",
        message: "classify sentiment",
        apiKeys: { anthropic: "sk-ant" },
        responseFormat: RESPONSE_FORMAT
      },
      {
        // An arbitrary/untrusted reason the model or a compromised path might emit.
        custom: [{ type: "CUSTOM", data: { name: "aex.result.refused", value: { reason: "meltdown" } } }],
        outcome: "succeeded",
        costUsd: 0.01
      }
    );
    // Coerced to the closed vocabulary — a raw 'meltdown' never reaches the caller.
    expect(result.outcome).toEqual({ kind: "refused", reason: "refused" });
  });
});
