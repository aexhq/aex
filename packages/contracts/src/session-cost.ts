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
  "output-object",
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
  "runtime.output_capture_ms",
  "runtime.cleanup_ms",
  "session.total_ms",
  "output.discovered_files",
  "output.captured_files",
  "output.failed_files",
  "output.captured_bytes",
  "retry.runtime_attempts",
  "retry.provider_poll",
  "retry.output_capture",
  "retry.output_upload",
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

const SESSION_USAGE_SAMPLE_METRIC_UNITS = {
  "provider.input_tokens": "token",
  "provider.output_tokens": "token",
  "provider.cache_read_input_tokens": "token",
  "provider.cache_creation_input_tokens": "token",
  "provider.total_tokens": "token",
  "runtime.queued_ms": "millisecond",
  "runtime.active_ms": "millisecond",
  "runtime.output_capture_ms": "millisecond",
  "runtime.cleanup_ms": "millisecond",
  "session.total_ms": "millisecond",
  "output.discovered_files": "file",
  "output.captured_files": "file",
  "output.failed_files": "file",
  "output.captured_bytes": "byte",
  "retry.runtime_attempts": "count",
  "retry.provider_poll": "count",
  "retry.output_capture": "count",
  "retry.output_upload": "count",
  "capture.uploaded_files": "file",
  "capture.failed_files": "file",
  "capture.total_bytes": "byte",
  "storage.current_bytes": "byte",
  "storage.byte_milliseconds": "byte_millisecond",
  "proxy.call_count": "count",
  "proxy.failed_call_count": "count",
  "proxy.request_bytes": "byte",
  "proxy.response_bytes": "byte",
  "proxy.duration_ms": "millisecond"
} satisfies Record<SessionUsageSampleMetric, SessionUsageSampleUnit>;

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
  readonly outputCaptureMs?: number;
  readonly cleanupMs?: number;
  readonly totalMs?: number;
}

export interface SessionCostOutputTelemetry {
  readonly discoveredFiles?: number;
  readonly capturedFiles?: number;
  readonly failedFiles?: number;
  readonly capturedBytes?: number;
}

