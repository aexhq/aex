import type { AexEvent, AexLogLevel } from "./event-envelope.js";
import { redactString } from "./sdk-secrets.js";

/** Signals exposed by the session telemetry pull API. */
export type OtlpSignal = "traces" | "logs";

/** OTLP/HTTP JSON AnyValue. The projection emits only public scalar values. */
export type OtlpAnyValue =
  | { readonly stringValue: string }
  | { readonly boolValue: boolean }
  | { readonly intValue: string }
  | { readonly doubleValue: number };

export interface OtlpKeyValue {
  readonly key: string;
  readonly value: OtlpAnyValue;
}

export interface OtlpResource {
  readonly attributes: readonly OtlpKeyValue[];
}

export interface OtlpInstrumentationScope {
  readonly name: string;
  readonly version?: string;
}

export interface OtlpTraceSpan {
  readonly traceId: string;
  readonly spanId: string;
  readonly parentSpanId?: string;
  readonly name: string;
  readonly kind: 1;
  readonly startTimeUnixNano: string;
  readonly endTimeUnixNano: string;
  readonly attributes: readonly OtlpKeyValue[];
  readonly status: { readonly code: 1 | 2 };
}

export interface OtlpScopeSpans {
  readonly scope: OtlpInstrumentationScope;
  readonly spans: readonly OtlpTraceSpan[];
}

export interface OtlpResourceSpans {
  readonly resource: OtlpResource;
  readonly scopeSpans: readonly OtlpScopeSpans[];
}

/** Standards-pure OTLP ExportTraceServiceRequest JSON body. */
export interface OtlpExportTraceServiceRequest {
  readonly resourceSpans: readonly OtlpResourceSpans[];
}

export interface OtlpLogRecord {
  readonly timeUnixNano: string;
  readonly observedTimeUnixNano: string;
  readonly severityNumber: 9 | 13 | 17;
  readonly severityText: "INFO" | "WARN" | "ERROR";
  readonly body: OtlpAnyValue;
  readonly attributes: readonly OtlpKeyValue[];
  readonly traceId?: string;
  readonly spanId?: string;
}

export interface OtlpScopeLogs {
  readonly scope: OtlpInstrumentationScope;
  readonly logRecords: readonly OtlpLogRecord[];
}

export interface OtlpResourceLogs {
  readonly resource: OtlpResource;
  readonly scopeLogs: readonly OtlpScopeLogs[];
}

/** Standards-pure OTLP ExportLogsServiceRequest JSON body. */
export interface OtlpExportLogsServiceRequest {
  readonly resourceLogs: readonly OtlpResourceLogs[];
}

export type OtlpExportRequest = OtlpExportTraceServiceRequest | OtlpExportLogsServiceRequest;

export interface ToOtlpOptions<Signal extends OtlpSignal = OtlpSignal> {
  readonly signal: Signal;
}

const TRACE_ID = /^[0-9a-f]{32}$/;
const SPAN_ID = /^[0-9a-f]{16}$/;
const SCOPE = Object.freeze({ name: "@aexhq/contracts", version: "1" });

export function toOTLP(
  events: readonly AexEvent[],
  options: ToOtlpOptions<"traces">
): OtlpExportTraceServiceRequest;
export function toOTLP(
  events: readonly AexEvent[],
  options: ToOtlpOptions<"logs">
): OtlpExportLogsServiceRequest;
export function toOTLP(events: readonly AexEvent[], options: ToOtlpOptions): OtlpExportRequest;
/**
 * Project public event envelopes into a standards-pure OTLP/HTTP JSON body.
 *
 * This function accepts only the public event contract. It never accepts or
 * imports private journal rows. Projection is allow-list based: arbitrary
 * event data is ignored, and text that is intentionally public is passed
 * through the canonical public redactor before it becomes telemetry.
 */
export function toOTLP(events: readonly AexEvent[], options: ToOtlpOptions): OtlpExportRequest {
  if (options.signal === "traces") return projectTraces(events);
  if (options.signal === "logs") return projectLogs(events);
  throw new TypeError(`unsupported OTLP signal: ${String((options as { signal?: unknown }).signal)}`);
}

