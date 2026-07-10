/**
 * SessionUnit — the self-contained read shape of a session.
 *
 * One canonical struct that captures every non-secret artifact persisted
 * for a single session: parsed submission inputs, status/lifecycle, attempts,
 * indexed events, raw-event Storage manifest, session files, and capture failures.
 *
 * Wire contract for `GET /api/sessions/:sessionId`, the per-session archive's
 * `session.json`/`submission.json`/`caps.json`, and the SDK/CLI
 * `client.sessions.get(sessionId)` return type.
 *
 * Immutability: every field here is read-only. Edit endpoints do not
 * exist by design.
 *
 * Raw event payloads are not embedded in this struct. They live in
 * private object storage as gzipped JSONL pages and are listed via
 * `rawEventPages` (manifest only; bytes downloaded out-of-band so the
 * detail response stays bounded). The archive zip carries the bytes.
 */

import type { McpServerRef } from "./session-config.js";
import { parseMcpServerRef } from "./session-config.js";
import type { CleanupStatus } from "./status.js";
import { CLEANUP_STATUSES } from "./status.js";
import type {
  JsonValue,
  PlatformPackage,
  PlatformPackageEcosystem,
  PlatformSubmission,
  PlatformEnvironment
} from "./submission.js";
import type { RuntimeSecurityProfileName } from "./runtime-security-profile.js";
import { Models, parseModelName } from "./models.js";
import { PLATFORM_PACKAGE_ECOSYSTEMS } from "./submission.js";

// ---------------------------------------------------------------------------
// Submission projection
// ---------------------------------------------------------------------------

/**
 * Parsed view of the legacy session snapshot jsonb. Stored shape is
 * `{kind:"submission", submission}` written by the hosted API's
 * server-side session creation for all sessions.
 */
export type SessionUnitSubmission = SessionUnitFlatSubmission;

export interface SessionUnitFlatSubmission {
  readonly kind: "submission";
  readonly submission: PlatformSubmission;
}

// ---------------------------------------------------------------------------
// Child rows (read-side)
// ---------------------------------------------------------------------------

export interface SessionUnitAttempt {
  readonly id: string;
  readonly attemptNumber: number;
  readonly status: string;
  readonly providerSessionId?: string;
  readonly errorClass?: string;
  readonly errorCode?: string;
  readonly errorMessage?: string;
  readonly startedAt?: string;
  readonly terminalAt?: string;
  readonly createdAt: string;
}

/**
 * Indexed event metadata (dedupe-key + summary). Raw payload bytes for
 * each event are NOT here — they ride in `rawEventPages`. This struct
 * stays small so the detail response is bounded.
 */
export interface SessionUnitEvent {
  readonly id: string;
  readonly attemptId?: string;
  readonly providerEventId?: string;
  readonly type: string;
  readonly summary?: string;
  readonly occurredAt?: string;
  readonly processedAt: string;
  readonly usageDelta?: Record<string, JsonValue>;
}

/**
 * Inline slice of events plus an optional cursor for the tail. Most
 * sessions fit entirely inline; long-running ones overflow and the
 * consumer paginates via `GET /api/sessions/:sessionId/events?cursor=...`.
 */
export interface SessionUnitEventPage {
  readonly entries: readonly SessionUnitEvent[];
  readonly totalCount: number;
  readonly truncated: boolean;
  readonly nextCursor?: string;
}

/**
 * One gzipped JSONL page of raw provider events captured for the session record.
 * Bytes are downloaded through auth-gated routes or surfaced inside the
 * per-session archive zip. `artifactPath` is session-record relative.
 */
export interface SessionUnitRawEventPage {
  readonly attempt: number;
  readonly page: number;
  readonly byteSize: number;
  readonly eventCount: number;
  readonly artifactPath: string;
  readonly contentEncoding: "gzip";
  readonly createdAt: string;
}

export interface SessionUnitFile {
  readonly id: string;
  readonly fileName: string;
  readonly byteSize: number;
  readonly contentType?: string;
}

export interface SessionUnitFileCaptureFailure {
  readonly id: string;
  readonly providerFileId?: string;
  readonly filename?: string;
  readonly byteSize?: number;
  readonly reason: string;
  readonly errorMessage?: string;
  readonly createdAt: string;
}

