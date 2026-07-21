import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/http.js";
import { whoami } from "../src/operations.js";
import { validateRunnerEventBatch, RUNNER_EVENT_VERSION } from "../src/runner-event.js";
import { SessionStateError } from "../src/sdk-errors.js";
import { parseSubmission } from "../src/submission.js";

function submission(overrides: Readonly<Record<string, unknown>> = {}): Record<string, unknown> {
  return {
    model: "claude-haiku-4-5",
    prompt: ["hello"],
    assets: { files: [], skills: [], tools: [], instructions: [] },
    mcpServers: [],
    builtinTools: "default",
    ...overrides
  };
}

const limits = {
  maxConcurrentSessions: 50,
  submitRatePerMinute: 120,
  spendCapUsd: 250,
  monthSpendUsd: 12.5,
  balanceUsd: 100,
  balanceGraceFloorUsd: 0,
  balanceGateActive: true,
  paymentMethodStatus: "none",
  planKey: "free",
  accountType: "standard",
  subscriptionStatus: "none",
  subscriptionGate: "ok"
} as const;

function clientReturning(body: unknown): HttpClient {
  return new HttpClient({
    baseUrl: "https://api.example.test",
    apiKey: "test-token",
    fetch: async () => new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" }
    })
  });
}

describe("shared value guard parser goldens", () => {
  it("preserves submission enum and recursive JSON diagnostics", () => {
    expect(() => parseSubmission(submission({
      environment: { networking: { mode: "closed" } }
    }))).toThrowError("submission.environment.networking.mode must be one of: limited, open");

    expect(() => parseSubmission(submission({
      metadata: { first: { nested: Number.NaN }, second: undefined }
    }))).toThrowError("submission.metadata.first must be JSON-serializable");
  });

  it("preserves runner Result errors, additive acceptance, freezing, and key order", () => {
    expect(validateRunnerEventBatch({
      v: RUNNER_EVENT_VERSION,
      sessionId: "ses_guard",
      events: [{ seq: 1, tMs: 2, kind: "future_kind", data: {} }]
    })).toEqual({
      ok: false,
      code: "invalid_event",
      message: "events[0].kind must be one of: runtime_started, assistant_text, tool_request, tool_response, skill_loaded, file_uploaded, notification, stream_error, runtime_terminal (got \"future_kind\")"
    });

    expect(validateRunnerEventBatch({
      v: RUNNER_EVENT_VERSION,
      sessionId: "ses_guard",
      events: [{ seq: 1, tMs: 2, kind: "notification", data: { nested: Number.NaN } }]
    })).toEqual({
      ok: false,
      code: "invalid_event",
      message: "events[0].data must be JSON-serializable"
    });

    const result = validateRunnerEventBatch({
      v: RUNNER_EVENT_VERSION,
      sessionId: "ses_guard",
      futureEnvelope: true,
      events: [{
        seq: 1,
        tMs: 2,
        sourceSeq: 3,
        emittedAt: 4,
        kind: "notification",
        data: { value: true },
        futureEvent: "retained only on the raw input"
      }]
    });
    expect(result.ok).toBe(true);
    if (!result.ok) throw new Error(result.message);
    expect(Object.keys(result.batch)).toEqual(["v", "sessionId", "events"]);
    expect(Object.keys(result.batch.events[0]!)).toEqual([
      "seq", "tMs", "sourceSeq", "emittedAt", "kind", "data"
    ]);
    expect(result.batch.events[0]).toEqual({
      seq: 1,
      tMs: 2,
      sourceSeq: 3,
      emittedAt: 4,
      kind: "notification",
      data: { value: true }
    });
    expect(Object.isFrozen(result.batch.events[0]!.data)).toBe(true);
  });

  it("preserves whoami error identity, additive projection, and key order", async () => {
    const invalid = whoami(clientReturning({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_guard",
      scopes: [],
      limits: { ...limits, planKey: "enterprise" }
    }));
    await expect(invalid).rejects.toEqual(expect.objectContaining({
      name: "SessionStateError",
      message: "whoami response limits.planKey is invalid"
    }));
    await expect(invalid).rejects.toBeInstanceOf(SessionStateError);

    const parsed = await whoami(clientReturning({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_guard",
      scopes: ["sessions:read"],
      futureTopLevel: "ignored",
      limits: { ...limits, futureBurstWindow: 30 }
    }));
    expect(Object.keys(parsed)).toEqual([
      "ok", "principalType", "workspaceId", "scopes", "limits"
    ]);
    expect(Object.keys(parsed.limits)).toEqual([
      "maxConcurrentSessions",
      "submitRatePerMinute",
      "spendCapUsd",
      "monthSpendUsd",
      "balanceUsd",
      "balanceGraceFloorUsd",
      "balanceGateActive",
      "paymentMethodStatus",
      "planKey",
      "accountType",
      "subscriptionStatus",
      "subscriptionGate"
    ]);
    expect(parsed.limits).toEqual(limits);
  });
});
