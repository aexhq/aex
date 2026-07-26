/**
 * Response schemas for the `sessions.*` family.
 *
 * **These describe the WIRE, not the client types.** `Session` in
 * `runtime-types.ts` is a POST-NORMALISATION shape: `operations.ts` folds the
 * flat `runtimeKind` / `runtimeSize` the server sends into a grouped
 * `runtime: { kind, size }` before a caller sees it. The C4 harness observes the
 * bytes BEFORE that fold (`HttpClient` reports from inside `request()`), so a
 * schema built from `Session` verbatim would reject every real response. The
 * shapes here are the server's, and every place they and the declared type
 * disagree is called out at the field.
 */
import * as z from "zod/mini";
import { SESSION_LIFECYCLE_STATUSES, SESSION_TERMINAL_OUTCOMES } from "../status.js";
import { SESSION_RUN_PHASES } from "../runtime-types.js";
import { PROVIDER_FAULT_KINDS } from "../provider-fault.js";
import { AEX_EVENT_CHANNELS, AEX_LOG_LEVELS } from "../event-envelope.js";
import { RUNTIME_KINDS } from "./runtime-kind.js";
import { RUNTIME_SIZES } from "./runtime-sizes.js";
import {
  describeResponse,
  responseObject,
  wireBoolean,
  wireEnum,
  wireInteger,
  wireLiteral,
  wireNonEmptyString,
  wireNonNegativeInteger,
  wireNonNegativeNumber,
  wireNumber,
  wirePositiveInteger,
  wireString,
  wireTimestamp
} from "./response-common.js";

const optional = z.optional;

/** An open JSON payload: an object whose members this contract does not fix. */
const openObject = z.record(wireString, z.unknown());

// ===========================================================================
// Building blocks
// ===========================================================================

/** Public identity of one immutable session-files checkpoint. */
export const CheckpointRevisionSchema = describeResponse(
  "CheckpointRevision",
  "Identity of one complete, immutable session-files checkpoint.",
  responseObject({
    checkpointId: wireNonEmptyString,
    runId: wireNonEmptyString,
    turnSeq: wirePositiveInteger,
    committedAt: wireTimestamp,
    throughSeq: wireNonNegativeInteger
  })
);

/**
 * Redaction-safe provider detail for the most recent failed run.
 *
 * `kind` is deliberately an open token rather than the closed
 * {@link PROVIDER_FAULT_KINDS} set: `parseProviderFault` accepts a syntactically
 * valid future kind on purpose (forward compatibility), so the schema must too
 * or C4 would fail on the very case the parser was written to survive.
 */
export const ProviderFaultSchema = describeResponse(
  "ProviderFault",
  `Terminal upstream-provider fault. Known kinds: ${PROVIDER_FAULT_KINDS.join(", ")}; ` +
    "an unknown but syntactically valid token is accepted for forward compatibility.",
  responseObject({
    kind: wireNonEmptyString,
    provider: optional(wireNonEmptyString),
    status: optional(wireInteger),
    retryAfterMs: optional(wireNonNegativeInteger),
    message: optional(wireNonEmptyString)
  })
);

const CostBasisSchema = responseObject({
  currency: wireLiteral("USD"),
  status: wireEnum(["estimated", "reconciled"])
});

const ProviderUsageSchema = responseObject({
  // Optional because the BUILDER makes it optional: `providerUsageFromManifestUsage`
  // spreads `...(provider ? { provider } : {})`, so an entry with no provider is
  // constructible and reaches the wire. Requiring it here would fail C4 on a real
  // response — the schema describes what the server sends, not what we wish it sent.
  provider: optional(wireNonEmptyString),
  model: optional(wireString),
  inputTokens: optional(wireNonNegativeNumber),
  outputTokens: optional(wireNonNegativeNumber),
  cacheReadInputTokens: optional(wireNonNegativeNumber),
  cacheCreationInputTokens: optional(wireNonNegativeNumber),
  totalTokens: optional(wireNonNegativeNumber),
  sourceEventId: optional(wireString),
  sourceSampleIds: optional(z.array(wireString))
});

