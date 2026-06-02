import type { CredentialMode } from "./managed-key.js";
import type { RunProvider, RuntimeKind } from "./submission.js";

export const RUN_COST_TELEMETRY_SCHEMA_VERSION = 1;
export const RUN_USAGE_SAMPLE_SCHEMA_VERSION = 1;

export const RUN_COST_SUMMARY_STATUSES = [
  "pending",
  "partial",
  "complete",
  "unavailable",
  "not_applicable"
] as const;

export type RunCostSummaryStatus = (typeof RUN_COST_SUMMARY_STATUSES)[number];

export const RUN_USAGE_SAMPLE_UNITS = [
  "token",
  "millisecond",
  "byte",
  "byte_millisecond",
  "count",
  "file",
  "credit_unit"
] as const;

export type RunUsageSampleUnit = (typeof RUN_USAGE_SAMPLE_UNITS)[number];

export const RUN_USAGE_SAMPLE_SOURCE_TYPES = [
  "coordinator-event",
  "run-event",
  "usage-ledger",
  "output-object",
  "proxy-call",
  "runtime-job",
  "provider-session",
  "storage-accrual",
  "billing-reservation",
  "billing-settlement",
  "billing-release",
  "manual-adjustment"
] as const;

export type RunUsageSampleSourceType = (typeof RUN_USAGE_SAMPLE_SOURCE_TYPES)[number];

export const RUN_USAGE_SAMPLE_METRICS = [
  "provider.input_tokens",
  "provider.output_tokens",
  "provider.cache_read_input_tokens",
  "provider.cache_creation_input_tokens",
  "provider.total_tokens",
  "runtime.queued_ms",
  "runtime.active_ms",
  "runtime.output_capture_ms",
  "runtime.cleanup_ms",
  "run.total_ms",
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
  "proxy.duration_ms",
  "managed_key.reserved_credit_units",
  "managed_key.charged_credit_units",
  "managed_key.released_credit_units"
] as const;

export type RunUsageSampleMetric = (typeof RUN_USAGE_SAMPLE_METRICS)[number];

const RUN_USAGE_SAMPLE_METRIC_UNITS = {
  "provider.input_tokens": "token",
  "provider.output_tokens": "token",
  "provider.cache_read_input_tokens": "token",
  "provider.cache_creation_input_tokens": "token",
  "provider.total_tokens": "token",
  "runtime.queued_ms": "millisecond",
  "runtime.active_ms": "millisecond",
  "runtime.output_capture_ms": "millisecond",
  "runtime.cleanup_ms": "millisecond",
  "run.total_ms": "millisecond",
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
  "proxy.duration_ms": "millisecond",
  "managed_key.reserved_credit_units": "credit_unit",
  "managed_key.charged_credit_units": "credit_unit",
  "managed_key.released_credit_units": "credit_unit"
} satisfies Record<RunUsageSampleMetric, RunUsageSampleUnit>;

export interface RunUsageSampleSource {
  readonly type: RunUsageSampleSourceType;
  readonly id: string;
  readonly observedAt?: string;
}

export interface RunUsageSample {
  readonly schemaVersion: typeof RUN_USAGE_SAMPLE_SCHEMA_VERSION;
  readonly sampleId?: string;
  readonly runId?: string;
  readonly metric: RunUsageSampleMetric;
  readonly unit: RunUsageSampleUnit;
  readonly quantity: number;
  readonly source: RunUsageSampleSource;
  readonly provider?: RunProvider | string;
  readonly runtime?: RuntimeKind | string;
  readonly model?: string;
  readonly credentialMode?: CredentialMode;
  readonly recordedAt?: string;
}

export type RunUsageSampleInput = Omit<RunUsageSample, "schemaVersion" | "unit"> & {
  readonly unit?: RunUsageSampleUnit;
};

export interface RunCostSourceSummary {
  readonly sampleCount: number;
  readonly metrics?: readonly RunUsageSampleMetric[];
  readonly sourceTypes?: readonly RunUsageSampleSourceType[];
  readonly sourceSampleIds?: readonly string[];
}

export interface RunCostDurations {
  readonly queuedMs?: number;
  readonly runtimeMs?: number;
  readonly outputCaptureMs?: number;
  readonly cleanupMs?: number;
  readonly totalMs?: number;
}

