import { describe, expect, it } from "bun:test";
import {
  AEX_API_ERROR_CODES,
  MetricAggregationRequestSchema,
  ObservationFrameSchema,
  ObservationListenRequestSchema,
  ObservationQuerySchema,
  ObservationStreamRequestSchema,
  REGIONAL_API_ROUTE_DESCRIPTORS,
  TelemetryExportRequestSchema,
  TelemetryExportSchema,
  TelemetryGapSchema,
  TraceSummarySchema,
  newId
} from "../src/index.js";

const at = "2026-07-30T10:00:00.000Z";
const hash = `sha256:${"a".repeat(64)}`;

function accepts(schema: { safeParse(value: unknown): { success: boolean } }, value: unknown): boolean {
  return schema.safeParse(value).success;
}

describe("v1 bounded observation protocol", () => {
  it("accepts the typed filter AST and rejects unbounded operators/depth", () => {
    expect(accepts(ObservationQuerySchema, {
      signals: ["events", "logs"],
      where: {
        op: "and",
        filters: [
          { op: "eq", field: "sessionId", value: newId("session") },
          { op: "in", field: "severityText", values: ["WARN", "ERROR"] }
        ]
      },
      timeRange: { gte: at },
      order: { by: "accepted", direction: "desc" },
      consistency: { mode: "caught_up", waitMs: 1000 },
      limit: 100
    })).toBe(true);
    expect(accepts(ObservationQuerySchema, {
      where: { op: "regex", field: "body", value: ".*" }
    })).toBe(false);
    expect(accepts(ObservationQuerySchema, {
      where: { op: "prefix", field: "body", value: true }
    })).toBe(false);
    let filter: unknown = { op: "exists", field: "body" };
    for (let index = 0; index < 9; index += 1) filter = { op: "not", filter };
    expect(accepts(ObservationQuerySchema, { where: filter })).toBe(false);
    expect(accepts(ObservationQuerySchema, {
      where: {
        op: "in",
        field: "body",
        values: Array.from({ length: 100 }, () => "x".repeat(800))
      }
    })).toBe(false);
  });

  it("requires exactly one stream origin and models all NDJSON frames", () => {
    expect(accepts(ObservationStreamRequestSchema, {
      signals: ["logs"],
      origin: { earliest: true }
    })).toBe(true);
    expect(accepts(ObservationStreamRequestSchema, { signals: ["logs"] })).toBe(false);
    expect(accepts(ObservationStreamRequestSchema, {
      origin: { cursor: "cur_1", time: at }
    })).toBe(false);
    expect(accepts(ObservationListenRequestSchema, {
      origin: { earliest: true }
    })).toBe(false);
    for (const frame of [
      { type: "records", records: [], cursor: "cur_1" },
      { type: "cursor", cursor: "cur_1" },
      { type: "rotate", cursor: "cur_1" },
      { type: "gap", gapId: newId("telemetryGap"), cursor: "cur_1" }
    ]) {
      expect(accepts(ObservationFrameSchema, frame)).toBe(true);
    }
    expect(accepts(ObservationFrameSchema, {
      type: "records",
      records: Array.from({ length: 201 }, () => ({
        signal: "logs",
        id: newId("observation"),
        revision: 1,
        workspaceId: newId("workspace"),
        time: at,
        acceptedAt: at,
        severityNumber: 9,
        body: "bounded"
      })),
      cursor: "cur_1"
    })).toBe(false);
  });
});