/**
 * Cost/usage telemetry attached to a settled session.
 *
 * The KEY SET is the union of two disagreeing statements: `SessionCostTelemetry`
 * in `session-cost.ts` (our declared type) and the object `settle.ts` actually
 * writes. The declared type does not carry `basis`, `durationMs`, `turnSeq`,
 * `runtimeSize`, `runtimeKind`, `model`, `usage`, `byteCounts`, `childCostUsd`,
 * `childSessionCount` or `childProviderUsage`; settle writes all of them. Both
 * are declared here so the strict check is true against reality, and the
 * divergence is reported rather than papered over.
 *
 * `usage` and `byteCounts` are the one place this family stops at "is an
 * object": their members are assembled from provider manifests and were not
 * verified against a real response, and asserting a shape we have not seen would
 * be worse than saying so.
 */
export const CostTelemetrySchema = describeResponse(
  "CostTelemetry",
  "Showback cost and usage telemetry for a settled session or turn.",
  responseObject({
    schemaVersion: wireLiteral(1),
    sessionId: optional(wireString),
    provider: optional(wireString),
    model: optional(wireString),
    recordedAt: optional(wireString),
    status: optional(wireEnum(["pending", "partial", "complete", "unavailable", "not_applicable"])),
    basis: optional(wireString),
    durationMs: optional(wireNonNegativeNumber),
    turnSeq: optional(wireNonNegativeInteger),
    // Written by `telemetryFromManifest` whenever the runner manifest carries
    // one, and served on the CHILD session read via `finalizeChildSession`.
    // Undeclared here until now, which would have failed C4 on the first child
    // session settled from such a manifest — a strict schema is only as good as
    // its agreement with what the server actually writes.
    throughSeq: optional(wireNonNegativeInteger),
    runtimeSize: optional(wireString),
    runtimeKind: optional(wireString),
    billedCostUsd: optional(wireNonNegativeNumber),
    costBasis: optional(CostBasisSchema),
    usage: optional(openObject),
    byteCounts: optional(openObject),
    providerUsage: optional(z.array(ProviderUsageSchema)),
    childProviderUsage: optional(z.array(ProviderUsageSchema)),
    childCostUsd: optional(wireNonNegativeNumber),
    childSessionCount: optional(wireNonNegativeInteger),
    sourceSummary: optional(
      responseObject({
        sampleCount: wireNonNegativeNumber,
        metrics: optional(z.array(wireString)),
        sourceTypes: optional(z.array(wireString)),
        sourceSampleIds: optional(z.array(wireString))
      })
    ),
    durations: optional(
      responseObject({
        queuedMs: optional(wireNonNegativeNumber),
        runtimeMs: optional(wireNonNegativeNumber),
        fileCaptureMs: optional(wireNonNegativeNumber),
        cleanupMs: optional(wireNonNegativeNumber),
        totalMs: optional(wireNonNegativeNumber)
      })
    ),
    files: optional(
      responseObject({
        discoveredFiles: optional(wireNonNegativeNumber),
        capturedFiles: optional(wireNonNegativeNumber),
        failedFiles: optional(wireNonNegativeNumber),
        capturedBytes: optional(wireNonNegativeNumber)
      })
    ),
    retries: optional(
      responseObject({
        runtimeAttempts: optional(wireNonNegativeNumber),
        providerPollRetries: optional(wireNonNegativeNumber),
        fileCaptureRetries: optional(wireNonNegativeNumber),
        fileUploadRetries: optional(wireNonNegativeNumber)
      })
    ),
    capture: optional(
      responseObject({
        attempted: wireBoolean,
        uploadedFiles: optional(wireNonNegativeNumber),
        failedFiles: optional(wireNonNegativeNumber),
        totalBytes: optional(wireNonNegativeNumber),
        failureReasons: optional(z.array(wireString))
      })
    ),
    storage: optional(
      responseObject({
        storedBytes: optional(wireNonNegativeNumber),
        storedFiles: optional(wireNonNegativeNumber),
        byteMilliseconds: optional(wireNonNegativeNumber)
      })
    ),
    proxy: optional(
      responseObject({
        calls: optional(wireNonNegativeNumber),
        failedCalls: optional(wireNonNegativeNumber),
        requestBytes: optional(wireNonNegativeNumber),
        responseBytes: optional(wireNonNegativeNumber),
        durationMs: optional(wireNonNegativeNumber)
      })
    )
  })
);