// ---------------------------------------------------------------------------
// Top-level SessionUnit
// ---------------------------------------------------------------------------

export interface SessionUnit {
  readonly id: string;
  readonly workspaceId: string;
  readonly status: string;
  readonly lifecyclePhase?: string;
  readonly cleanupStatus: CleanupStatus;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly startedAt?: string;
  readonly terminalAt?: string;
  readonly deletedAt?: string;
  readonly attemptCount: number;
  readonly submission: SessionUnitSubmission;
  readonly capsSnapshot?: Record<string, JsonValue>;
  readonly attempts: readonly SessionUnitAttempt[];
  readonly events: SessionUnitEventPage;
  readonly rawEventPages: readonly SessionUnitRawEventPage[];
  readonly sessionFiles: readonly SessionUnitFile[];
  readonly fileCaptureFailures: readonly SessionUnitFileCaptureFailure[];
  readonly costTelemetry?: import("./session-cost.js").SessionCostTelemetry;
  /**
   * Per-session, per-provider runtime manifest — derived from the validated
   * submission + the chosen provider (`buildRuntimeManifest`). Tells
   * SDK consumers where aex placed things in-container and what
   * env vars the agent will see. Undefined on responses from BFFs
   * that predate Phase 2 of the runtime-environment rollout.
   */
  readonly runtimeManifest?: import("./runtime-manifest.js").RuntimeManifest;
}

// ---------------------------------------------------------------------------
// Submission parser
// ---------------------------------------------------------------------------

/**
 * Parse a legacy session snapshot jsonb payload into the typed flat
 * submission. Never throws on minor unknown keys so we can
 * forward-compat with hosted API enrichment.
 *
 * Returns a typed shape even for malformed snapshots by falling back to the
 * default public model and empty collection defaults, because the dashboard
 * must still render *something* for a buggy historical row rather than 500ing
 * the whole detail page.
 */
export function parseSessionUnitSubmission(input: unknown, fallbackModel?: unknown): SessionUnitSubmission {
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    return fallbackFlat(fallbackModel);
  }
  const value = input as Record<string, unknown>;
  if (value.kind === "submission") {
    return parseFlatProjection(value, fallbackModel);
  }
  // Snapshot exists but does not match the flat shape — surface as an
  // empty flat submission so consumers can still render lifecycle bits.
  return fallbackFlat(fallbackModel);
}

function parseFlatProjection(value: Record<string, unknown>, fallbackModel?: unknown): SessionUnitFlatSubmission {
  const submissionRaw = isRecord(value.submission) ? value.submission : {};
  const fileCaptureRaw = isRecord(submissionRaw.fileCapture) ? submissionRaw.fileCapture : {};
  const allowedDirs = toOptionalStringArray(fileCaptureRaw.allowedDirs);
  const deniedDirs = toOptionalStringArray(fileCaptureRaw.deniedDirs);
  const captureTimeoutMs = toOptionalPositiveInteger(fileCaptureRaw.captureTimeoutMs);
  const maxFileBytes = toOptionalPositiveInteger(fileCaptureRaw.maxFileBytes);
  const maxTotalBytes = toOptionalPositiveInteger(fileCaptureRaw.maxTotalBytes);
  const maxFiles = toOptionalPositiveInteger(fileCaptureRaw.maxFiles);
  const submission: PlatformSubmission = {
    model: coerceSessionUnitModel(submissionRaw.model ?? fallbackModel),
    ...(typeof submissionRaw.system === "string" ? { system: submissionRaw.system } : {}),
    prompt: toStringArray(submissionRaw.prompt),
    agentsMd: [],
    files: [],
    mcpServers: toMcpServerRefArray(submissionRaw.mcpServers),
    tools: [],
    ...(parseEnvironment(submissionRaw.environment)
      ? { environment: parseEnvironment(submissionRaw.environment) as PlatformEnvironment }
      : {}),
    ...(parseSecurityProfile(submissionRaw.securityProfile)
      ? { securityProfile: parseSecurityProfile(submissionRaw.securityProfile) as RuntimeSecurityProfileName }
      : {}),
    ...(isJsonRecord(submissionRaw.metadata) ? { metadata: submissionRaw.metadata as Record<string, JsonValue> } : {}),
    ...(allowedDirs ||
    deniedDirs ||
    captureTimeoutMs !== undefined ||
    maxFileBytes !== undefined ||
    maxTotalBytes !== undefined ||
    maxFiles !== undefined
      ? {
          fileCapture: {
            ...(allowedDirs ? { allowedDirs } : {}),
            ...(deniedDirs ? { deniedDirs } : {}),
            ...(captureTimeoutMs !== undefined ? { captureTimeoutMs } : {}),
            ...(maxFileBytes !== undefined ? { maxFileBytes } : {}),
            ...(maxTotalBytes !== undefined ? { maxTotalBytes } : {}),
            ...(maxFiles !== undefined ? { maxFiles } : {})
          }
        }
      : {})
  };

  return {
    kind: "submission",
    submission
  };
}