export interface RunCostOutputTelemetry {
  readonly discoveredFiles?: number;
  readonly capturedFiles?: number;
  readonly failedFiles?: number;
  readonly capturedBytes?: number;
}

export interface RunCostRetryTelemetry {
  readonly runtimeAttempts?: number;
  readonly providerPollRetries?: number;
  readonly outputCaptureRetries?: number;
  readonly outputUploadRetries?: number;
}

export interface RunCostCaptureTelemetry {
  readonly attempted: boolean;
  readonly uploadedFiles?: number;
  readonly failedFiles?: number;
  readonly totalBytes?: number;
  readonly failureReasons?: readonly string[];
}

export interface RunCostProviderUsage {
  readonly provider: RunProvider | string;
  readonly model?: string;
  readonly inputTokens?: number;
  readonly outputTokens?: number;
  readonly cacheReadInputTokens?: number;
  readonly cacheCreationInputTokens?: number;
  readonly totalTokens?: number;
  readonly sourceEventId?: string;
  readonly sourceSampleIds?: readonly string[];
}

export interface RunCostStorageTelemetry {
  readonly storedBytes?: number;
  readonly byteMilliseconds?: number;
}

export interface RunCostProxyTelemetry {
  readonly calls?: number;
  readonly failedCalls?: number;
  readonly requestBytes?: number;
  readonly responseBytes?: number;
  readonly durationMs?: number;
}

export interface RunCostManagedKeyBudgetTelemetry {
  readonly credentialMode?: CredentialMode;
  readonly reservedCreditUnits?: number;
  readonly chargedCreditUnits?: number;
  readonly releasedCreditUnits?: number;
}

export interface RunCostTelemetry {
  readonly schemaVersion: typeof RUN_COST_TELEMETRY_SCHEMA_VERSION;
  readonly runId?: string;
  readonly provider?: RunProvider | string;
  readonly runtime?: RuntimeKind | string;
  readonly recordedAt?: string;
  readonly status?: RunCostSummaryStatus;
  readonly sourceSummary?: RunCostSourceSummary;
  readonly durations?: RunCostDurations;
  readonly outputs?: RunCostOutputTelemetry;
  readonly retries?: RunCostRetryTelemetry;
  readonly capture?: RunCostCaptureTelemetry;
  readonly providerUsage?: readonly RunCostProviderUsage[];
  readonly storage?: RunCostStorageTelemetry;
  readonly proxy?: RunCostProxyTelemetry;
  readonly managedKey?: RunCostManagedKeyBudgetTelemetry;
}

export type RunCostTelemetryInput = Omit<RunCostTelemetry, "schemaVersion">;

export interface RunCostTelemetryFromUsageSamplesInput {
  readonly runId?: string;
  readonly provider?: RunProvider | string;
  readonly runtime?: RuntimeKind | string;
  readonly recordedAt?: string;
  readonly status?: RunCostSummaryStatus;
  readonly samples: readonly RunUsageSampleInput[];
}

type MutableRunCostTelemetryInput = {
  -readonly [K in keyof RunCostTelemetryInput]?: RunCostTelemetryInput[K]
};

type ProviderUsageNumberField =
  | "inputTokens"
  | "outputTokens"
  | "cacheReadInputTokens"
  | "cacheCreationInputTokens"
  | "totalTokens";

interface MutableProviderUsage {
  provider: RunProvider | string;
  model?: string;
  inputTokens?: number;
  outputTokens?: number;
  cacheReadInputTokens?: number;
  cacheCreationInputTokens?: number;
  totalTokens?: number;
  sourceEventId?: string;
  sourceSampleIds: string[];
}

export function buildRunUsageSample(input: RunUsageSampleInput): RunUsageSample {
  const metric = normalizeUsageSampleMetric(input.metric);
  const expectedUnit = RUN_USAGE_SAMPLE_METRIC_UNITS[metric];
  const unit = input.unit ? normalizeUsageSampleUnit(input.unit) : expectedUnit;
  if (unit !== expectedUnit) {
    throw new Error(`run usage sample ${metric} must use ${expectedUnit} units`);
  }
  return Object.freeze({
    schemaVersion: RUN_USAGE_SAMPLE_SCHEMA_VERSION,
    ...(input.sampleId ? { sampleId: nonEmptyString(input.sampleId, "sampleId") } : {}),
    ...(input.runId ? { runId: input.runId } : {}),
    metric,
    unit,
    quantity: nonNegativeFinite(input.quantity, "quantity"),
    source: normalizeUsageSampleSource(input.source),
    ...(input.provider ? { provider: input.provider } : {}),
    ...(input.runtime ? { runtime: input.runtime } : {}),
    ...(input.model ? { model: input.model } : {}),
    ...(input.credentialMode ? { credentialMode: input.credentialMode } : {}),
    ...(input.recordedAt ? { recordedAt: input.recordedAt } : {})
  });
}