/**
 * One run of a session.
 *
 * `eventCursor` rides only on the run returned by `sendMessage`; the
 * `currentRun` / `lastRun` projections never carry it. One schema covers both
 * because the field is optional in the declared type and a stricter split would
 * assert an absence the server has no obligation to keep.
 */
export const SessionRunSchema = describeResponse(
  "SessionRun",
  "One run (turn) of a session. `phase` is the live position; `outcome` is the terminal verdict.",
  responseObject({
    sessionId: wireNonEmptyString,
    turnSeq: wirePositiveInteger,
    runId: wireNonEmptyString,
    phase: wireEnum(SESSION_RUN_PHASES),
    outcome: optional(wireEnum(SESSION_TERMINAL_OUTCOMES)),
    startedAt: optional(wireString),
    finishedAt: optional(wireString),
    checkpoint: optional(CheckpointRevisionSchema),
    eventCursor: optional(wireNonNegativeInteger)
  })
);

/**
 * The session projection every `{ session }` envelope carries.
 *
 * This schema is the SOURCE of the `SessionWire` type in `runtime-types.ts`
 * (`z.infer`), and `Session` — the client shape — is pinned to it by a key-set
 * assertion in that module. The one deliberate difference between them:
 *
 * - **`runtime` is absent here.** The wire carries flat `runtimeKind` +
 *   `runtimeSize`; `runtime: { kind, size }` is built client-side by
 *   `normalizeSessionRuntime`.
 *
 * `usage` and `runtimeManifest` USED to be declared on both sides and are
 * emitted by neither `publicSessionFromItem` nor anything downstream of it.
 * They were kept "because our own published type declares them"; the type no
 * longer does (token counts live under `costTelemetry.providerUsage`), so the
 * justification went with it and both are gone from here too.
 */
export const SessionWireSchema = describeResponse(
  "Session",
  "A session as the data plane serves it — flat `runtimeKind`/`runtimeSize`, " +
    "before the SDK folds them into the grouped `runtime` client shape.",
  responseObject({
    id: wireNonEmptyString,
    status: wireEnum(SESSION_LIFECYCLE_STATUSES),
    acceptsMessages: wireBoolean,
    currentRun: optional(SessionRunSchema),
    lastRun: optional(SessionRunSchema),
    providerFault: optional(ProviderFaultSchema),
    idleTtl: optional(wireString),
    idleTtlMs: optional(wireNonNegativeNumber),
    runtimeSize: wireEnum(RUNTIME_SIZES),
    runtimeKind: wireEnum(RUNTIME_KINDS),
    workspaceId: optional(wireNonEmptyString),
    createdAt: optional(wireTimestamp),
    updatedAt: optional(wireTimestamp),
    idleAt: optional(z.nullable(wireString)),
    suspendedAt: optional(z.nullable(wireString)),
    activeDurationMs: optional(wireNonNegativeNumber),
    provider: optional(wireString),
    model: optional(wireString),
    retainedStorageBytes: optional(wireNonNegativeNumber),
    costUsd: optional(wireNonNegativeNumber),
    costTelemetry: optional(CostTelemetrySchema),
    errorMessage: optional(z.nullable(wireString)),
    failureClass: optional(z.nullable(wireString)),
    dataState: wireEnum(["active", "metadata_only"]),
    contentPurgedAt: optional(wireString),
    contentDeletedBy: optional(wireEnum(["retention", "user"]))
  })
);

/** One captured session file, as `publicFileFromObject` projects it. */
export const SessionFileSchema = describeResponse(
  "SessionFile",
  "One file pinned to a committed checkpoint. `filename` is the workspace-relative path.",
  responseObject({
    id: wireNonEmptyString,
    checkpointId: wireNonEmptyString,
    filename: wireString,
    sizeBytes: wireNonNegativeInteger,
    contentType: wireString,
    createdAt: wireString,
    sha256: wireString
  })
);

