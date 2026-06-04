/**
 * RunUnit — the self-contained read shape of a run.
 *
 * One canonical struct that captures every non-secret artifact persisted
 * for a single run: parsed submission inputs, status/lifecycle, attempts,
 * indexed events, raw-event Storage manifest, outputs (+ capture
 * failures), proxy-call audit log, pinned workspace skills, provider
 * built-in skills, and transient (Anthropic Files) skill records.
 *
 * Wire contract for `GET /api/runs/:runId`, the per-run archive's
 * `run.json`/`submission.json`/`caps.json`, and the SDK/CLI
 * `client.runs.get(runId)` return type.
 *
 * Immutability: every field here is read-only. Edit endpoints do not
 * exist by design.
 *
 * Raw event payloads are not embedded in this struct. They live in
 * private object storage as gzipped JSONL pages and are listed via
 * `rawEventPages` (manifest only; bytes downloaded out-of-band so the
 * detail response stays bounded). The archive zip carries the bytes.
 */

import type { McpServerRef, SkillRef } from "./run-config.js";
import {
  parseMcpServerRef,
  parseSkillRef
} from "./run-config.js";
import type { CleanupStatus } from "./status.js";
import type {
  JsonValue,
  PlatformPackage,
  PlatformPackageEcosystem,
  PlatformSubmission,
  PlatformProxyEndpoint,
  PlatformEnvironment
} from "./submission.js";
import type { RuntimeSecurityProfileName } from "./runtime-security-profile.js";
import { PLATFORM_PACKAGE_ECOSYSTEMS } from "./submission.js";

// ---------------------------------------------------------------------------
// Submission projection
// ---------------------------------------------------------------------------

/**
 * Parsed view of the legacy run snapshot jsonb. Stored shape is
 * `{kind:"submission", submission}` written by the hosted API's
 * server-side run creation for all runs.
 */
export type RunUnitSubmission = RunUnitFlatSubmission;

export interface RunUnitFlatSubmission {
  readonly kind: "submission";
  readonly submission: PlatformSubmission;
}

// ---------------------------------------------------------------------------
// Child rows (read-side)
// ---------------------------------------------------------------------------

