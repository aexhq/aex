/** Strict v1 observation, telemetry-gap, and export contracts. */
import * as z from "zod/mini";
import { isId, type IdKind } from "./ids.js";
import { DownloadGrantSchema } from "./v1-content.js";

const timestamp = z.string().check(
  z.regex(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/)
);
const nonEmptyString = z.string().check(z.minLength(1));
const nonNegativeInteger = z.int().check(z.gte(0));
const positiveInteger = z.int().check(z.gte(1));
const sha256 = z.string().check(z.regex(/^sha256:[0-9a-f]{64}$/));

function resourceId<K extends IdKind>(kind: K) {
  return z.string().check(z.refine((value) => isId(kind, value)));
}

export const OBSERVATION_SIGNALS = [
  "events",
  "logs",
  "spans",
  "metrics",
  "traces"
] as const;
export type ObservationSignal = (typeof OBSERVATION_SIGNALS)[number];
export type ObservationScalar = string | number | boolean | null;

export type ObservationFilter =
  | { readonly op: "and" | "or"; readonly filters: readonly ObservationFilter[] }
  | { readonly op: "not"; readonly filter: ObservationFilter }
  | {
      readonly op: "eq" | "ne" | "lt" | "lte" | "gt" | "gte";
      readonly field: string;
      readonly value: ObservationScalar;
    }
  | {
      readonly op: "prefix" | "contains";
      readonly field: string;
      readonly value: string;
    }
  | { readonly op: "in" | "not_in"; readonly field: string; readonly values: readonly ObservationScalar[] }
  | { readonly op: "exists"; readonly field: string };

function isScalar(value: unknown): value is ObservationScalar {
  return value === null ||
    typeof value === "string" ||
    typeof value === "boolean" ||
    (typeof value === "number" && Number.isFinite(value));
}

function boundedString(value: unknown): boolean {
  return typeof value !== "string" || new TextEncoder().encode(value).byteLength <= 1_024;
}

function inspectFilter(
  value: unknown,
  depth = 1
): { readonly valid: boolean; readonly leaves: number } {
  if (depth > 8 || value === null || typeof value !== "object" || Array.isArray(value)) {
    return { valid: false, leaves: 0 };
  }
  const node = value as Record<string, unknown>;
  if (node.op === "and" || node.op === "or") {
    if (!Array.isArray(node.filters) || node.filters.length < 1 || node.filters.length > 16) {
      return { valid: false, leaves: 0 };
    }
    let leaves = 0;
    for (const child of node.filters) {
      const inspected = inspectFilter(child, depth + 1);
      if (!inspected.valid) return inspected;
      leaves += inspected.leaves;
    }
    return { valid: leaves <= 32, leaves };
  }
  if (node.op === "not") return inspectFilter(node.filter, depth + 1);
  if (typeof node.field !== "string" || node.field.length === 0 || !boundedString(node.field)) {
    return { valid: false, leaves: 0 };
  }
  if (node.op === "exists") {
    return { valid: Object.keys(node).length === 2, leaves: 1 };
  }
  if (node.op === "in" || node.op === "not_in") {
    const values = node.values;
    return {
      valid: Array.isArray(values) && values.length <= 100 &&
        values.every((entry) => isScalar(entry) && boundedString(entry)) &&
        Object.keys(node).length === 3,
      leaves: 1
    };
  }
  if (node.op === "prefix" || node.op === "contains") {
    return {
      valid: typeof node.value === "string" && boundedString(node.value) &&
        Object.keys(node).length === 3,
      leaves: 1
    };
  }
  if (["eq", "ne", "lt", "lte", "gt", "gte"].includes(String(node.op))) {
    return {
      valid: isScalar(node.value) && boundedString(node.value) &&
        Object.keys(node).length === 3,
      leaves: 1
    };
  }
  return { valid: false, leaves: 0 };
}

export const ObservationFilterSchema = z.unknown().check(
  z.refine((value) => inspectFilter(value).valid)
);

export interface ObservationQuery {
  readonly signals?: readonly ObservationSignal[];
  readonly where?: ObservationFilter;
  readonly timeRange?: { readonly gte?: string; readonly lt?: string };
  readonly order?: {
    readonly by: "time" | "accepted";
    readonly direction: "asc" | "desc";
  };
  readonly limit?: number;
  readonly cursor?: string;
  readonly consistency?: "available" | {
    readonly mode: "caught_up";
    readonly waitMs?: number;
  };
}
export type ObservationOrder = NonNullable<ObservationQuery["order"]>;

function withinEncodedByteLimit(value: unknown, limit: number): boolean {
  try {
    return new TextEncoder().encode(JSON.stringify(value)).byteLength <= limit;
  } catch {
    return false;
  }
}