function parseSecurityProfile(value: unknown): RuntimeSecurityProfileName | undefined {
  return value === "strict" || value === "standard" || value === "developer" ? value : undefined;
}

function toOptionalPositiveInteger(value: unknown): number | undefined {
  return typeof value === "number" && Number.isInteger(value) && value > 0 ? value : undefined;
}

function fallbackFlat(fallbackModel?: unknown): SessionUnitFlatSubmission {
  return {
    kind: "submission",
    submission: {
      model: coerceSessionUnitModel(fallbackModel),
      prompt: [],
      agentsMd: [],
      files: [],
      mcpServers: [],
      tools: []
    }
  };
}

// ---------------------------------------------------------------------------
// Coercion helpers — deliberately lenient. We never throw on a single
// malformed sub-field; we collapse it to a safe default and keep going
// so a dashboard read of a malformed snapshot still surfaces the rest.
// ---------------------------------------------------------------------------

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Normalize a `GET /api/sessions/:sessionId` payload into a SessionUnit whose type contract
 * holds AT RUNTIME. The managed (AWS) plane returns a LEAN record (scalars +
 * costTelemetry only) and omits the aggregate collections; the SessionUnit type
 * declares those non-optional, so a naive cast leaves `unit.sessionFiles` /
 * `unit.events.totalCount` `undefined` and a typed consumer crashes on
 * `.map()` / `.totalCount` (pre-launch edge-sweep F25). We fill the aggregates
 * with their empty defaults so array/page access is always safe. NOTE: on the
 * managed plane these summaries are best-effort — read `files()` / `events()`
 * / `messages()` for the authoritative per-session data.
 */
export function normalizeSessionUnit(raw: unknown): SessionUnit {
  const r: Record<string, unknown> = isRecord(raw) ? raw : {};
  const eventsRaw: Record<string, unknown> = isRecord(r.events) ? r.events : {};
  const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
  const arr = <T>(v: unknown): readonly T[] => (Array.isArray(v) ? (v as readonly T[]) : []);
  return {
    id: str(r.id) ?? "",
    workspaceId: str(r.workspaceId) ?? "",
    status: str(r.status) ?? "unknown",
    ...(str(r.lifecyclePhase) ? { lifecyclePhase: r.lifecyclePhase as string } : {}),
    cleanupStatus: (CLEANUP_STATUSES as readonly string[]).includes(r.cleanupStatus as string)
      ? (r.cleanupStatus as CleanupStatus)
      : "not_started",
    createdAt: str(r.createdAt) ?? "",
    updatedAt: str(r.updatedAt) ?? "",
    ...(str(r.startedAt) ? { startedAt: r.startedAt as string } : {}),
    ...(str(r.terminalAt) ? { terminalAt: r.terminalAt as string } : {}),
    ...(str(r.deletedAt) ? { deletedAt: r.deletedAt as string } : {}),
    attemptCount:
      typeof r.attemptCount === "number"
        ? r.attemptCount
        : Array.isArray(r.attempts)
          ? r.attempts.length
          : 0,
    // Plane responses that project a flat record (no `submission` snapshot)
    // still carry the session's `model` at the top level — prefer it over the
    // static fallback so `unit()` never claims a model the session did not use.
    submission: parseSessionUnitSubmission(r.submission, r.model),
    ...(isRecord(r.capsSnapshot) ? { capsSnapshot: r.capsSnapshot as Record<string, JsonValue> } : {}),
    attempts: arr<SessionUnitAttempt>(r.attempts),
    events: {
      entries: arr<SessionUnitEvent>(eventsRaw.entries),
      totalCount: typeof eventsRaw.totalCount === "number" ? eventsRaw.totalCount : 0,
      truncated: eventsRaw.truncated === true,
      ...(str(eventsRaw.nextCursor) ? { nextCursor: eventsRaw.nextCursor as string } : {})
    },
    rawEventPages: arr<SessionUnitRawEventPage>(r.rawEventPages),
    sessionFiles: arr<SessionUnitFile>(r.sessionFiles),
    fileCaptureFailures: arr<SessionUnitFileCaptureFailure>(r.fileCaptureFailures),
    ...(isRecord(r.costTelemetry)
      ? { costTelemetry: r.costTelemetry as unknown as NonNullable<SessionUnit["costTelemetry"]> }
      : {}),
    ...(isRecord(r.runtimeManifest)
      ? { runtimeManifest: r.runtimeManifest as unknown as NonNullable<SessionUnit["runtimeManifest"]> }
      : {})
  };
}

