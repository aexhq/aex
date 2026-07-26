/**
 * The session cost/usage VOCABULARY and the record shapes built from it: the
 * closed metric / unit / source-type / status sets, the usage-sample and
 * cost-telemetry interfaces, and the cost basis.
 *
 * Split out of `session-cost.ts`, which keeps the BEHAVIOUR — the builders, the
 * sample folding, the merge, and the normalizers that validate against the sets
 * declared here. Declarations do not import behaviour, so this module is a leaf
 * over `submission.js` alone and the dependency runs one way.
 *
 * Every name below is re-exported from `session-cost.ts` and therefore from the
 * package root barrel, unchanged.
 */
import type { ProviderName } from "./submission.js";

export const SESSION_COST_TELEMETRY_SCHEMA_VERSION = 1;
export const SESSION_USAGE_SAMPLE_SCHEMA_VERSION = 1;

export const SESSION_COST_SUMMARY_STATUSES = [
  "pending",
  "partial",
  "complete",
  "unavailable",
  "not_applicable"
] as const;

export type SessionCostSummaryStatus = (typeof SESSION_COST_SUMMARY_STATUSES)[number];

export const SESSION_USAGE_SAMPLE_UNITS = [
  "token",
  "millisecond",
  "byte",
  "byte_millisecond",
  "count",
  "file",
  "credit_unit"
] as const;

export type SessionUsageSampleUnit = (typeof SESSION_USAGE_SAMPLE_UNITS)[number];

export const SESSION_USAGE_SAMPLE_SOURCE_TYPES = [
  "coordinator-event",
  "session-event",
  "usage-ledger",
  "file-object",
  "proxy-call",
  "runtime-job",
  "provider-session",
  "storage-accrual",
  "manual-adjustment"
] as const;

export type SessionUsageSampleSourceType = (typeof SESSION_USAGE_SAMPLE_SOURCE_TYPES)[number];

export const SESSION_USAGE_SAMPLE_METRICS = [
  "provider.input_tokens",
  "provider.output_tokens",
  "provider.cache_read_input_tokens",
  "provider.cache_creation_input_tokens",
  "provider.total_tokens",
  "runtime.queued_ms",
  "runtime.active_ms",
  "runtime.file_capture_ms",
  "runtime.cleanup_ms",
  "session.total_ms",
  "file.discovered_files",
  "file.captured_files",
  "file.failed_files",
  "file.captured_bytes",
  "retry.runtime_attempts",
  "retry.provider_poll",
  "retry.file_capture",
  "retry.file_upload",
  "capture.uploaded_files",
  "capture.failed_files",
  "capture.total_bytes",
  "storage.current_bytes",
  "storage.byte_milliseconds",
  "proxy.call_count",
  "proxy.failed_call_count",
  "proxy.request_bytes",
  "proxy.response_bytes",
  "proxy.duration_ms"
] as const;

export type SessionUsageSampleMetric = (typeof SESSION_USAGE_SAMPLE_METRICS)[number];

export interface SessionUsageSampleSource {
  readonly type: SessionUsageSampleSourceType;
  readonly id: string;
  readonly observedAt?: string;
}

export interface SessionUsageSample {
  readonly schemaVersion: typeof SESSION_USAGE_SAMPLE_SCHEMA_VERSION;
  readonly sampleId?: string;
  readonly sessionId?: string;
  readonly metric: SessionUsageSampleMetric;
  readonly unit: SessionUsageSampleUnit;
  readonly quantity: number;
  readonly source: SessionUsageSampleSource;
  readonly provider?: ProviderName | string;
  readonly model?: string;
  readonly recordedAt?: string;
}

export type SessionUsageSampleInput = Omit<SessionUsageSample, "schemaVersion" | "unit"> & {
  readonly unit?: SessionUsageSampleUnit;
};

export interface SessionCostSourceSummary {
  readonly sampleCount: number;
  readonly metrics?: readonly SessionUsageSampleMetric[];
  readonly sourceTypes?: readonly SessionUsageSampleSourceType[];
  readonly sourceSampleIds?: readonly string[];
}

export interface SessionCostDurations {
  readonly queuedMs?: number;
  readonly runtimeMs?: number;
  readonly fileCaptureMs?: number;
  readonly cleanupMs?: number;
  readonly totalMs?: number;
}

export interface SessionCostFileTelemetry {
  readonly discoveredFiles?: number;
  readonly capturedFiles?: number;
  readonly failedFiles?: number;
  readonly capturedBytes?: number;
}

export interface SessionCostRetryTelemetry {
  readonly runtimeAttempts?: number;
  readonly providerPollRetries?: number;
  readonly fileCaptureRetries?: number;
  readonly fileUploadRetries?: number;
}

export interface SessionCostCaptureTelemetry {
  readonly attempted: boolean;
  readonly uploadedFiles?: number;
  readonly failedFiles?: number;
  readonly totalBytes?: number;
  readonly failureReasons?: readonly string[];
}

export interface SessionCostProviderUsage {
  readonly provider: ProviderName | string;
  readonly model?: string;
  readonly inputTokens?: number;
  readonly outputTokens?: number;
  readonly cacheReadInputTokens?: number;
  readonly cacheCreationInputTokens?: number;
  readonly totalTokens?: number;
  readonly sourceEventId?: string;
  readonly sourceSampleIds?: readonly string[];
}

export interface SessionCostStorageTelemetry {
  readonly storedBytes?: number;
  /** Number of files retained in the checkpoint snapshot. */
  readonly storedFiles?: number;
  readonly byteMilliseconds?: number;
}