describe("v1 metrics, traces, gaps, and exports", () => {
  it("pins bounded metric aggregation and W3C trace identities", () => {
    expect(accepts(MetricAggregationRequestSchema, {
      name: "http.server.duration",
      timeRange: { gte: at, lt: "2026-07-30T11:00:00.000Z" },
      interval: "60s",
      groupBy: ["serviceName"],
      calculations: [{ op: "mean" }, { op: "quantile", q: 0.95 }]
    })).toBe(true);
    for (const interval of ["500ms", "31d"]) {
      expect(accepts(MetricAggregationRequestSchema, {
        name: "http.server.duration",
        timeRange: { gte: at, lt: "2026-07-30T11:00:00.000Z" },
        interval,
        calculations: [{ op: "mean" }]
      })).toBe(false);
    }
    expect(accepts(MetricAggregationRequestSchema, {
      name: "http.server.duration",
      timeRange: { gte: at, lt: "2026-07-30T11:00:00.000Z" },
      interval: "1s",
      calculations: [{ op: "mean" }]
    })).toBe(true);
    expect(accepts(MetricAggregationRequestSchema, {
      name: "http.server.duration",
      timeRange: { gte: at, lt: "2026-07-30T13:00:00.000Z" },
      interval: "1s",
      calculations: [{ op: "mean" }]
    })).toBe(false);
    expect(accepts(TraceSummarySchema, {
      id: newId("observation"),
      revision: 2,
      sessionId: newId("session"),
      traceId: "0123456789abcdef0123456789abcdef",
      state: "quiescent",
      rootName: "request",
      durationNs: 1000,
      spanCount: 2,
      errorCount: 0,
      serviceName: "api",
      time: at,
      acceptedAt: at
    })).toBe(true);
    expect(accepts(TraceSummarySchema, {
      traceId: "not-w3c"
    })).toBe(false);
  });

  it("pins revisioned gaps and require|allow_gaps export completeness", () => {
    expect(accepts(TelemetryGapSchema, {
      id: newId("telemetryGap"),
      revision: 1,
      workspaceId: newId("workspace"),
      signals: ["logs"],
      status: "open",
      reason: "pipeline_loss",
      recoverable: false,
      attemptedRecords: 2,
      attemptedBytes: 100,
      createdAt: at,
      updatedAt: at
    })).toBe(true);
    expect(accepts(TelemetryExportRequestSchema, {
      query: { signals: ["logs"] },
      format: "ndjson",
      completeness: "require"
    })).toBe(true);
    expect(accepts(TelemetryExportSchema, {
      id: newId("export"),
      status: "ready",
      format: "ndjson",
      manifestHash: hash,
      createdAt: at,
      updatedAt: at,
      expiresAt: "2026-07-31T10:00:00.000Z"
    })).toBe(true);
  });
});

describe("v1 telemetry route and error authority", () => {
  it("pins read-only POSTs, operation-only export, and idempotent downloads/OTLP", () => {
    const selected = new Set([
      "telemetry.query", "telemetry.stream", "telemetry.listen",
      "session.telemetry.query", "session.telemetry.stream", "session.telemetry.listen",
      "telemetry.otlp.logs", "telemetry.exports.create",
      "telemetry.exports.download", "telemetry.gaps.query"
    ]);
    expect(REGIONAL_API_ROUTE_DESCRIPTORS
      .filter(({ name }) => selected.has(name))
      .map(({ name, idempotency }) => [name, idempotency])).toEqual([
      ["telemetry.query", "none"],
      ["telemetry.stream", "none"],
      ["telemetry.listen", "none"],
      ["session.telemetry.query", "none"],
      ["session.telemetry.stream", "none"],
      ["session.telemetry.listen", "none"],
      ["telemetry.otlp.logs", "idempotency-key"],
      ["telemetry.gaps.query", "none"],
      ["telemetry.exports.create", "operation-id"],
      ["telemetry.exports.download", "idempotency-key"]
    ]);
  });

  it("declares the stable telemetry failures and no ticket/archive route", () => {
    for (const code of [
      "invalid_cursor",
      "invalid_query",
      "invalid_metric_aggregation",
      "invalid_telemetry",
      "telemetry_payload_too_large",
      "telemetry_incomplete",
      "unsupported_export_signal",
      "export_not_ready",
      "export_expired",
      "export_revoked"
    ] as const) {
      expect(AEX_API_ERROR_CODES).toContain(code);
    }
    const routes = REGIONAL_API_ROUTE_DESCRIPTORS
      .map(({ samplePath }) => samplePath)
      .join("\n");
    for (const removed of ["/ticket", "/archive", "/otel", "/follow"]) {
      expect(routes).not.toContain(removed);
    }
  });
});