const timeRange = z.strictObject({
  gte: z.optional(timestamp),
  lt: z.optional(timestamp)
});
const queryBase = {
  signals: z.optional(z.array(z.enum(OBSERVATION_SIGNALS))),
  where: z.optional(ObservationFilterSchema),
  timeRange: z.optional(timeRange),
  order: z.optional(z.strictObject({
    by: z.enum(["time", "accepted"]),
    direction: z.enum(["asc", "desc"])
  })),
  limit: z.optional(z.int().check(z.gte(1), z.lte(1_000))),
  consistency: z.optional(z.union([
    z.literal("available"),
    z.strictObject({
      mode: z.literal("caught_up"),
      waitMs: z.optional(nonNegativeInteger)
    })
  ]))
} as const;

export const ObservationQuerySchema = z.strictObject({
  ...queryBase,
  cursor: z.optional(z.string().check(z.regex(/^cur_/)))
}).check(z.refine((value) => withinEncodedByteLimit(value, 65_536)));

export const StreamOriginSchema = z.union([
  z.strictObject({ cursor: z.string().check(z.regex(/^cur_/)) }),
  z.strictObject({ time: timestamp }),
  z.strictObject({ earliest: z.literal(true) })
]);
export type StreamOrigin = z.infer<typeof StreamOriginSchema>;

export const ObservationStreamRequestSchema = z.strictObject({
  ...queryBase,
  origin: StreamOriginSchema
}).check(z.refine((value) => withinEncodedByteLimit(value, 65_536)));
export const ObservationListenRequestSchema = z.strictObject(queryBase)
  .check(z.refine((value) => withinEncodedByteLimit(value, 65_536)));
export type ObservationStreamRequest = Omit<ObservationQuery, "cursor"> & {
  readonly origin: StreamOrigin;
};
export type ObservationListenRequest = Omit<ObservationQuery, "cursor">;

export const ObservationWatermarkSchema = z.strictObject({
  cursor: z.string().check(z.regex(/^cur_/)),
  time: timestamp
});
export type ObservationWatermark = z.infer<typeof ObservationWatermarkSchema>;
export const MissingIntervalSchema = z.strictObject({
  gte: timestamp,
  lt: timestamp,
  signals: z.optional(z.array(z.enum(OBSERVATION_SIGNALS)))
});
export const ObservationCoverageSchema = z.strictObject({
  snapshot: ObservationWatermarkSchema,
  accepted: ObservationWatermarkSchema,
  indexed: ObservationWatermarkSchema,
  earliestReplay: ObservationWatermarkSchema,
  caughtUp: z.boolean(),
  complete: z.boolean(),
  missingIntervals: z.array(MissingIntervalSchema),
  unboundedGaps: nonNegativeInteger
});
export type ObservationCoverage = z.infer<typeof ObservationCoverageSchema>;

const observationBase = {
  id: resourceId("observation"),
  revision: positiveInteger,
  workspaceId: resourceId("workspace"),
  sessionId: z.optional(resourceId("session")),
  runId: z.optional(resourceId("run")),
  agentId: z.optional(resourceId("agent")),
  operationId: z.optional(resourceId("operation")),
  time: timestamp,
  acceptedAt: timestamp,
  attributes: z.optional(z.record(z.string(), z.union([
    z.string(), z.number(), z.boolean(), z.null()
  ])))
} as const;

export const TraceIdSchema = z.string().check(z.regex(/^[0-9a-f]{32}$/));
export const SpanIdSchema = z.string().check(z.regex(/^[0-9a-f]{16}$/));

export const TraceSummarySchema = z.strictObject({
  id: resourceId("observation"),
  revision: positiveInteger,
  workspaceId: z.optional(resourceId("workspace")),
  sessionId: resourceId("session"),
  traceId: TraceIdSchema,
  state: z.enum(["active", "quiescent"]),
  rootName: nonEmptyString,
  durationNs: nonNegativeInteger,
  spanCount: nonNegativeInteger,
  errorCount: nonNegativeInteger,
  serviceName: nonEmptyString,
  time: timestamp,
  acceptedAt: timestamp
});
export type TraceSummary = z.infer<typeof TraceSummarySchema>;