function coerceSessionUnitModel(value: unknown) {
  if (typeof value !== "string") return Models.CLAUDE_HAIKU_4_5;
  try {
    return parseModelName(value, "session unit submission.model");
  } catch {
    return Models.CLAUDE_HAIKU_4_5;
  }
}

function isJsonRecord(value: unknown): boolean {
  return isRecord(value);
}

function toStringArray(value: unknown): readonly string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.filter((item): item is string => typeof item === "string");
}

function toOptionalStringArray(value: unknown): readonly string[] | undefined {
  if (!Array.isArray(value) || value.length === 0) {
    return undefined;
  }
  const filtered = value.filter((item): item is string => typeof item === "string");
  return filtered.length === 0 ? undefined : filtered;
}

function toMcpServerRefArray(value: unknown): readonly McpServerRef[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const out: McpServerRef[] = [];
  for (let i = 0; i < value.length; i++) {
    try {
      out.push(parseMcpServerRef(value[i], `submission.mcpServers[${i}]`));
    } catch {
      // ignore malformed
    }
  }
  return out;
}

function parseEnvironment(value: unknown): PlatformEnvironment | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const env: { networking?: unknown; packages?: unknown; envVars?: Record<string, string> } = {};
  if (isRecord(value.networking)) {
    const mode = (value.networking as Record<string, unknown>).mode;
    const allowedHosts = (value.networking as Record<string, unknown>).allowedHosts;
    if (mode === "limited" || mode === "open") {
      env.networking = {
        mode,
        ...(Array.isArray(allowedHosts)
          ? { allowedHosts: toStringArray(allowedHosts) }
          : {})
      };
    }
  }
  if (Array.isArray(value.packages)) {
    const pkgs = (value.packages as unknown[])
      .filter(isRecord)
      .map((p): PlatformPackage | null => {
        const r = p as Record<string, unknown>;
        if (typeof r.name !== "string") return null;
        // Snapshots persisted after the ecosystem split carry it verbatim;
        // older snapshots (pre-split) default to `apt` to match the strict
        // parser's unprefixed default.
        const ecosystem: PlatformPackageEcosystem =
          typeof r.ecosystem === "string" &&
          (PLATFORM_PACKAGE_ECOSYSTEMS as readonly string[]).includes(r.ecosystem)
            ? (r.ecosystem as PlatformPackageEcosystem)
            : "apt";
        return {
          name: r.name,
          ...(typeof r.version === "string" ? { version: r.version } : {}),
          ecosystem
        };
      })
      .filter((p): p is PlatformPackage => p !== null);
    if (pkgs.length > 0) {
      env.packages = pkgs;
    }
  }
  // Lenient pass-through for envVars stored in already-validated
  // snapshots. The strict parser in submission.ts enforces shape /
  // size / reserved-prefix rules at submission time; here we just
  // accept whatever shape was persisted.
  if (isRecord(value.envVars)) {
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(value.envVars as Record<string, unknown>)) {
      if (typeof v === "string") {
        out[k] = v;
      }
    }
    if (Object.keys(out).length > 0) {
      env.envVars = out;
    }
  }
  return env.networking || env.packages || env.envVars
    ? (env as PlatformEnvironment)
    : undefined;
}