export function buildRunCostTelemetry(input: RunCostTelemetryInput): RunCostTelemetry {
  return freezeTelemetry({
    schemaVersion: RUN_COST_TELEMETRY_SCHEMA_VERSION,
    ...(input.runId ? { runId: input.runId } : {}),
    ...(input.provider ? { provider: input.provider } : {}),
    ...(input.runtime ? { runtime: input.runtime } : {}),
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
    ...(input.managedKey ? { managedKey: normalizeManagedKey(input.managedKey) } : {})
  });
}

export function buildRunCostTelemetryFromUsageSamples(
  input: RunCostTelemetryFromUsageSamplesInput
): RunCostTelemetry {
  const samples = input.samples.map(buildRunUsageSample);
  const telemetry: MutableRunCostTelemetryInput = {
    sourceSummary: summarizeUsageSamples(samples)
  };
  if (input.runId) telemetry.runId = input.runId;
  if (input.provider) telemetry.provider = input.provider;
  if (input.runtime) telemetry.runtime = input.runtime;
  if (input.recordedAt) telemetry.recordedAt = input.recordedAt;
  if (input.status) telemetry.status = input.status;

  const durations: Partial<Record<keyof RunCostDurations, number>> = {};
  const outputs: Partial<Record<keyof RunCostOutputTelemetry, number>> = {};
  const retries: Partial<Record<keyof RunCostRetryTelemetry, number>> = {};
  const capture: Partial<Omit<RunCostCaptureTelemetry, "attempted" | "failureReasons">> = {};
  const storage: Partial<Record<keyof RunCostStorageTelemetry, number>> = {};
  const proxy: Partial<Record<keyof RunCostProxyTelemetry, number>> = {};
  const managedKey: Partial<Record<"reservedCreditUnits" | "chargedCreditUnits" | "releasedCreditUnits", number>> = {};
  const providerUsage = new Map<string, MutableProviderUsage>();
  let captureAttempted = false;
  let managedKeyCredentialMode: CredentialMode | undefined;

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
      case "run.total_ms":
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
      case "managed_key.reserved_credit_units":
        managedKeyCredentialMode = sample.credentialMode ?? managedKeyCredentialMode;
        addDraftNumber(managedKey, "reservedCreditUnits", sample.quantity);
        break;
      case "managed_key.charged_credit_units":
        managedKeyCredentialMode = sample.credentialMode ?? managedKeyCredentialMode;
        addDraftNumber(managedKey, "chargedCreditUnits", sample.quantity);
        break;
      case "managed_key.released_credit_units":
        managedKeyCredentialMode = sample.credentialMode ?? managedKeyCredentialMode;
        addDraftNumber(managedKey, "releasedCreditUnits", sample.quantity);
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
  if (Object.keys(managedKey).length > 0 || managedKeyCredentialMode) {
    telemetry.managedKey = {
      ...(managedKeyCredentialMode ? { credentialMode: managedKeyCredentialMode } : {}),
      ...managedKey
    };
  }

  return buildRunCostTelemetry(telemetry);
}