export interface SessionCostRetryTelemetry {
  readonly runtimeAttempts?: number;
  readonly providerPollRetries?: number;
  readonly outputCaptureRetries?: number;
  readonly outputUploadRetries?: number;
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
 * whether the figure is a settle-time ESTIMATE or has been RECONCILED against
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

export interface SessionCostTelemetry {
  readonly schemaVersion: typeof SESSION_COST_TELEMETRY_SCHEMA_VERSION;
  readonly sessionId?: string;
  readonly provider?: ProviderName | string;
  readonly recordedAt?: string;
  readonly status?: SessionCostSummaryStatus;
  readonly sourceSummary?: SessionCostSourceSummary;
  readonly durations?: SessionCostDurations;
  readonly outputs?: SessionCostOutputTelemetry;
  readonly retries?: SessionCostRetryTelemetry;
  readonly capture?: SessionCostCaptureTelemetry;
  readonly providerUsage?: readonly SessionCostProviderUsage[];
  readonly storage?: SessionCostStorageTelemetry;
  readonly proxy?: SessionCostProxyTelemetry;
  /**
   * Customer-facing AEX cost of serving this session, USD — a REPORTED ESTIMATE,
   * not a charge (telemetry/showback only; no invoicing or credit deduction).
   * = rawCostUsd × marginMultiplier (margin currently a global 1.0). EXCLUDES
   * the customer's BYOK provider spend. The raw (pre-margin) figure is kept
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

type MutableSessionCostTelemetryInput = {
  -readonly [K in keyof SessionCostTelemetryInput]?: SessionCostTelemetryInput[K]
};

type ProviderUsageNumberField =
  | "inputTokens"
  | "outputTokens"
  | "cacheReadInputTokens"
  | "cacheCreationInputTokens"
  | "totalTokens";

interface MutableProviderUsage {
  provider: ProviderName | string;
  model?: string;
  inputTokens?: number;
  outputTokens?: number;
  cacheReadInputTokens?: number;
  cacheCreationInputTokens?: number;
  totalTokens?: number;
  sourceEventId?: string;
  sourceSampleIds: string[];
}

export function buildSessionUsageSample(input: SessionUsageSampleInput): SessionUsageSample {
  const metric = normalizeUsageSampleMetric(input.metric);
  const expectedUnit = SESSION_USAGE_SAMPLE_METRIC_UNITS[metric];
  const unit = input.unit ? normalizeUsageSampleUnit(input.unit) : expectedUnit;
  if (unit !== expectedUnit) {
    throw new Error(`session usage sample ${metric} must use ${expectedUnit} units`);
  }
  return Object.freeze({
    schemaVersion: SESSION_USAGE_SAMPLE_SCHEMA_VERSION,
    ...(input.sampleId ? { sampleId: nonEmptyString(input.sampleId, "sampleId") } : {}),
    ...(input.sessionId ? { sessionId: input.sessionId } : {}),
    metric,
    unit,
    quantity: nonNegativeFinite(input.quantity, "quantity"),
    source: normalizeUsageSampleSource(input.source),
    ...(input.provider ? { provider: input.provider } : {}),
    ...(input.model ? { model: input.model } : {}),
    ...(input.recordedAt ? { recordedAt: input.recordedAt } : {})
  });
}

export function buildSessionCostTelemetry(input: SessionCostTelemetryInput): SessionCostTelemetry {
  return freezeTelemetry({
    schemaVersion: SESSION_COST_TELEMETRY_SCHEMA_VERSION,
    ...(input.sessionId ? { sessionId: input.sessionId } : {}),
    ...(input.provider ? { provider: input.provider } : {}),
    ...(input.recordedAt ? { recordedAt: input.recordedAt } : {}),
    ...(input.status ? { status: normalizeSummaryStatus(input.status) } : {}),
    ...(input.sourceSummary ? { sourceSummary: normalizeSourceSummary(input.sourceSummary) } : {}),
    ...(input.durations ? { durations: normalizeDurations(input.durations) } : {}),
    ...(input.outputs ? { outputs: normalizeOutputs(input.outputs) } : {}),
    ...(input.retries ? { retries: normalizeRetries(input.retries) } : {}),
    ...(input.capture ? { capture: normalizeCapture(input.capture) } : {}),
    ...(input.providerUsage ? { providerUsage: input.providerUsage.map(normalizeProviderUsage) } : {}),
    ...(input.storage ? { storage: normalizeStorage(input.storage) } : {}),
    ...(input.proxy ? { proxy: normalizeProxy(input.proxy) } : {}),
    ...(input.billedCostUsd !== undefined ? { billedCostUsd: nonNegativeFinite(input.billedCostUsd, "billedCostUsd") } : {}),
    ...(input.costBasis ? { costBasis: normalizeCostBasis(input.costBasis) } : {})
  });
}

export function buildSessionCostTelemetryFromUsageSamples(
  input: SessionCostTelemetryFromUsageSamplesInput
): SessionCostTelemetry {
  const samples = input.samples.map(buildSessionUsageSample);
  const telemetry: MutableSessionCostTelemetryInput = {
    sourceSummary: summarizeUsageSamples(samples)
  };
  if (input.sessionId) telemetry.sessionId = input.sessionId;
  if (input.provider) telemetry.provider = input.provider;
  if (input.recordedAt) telemetry.recordedAt = input.recordedAt;
  if (input.status) telemetry.status = input.status;

  const durations: Partial<Record<keyof SessionCostDurations, number>> = {};
  const outputs: Partial<Record<keyof SessionCostOutputTelemetry, number>> = {};
  const retries: Partial<Record<keyof SessionCostRetryTelemetry, number>> = {};
  const capture: Partial<Omit<SessionCostCaptureTelemetry, "attempted" | "failureReasons">> = {};
  const storage: Partial<Record<keyof SessionCostStorageTelemetry, number>> = {};
  const proxy: Partial<Record<keyof SessionCostProxyTelemetry, number>> = {};
  const providerUsage = new Map<string, MutableProviderUsage>();
  let captureAttempted = false;

  for (const sample of samples) {
    switch (sample.metric) {
      case "provider.input_tokens":
        addProviderUsage(providerUsage, input, sample, "inputTokens");
        break;
      case "provider.output_tokens":
        addProviderUsage(providerUsage, input, sample, "outputTokens");
        break;
      case "provider.cache_read_input_tokens":
        addProviderUsage(providerUsage, input, sample, "cacheReadInputTokens");
        break;
      case "provider.cache_creation_input_tokens":
        addProviderUsage(providerUsage, input, sample, "cacheCreationInputTokens");
        break;
      case "provider.total_tokens":
        addProviderUsage(providerUsage, input, sample, "totalTokens");
        break;
      case "runtime.queued_ms":
        addDraftNumber(durations, "queuedMs", sample.quantity);
        break;
      case "runtime.active_ms":
        addDraftNumber(durations, "runtimeMs", sample.quantity);
        break;
      case "runtime.output_capture_ms":
        addDraftNumber(durations, "outputCaptureMs", sample.quantity);
        break;
      case "runtime.cleanup_ms":
        addDraftNumber(durations, "cleanupMs", sample.quantity);
        break;
      case "session.total_ms":
        addDraftNumber(durations, "totalMs", sample.quantity);
        break;
      case "output.discovered_files":
        addDraftNumber(outputs, "discoveredFiles", sample.quantity);
        break;
      case "output.captured_files":
        addDraftNumber(outputs, "capturedFiles", sample.quantity);
        break;
      case "output.failed_files":
        addDraftNumber(outputs, "failedFiles", sample.quantity);
        break;
      case "output.captured_bytes":
        addDraftNumber(outputs, "capturedBytes", sample.quantity);
        break;
      case "retry.runtime_attempts":
        addDraftNumber(retries, "runtimeAttempts", sample.quantity);
        break;
      case "retry.provider_poll":
        addDraftNumber(retries, "providerPollRetries", sample.quantity);
        break;
      case "retry.output_capture":
        addDraftNumber(retries, "outputCaptureRetries", sample.quantity);
        break;
      case "retry.output_upload":
        addDraftNumber(retries, "outputUploadRetries", sample.quantity);
        break;
      case "capture.uploaded_files":
        captureAttempted = true;
        addDraftNumber(capture, "uploadedFiles", sample.quantity);
        break;
      case "capture.failed_files":
        captureAttempted = true;
        addDraftNumber(capture, "failedFiles", sample.quantity);
        break;
      case "capture.total_bytes":
        captureAttempted = true;
        addDraftNumber(capture, "totalBytes", sample.quantity);
        break;
      case "storage.current_bytes":
        addDraftNumber(storage, "storedBytes", sample.quantity);
        break;
      case "storage.byte_milliseconds":
        addDraftNumber(storage, "byteMilliseconds", sample.quantity);
        break;
      case "proxy.call_count":
        addDraftNumber(proxy, "calls", sample.quantity);
        break;
      case "proxy.failed_call_count":
        addDraftNumber(proxy, "failedCalls", sample.quantity);
        break;
      case "proxy.request_bytes":
        addDraftNumber(proxy, "requestBytes", sample.quantity);
        break;
      case "proxy.response_bytes":
        addDraftNumber(proxy, "responseBytes", sample.quantity);
        break;
      case "proxy.duration_ms":
        addDraftNumber(proxy, "durationMs", sample.quantity);
        break;
    }
  }

  if (Object.keys(durations).length > 0) telemetry.durations = durations;
  if (Object.keys(outputs).length > 0) telemetry.outputs = outputs;
  if (Object.keys(retries).length > 0) telemetry.retries = retries;
  if (captureAttempted || Object.keys(capture).length > 0) {
    telemetry.capture = { attempted: true, ...capture };
  }
  if (providerUsage.size > 0) {
    telemetry.providerUsage = Array.from(providerUsage.values()).map(publicProviderUsage);
  }
  if (Object.keys(storage).length > 0) telemetry.storage = storage;
  if (Object.keys(proxy).length > 0) telemetry.proxy = proxy;

  return buildSessionCostTelemetry(telemetry);
}

export function summarizeUsageSamples(samples: readonly SessionUsageSampleInput[]): SessionCostSourceSummary {
  const normalized = samples.map(buildSessionUsageSample);
  const metrics = unique(normalized.map((sample) => sample.metric));
  const sourceTypes = unique(normalized.map((sample) => sample.source.type));
  const sourceSampleIds = unique(normalized.flatMap((sample) => (sample.sampleId ? [sample.sampleId] : [])));
  return normalizeSourceSummary({
    sampleCount: normalized.length,
    ...(metrics.length > 0 ? { metrics } : {}),
    ...(sourceTypes.length > 0 ? { sourceTypes } : {}),
    ...(sourceSampleIds.length > 0 ? { sourceSampleIds } : {})
  });
}

export function mergeSessionCostTelemetry(
  base: SessionCostTelemetry,
  next: SessionCostTelemetryInput
): SessionCostTelemetry {
  const patch = buildSessionCostTelemetry(next);
  const merged: MutableSessionCostTelemetryInput = {};
  const sessionId = patch.sessionId ?? base.sessionId;
  const provider = patch.provider ?? base.provider;
  const recordedAt = patch.recordedAt ?? base.recordedAt;
  const status = patch.status ?? base.status;
  const sourceSummary = mergeSourceSummary(base.sourceSummary, patch.sourceSummary);
  const durations = sumDurations(base.durations, patch.durations);
  const outputs = sumOutputs(base.outputs, patch.outputs);
  const retries = sumRetries(base.retries, patch.retries);
  const capture = mergeCapture(base.capture, patch.capture);
  const providerUsage = [...(base.providerUsage ?? []), ...(patch.providerUsage ?? [])];
  const storage = sumStorage(base.storage, patch.storage);
  const proxy = sumProxy(base.proxy, patch.proxy);
  // Derived cost fields are LAST-WRITER-WINS (a re-derivation supersedes the
  // prior estimate), not summed — they are projections of the whole sample set,
  // not additive metrics.
  const billedCostUsd = patch.billedCostUsd ?? base.billedCostUsd;
  const costBasis = patch.costBasis ?? base.costBasis;
  if (sessionId) merged.sessionId = sessionId;
  if (provider) merged.provider = provider;
  if (recordedAt) merged.recordedAt = recordedAt;
  if (status) merged.status = status;
  if (sourceSummary) merged.sourceSummary = sourceSummary;
  if (durations) merged.durations = durations;
  if (outputs) merged.outputs = outputs;
  if (retries) merged.retries = retries;
  if (capture) merged.capture = capture;
  if (providerUsage.length > 0) merged.providerUsage = providerUsage;
  if (storage) merged.storage = storage;
  if (proxy) merged.proxy = proxy;
  if (billedCostUsd !== undefined) merged.billedCostUsd = billedCostUsd;
  if (costBasis) merged.costBasis = costBasis;
  return buildSessionCostTelemetry(merged);
}

function normalizeDurations(input: SessionCostDurations): SessionCostDurations {
  return freezeOptionalNumbers<keyof SessionCostDurations>({
    queuedMs: input.queuedMs,
    runtimeMs: input.runtimeMs,
    outputCaptureMs: input.outputCaptureMs,
    cleanupMs: input.cleanupMs,
    totalMs: input.totalMs
  });
}

function normalizeOutputs(input: SessionCostOutputTelemetry): SessionCostOutputTelemetry {
  return freezeOptionalNumbers<keyof SessionCostOutputTelemetry>({
    discoveredFiles: input.discoveredFiles,
    capturedFiles: input.capturedFiles,
    failedFiles: input.failedFiles,
    capturedBytes: input.capturedBytes
  });
}

function normalizeRetries(input: SessionCostRetryTelemetry): SessionCostRetryTelemetry {
  return freezeOptionalNumbers<keyof SessionCostRetryTelemetry>({
    runtimeAttempts: input.runtimeAttempts,
    providerPollRetries: input.providerPollRetries,
    outputCaptureRetries: input.outputCaptureRetries,
    outputUploadRetries: input.outputUploadRetries
  });
}

function normalizeCapture(input: SessionCostCaptureTelemetry): SessionCostCaptureTelemetry {
  return Object.freeze({
    attempted: input.attempted,
    ...freezeOptionalNumbers<"uploadedFiles" | "failedFiles" | "totalBytes">({
      uploadedFiles: input.uploadedFiles,
      failedFiles: input.failedFiles,
      totalBytes: input.totalBytes
    }),
    ...(input.failureReasons ? { failureReasons: Object.freeze([...input.failureReasons]) } : {})
  });
}

function normalizeProviderUsage(input: SessionCostProviderUsage): SessionCostProviderUsage {
  return Object.freeze({
    provider: input.provider,
    ...(input.model ? { model: input.model } : {}),
    ...freezeOptionalNumbers<
      | "inputTokens"
      | "outputTokens"
      | "cacheReadInputTokens"
      | "cacheCreationInputTokens"
      | "totalTokens"
    >({
      inputTokens: input.inputTokens,
      outputTokens: input.outputTokens,
      cacheReadInputTokens: input.cacheReadInputTokens,
      cacheCreationInputTokens: input.cacheCreationInputTokens,
      totalTokens: input.totalTokens
    }),
    ...(input.sourceEventId ? { sourceEventId: input.sourceEventId } : {}),
    ...(input.sourceSampleIds ? { sourceSampleIds: Object.freeze([...input.sourceSampleIds]) } : {})
  });
}

function normalizeStorage(input: SessionCostStorageTelemetry): SessionCostStorageTelemetry {
  return freezeOptionalNumbers<keyof SessionCostStorageTelemetry>({
    storedBytes: input.storedBytes,
    byteMilliseconds: input.byteMilliseconds
  });
}

function normalizeProxy(input: SessionCostProxyTelemetry): SessionCostProxyTelemetry {
  return freezeOptionalNumbers<keyof SessionCostProxyTelemetry>({
    calls: input.calls,
    failedCalls: input.failedCalls,
    requestBytes: input.requestBytes,
    responseBytes: input.responseBytes,
    durationMs: input.durationMs
  });
}

function normalizeSourceSummary(input: SessionCostSourceSummary): SessionCostSourceSummary {
  return Object.freeze({
    sampleCount: nonNegativeFinite(input.sampleCount, "sourceSummary.sampleCount"),
    ...(input.metrics ? { metrics: Object.freeze(input.metrics.map(normalizeUsageSampleMetric)) } : {}),
    ...(input.sourceTypes
      ? { sourceTypes: Object.freeze(input.sourceTypes.map(normalizeUsageSampleSourceType)) }
      : {}),
    ...(input.sourceSampleIds
      ? { sourceSampleIds: Object.freeze(input.sourceSampleIds.map((id) => nonEmptyString(id, "sourceSampleIds"))) }
      : {})
  });
}

function normalizeUsageSampleSource(input: SessionUsageSampleSource): SessionUsageSampleSource {
  if (!input || typeof input !== "object") {
    throw new Error("session usage sample source must be an object");
  }
  return Object.freeze({
    type: normalizeUsageSampleSourceType(input.type),
    id: nonEmptyString(input.id, "source.id"),
    ...(input.observedAt ? { observedAt: input.observedAt } : {})
  });
}

function normalizeUsageSampleMetric(input: SessionUsageSampleMetric): SessionUsageSampleMetric {
  if (!isStringIn(input, SESSION_USAGE_SAMPLE_METRICS)) {
    throw new Error(`session usage sample metric ${String(input)} is not supported`);
  }
  return input;
}

function normalizeUsageSampleUnit(input: SessionUsageSampleUnit): SessionUsageSampleUnit {
  if (!isStringIn(input, SESSION_USAGE_SAMPLE_UNITS)) {
    throw new Error(`session usage sample unit ${String(input)} is not supported`);
  }
  return input;
}

function normalizeUsageSampleSourceType(input: SessionUsageSampleSourceType): SessionUsageSampleSourceType {
  if (!isStringIn(input, SESSION_USAGE_SAMPLE_SOURCE_TYPES)) {
    throw new Error(`session usage sample source type ${String(input)} is not supported`);
  }
  return input;
}

function normalizeSummaryStatus(input: SessionCostSummaryStatus): SessionCostSummaryStatus {
  if (!isStringIn(input, SESSION_COST_SUMMARY_STATUSES)) {
    throw new Error(`session cost telemetry status ${String(input)} is not supported`);
  }
  return input;
}

function normalizeCostBasis(input: SessionCostBasis): SessionCostBasis {
  if (!input || typeof input !== "object") {
    throw new Error("session cost basis must be an object");
  }
  if (input.currency !== "USD") {
    throw new Error(`session cost basis currency ${String(input.currency)} is not supported`);
  }
  if (!isStringIn(input.status, SESSION_COST_BASIS_STATUSES)) {
    throw new Error(`session cost basis status ${String(input.status)} is not supported`);
  }
  return Object.freeze({ currency: "USD", status: input.status });
}

function addProviderUsage(
  usageByKey: Map<string, MutableProviderUsage>,
  input: SessionCostTelemetryFromUsageSamplesInput,
  sample: SessionUsageSample,
  field: ProviderUsageNumberField
): void {
  const provider = sample.provider ?? input.provider;
  if (!provider) {
    throw new Error(`session usage sample ${sample.metric} requires a provider`);
  }
  const key = `${provider}\u0000${sample.model ?? ""}`;
  const existing = usageByKey.get(key);
  const usage = existing ?? {
    provider,
    ...(sample.model ? { model: sample.model } : {}),
    sourceSampleIds: []
  };
  usage[field] = (usage[field] ?? 0) + sample.quantity;
  if (!usage.sourceEventId && (sample.source.type === "coordinator-event" || sample.source.type === "session-event")) {
    usage.sourceEventId = sample.source.id;
  }
  if (sample.sampleId) {
    usage.sourceSampleIds.push(sample.sampleId);
  }
  usageByKey.set(key, usage);
}

function publicProviderUsage(input: MutableProviderUsage): SessionCostProviderUsage {
  return {
    provider: input.provider,
    ...(input.model ? { model: input.model } : {}),
    ...(input.inputTokens !== undefined ? { inputTokens: input.inputTokens } : {}),
    ...(input.outputTokens !== undefined ? { outputTokens: input.outputTokens } : {}),
    ...(input.cacheReadInputTokens !== undefined ? { cacheReadInputTokens: input.cacheReadInputTokens } : {}),
    ...(input.cacheCreationInputTokens !== undefined
      ? { cacheCreationInputTokens: input.cacheCreationInputTokens }
      : {}),
    ...(input.totalTokens !== undefined ? { totalTokens: input.totalTokens } : {}),
    ...(input.sourceEventId ? { sourceEventId: input.sourceEventId } : {}),
    ...(input.sourceSampleIds.length > 0 ? { sourceSampleIds: input.sourceSampleIds } : {})
  };
}

function addDraftNumber<K extends string>(
  target: Partial<Record<K, number>>,
  key: K,
  quantity: number
): void {
  target[key] = (target[key] ?? 0) + quantity;
}

function freezeOptionalNumbers<K extends string>(
  input: Partial<Record<K, number | undefined>>
): Readonly<Partial<Record<K, number>>> {
  const out: Partial<Record<K, number>> = {};
  for (const key of Object.keys(input) as K[]) {
    const value = input[key];
    if (value === undefined) {
      continue;
    }
    out[key] = nonNegativeFinite(value, key);
  }
  return Object.freeze(out) as Readonly<Partial<Record<K, number>>>;
}

function sumDurations(
  base: SessionCostDurations | undefined,
  next: SessionCostDurations | undefined
): SessionCostDurations | undefined {
  return sumNumberFields<keyof SessionCostDurations>(
    ["queuedMs", "runtimeMs", "outputCaptureMs", "cleanupMs", "totalMs"],
    base,
    next
  );
}

function sumOutputs(
  base: SessionCostOutputTelemetry | undefined,
  next: SessionCostOutputTelemetry | undefined
): SessionCostOutputTelemetry | undefined {
  return sumNumberFields<keyof SessionCostOutputTelemetry>(
    ["discoveredFiles", "capturedFiles", "failedFiles", "capturedBytes"],
    base,
    next
  );
}

function sumRetries(
  base: SessionCostRetryTelemetry | undefined,
  next: SessionCostRetryTelemetry | undefined
): SessionCostRetryTelemetry | undefined {
  return sumNumberFields<keyof SessionCostRetryTelemetry>(
    ["runtimeAttempts", "providerPollRetries", "outputCaptureRetries", "outputUploadRetries"],
    base,
    next
  );
}

function sumStorage(
  base: SessionCostStorageTelemetry | undefined,
  next: SessionCostStorageTelemetry | undefined
): SessionCostStorageTelemetry | undefined {
  return sumNumberFields<keyof SessionCostStorageTelemetry>(["storedBytes", "byteMilliseconds"], base, next);
}

function sumProxy(
  base: SessionCostProxyTelemetry | undefined,
  next: SessionCostProxyTelemetry | undefined
): SessionCostProxyTelemetry | undefined {
  return sumNumberFields<keyof SessionCostProxyTelemetry>(
    ["calls", "failedCalls", "requestBytes", "responseBytes", "durationMs"],
    base,
    next
  );
}

function sumNumberFields<K extends string>(
  keys: readonly K[],
  base: Partial<Record<K, number>> | undefined,
  next: Partial<Record<K, number>> | undefined
): Readonly<Partial<Record<K, number>>> | undefined {
  if (!base && !next) {
    return undefined;
  }
  const out: Partial<Record<K, number>> = {};
  for (const key of keys) {
    for (const source of [base, next]) {
      const value = source?.[key];
      if (value !== undefined) {
        out[key] = (out[key] ?? 0) + value;
      }
    }
  }
  return Object.keys(out).length > 0 ? Object.freeze(out) : undefined;
}

function mergeCapture(
  base: SessionCostCaptureTelemetry | undefined,
  next: SessionCostCaptureTelemetry | undefined
): SessionCostCaptureTelemetry | undefined {
  if (!base && !next) {
    return undefined;
  }
  const failureReasons = [
    ...(base?.failureReasons ?? []),
    ...(next?.failureReasons ?? [])
  ];
  return normalizeCapture({
    attempted: Boolean(base?.attempted || next?.attempted),
    uploadedFiles: (base?.uploadedFiles ?? 0) + (next?.uploadedFiles ?? 0),
    failedFiles: (base?.failedFiles ?? 0) + (next?.failedFiles ?? 0),
    totalBytes: (base?.totalBytes ?? 0) + (next?.totalBytes ?? 0),
    ...(failureReasons.length > 0 ? { failureReasons } : {})
  });
}

function mergeSourceSummary(
  base: SessionCostSourceSummary | undefined,
  next: SessionCostSourceSummary | undefined
): SessionCostSourceSummary | undefined {
  if (!base && !next) {
    return undefined;
  }
  return normalizeSourceSummary({
    sampleCount: (base?.sampleCount ?? 0) + (next?.sampleCount ?? 0),
    metrics: unique([...(base?.metrics ?? []), ...(next?.metrics ?? [])]),
    sourceTypes: unique([...(base?.sourceTypes ?? []), ...(next?.sourceTypes ?? [])]),
    sourceSampleIds: unique([...(base?.sourceSampleIds ?? []), ...(next?.sourceSampleIds ?? [])])
  });
}

function freezeTelemetry(input: SessionCostTelemetry): SessionCostTelemetry {
  return Object.freeze({
    ...input,
    ...(input.providerUsage ? { providerUsage: Object.freeze([...input.providerUsage]) } : {})
  });
}

function unique<T extends string>(input: readonly T[]): readonly T[] {
  return Object.freeze([...new Set(input)]);
}

function nonEmptyString(value: string, field: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`session cost telemetry ${field} must be a non-empty string`);
  }
  return value;
}

function nonNegativeFinite(value: number, field: string): number {
  if (!Number.isFinite(value) || value < 0) {
    throw new Error(`session cost telemetry ${field} must be a non-negative finite number`);
  }
  return value;
}

function isStringIn<T extends string>(value: unknown, allowed: readonly T[]): value is T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value);
}