function projectTraces(events: readonly AexEvent[]): OtlpExportTraceServiceRequest {
  const spans = events.flatMap((event) => {
    if (!validTraceIdentity(event)) return [];
    const start = unixNano(event.time);
    return [{
      traceId: event.traceId,
      spanId: event.spanId,
      ...(validSpanId(event.parentSpanId) ? { parentSpanId: event.parentSpanId } : {}),
      name: publicSpanName(event),
      kind: 1 as const,
      startTimeUnixNano: start,
      endTimeUnixNano: (BigInt(start) + 1n).toString(),
      attributes: publicAttributes(event),
      status: { code: event.type === "RUN_ERROR" ? 2 as const : 1 as const }
    }];
  });
  return {
    resourceSpans: spans.length === 0 ? [] : [{
      resource: publicResource(),
      scopeSpans: [{ scope: SCOPE, spans }]
    }]
  };
}

function projectLogs(events: readonly AexEvent[]): OtlpExportLogsServiceRequest {
  const logRecords = events
    .filter((event) => event.channel === "log" || event.type === "LOG")
    .map((event): OtlpLogRecord => {
      const time = unixNano(event.time);
      const severity = severityOf(event.level);
      const message = typeof event.message === "string"
        ? event.message
        : typeof event.data["message"] === "string"
          ? event.data["message"]
          : event.type;
      return {
        timeUnixNano: time,
        observedTimeUnixNano: time,
        ...severity,
        body: { stringValue: redactString(message) },
        attributes: publicAttributes(event),
        ...(validTraceId(event.traceId) ? { traceId: event.traceId } : {}),
        ...(validSpanId(event.spanId) ? { spanId: event.spanId } : {})
      };
    });
  return {
    resourceLogs: logRecords.length === 0 ? [] : [{
      resource: publicResource(),
      scopeLogs: [{ scope: SCOPE, logRecords }]
    }]
  };
}

function publicResource(): OtlpResource {
  return {
    attributes: [
      attribute("service.name", "aex"),
      attribute("aex.visibility", "external")
    ]
  };
}

function publicAttributes(event: AexEvent): readonly OtlpKeyValue[] {
  const attributes: OtlpKeyValue[] = [
    attribute("aex.visibility", "external"),
    attribute("aex.session.id", event.subject),
    attribute("aex.run.id", event.runId)
  ];
  const turnSeq = publicNumber(event.data["turnSeq"])
    ?? publicNestedNumber(event.data["value"], "turnSeq");
  if (turnSeq !== undefined) attributes.push(attribute("aex.turn.seq", turnSeq));
  const callId = publicString(event.data["callId"])
    ?? publicNestedString(event.data["fields"], "callId")
    ?? publicNestedString(event.data["value"], "callId");
  if (callId !== undefined) attributes.push(attribute("aex.call.id", callId));
  const failureClass = publicString(event.data["failureClass"]);
  if (failureClass !== undefined) attributes.push(attribute("aex.failure.class_public", failureClass));
  return attributes;
}

function publicSpanName(event: AexEvent): string {
  const customName = event.type === "CUSTOM" ? publicString(event.data["name"]) : undefined;
  return redactString(customName ?? event.type);
}

function attribute(key: string, value: string | number | boolean): OtlpKeyValue {
  if (typeof value === "string") return { key, value: { stringValue: redactString(value) } };
  if (typeof value === "boolean") return { key, value: { boolValue: value } };
  return Number.isSafeInteger(value)
    ? { key, value: { intValue: String(value) } }
    : { key, value: { doubleValue: value } };
}

function severityOf(level: AexLogLevel | undefined): Pick<OtlpLogRecord, "severityNumber" | "severityText"> {
  if (level === "error") return { severityNumber: 17, severityText: "ERROR" };
  if (level === "warn") return { severityNumber: 13, severityText: "WARN" };
  return { severityNumber: 9, severityText: "INFO" };
}

function unixNano(value: string): string {
  const milliseconds = Date.parse(value);
  return (BigInt(Number.isFinite(milliseconds) ? Math.max(0, Math.trunc(milliseconds)) : 0) * 1_000_000n).toString();
}

function validTraceIdentity(event: AexEvent): event is AexEvent & { readonly traceId: string; readonly spanId: string } {
  return validTraceId(event.traceId) && validSpanId(event.spanId);
}

function validTraceId(value: unknown): value is string {
  return typeof value === "string" && TRACE_ID.test(value) && !/^0+$/.test(value);
}

function validSpanId(value: unknown): value is string {
  return typeof value === "string" && SPAN_ID.test(value) && !/^0+$/.test(value);
}

function publicString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? redactString(value) : undefined;
}

function publicNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function publicNestedString(value: unknown, key: string): string | undefined {
  return isPlainRecord(value) ? publicString(value[key]) : undefined;
}

function publicNestedNumber(value: unknown, key: string): number | undefined {
  return isPlainRecord(value) ? publicNumber(value[key]) : undefined;
}

function isPlainRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