// ===========================================================================
// Route responses
// ===========================================================================

/**
 * `POST /sessions`, `GET /sessions/{id}`, and every state-change route.
 *
 * ONE key. `run` and `eventCursor` were declared optional here because
 * `SessionStateChangeAccepted` declared them; no handler on any of these eight
 * routes emits either — each answers `json(200, { session })` — and the client
 * type no longer claims otherwise. The run-bearing envelope is
 * {@link SessionMessageAcceptedResponseSchema}, a different route.
 */
export const SessionEnvelopeResponseSchema = describeResponse(
  "SessionEnvelopeResponse",
  "A single session, and nothing else.",
  responseObject({ session: SessionWireSchema })
);

export const SessionListResponseSchema = describeResponse(
  "SessionListResponse",
  "One page of sessions, newest first. `nextCursor` is omitted on the last page.",
  responseObject({
    sessions: z.array(SessionWireSchema),
    nextCursor: optional(wireNonEmptyString)
  })
);

export const SessionMessageAcceptedResponseSchema = describeResponse(
  "SessionMessageAcceptedResponse",
  "HTTP 202 for an accepted turn. `eventCursor` is unconditional on a fresh " +
    "accept and conditional on an idempotent replay.",
  responseObject({
    session: SessionWireSchema,
    run: SessionRunSchema,
    eventCursor: optional(wireNonNegativeInteger)
  })
);

/**
 * `DELETE /sessions/{id}` — a 200 with a body, not a 204.
 *
 * `purgedSessionFileObjects` and `cleanupComplete` are on the wire.
 * `deleteSession()` used to declare it returned `SessionStateChangeAccepted |
 * void`, which carries neither and made the body look optional; it now returns
 * `SessionDeleteAccepted`, pinned to this schema by a key-set assertion.
 */
export const SessionDeleteResponseSchema = describeResponse(
  "SessionDeleteResponse",
  "The deleted session plus the footprint-cleanup counters.",
  responseObject({
    session: SessionWireSchema,
    purgedSessionFileObjects: wireNonNegativeInteger,
    cleanupComplete: wireBoolean
  })
);

export const SessionMessagesPageResponseSchema = describeResponse(
  "SessionMessagesPageResponse",
  "One page of the projected transcript.",
  responseObject({
    messages: z.array(
      responseObject({
        id: wireNonEmptyString,
        sender: wireEnum(["user", "assistant"]),
        text: wireString,
        timestamp: wireString,
        sequence: wireNonNegativeInteger,
        content: z.array(z.unknown()),
        turnSeq: optional(wireNonNegativeInteger),
        messageId: optional(wireString)
      })
    ),
    nextCursor: optional(wireNonEmptyString)
  })
);

/**
 * One durable event.
 *
 * `type` and `source` are open vocabularies by declaration — `AexEventType` is
 * `(typeof AEX_EVENT_TYPES)[number] | string` — so pinning them here would be
 * stricter than the type it mirrors.
 */
export const AexEventSchema = describeResponse(
  "AexEvent",
  "A durable, replayable session event in the CloudEvents-shaped aex envelope.",
  responseObject({
    specversion: wireLiteral("1.0"),
    id: wireNonEmptyString,
    source: wireNonEmptyString,
    type: wireNonEmptyString,
    subject: wireNonEmptyString,
    threadId: wireNonEmptyString,
    runId: wireNonEmptyString,
    time: wireString,
    sequence: wireNonNegativeInteger,
    data: openObject,
    traceId: optional(wireString),
    spanId: optional(wireString),
    parentSpanId: optional(wireString),
    channel: optional(wireEnum(AEX_EVENT_CHANNELS)),
    sourceSeq: optional(wireNonNegativeInteger),
    emittedAt: optional(wireNumber),
    receivedAt: optional(wireNumber),
    level: optional(wireEnum(AEX_LOG_LEVELS)),
    message: optional(wireString),
    dedup: optional(responseObject({ source: wireString, sourceSeq: wireNumber })),
    ephemeral: optional(wireBoolean),
    replayable: optional(wireBoolean)
  })
);