export interface SessionCostProxyTelemetry {
  readonly calls?: number;
  readonly failedCalls?: number;
  readonly requestBytes?: number;
  readonly responseBytes?: number;
  readonly durationMs?: number;
}

/**
 * The basis for a {@link SessionCostTelemetry.billedCostUsd}: an honest marker of
 * whether the figure is a RUN-terminal ESTIMATE or has been RECONCILED against
 * authoritative actuals. Deliberately carries NO rate-card version or unit
 * rates — the platform's public-safe convention treats `rateCard`/`margin` as
 * private tokens (session-cost.test.ts privateCostFieldPattern), so the version the
 * figure was derived under stays internal (recorded only in the platform's
 * internal raw-usage export).
 */
export const SESSION_COST_BASIS_STATUSES = ["estimated", "reconciled"] as const;
export type SessionCostBasisStatus = (typeof SESSION_COST_BASIS_STATUSES)[number];

export interface SessionCostBasis {
  readonly currency: "USD";
  readonly status: SessionCostBasisStatus;
}

/**
 * How a `billedCostUsd` was arrived at — the settle-basis TYPE label, NOT the
 * runtime (the runtime is {@link SessionCostTelemetry.runtimeKind}). `settle.ts`
 * writes `"session_turn"` on the per-turn session path and `"container"` on the
 * in-process child path.
 *
 * Deliberately an open string rather than a union of those two: it is a server
 * vocabulary this package does not own, and narrowing it would make a new basis
 * label a breaking change in a consumer rather than an unrecognised value.
 */
export type SessionCostBasisLabel = string;

/**
 * Cost and usage telemetry for a settled session or turn.
 *
 * The eleven fields below `proxy` — `basis` through `childProviderUsage` — are
 * written by `settle.ts` on every settle and were undeclared here, which made
 * `costTelemetry.usage` (the token counts) unreachable without a cast even
 * though it is the ONLY place the server reports them.
 */
export interface SessionCostTelemetry {
  readonly schemaVersion: typeof SESSION_COST_TELEMETRY_SCHEMA_VERSION;
  readonly sessionId?: string;
  readonly provider?: ProviderName | string;
  readonly recordedAt?: string;
  readonly status?: SessionCostSummaryStatus;
  readonly sourceSummary?: SessionCostSourceSummary;
  readonly durations?: SessionCostDurations;
  readonly files?: SessionCostFileTelemetry;
  readonly retries?: SessionCostRetryTelemetry;
  readonly capture?: SessionCostCaptureTelemetry;
  readonly providerUsage?: readonly SessionCostProviderUsage[];
  readonly storage?: SessionCostStorageTelemetry;
  readonly proxy?: SessionCostProxyTelemetry;
  /** Which settle path produced this figure. See {@link SessionCostBasisLabel}. */
  readonly basis?: SessionCostBasisLabel;
  /** Billed compute duration for the turn, ms. */
  readonly durationMs?: number;
  /** The turn this telemetry settles. Also the per-turn fence on the Aurora upsert. */
  readonly turnSeq?: number;
  /**
   * The last event sequence the settled runner manifest covered.
   *
   * `telemetryFromManifest` writes it whenever the manifest carries one, and
   * `finalizeChildSession` puts that object on the child session read. Declared
   * here so this type and `CostTelemetrySchema` stay one statement of the shape.
   */
  readonly throughSeq?: number;
  /** The managed box preset the turn ran on. */
  readonly runtimeSize?: string;
  /** The execution backend the turn ran on (`container` | `spot_container` | `lambda`). */
  readonly runtimeKind?: string;
  /** The model the turn ran, when the session record carries one. */
  readonly model?: string;
  /**
   * The settle manifest's RAW usage counters, verbatim.
   *
   * Deliberately untyped. Its members are assembled from provider manifests and
   * have never been verified against a real response, and asserting a shape
   * nobody has observed would be worse than saying so. The response schema stops
   * at "is an object" for the same reason. Read {@link providerUsage} for the
   * token counts this package DOES declare.
   */
  readonly usage?: Readonly<Record<string, unknown>>;
  /**
   * Operational byte counters from the settle manifest (journal / backup / file
   * bytes). Untyped for the same reason as {@link usage}.
   */
  readonly byteCounts?: Readonly<Record<string, unknown>>;
  /** Rolled-up cost of this session's subagent children, USD. */
  readonly childCostUsd?: number;
  /** How many subagent children the rollup covered. */
  readonly childSessionCount?: number;
  /** Per-provider token usage rolled up from the subagent children. */
  readonly childProviderUsage?: readonly SessionCostProviderUsage[];
  /**
   * Customer-facing AEX cost of serving this session, USD — a REPORTED ESTIMATE,
   * not a charge (telemetry/showback only; no invoicing or credit deduction).
   * = rawCostUsd × marginMultiplier (margin currently a global 1.0). INCLUDES
   * the run's managed-gateway model tokens, which aex serves on its own key and
   * bills as a usage dimension. The raw (pre-margin) figure is kept
   * internal and never appears on this public-safe shape. A plain number, so it
   * passes the session-record public-safe archive scan. Absent when the session incurred
   * no priced AEX usage.
   */
  readonly billedCostUsd?: number;
  /** Currency + estimate/reconciled basis for {@link billedCostUsd}. */
  readonly costBasis?: SessionCostBasis;
}

export type SessionCostTelemetryInput = Omit<SessionCostTelemetry, "schemaVersion">;

export interface SessionCostTelemetryFromUsageSamplesInput {
  readonly sessionId?: string;
  readonly provider?: ProviderName | string;
  readonly recordedAt?: string;
  readonly status?: SessionCostSummaryStatus;
  readonly samples: readonly SessionUsageSampleInput[];
}