export const ObservationSchema = z.discriminatedUnion("signal", [
  z.strictObject({
    signal: z.literal("events"),
    ...observationBase,
    type: nonEmptyString,
    name: z.optional(nonEmptyString),
    source: z.optional(nonEmptyString),
    outcome: z.optional(nonEmptyString),
    messageId: z.optional(resourceId("message")),
    toolCallId: z.optional(resourceId("toolCall"))
  }),
  z.strictObject({
    signal: z.literal("logs"),
    ...observationBase,
    severityNumber: nonNegativeInteger,
    severityText: z.optional(z.string()),
    body: z.string(),
    stream: z.optional(z.enum(["stdout", "stderr"])),
    logger: z.optional(z.string()),
    traceId: z.optional(TraceIdSchema),
    spanId: z.optional(SpanIdSchema)
  }),
  z.strictObject({
    signal: z.literal("spans"),
    ...observationBase,
    traceId: TraceIdSchema,
    spanId: SpanIdSchema,
    parentSpanId: z.optional(SpanIdSchema),
    name: nonEmptyString,
    kind: nonEmptyString,
    status: nonEmptyString,
    durationNs: nonNegativeInteger,
    serviceName: z.optional(nonEmptyString),
    scopeName: z.optional(nonEmptyString)
  }),
  z.strictObject({
    signal: z.literal("metrics"),
    ...observationBase,
    name: nonEmptyString,
    kind: nonEmptyString,
    unit: z.optional(z.string()),
    temporality: z.optional(z.enum(["delta", "cumulative"])),
    monotonic: z.optional(z.boolean()),
    value: z.number()
  }),
  z.strictObject({
    signal: z.literal("traces"),
    ...observationBase,
    sessionId: resourceId("session"),
    traceId: TraceIdSchema,
    state: z.enum(["active", "quiescent"]),
    rootName: nonEmptyString,
    durationNs: nonNegativeInteger,
    spanCount: nonNegativeInteger,
    errorCount: nonNegativeInteger,
    serviceName: nonEmptyString
  })
]);
export type Observation = z.infer<typeof ObservationSchema>;

export const ObservationPageSchema = z.strictObject({
  items: z.array(ObservationSchema),
  nextCursor: z.optional(z.string().check(z.regex(/^cur_/))),
  coverage: ObservationCoverageSchema
});
export type ObservationPage = z.infer<typeof ObservationPageSchema>;

export const ObservationFrameSchema = z.discriminatedUnion("type", [
  z.strictObject({
    type: z.literal("records"),
    records: z.array(ObservationSchema).check(z.maxLength(200)),
    cursor: z.string().check(z.regex(/^cur_/))
  }),
  z.strictObject({
    type: z.literal("gap"),
    gapId: resourceId("telemetryGap"),
    cursor: z.string().check(z.regex(/^cur_/))
  }),
  z.strictObject({
    type: z.literal("checkpoint"),
    cursor: z.string().check(z.regex(/^cur_/))
  }),
  z.strictObject({
    type: z.literal("rotate"),
    cursor: z.string().check(z.regex(/^cur_/))
  })
]).check(z.refine((value) => withinEncodedByteLimit(value, 1_048_576)));
export type ObservationFrame = z.infer<typeof ObservationFrameSchema>;

function fixedIntervalMilliseconds(value: unknown): number | undefined {
  if (typeof value !== "string") return undefined;
  const match = /^([1-9]\d*)(s|m|h|d)$/.exec(value);
  if (match === null) return undefined;
  const amount = Number(match[1]);
  if (!Number.isSafeInteger(amount)) return undefined;
  const unit = match[2] as "s" | "m" | "h" | "d";
  const milliseconds = amount * {
    s: 1_000,
    m: 60_000,
    h: 3_600_000,
    d: 86_400_000
  }[unit];
  return milliseconds >= 1_000 && milliseconds <= 30 * 86_400_000
    ? milliseconds
    : undefined;
}

function validMetricAggregationWindow(value: unknown): boolean {
  if (value === null || typeof value !== "object") return false;
  const request = value as {
    readonly interval?: unknown;
    readonly timeRange?: { readonly gte?: unknown; readonly lt?: unknown };
  };
  const interval = fixedIntervalMilliseconds(request.interval);
  if (
    interval === undefined ||
    typeof request.timeRange?.gte !== "string" ||
    typeof request.timeRange.lt !== "string"
  ) {
    return false;
  }
  const gte = Date.parse(request.timeRange.gte);
  const lt = Date.parse(request.timeRange.lt);
  return Number.isFinite(gte) && Number.isFinite(lt) && lt > gte &&
    Math.ceil((lt - gte) / interval) <= 10_000;
}

export const MetricAggregationRequestSchema = z.strictObject({
  name: nonEmptyString,
  timeRange: z.strictObject({ gte: timestamp, lt: timestamp }),
  interval: nonEmptyString,
  groupBy: z.optional(z.array(nonEmptyString).check(z.maxLength(8))),
  calculations: z.array(z.union([
    z.strictObject({ op: z.enum(["count", "sum", "min", "max", "mean", "increase", "rate"]) }),
    z.strictObject({
      op: z.literal("quantile"),
      q: z.number().check(z.gt(0), z.lt(1))
    })
  ])).check(z.minLength(1), z.maxLength(10)),
  where: z.optional(ObservationFilterSchema)
}).check(
  z.refine(validMetricAggregationWindow),
  z.refine((value) => withinEncodedByteLimit(value, 65_536))
);
export type MetricAggregationRequest = z.infer<typeof MetricAggregationRequestSchema>;
export type MetricAggregationCalculation =
  MetricAggregationRequest["calculations"][number];