export const SessionEventsPageResponseSchema = describeResponse(
  "SessionEventsPageResponse",
  "One page of durable session events.",
  responseObject({
    events: z.array(AexEventSchema),
    nextCursor: optional(wireNonEmptyString)
  })
);

/**
 * `POST /sessions/{id}/events/ticket`.
 *
 * `ok` and `region` are on the wire. The declared `CoordinatorTicket` interface
 * in `operations.ts` used to stop at `wsUrl` / `ticket` / `expiresAtMs`, hiding
 * the region the grant is only valid against; it now carries all five.
 */
export const CoordinatorTicketResponseSchema = describeResponse(
  "CoordinatorTicketResponse",
  "A short-lived grant for the coordinator WebSocket upgrade.",
  responseObject({
    ok: wireLiteral(true),
    wsUrl: wireNonEmptyString,
    ticket: wireNonEmptyString,
    expiresAtMs: wireNonNegativeInteger,
    region: wireNonEmptyString
  })
);

export const SessionChildrenResponseSchema = describeResponse(
  "SessionChildrenResponse",
  "A session's subagent child sessions. `createdAt`/`updatedAt` are empty strings " +
    "when the record carries none, so they are NOT validated as timestamps.",
  responseObject({
    children: z.array(
      responseObject({
        id: wireNonEmptyString,
        parentSessionId: wireNonEmptyString,
        status: wireEnum(SESSION_LIFECYCLE_STATUSES),
        createdAt: wireString,
        updatedAt: wireString,
        depth: optional(wireNonNegativeInteger),
        costUsd: optional(wireNonNegativeNumber),
        terminalAt: optional(z.nullable(wireString)),
        lastRun: optional(SessionRunSchema)
      })
    )
  })
);

export const SessionFilesResponseSchema = describeResponse(
  "SessionFilesResponse",
  "The complete file listing for one immutable checkpoint.",
  responseObject({
    revision: CheckpointRevisionSchema,
    files: z.array(SessionFileSchema)
  })
);

/**
 * `POST /sessions/{id}/files/{fileId}/link`.
 *
 * The declared `SessionFileLink` also carries `expiresAt`; the server does not
 * send one — `sessionFileLink()` synthesises it client-side from the mint time.
 */
export const SessionFileLinkResponseSchema = describeResponse(
  "SessionFileLinkResponse",
  "A presigned direct-download URL for one checkpointed file.",
  responseObject({
    url: wireNonEmptyString,
    expiresInSeconds: wirePositiveInteger,
    file: SessionFileSchema
  })
);

export const EventArchiveLinkResponseSchema = describeResponse(
  "EventArchiveLinkResponse",
  "A presigned URL for the generated event archive. No `file`, unlike the file link.",
  responseObject({
    url: wireNonEmptyString,
    expiresInSeconds: wirePositiveInteger
  })
);

/** One run-terminal webhook delivery row, session-scoped view. */
export const SessionWebhookDeliverySchema = describeResponse(
  "SessionWebhookDelivery",
  "One run-scoped webhook delivery attempt ledger row.",
  responseObject({
    id: wireNonEmptyString,
    runId: wireNonEmptyString,
    turnSeq: wirePositiveInteger,
    eventType: wireEnum(["run.finished", "run.error"]),
    status: wireEnum(["pending", "delivering", "retrying", "delivered", "exhausted", "invalid"]),
    attemptCount: wireNonNegativeInteger,
    createdAt: wireString,
    lastStatusCode: optional(wireInteger),
    lastError: optional(wireString)
  })
);

export const SessionWebhookDeliveriesResponseSchema = describeResponse(
  "SessionWebhookDeliveriesResponse",
  "Every run-terminal webhook delivery for one session.",
  responseObject({ deliveries: z.array(SessionWebhookDeliverySchema) })
);

/** `POST .../redeliver` — HTTP 202 `{ ok: true }`. */
export const AcknowledgedResponseSchema = describeResponse(
  "AcknowledgedResponse",
  "A bare acknowledgement. The only key is `ok`.",
  responseObject({ ok: wireLiteral(true) })
);