export function summarizeUsageSamples(samples: readonly RunUsageSampleInput[]): RunCostSourceSummary {
  const normalized = samples.map(buildRunUsageSample);
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

export function mergeRunCostTelemetry(
  base: RunCostTelemetry,
  next: RunCostTelemetryInput
): RunCostTelemetry {
  const patch = buildRunCostTelemetry(next);
  const merged: MutableRunCostTelemetryInput = {};
  const runId = patch.runId ?? base.runId;
  const provider = patch.provider ?? base.provider;
  const runtime = patch.runtime ?? base.runtime;
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
  const managedKey = mergeManagedKey(base.managedKey, patch.managedKey);
  if (runId) merged.runId = runId;
  if (provider) merged.provider = provider;
  if (runtime) merged.runtime = runtime;
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
  if (managedKey) merged.managedKey = managedKey;
  return buildRunCostTelemetry(merged);
}

function normalizeDurations(input: RunCostDurations): RunCostDurations {
  return freezeOptionalNumbers<keyof RunCostDurations>({
    queuedMs: input.queuedMs,
    runtimeMs: input.runtimeMs,
    outputCaptureMs: input.outputCaptureMs,
    cleanupMs: input.cleanupMs,
    totalMs: input.totalMs
  });
}

function normalizeOutputs(input: RunCostOutputTelemetry): RunCostOutputTelemetry {
  return freezeOptionalNumbers<keyof RunCostOutputTelemetry>({
    discoveredFiles: input.discoveredFiles,
    capturedFiles: input.capturedFiles,
    failedFiles: input.failedFiles,
    capturedBytes: input.capturedBytes
  });
}

function normalizeRetries(input: RunCostRetryTelemetry): RunCostRetryTelemetry {
  return freezeOptionalNumbers<keyof RunCostRetryTelemetry>({
    runtimeAttempts: input.runtimeAttempts,
    providerPollRetries: input.providerPollRetries,
    outputCaptureRetries: input.outputCaptureRetries,
    outputUploadRetries: input.outputUploadRetries
  });
}

function normalizeCapture(input: RunCostCaptureTelemetry): RunCostCaptureTelemetry {
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

function normalizeProviderUsage(input: RunCostProviderUsage): RunCostProviderUsage {
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

function normalizeStorage(input: RunCostStorageTelemetry): RunCostStorageTelemetry {
  return freezeOptionalNumbers<keyof RunCostStorageTelemetry>({
    storedBytes: input.storedBytes,
    byteMilliseconds: input.byteMilliseconds
  });
}

function normalizeProxy(input: RunCostProxyTelemetry): RunCostProxyTelemetry {
  return freezeOptionalNumbers<keyof RunCostProxyTelemetry>({
    calls: input.calls,
    failedCalls: input.failedCalls,
    requestBytes: input.requestBytes,
    responseBytes: input.responseBytes,
    durationMs: input.durationMs
  });
}

function normalizeManagedKey(input: RunCostManagedKeyBudgetTelemetry): RunCostManagedKeyBudgetTelemetry {
  return Object.freeze({
    ...(input.credentialMode ? { credentialMode: input.credentialMode } : {}),
    ...freezeOptionalNumbers<"reservedCreditUnits" | "chargedCreditUnits" | "releasedCreditUnits">({
      reservedCreditUnits: input.reservedCreditUnits,
      chargedCreditUnits: input.chargedCreditUnits,
      releasedCreditUnits: input.releasedCreditUnits
    })
  });
}

function normalizeSourceSummary(input: RunCostSourceSummary): RunCostSourceSummary {
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

function normalizeUsageSampleSource(input: RunUsageSampleSource): RunUsageSampleSource {
  if (!input || typeof input !== "object") {
    throw new Error("run usage sample source must be an object");
  }
  return Object.freeze({
    type: normalizeUsageSampleSourceType(input.type),
    id: nonEmptyString(input.id, "source.id"),
    ...(input.observedAt ? { observedAt: input.observedAt } : {})
  });
}

function normalizeUsageSampleMetric(input: RunUsageSampleMetric): RunUsageSampleMetric {
  if (!isStringIn(input, RUN_USAGE_SAMPLE_METRICS)) {
    throw new Error(`run usage sample metric ${String(input)} is not supported`);
  }
  return input;
}

function normalizeUsageSampleUnit(input: RunUsageSampleUnit): RunUsageSampleUnit {
  if (!isStringIn(input, RUN_USAGE_SAMPLE_UNITS)) {
    throw new Error(`run usage sample unit ${String(input)} is not supported`);
  }
  return input;
}

function normalizeUsageSampleSourceType(input: RunUsageSampleSourceType): RunUsageSampleSourceType {
  if (!isStringIn(input, RUN_USAGE_SAMPLE_SOURCE_TYPES)) {
    throw new Error(`run usage sample source type ${String(input)} is not supported`);
  }
  return input;
}

function normalizeSummaryStatus(input: RunCostSummaryStatus): RunCostSummaryStatus {
  if (!isStringIn(input, RUN_COST_SUMMARY_STATUSES)) {
    throw new Error(`run cost telemetry status ${String(input)} is not supported`);
  }
  return input;
}

function addProviderUsage(
  usageByKey: Map<string, MutableProviderUsage>,
  input: RunCostTelemetryFromUsageSamplesInput,
  sample: RunUsageSample,
  field: ProviderUsageNumberField
): void {
  const provider = sample.provider ?? input.provider;
  if (!provider) {
    throw new Error(`run usage sample ${sample.metric} requires a provider`);
  }
  const key = `${provider}\u0000${sample.model ?? ""}`;
  const existing = usageByKey.get(key);
  const usage = existing ?? {
    provider,
    ...(sample.model ? { model: sample.model } : {}),
    sourceSampleIds: []
  };
  usage[field] = (usage[field] ?? 0) + sample.quantity;
  if (!usage.sourceEventId && (sample.source.type === "coordinator-event" || sample.source.type === "run-event")) {
    usage.sourceEventId = sample.source.id;
  }
  if (sample.sampleId) {
    usage.sourceSampleIds.push(sample.sampleId);
  }
  usageByKey.set(key, usage);
}

function publicProviderUsage(input: MutableProviderUsage): RunCostProviderUsage {
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
  base: RunCostDurations | undefined,
  next: RunCostDurations | undefined
): RunCostDurations | undefined {
  return sumNumberFields<keyof RunCostDurations>(
    ["queuedMs", "runtimeMs", "outputCaptureMs", "cleanupMs", "totalMs"],
    base,
    next
  );
}

function sumOutputs(
  base: RunCostOutputTelemetry | undefined,
  next: RunCostOutputTelemetry | undefined
): RunCostOutputTelemetry | undefined {
  return sumNumberFields<keyof RunCostOutputTelemetry>(
    ["discoveredFiles", "capturedFiles", "failedFiles", "capturedBytes"],
    base,
    next
  );
}

function sumRetries(
  base: RunCostRetryTelemetry | undefined,
  next: RunCostRetryTelemetry | undefined
): RunCostRetryTelemetry | undefined {
  return sumNumberFields<keyof RunCostRetryTelemetry>(
    ["runtimeAttempts", "providerPollRetries", "outputCaptureRetries", "outputUploadRetries"],
    base,
    next
  );
}

function sumStorage(
  base: RunCostStorageTelemetry | undefined,
  next: RunCostStorageTelemetry | undefined
): RunCostStorageTelemetry | undefined {
  return sumNumberFields<keyof RunCostStorageTelemetry>(["storedBytes", "byteMilliseconds"], base, next);
}

function sumProxy(
  base: RunCostProxyTelemetry | undefined,
  next: RunCostProxyTelemetry | undefined
): RunCostProxyTelemetry | undefined {
  return sumNumberFields<keyof RunCostProxyTelemetry>(
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
  base: RunCostCaptureTelemetry | undefined,
  next: RunCostCaptureTelemetry | undefined
): RunCostCaptureTelemetry | undefined {
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

function mergeManagedKey(
  base: RunCostManagedKeyBudgetTelemetry | undefined,
  next: RunCostManagedKeyBudgetTelemetry | undefined
): RunCostManagedKeyBudgetTelemetry | undefined {
  if (!base && !next) {
    return undefined;
  }
  const credentialMode = next?.credentialMode ?? base?.credentialMode;
  return normalizeManagedKey({
    ...(credentialMode ? { credentialMode } : {}),
    reservedCreditUnits: (base?.reservedCreditUnits ?? 0) + (next?.reservedCreditUnits ?? 0),
    chargedCreditUnits: (base?.chargedCreditUnits ?? 0) + (next?.chargedCreditUnits ?? 0),
    releasedCreditUnits: (base?.releasedCreditUnits ?? 0) + (next?.releasedCreditUnits ?? 0)
  });
}

function mergeSourceSummary(
  base: RunCostSourceSummary | undefined,
  next: RunCostSourceSummary | undefined
): RunCostSourceSummary | undefined {
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

function freezeTelemetry(input: RunCostTelemetry): RunCostTelemetry {
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
    throw new Error(`run cost telemetry ${field} must be a non-empty string`);
  }
  return value;
}

function nonNegativeFinite(value: number, field: string): number {
  if (!Number.isFinite(value) || value < 0) {
    throw new Error(`run cost telemetry ${field} must be a non-negative finite number`);
  }
  return value;
}

function isStringIn<T extends string>(value: unknown, allowed: readonly T[]): value is T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value);
}
