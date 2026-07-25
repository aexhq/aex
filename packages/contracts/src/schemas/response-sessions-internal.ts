/**
 * Response schemas for the three session routes that are JSON but are not part
 * of the bearer-key SDK surface: the OTLP telemetry pull, and the two
 * writer-token hops the in-container runtime uses to read a child's result and
 * to finalize it.
 *
 * They are separated from `response-sessions.ts` because they have a different
 * audience and a different failure mode: nothing in this repository's
 * `HttpClient` call sites can produce them, so they are permanently
 * "unexercised" in a C4 report. `testing/response-bindings.ts` names that fact
 * rather than leaving a reader to infer thin coverage.
 *
 * The OTLP shapes are the ONE place where the wire is standards-defined rather
 * than ours: they mirror `otlp-projection.ts`, which is itself the declaration
 * of what `toOTLP` emits, so this schema and that type must agree exactly.
 */
import * as z from "zod/mini";
import { SESSION_TERMINAL_OUTCOMES } from "../status.js";
import { CheckpointRevisionSchema } from "./response-sessions.js";
import {
  describeResponse,
  responseObject,
  wireEnum,
  wireLiteral,
  wireNonEmptyString,
  wireNonNegativeInteger,
  wireNonNegativeNumber,
  wireString
} from "./response-common.js";

const optional = z.optional;

// ===========================================================================
// GET /sessions/{id}/result — writer-token child result
// ===========================================================================

/**
 * A child file as the result route projects it.
 *
 * NOTE the key is `path`, not `filename` — this is NOT the
 * `publicFileFromObject` shape the public `/files` routes emit. Two projections
 * of one concept, differing in one key name.
 */
const ChildResultFileSchema = responseObject({
  id: wireNonEmptyString,
  path: wireString,
  checkpointId: wireNonEmptyString,
  sizeBytes: wireNonNegativeInteger,
  sha256: wireString,
  contentType: wireString
});

const ChildResultPendingSchema = responseObject({
  id: wireNonEmptyString,
  state: wireEnum(["settling", "running", "queued"]),
  files: z.array(ChildResultFileSchema)
});

const ChildResultFinishedSchema = responseObject({
  id: wireNonEmptyString,
  state: wireLiteral("finished"),
  outcome: wireEnum(SESSION_TERMINAL_OUTCOMES),
  files: z.array(ChildResultFileSchema),
  text: optional(wireString),
  failureClass: optional(wireString),
  failureMessage: optional(wireString),
  conflicts: optional(
    z.array(
      responseObject({
        path: wireString,
        kind: wireEnum(["stale", "blocked"]),
        resolved: wireLiteral(false)
      })
    )
  )
});

export const ChildResultResponseSchema = describeResponse(
  "ChildResultResponse",
  "A subagent child's result: still in flight, or finished with its outcome and files.",
  z.union([ChildResultPendingSchema, ChildResultFinishedSchema], {
    error: 'child result must be state "settling"/"running"/"queued", or "finished" with an outcome'
  })
);

// ===========================================================================
// POST /sessions/{id}/finalize — writer-token child settle hop
// ===========================================================================

export const ChildFinalizeResponseSchema = describeResponse(
  "ChildFinalizeResponse",
  "The settled child's terminal facts. No `session` envelope — this route is not " +
    "shaped like the public session routes.",
  responseObject({
    id: wireNonEmptyString,
    lifecycleStatus: wireEnum(["idle", "error"]),
    outcome: wireEnum(SESSION_TERMINAL_OUTCOMES),
    checkpoint: optional(CheckpointRevisionSchema),
    failureClass: optional(wireString),
    failureMessage: optional(wireString),
    childCostUsd: wireNonNegativeNumber,
    providerUsage: z.array(z.record(wireString, z.unknown()))
  })
);

// ===========================================================================
// GET /sessions/{id}/otel — standards-pure OTLP/HTTP JSON
// ===========================================================================

/**
 * OTLP `AnyValue`, restricted to the scalars the projection emits.
 *
 * A strict union rather than a loose object: OTLP's own `AnyValue` admits
 * arrays and kvlists, and `otlp-projection.ts` deliberately emits neither.
 */
const OtlpAnyValueSchema = z.union(
  [
    responseObject({ stringValue: wireString }),
    responseObject({ boolValue: z.boolean() }),
    responseObject({ intValue: wireString }),
    responseObject({ doubleValue: z.number() })
  ],
  { error: "OTLP AnyValue must be one of stringValue, boolValue, intValue, doubleValue" }
);

const OtlpKeyValueSchema = responseObject({ key: wireString, value: OtlpAnyValueSchema });
const OtlpResourceSchema = responseObject({ attributes: z.array(OtlpKeyValueSchema) });
const OtlpScopeSchema = responseObject({
  name: wireString,
  version: optional(wireString)
});

const OtlpTraceSpanSchema = responseObject({
  traceId: wireString,
  spanId: wireString,
  parentSpanId: optional(wireString),
  name: wireString,
  kind: wireLiteral(1),
  startTimeUnixNano: wireString,
  endTimeUnixNano: wireString,
  attributes: z.array(OtlpKeyValueSchema),
  status: responseObject({ code: z.union([wireLiteral(1), wireLiteral(2)]) })
});

const OtlpLogRecordSchema = responseObject({
  timeUnixNano: wireString,
  observedTimeUnixNano: wireString,
  severityNumber: z.union([wireLiteral(9), wireLiteral(13), wireLiteral(17)]),
  severityText: wireEnum(["INFO", "WARN", "ERROR"]),
  body: OtlpAnyValueSchema,
  attributes: z.array(OtlpKeyValueSchema),
  traceId: optional(wireString),
  spanId: optional(wireString)
});

const OtlpTracesSchema = responseObject({
  resourceSpans: z.array(
    responseObject({
      resource: OtlpResourceSchema,
      scopeSpans: z.array(
        responseObject({ scope: OtlpScopeSchema, spans: z.array(OtlpTraceSpanSchema) })
      )
    })
  )
});

const OtlpLogsSchema = responseObject({
  resourceLogs: z.array(
    responseObject({
      resource: OtlpResourceSchema,
      scopeLogs: z.array(
        responseObject({ scope: OtlpScopeSchema, logRecords: z.array(OtlpLogRecordSchema) })
      )
    })
  )
});

/**
 * The OTLP body, keyed by the requested signal.
 *
 * A union of two SINGLE-KEY strict objects is the schema form of what
 * `assertStandardsPureOtlpBody` asserts by hand: exactly one root key, and no
 * aex pagination smuggled into a standards body — the cursor rides in the
 * `x-aex-next-cursor` HEADER, which this schema cannot see and must therefore
 * not tolerate in the body.
 */
export const SessionOtlpResponseSchema = describeResponse(
  "SessionOtlpResponse",
  "A standards-pure OTLP/HTTP JSON export body: `resourceSpans` for traces, " +
    "`resourceLogs` for logs, and nothing else. Pagination is a response header.",
  z.union([OtlpTracesSchema, OtlpLogsSchema], {
    error: "session OTLP response must contain exactly one of resourceSpans or resourceLogs"
  })
);
