import type { ProviderName } from "./submission.js";
import {
  SESSION_COST_BASIS_STATUSES,
  SESSION_COST_SUMMARY_STATUSES,
  SESSION_COST_TELEMETRY_SCHEMA_VERSION,
  SESSION_USAGE_SAMPLE_METRICS,
  SESSION_USAGE_SAMPLE_SCHEMA_VERSION,
  SESSION_USAGE_SAMPLE_SOURCE_TYPES,
  SESSION_USAGE_SAMPLE_UNITS,
  type SessionCostBasis,
  type SessionCostCaptureTelemetry,
  type SessionCostDurations,
  type SessionCostFileTelemetry,
  type SessionCostProviderUsage,
  type SessionCostProxyTelemetry,
  type SessionCostRetryTelemetry,
  type SessionCostSourceSummary,
  type SessionCostStorageTelemetry,
  type SessionCostSummaryStatus,
  type SessionCostTelemetry,
  type SessionCostTelemetryFromUsageSamplesInput,
  type SessionCostTelemetryInput,
  type SessionUsageSample,
  type SessionUsageSampleInput,
  type SessionUsageSampleMetric,
  type SessionUsageSampleSource,
  type SessionUsageSampleSourceType,
  type SessionUsageSampleUnit
} from "./session-cost-types.js";

// Keep the named list explicit so the platform mirror gate can follow the source boundary.
export { SESSION_COST_BASIS_STATUSES, SESSION_COST_SUMMARY_STATUSES, SESSION_COST_TELEMETRY_SCHEMA_VERSION, SESSION_USAGE_SAMPLE_METRICS, SESSION_USAGE_SAMPLE_SCHEMA_VERSION, SESSION_USAGE_SAMPLE_SOURCE_TYPES, SESSION_USAGE_SAMPLE_UNITS } from "./session-cost-types.js";
export type { SessionCostBasis, SessionCostBasisLabel, SessionCostBasisStatus, SessionCostCaptureTelemetry, SessionCostDurations, SessionCostFileTelemetry, SessionCostProviderUsage, SessionCostProxyTelemetry, SessionCostRetryTelemetry, SessionCostSourceSummary, SessionCostStorageTelemetry, SessionCostSummaryStatus, SessionCostTelemetry, SessionCostTelemetryFromUsageSamplesInput, SessionCostTelemetryInput, SessionUsageSample, SessionUsageSampleInput, SessionUsageSampleMetric, SessionUsageSampleSource, SessionUsageSampleSourceType, SessionUsageSampleUnit } from "./session-cost-types.js";

/**
 * The unit each metric is denominated in. Module-private: it is an
 * implementation detail of {@link buildSessionUsageSample}, which rejects a
 * sample whose declared unit disagrees with its metric.
 */
