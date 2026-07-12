import { describe, expect, it } from "vitest";
import {
  SESSION_COST_TELEMETRY_SCHEMA_VERSION,
  SESSION_USAGE_SAMPLE_SCHEMA_VERSION,
  buildSessionCostTelemetry,
  buildSessionCostTelemetryFromUsageSamples,
  buildSessionUsageSample,
  mergeSessionCostTelemetry
} from "../src/index.js";

describe("session cost telemetry", () => {
  const privateCostFieldPattern =
    /estimatedCostUsd|unitRate|rateCard|margin|discount|providerAccount|systemKey|apiKey|reconciliation|calculator|ledgerCursor/i;

  it("records duration, file, retry, capture, and provider usage data", () => {
    const telemetry = buildSessionCostTelemetry({
      sessionId: "11111111-1111-4111-8111-111111111111",
      provider: "anthropic",
      recordedAt: "2026-06-01T00:00:00.000Z",
      durations: {
        runtimeMs: 1200,
        fileCaptureMs: 300,
        cleanupMs: 80,
        totalMs: 1580
      },
      files: {
        discoveredFiles: 3,
        capturedFiles: 2,
        failedFiles: 1,
        capturedBytes: 4096
      },
      retries: {
        runtimeAttempts: 1,
        providerPollRetries: 2,
        fileUploadRetries: 1
      },
      capture: {
        attempted: true,
        uploadedFiles: 2,
        failedFiles: 1,
        totalBytes: 4096,
        failureReasons: ["provider_file_missing"]
      },
      providerUsage: [
        {
          provider: "anthropic",
          model: "claude-haiku-4-5",
          inputTokens: 24,
          outputTokens: 11,
          cacheReadInputTokens: 0,
          cacheCreationInputTokens: 0,
          totalTokens: 35,
          sourceEventId: "evt_1",
          sourceSampleIds: ["usage-sample-1"]
        }
      ],
      storage: {
        storedBytes: 4096,
        storedFiles: 2,
        byteMilliseconds: 8192
      },
      proxy: {
        calls: 1,
        requestBytes: 128,
        responseBytes: 256,
        durationMs: 40
      }
    });

    expect(telemetry.schemaVersion).toBe(SESSION_COST_TELEMETRY_SCHEMA_VERSION);
    expect(telemetry.durations?.runtimeMs).toBe(1200);
    expect(telemetry.files?.capturedBytes).toBe(4096);
    expect(telemetry.retries?.providerPollRetries).toBe(2);
    expect(telemetry.capture?.failureReasons).toEqual(["provider_file_missing"]);
    expect(telemetry.providerUsage?.[0]?.totalTokens).toBe(35);
    expect(telemetry.providerUsage?.[0]?.sourceSampleIds).toEqual(["usage-sample-1"]);
    expect(telemetry.storage?.storedBytes).toBe(4096);
    expect(telemetry.storage?.storedFiles).toBe(2);
    expect(telemetry.proxy?.responseBytes).toBe(256);
    expect(JSON.parse(JSON.stringify(telemetry))).toEqual(telemetry);
  });

  it("requires source-linked usage samples with supported metric units", () => {
    const sample = buildSessionUsageSample({
      sampleId: "usage-sample-1",
      sessionId: "11111111-1111-4111-8111-111111111111",
      metric: "provider.total_tokens",
      quantity: 35,
      provider: "anthropic",
      model: "claude-haiku-4-5",
      source: {
        type: "coordinator-event",
        id: "evt_1",
        observedAt: "2026-06-01T00:00:00.000Z"
      },
      recordedAt: "2026-06-01T00:00:01.000Z"
    });

    expect(sample.schemaVersion).toBe(SESSION_USAGE_SAMPLE_SCHEMA_VERSION);
    expect(sample.unit).toBe("token");
    expect(sample.source.id).toBe("evt_1");
    expect(JSON.parse(JSON.stringify(sample))).toEqual(sample);

    expect(() =>
      buildSessionUsageSample({
        metric: "provider.total_tokens",
        unit: "byte",
        quantity: 1,
        source: { type: "coordinator-event", id: "evt_1" }
      })
    ).toThrow(/provider\.total_tokens must use token units/);

    expect(() =>
      buildSessionUsageSample({
        metric: "provider.total_tokens",
        quantity: 1,
        source: { type: "coordinator-event", id: "" }
      })
    ).toThrow(/source\.id must be a non-empty string/);
  });

  it("summarizes source-linked samples into public-safe telemetry", () => {
    const telemetry = buildSessionCostTelemetryFromUsageSamples({
      sessionId: "11111111-1111-4111-8111-111111111111",
      provider: "anthropic",
      status: "complete",
      recordedAt: "2026-06-01T00:00:05.000Z",
      samples: [
        {
          sampleId: "usage-provider-input",
          metric: "provider.input_tokens",
          quantity: 24,
          provider: "anthropic",
          model: "claude-haiku-4-5",
          source: { type: "coordinator-event", id: "evt_model_usage" }
        },
        {
          sampleId: "usage-provider-output",
          metric: "provider.output_tokens",
          quantity: 11,
          provider: "anthropic",
          model: "claude-haiku-4-5",
          source: { type: "coordinator-event", id: "evt_model_usage" }
        },
        {
          sampleId: "usage-provider-total",
          metric: "provider.total_tokens",
          quantity: 35,
          provider: "anthropic",
          model: "claude-haiku-4-5",
          source: { type: "coordinator-event", id: "evt_model_usage" }
        },
        {
          sampleId: "usage-runtime",
          metric: "runtime.active_ms",
          quantity: 1200,
          source: { type: "runtime-job", id: "runtime-interval-1" }
        },
        {
          sampleId: "usage-file",
          metric: "file.captured_bytes",
          quantity: 4096,
          source: { type: "file-object", id: "file_1" }
        },
        {
          sampleId: "usage-retry",
          metric: "retry.provider_poll",
          quantity: 2,
          source: { type: "session-event", id: "evt_retry_summary" }
        },
        {
          sampleId: "usage-storage",
          metric: "storage.current_bytes",
          quantity: 4096,
          source: { type: "storage-accrual", id: "storage-window-1" }
        },
        {
          sampleId: "usage-proxy-call",
          metric: "proxy.call_count",
          quantity: 1,
          source: { type: "proxy-call", id: "proxy_1" }
        },
        {
          sampleId: "usage-proxy-response",
          metric: "proxy.response_bytes",
          quantity: 256,
          source: { type: "proxy-call", id: "proxy_1" }
        }
      ]
    });

    expect(telemetry.sourceSummary?.sampleCount).toBe(9);
    expect(telemetry.sourceSummary?.metrics).toContain("provider.total_tokens");
    expect(telemetry.sourceSummary?.sourceTypes).toContain("proxy-call");
    expect(telemetry.providerUsage?.[0]).toMatchObject({
      inputTokens: 24,
      outputTokens: 11,
      totalTokens: 35,
      sourceEventId: "evt_model_usage"
    });
    expect(telemetry.providerUsage?.[0]?.sourceSampleIds).toEqual([
      "usage-provider-input",
      "usage-provider-output",
      "usage-provider-total"
    ]);
    expect(telemetry.durations?.runtimeMs).toBe(1200);
    expect(telemetry.files?.capturedBytes).toBe(4096);
    expect(telemetry.retries?.providerPollRetries).toBe(2);
    expect(telemetry.storage?.storedBytes).toBe(4096);
    expect(telemetry.proxy?.calls).toBe(1);
    expect(telemetry.proxy?.responseBytes).toBe(256);

    expect(JSON.stringify(telemetry)).not.toMatch(privateCostFieldPattern);
  });

  it("merges additive samples without mutating the existing telemetry", () => {
    const first = buildSessionCostTelemetry({
      provider: "anthropic",
      durations: { runtimeMs: 10 },
      files: { capturedBytes: 20 },
      storage: { storedBytes: 20, storedFiles: 1 },
      providerUsage: [{ provider: "anthropic", inputTokens: 1 }]
    });

    const merged = mergeSessionCostTelemetry(first, {
      durations: { runtimeMs: 5, cleanupMs: 2 },
      files: { capturedBytes: 7, failedFiles: 1 },
      storage: { storedBytes: 7, storedFiles: 2 },
      providerUsage: [{ provider: "anthropic", outputTokens: 3 }]
    });

    expect(first.durations?.runtimeMs).toBe(10);
    expect(merged.durations).toEqual({ runtimeMs: 15, cleanupMs: 2 });
    expect(merged.files).toEqual({ capturedBytes: 27, failedFiles: 1 });
    expect(merged.storage).toEqual({ storedBytes: 27, storedFiles: 3 });
    expect(merged.providerUsage).toHaveLength(2);
  });

  it("rejects negative or non-finite numbers", () => {
    expect(() => buildSessionCostTelemetry({ durations: { runtimeMs: -1 } })).toThrow(
      /runtimeMs must be a non-negative finite number/
    );
    expect(() =>
      buildSessionCostTelemetry({ providerUsage: [{ provider: "anthropic", totalTokens: Number.NaN }] })
    ).toThrow(/totalTokens must be a non-negative finite number/);
    expect(() => buildSessionCostTelemetry({ storage: { storedFiles: -1 } })).toThrow(
      /storedFiles must be a non-negative finite number/
    );
    expect(() =>
      buildSessionUsageSample({
        metric: "runtime.active_ms",
        quantity: Number.NaN,
        source: { type: "runtime-job", id: "runtime-interval-1" }
      })
    ).toThrow(/quantity must be a non-negative finite number/);
  });

  it("does not copy private pricing fields into the public summary", () => {
    const telemetry = buildSessionCostTelemetry({
      privateRateCardVersion: "rate-card-private",
      marginBasisPoints: 2000,
      providerAccountId: "acct_private",
      managedSystemKeyId: "system-key-private",
      reconciliationBatchId: "recon-private",
      providerUsage: [
        {
          provider: "anthropic",
          totalTokens: 35,
          estimatedCostUsd: 100,
          rateCardVersion: "private-rate-card",
          providerAccountId: "acct_private",
          marginBasisPoints: 2000
        } as never
      ]
    } as never);

    expect(telemetry).not.toHaveProperty("privateRateCardVersion");
    expect(telemetry).not.toHaveProperty("marginBasisPoints");
    expect(telemetry).not.toHaveProperty("providerAccountId");
    expect(telemetry).not.toHaveProperty("managedSystemKeyId");
    expect(telemetry).not.toHaveProperty("reconciliationBatchId");
    expect(telemetry.providerUsage?.[0]).not.toHaveProperty("estimatedCostUsd");
    expect(telemetry.providerUsage?.[0]).not.toHaveProperty("rateCardVersion");
    expect(telemetry.providerUsage?.[0]).not.toHaveProperty("providerAccountId");
    expect(telemetry.providerUsage?.[0]).not.toHaveProperty("marginBasisPoints");
    expect(telemetry).not.toHaveProperty("managedKey");
    expect(JSON.stringify(telemetry)).not.toMatch(privateCostFieldPattern);
  });

  it("does not project private sample internals into source-linked public telemetry", () => {
    const telemetry = buildSessionCostTelemetryFromUsageSamples({
      sessionId: "11111111-1111-4111-8111-111111111111",
      provider: "anthropic",
      samples: [
        {
          sampleId: "usage-provider-total",
          metric: "provider.total_tokens",
          quantity: 35,
          provider: "anthropic",
          model: "claude-haiku-4-5",
          source: {
            type: "usage-ledger",
            id: "usage-sample-public-id",
            providerAccountId: "acct_private",
            reconciliationCursor: "recon-private",
            unitRate: "private-rate"
          },
          providerAccountId: "acct_private",
          rateCardVersion: "private-rate-card",
          marginBasisPoints: 2000,
          systemKeyId: "system-key-private",
          reconciliationBatchId: "recon-private"
        } as never
      ]
    });

    expect(telemetry.providerUsage?.[0]).toMatchObject({
      provider: "anthropic",
      totalTokens: 35,
      sourceSampleIds: ["usage-provider-total"]
    });
    expect(telemetry).not.toHaveProperty("managedKey");
    expect(JSON.stringify(telemetry)).not.toMatch(privateCostFieldPattern);
  });
});