export interface RunUnitAttempt {
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
export interface RunUnitEvent {
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
 * runs fit entirely inline; long-running ones overflow and the
 * consumer paginates via `GET /api/runs/:runId/events?cursor=...`.
 */
export interface RunUnitEventPage {
  readonly entries: readonly RunUnitEvent[];
  readonly totalCount: number;
  readonly truncated: boolean;
  readonly nextCursor?: string;
}

/**
 * One gzipped JSONL page of raw provider events captured to Storage.
 * Bytes are downloaded via signed URL or surfaced inside the per-run
 * archive zip. `storagePath` is bucket-relative; the BFF turns it
 * into a signed URL for clients.
 */
export interface RunUnitRawEventPage {
  readonly attempt: number;
  readonly page: number;
  readonly byteSize: number;
  readonly eventCount: number;
  readonly storagePath: string;
  readonly contentEncoding: "gzip";
  readonly createdAt: string;
}

export interface RunUnitOutput {
  readonly id: string;
  readonly fileName: string;
  readonly byteSize: number;
  readonly contentType?: string;
}

export interface RunUnitOutputCaptureFailure {
  readonly id: string;
  readonly providerFileId?: string;
  readonly filename?: string;
  readonly byteSize?: number;
  readonly reason: string;
  readonly errorMessage?: string;
  readonly createdAt: string;
}

export interface RunUnitProxyCall {
  readonly id: string;
  readonly endpointName: string;
  readonly method: string;
  readonly requestPathRedacted: string | null;
  readonly requestByteSize: number;
  readonly responseStatus: number | null;
  readonly responseByteSize: number;
  readonly outcome: string;
  readonly errorClass: string | null;
  readonly startedAt: string;
  readonly finishedAt: string | null;
  readonly durationMs: number | null;
}

export interface RunUnitProxyCallPage {
  readonly entries: readonly RunUnitProxyCall[];
  readonly totalCount: number;
  readonly truncated: boolean;
  readonly nextCursor?: string;
}

/**
 * Workspace skill bundle pinned at submission. `liveSkillId` is `null`
 * when the corresponding `skill_bundles` row has been soft-deleted —
 * the UI uses that to render a tombstoned link.
 */
export interface RunUnitSkillSnapshot {
  readonly skillId: string;
  readonly name: string;
  readonly hash: string;
  readonly sizeBytes: number;
  readonly fileCount: number;
  readonly liveSkillId: string | null;
}

export interface RunUnitProviderSkill {
  readonly vendor: string;
  readonly skillId: string;
  readonly version?: string;
}

export interface RunUnitInlineSkill {
  readonly id: string;
  readonly slotId: string;
  readonly skillName: string;
  readonly contentHash: string;
  readonly anthropicFileId: string | null;
  readonly status: string;
  readonly createdAt: string;
  readonly updatedAt: string;
}

// ---------------------------------------------------------------------------
// Top-level RunUnit
// ---------------------------------------------------------------------------

export interface RunUnit {
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
  readonly submission: RunUnitSubmission;
  readonly capsSnapshot?: Record<string, JsonValue>;
  readonly proxyEndpointsSnapshot?: readonly PlatformProxyEndpoint[];
  readonly attempts: readonly RunUnitAttempt[];
  readonly events: RunUnitEventPage;
  readonly rawEventPages: readonly RunUnitRawEventPage[];
  readonly outputs: readonly RunUnitOutput[];
  readonly outputCaptureFailures: readonly RunUnitOutputCaptureFailure[];
  readonly costTelemetry?: import("./run-cost.js").RunCostTelemetry;
  readonly proxyCalls: RunUnitProxyCallPage;
  readonly skillSnapshots: readonly RunUnitSkillSnapshot[];
  readonly providerSkills: readonly RunUnitProviderSkill[];
  readonly inlineSkills: readonly RunUnitInlineSkill[];
  /**
   * Per-run, per-provider runtime manifest — derived from the validated
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
 * Parse a legacy run snapshot jsonb payload into the typed flat
 * submission. Never throws on minor unknown keys so we can
 * forward-compat with worker-side enrichment.
 *
 * Returns a typed shape even for malformed snapshots — the worst case
 * is `{kind: "submission", submission: {model: "", ...}}` with empty
 * defaults — because the dashboard must still render *something* for a
 * buggy historical row rather than 500ing the whole detail page.
 */
export function parseRunUnitSubmission(input: unknown): RunUnitSubmission {
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    return fallbackFlat();
  }
  const value = input as Record<string, unknown>;
  if (value.kind === "submission") {
    return parseFlatProjection(value);
  }
  // Snapshot exists but does not match the flat shape — surface as an
  // empty flat submission so consumers can still render lifecycle bits.
  return fallbackFlat();
}

function parseFlatProjection(value: Record<string, unknown>): RunUnitFlatSubmission {
  const submissionRaw = isRecord(value.submission) ? value.submission : {};
  const outputsRaw = isRecord(submissionRaw.outputs) ? submissionRaw.outputs : {};
  const allowedDirs = toOptionalStringArray(outputsRaw.allowedDirs);
  const deniedDirs = toOptionalStringArray(outputsRaw.deniedDirs);
  const submission: PlatformSubmission = {
    model: typeof submissionRaw.model === "string" ? submissionRaw.model : "",
    ...(typeof submissionRaw.system === "string" ? { system: submissionRaw.system } : {}),
    prompt: toStringArray(submissionRaw.prompt),
    skills: toSkillRefArray(submissionRaw.skills),
    agentsMd: [],
    files: [],
    mcpServers: toMcpServerRefArray(submissionRaw.mcpServers),
    ...(parseEnvironment(submissionRaw.environment)
      ? { environment: parseEnvironment(submissionRaw.environment) as PlatformEnvironment }
      : {}),
    ...(parseSecurityProfile(submissionRaw.securityProfile)
      ? { securityProfile: parseSecurityProfile(submissionRaw.securityProfile) as RuntimeSecurityProfileName }
      : {}),
    ...(isJsonRecord(submissionRaw.metadata) ? { metadata: submissionRaw.metadata as Record<string, JsonValue> } : {}),
    ...(allowedDirs || deniedDirs
      ? {
          outputs: {
            ...(allowedDirs ? { allowedDirs } : {}),
            ...(deniedDirs ? { deniedDirs } : {})
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

function fallbackFlat(): RunUnitFlatSubmission {
  return {
    kind: "submission",
    submission: {
      model: "",
      prompt: [],
      skills: [],
      agentsMd: [],
      files: [],
      mcpServers: []
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

function toSkillRefArray(value: unknown): readonly SkillRef[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const out: SkillRef[] = [];
  for (let i = 0; i < value.length; i++) {
    try {
      out.push(parseSkillRef(value[i], `submission.skills[${i}]`));
    } catch {
      // Skip malformed entries rather than failing the whole detail
      // read. Worker-side enrichment may add fields we don't recognise.
    }
  }
  return out;
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