const SESSION_USAGE_SAMPLE_METRIC_UNITS = {
  "provider.input_tokens": "token",
  "provider.output_tokens": "token",
  "provider.cache_read_input_tokens": "token",
  "provider.cache_creation_input_tokens": "token",
  "provider.total_tokens": "token",
  "runtime.queued_ms": "millisecond",
  "runtime.active_ms": "millisecond",
  "runtime.file_capture_ms": "millisecond",
  "runtime.cleanup_ms": "millisecond",
  "session.total_ms": "millisecond",
  "file.discovered_files": "file",
  "file.captured_files": "file",
  "file.failed_files": "file",
  "file.captured_bytes": "byte",
  "retry.runtime_attempts": "count",
  "retry.provider_poll": "count",
  "retry.file_capture": "count",
  "retry.file_upload": "count",
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
    ...(input.files ? { files: normalizeFiles(input.files) } : {}),
    ...(input.retries ? { retries: normalizeRetries(input.retries) } : {}),
    ...(input.capture ? { capture: normalizeCapture(input.capture) } : {}),
    ...(input.providerUsage ? { providerUsage: input.providerUsage.map(normalizeProviderUsage) } : {}),
    ...(input.storage ? { storage: normalizeStorage(input.storage) } : {}),
    ...(input.proxy ? { proxy: normalizeProxy(input.proxy) } : {}),
    ...(input.billedCostUsd !== undefined ? { billedCostUsd: nonNegativeFinite(input.billedCostUsd, "billedCostUsd") } : {}),
    ...(input.costBasis ? { costBasis: normalizeCostBasis(input.costBasis) } : {}),
    // The settle-written half. Carried through rather than dropped: a declared
    // field this builder silently discards is worse than an undeclared one,
    // because the loss is invisible at the call site.
    ...(input.basis ? { basis: nonEmptyString(input.basis, "basis") } : {}),
    ...(input.durationMs !== undefined ? { durationMs: nonNegativeFinite(input.durationMs, "durationMs") } : {}),
    ...(input.turnSeq !== undefined ? { turnSeq: nonNegativeFinite(input.turnSeq, "turnSeq") } : {}),
    ...(input.runtimeSize ? { runtimeSize: nonEmptyString(input.runtimeSize, "runtimeSize") } : {}),
    ...(input.runtimeKind ? { runtimeKind: nonEmptyString(input.runtimeKind, "runtimeKind") } : {}),
    ...(input.model ? { model: input.model } : {}),
    ...(input.usage ? { usage: Object.freeze({ ...input.usage }) } : {}),
    ...(input.byteCounts ? { byteCounts: Object.freeze({ ...input.byteCounts }) } : {}),
    ...(input.childCostUsd !== undefined
      ? { childCostUsd: nonNegativeFinite(input.childCostUsd, "childCostUsd") }
      : {}),
    ...(input.childSessionCount !== undefined
      ? { childSessionCount: nonNegativeFinite(input.childSessionCount, "childSessionCount") }
      : {}),
    ...(input.childProviderUsage
      ? { childProviderUsage: input.childProviderUsage.map(normalizeProviderUsage) }
      : {})
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
  const files: Partial<Record<keyof SessionCostFileTelemetry, number>> = {};
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
      case "runtime.file_capture_ms":
        addDraftNumber(durations, "fileCaptureMs", sample.quantity);
        break;
      case "runtime.cleanup_ms":
        addDraftNumber(durations, "cleanupMs", sample.quantity);
        break;
      case "session.total_ms":
        addDraftNumber(durations, "totalMs", sample.quantity);
        break;
      case "file.discovered_files":
        addDraftNumber(files, "discoveredFiles", sample.quantity);
        break;
      case "file.captured_files":
        addDraftNumber(files, "capturedFiles", sample.quantity);
        break;
      case "file.failed_files":
        addDraftNumber(files, "failedFiles", sample.quantity);
        break;
      case "file.captured_bytes":
        addDraftNumber(files, "capturedBytes", sample.quantity);
        break;
      case "retry.runtime_attempts":
        addDraftNumber(retries, "runtimeAttempts", sample.quantity);
        break;
      case "retry.provider_poll":
        addDraftNumber(retries, "providerPollRetries", sample.quantity);
        break;
      case "retry.file_capture":
        addDraftNumber(retries, "fileCaptureRetries", sample.quantity);
        break;
      case "retry.file_upload":
        addDraftNumber(retries, "fileUploadRetries", sample.quantity);
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
  if (Object.keys(files).length > 0) telemetry.files = files;
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
  const files = sumFiles(base.files, patch.files);
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
  // The settle-written half is DESCRIPTIVE of one settle (which basis, which
  // turn, which box, what the manifest counted), not additive across turns, so
  // it is last-writer-wins like the derived cost fields above. `childCostUsd` /
  // `childSessionCount` are already rollups over the whole child set.
  const basis = patch.basis ?? base.basis;
  const durationMs = patch.durationMs ?? base.durationMs;
  const turnSeq = patch.turnSeq ?? base.turnSeq;
  const runtimeSize = patch.runtimeSize ?? base.runtimeSize;
  const runtimeKind = patch.runtimeKind ?? base.runtimeKind;
  const model = patch.model ?? base.model;
  const usage = patch.usage ?? base.usage;
  const byteCounts = patch.byteCounts ?? base.byteCounts;
  const childCostUsd = patch.childCostUsd ?? base.childCostUsd;
  const childSessionCount = patch.childSessionCount ?? base.childSessionCount;
  const childProviderUsage = [...(base.childProviderUsage ?? []), ...(patch.childProviderUsage ?? [])];
  if (sessionId) merged.sessionId = sessionId;
  if (provider) merged.provider = provider;
  if (recordedAt) merged.recordedAt = recordedAt;
  if (status) merged.status = status;
  if (sourceSummary) merged.sourceSummary = sourceSummary;
  if (durations) merged.durations = durations;
  if (files) merged.files = files;
  if (retries) merged.retries = retries;
  if (capture) merged.capture = capture;
  if (providerUsage.length > 0) merged.providerUsage = providerUsage;
  if (storage) merged.storage = storage;
  if (proxy) merged.proxy = proxy;
  if (billedCostUsd !== undefined) merged.billedCostUsd = billedCostUsd;
  if (costBasis) merged.costBasis = costBasis;
  if (basis) merged.basis = basis;
  if (durationMs !== undefined) merged.durationMs = durationMs;
  if (turnSeq !== undefined) merged.turnSeq = turnSeq;
  if (runtimeSize) merged.runtimeSize = runtimeSize;
  if (runtimeKind) merged.runtimeKind = runtimeKind;
  if (model) merged.model = model;
  if (usage) merged.usage = usage;
  if (byteCounts) merged.byteCounts = byteCounts;
  if (childCostUsd !== undefined) merged.childCostUsd = childCostUsd;
  if (childSessionCount !== undefined) merged.childSessionCount = childSessionCount;
  if (childProviderUsage.length > 0) merged.childProviderUsage = childProviderUsage;
  return buildSessionCostTelemetry(merged);
}

function normalizeDurations(input: SessionCostDurations): SessionCostDurations {
  return freezeOptionalNumbers<keyof SessionCostDurations>({
    queuedMs: input.queuedMs,
    runtimeMs: input.runtimeMs,
    fileCaptureMs: input.fileCaptureMs,
    cleanupMs: input.cleanupMs,
    totalMs: input.totalMs
  });
}

function normalizeFiles(input: SessionCostFileTelemetry): SessionCostFileTelemetry {
  return freezeOptionalNumbers<keyof SessionCostFileTelemetry>({
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
    fileCaptureRetries: input.fileCaptureRetries,
    fileUploadRetries: input.fileUploadRetries
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
    storedFiles: input.storedFiles,
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
    ["queuedMs", "runtimeMs", "fileCaptureMs", "cleanupMs", "totalMs"],
    base,
    next
  );
}

function sumFiles(
  base: SessionCostFileTelemetry | undefined,
  next: SessionCostFileTelemetry | undefined
): SessionCostFileTelemetry | undefined {
  return sumNumberFields<keyof SessionCostFileTelemetry>(
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
    ["runtimeAttempts", "providerPollRetries", "fileCaptureRetries", "fileUploadRetries"],
    base,
    next
  );
}

function sumStorage(
  base: SessionCostStorageTelemetry | undefined,
  next: SessionCostStorageTelemetry | undefined
): SessionCostStorageTelemetry | undefined {
  return sumNumberFields<keyof SessionCostStorageTelemetry>(["storedBytes", "storedFiles", "byteMilliseconds"], base, next);
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