export const MetricAggregationPageSchema = z.strictObject({
  items: z.array(z.strictObject({
    time: timestamp,
    value: z.number(),
    groups: z.record(z.string(), z.union([z.string(), z.number(), z.boolean(), z.null()])),
    calculation: nonEmptyString,
    approximate: z.optional(z.boolean())
  })).check(z.maxLength(10_000)),
  nextCursor: z.optional(z.string().check(z.regex(/^cur_/))),
  coverage: ObservationCoverageSchema
});
export type MetricAggregationPage = z.infer<typeof MetricAggregationPageSchema>;
export type MetricAggregationGroup = MetricAggregationPage["items"][number];

export const TelemetryGapSchema = z.strictObject({
  id: resourceId("telemetryGap"),
  revision: positiveInteger,
  workspaceId: resourceId("workspace"),
  sessionId: z.optional(resourceId("session")),
  runId: z.optional(resourceId("run")),
  agentId: z.optional(resourceId("agent")),
  signals: z.array(z.enum(OBSERVATION_SIGNALS)).check(z.minLength(1)),
  status: z.enum(["pending_retry", "open", "repaired"]),
  ordinalRange: z.optional(z.strictObject({
    start: nonNegativeInteger,
    endExclusive: positiveInteger
  })),
  cursorRange: z.optional(z.strictObject({
    start: nonEmptyString,
    endExclusive: nonEmptyString
  })),
  timeRange: z.optional(z.strictObject({ gte: timestamp, lt: timestamp })),
  attemptedRecords: nonNegativeInteger,
  attemptedBytes: nonNegativeInteger,
  reason: z.enum([
    "producer_backpressure",
    "ingress_unavailable",
    "replay_expired",
    "pipeline_loss"
  ]),
  recoverable: z.boolean(),
  repair: z.optional(z.strictObject({
    repairedAt: timestamp,
    source: nonEmptyString
  })),
  createdAt: timestamp,
  updatedAt: timestamp
});
export type TelemetryGap = z.infer<typeof TelemetryGapSchema>;

export const TelemetryGapQuerySchema = z.strictObject({
  signals: z.optional(z.array(z.enum(OBSERVATION_SIGNALS))),
  status: z.optional(z.enum(["pending_retry", "open", "repaired"])),
  timeRange: z.optional(timeRange),
  cursor: z.optional(z.string().check(z.regex(/^cur_/))),
  limit: z.optional(z.int().check(z.gte(1), z.lte(1_000)))
});
export type TelemetryGapQuery = z.infer<typeof TelemetryGapQuerySchema>;

export const TelemetryExportRequestSchema = z.strictObject({
  query: ObservationQuerySchema,
  format: z.enum(["ndjson", "parquet", "otlp_json"]),
  completeness: z.enum(["require", "allow_gaps"])
});
export type TelemetryExportRequest = z.infer<typeof TelemetryExportRequestSchema>;
export type TelemetryExportFormat = TelemetryExportRequest["format"];
export type TelemetryExportCompleteness = TelemetryExportRequest["completeness"];

export const TelemetryExportSchema = z.discriminatedUnion("status", [
  z.strictObject({
    id: resourceId("export"),
    status: z.literal("preparing"),
    format: z.enum(["ndjson", "parquet", "otlp_json"]),
    createdAt: timestamp,
    updatedAt: timestamp
  }),
  z.strictObject({
    id: resourceId("export"),
    status: z.literal("ready"),
    format: z.enum(["ndjson", "parquet", "otlp_json"]),
    manifestHash: sha256,
    createdAt: timestamp,
    updatedAt: timestamp,
    expiresAt: timestamp
  }),
  z.strictObject({
    id: resourceId("export"),
    status: z.literal("expired"),
    format: z.enum(["ndjson", "parquet", "otlp_json"]),
    createdAt: timestamp,
    updatedAt: timestamp,
    expiresAt: timestamp
  }),
  z.strictObject({
    id: resourceId("export"),
    status: z.literal("revoked"),
    format: z.enum(["ndjson", "parquet", "otlp_json"]),
    createdAt: timestamp,
    updatedAt: timestamp,
    revokedAt: timestamp
  })
]);
export type TelemetryExport = z.infer<typeof TelemetryExportSchema>;

export const TelemetryExportDownloadGrantSchema = DownloadGrantSchema;
