/**
 * TODO(cross-stream): the contracts stream owns the generated TypeScript payload
 * types. Until `packages/sdk/src/generated/**` lands, the dashboard declares the
 * shapes it actually reads, field-for-field from `api/generated/schemas/*.json`.
 * `test/wire.test.ts` fails if a required field named here is absent from the
 * checked-in schema, so these declarations cannot invent a field the API does not
 * have. When the generated types land, delete this file and import them.
 *
 * Only the fields the dashboard renders are declared. A field that is optional in
 * the schema is optional here, which is what forces every surface to handle its
 * absence rather than assume a value.
 */

export interface Page<T> {
  readonly items: readonly T[];
  readonly nextCursor?: string;
}

export interface SessionListItem {
  readonly id: string;
  readonly workspaceId: string;
  readonly status: "idle" | "running" | "awaiting_approval" | "deleting";
  readonly provider: string;
  readonly model: string;
  readonly revision: number;
  readonly createdAt: string;
  readonly updatedAt: string;
}

export interface Session {
  readonly id: string;
  readonly workspaceId: string;
  readonly status: "idle" | "running" | "awaiting_approval" | "deleting";
  readonly revision: number;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly metadata?: Readonly<Record<string, string>>;
  readonly continuity: { readonly persistRevision: number; readonly lastPersistedAt?: string };
  readonly resolvedConfig: {
    readonly provider: string;
    readonly model: string;
    readonly providerCredentialId?: string;
  };
}

export interface Run {
  readonly id: string;
  readonly sessionId: string;
  readonly messageId: string;
  readonly status: "queued" | "running" | "succeeded" | "failed" | "timed_out" | "cancelled" | "interrupted";
  readonly maxSpendCents: string;
  readonly queuedAt: string;
  readonly startedAt?: string;
  readonly terminalAt?: string;
  readonly telemetryComplete?: boolean;
  readonly telemetryGapIds?: readonly string[];
  readonly error?: { readonly code: string; readonly message: string; readonly requestId: string };
}

export interface Approval {
  readonly id: string;
  readonly sessionId: string;
  readonly status: string;
  readonly createdAt: string;
  readonly expiresAt: string;
  readonly resolvedAt?: string;
  readonly decision?: "approve" | "deny";
  readonly boundCall: {
    readonly toolName: string;
    readonly toolCallId: string;
    readonly runId: string;
    readonly agentId: string;
    readonly argumentsDigest: string;
    readonly implementationDigest: string;
  };
}

export interface MissingInterval {
  readonly gapId: string;
  readonly range: { readonly gte: string; readonly lt: string };
}

export interface ObservationCoverage {
  readonly accepted: string;
  readonly indexed: string;
  readonly snapshot: string;
  readonly earliestReplay: string;
  readonly caughtUp: boolean;
  readonly complete: boolean;
  readonly missingIntervals: readonly MissingInterval[];
  readonly unboundedGaps: readonly string[];
}

export interface Observation {
  readonly id: string;
  readonly workspaceId: string;
  readonly signal: "events" | "logs" | "spans" | "metrics" | "traces" | "telemetry";
  readonly sequence: string;
  readonly observedAt: string;
  readonly acceptedAt: string;
  readonly body: unknown;
  readonly sessionId?: string;
  readonly runId?: string;
  readonly traceId?: string;
  readonly spanId?: string;
}

export interface ObservationPage {
  readonly items: readonly Observation[];
  readonly coverage: ObservationCoverage;
  readonly nextCursor?: string;
}

export interface TraceDetail {
  readonly summary: {
    readonly traceId: string;
    readonly rootName?: string;
    readonly spanCount: string;
    readonly startedAt: string;
    readonly endedAt: string;
  };
  readonly spans: readonly Observation[];
  readonly coverage: ObservationCoverage;
}

export interface TelemetryGap {
  readonly id: string;
  readonly workspaceId: string;
  readonly reason: "admission_rejected" | "spool_lost" | "producer_dropped" | "replay_expired" | "authority_unavailable";
  readonly recoverable: boolean;
  readonly detectedAt: string;
  readonly signals: readonly string[];
  readonly timeRange: { readonly gte: string; readonly lt: string };
  readonly sessionId?: string;
  readonly byteCount?: string;
  readonly observationCount?: string;
  readonly repairedAt?: string;
}

export interface UsageFrontier {
  readonly category: "storage" | "compute" | "memory" | "data_transfer";
  readonly region: string;
  readonly serviceThrough: string;
  readonly workspaceId: string;
}

export interface UsageAttribution {
  readonly workspaceId: string;
  readonly region: string;
  readonly source: string;
  readonly ratedCents: string;
  readonly serviceTime: { readonly gte: string; readonly lt: string };
  readonly sessionId?: string;
  readonly runId?: string;
  readonly operationId?: string;
}

export type UsageAggregate =
  | { readonly category: "storage"; readonly attribution: UsageAttribution; readonly byteMinutes: string }
  | { readonly category: "compute"; readonly attribution: UsageAttribution; readonly millicpuMilliseconds: string }
  | { readonly category: "memory"; readonly attribution: UsageAttribution; readonly byteMilliseconds: string }
  | { readonly category: "data_transfer"; readonly attribution: UsageAttribution; readonly egressBytes: string };

export interface UsagePage {
  readonly items: readonly UsageAggregate[];
  readonly frontiers: readonly UsageFrontier[];
  readonly nextCursor?: string;
}

export interface BillingBalance {
  readonly organizationId: string;
  readonly availableCents: string;
  readonly pendingCents: string;
  readonly reservedCents: string;
  readonly currency: string;
  readonly revision: number;
  readonly updatedAt: string;
  readonly operationalState: { readonly status: "active" | "paused" };
}

export interface AutoTopupPolicy {
  readonly organizationId: string;
  readonly enabled: boolean;
  readonly amountCents: string;
  readonly thresholdCents: string;
  readonly revision: number;
  readonly updatedAt: string;
}

export interface StatementSummary {
  readonly id: string;
  readonly organizationId: string;
  readonly totalCents: string;
  readonly currency: string;
  readonly issuedAt: string;
  readonly artifactHash: string;
  readonly period: { readonly gte: string; readonly lt: string };
}

export interface HostedSession {
  readonly url: string;
  readonly expiresAt: string;
}

export interface DownloadGrant {
  readonly url: string;
  readonly expiresAt: string;
  readonly sizeBytes: string;
  readonly sha256: string;
  readonly authorizedBytes: string;
}

export interface ApiKey {
  readonly id: string;
  readonly workspaceId: string;
  readonly name: string;
  readonly scopes: readonly string[];
  readonly createdAt: string;
  readonly revokedAt?: string;
}

export interface NewApiKey extends ApiKey {
  readonly value: string;
}

export interface SecretMetadata {
  readonly name: string;
  readonly state: "ready" | "revoked";
  readonly revision: number;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly revokedAt?: string;
}

export interface RegisteredEntry {
  readonly name: string;
  readonly state: "current";
  readonly revision: number;
  readonly sha256: string;
  readonly sizeBytes: string;
  readonly createdAt: string;
  readonly updatedAt: string;
}

export type LimitValue =
  | { readonly shape: "scalar"; readonly value: string }
  | { readonly shape: "map"; readonly values: Readonly<Record<string, string>> };

export interface EffectiveWorkspaceLimit {
  readonly id: string;
  readonly source: "default" | "workspace_override";
  readonly revision: number;
  readonly changedAt: string;
  readonly effectiveValue: LimitValue;
}

export interface FileEntry {
  readonly path: string;
  readonly type: string;
  readonly mode: string;
  readonly mtime: string;
  readonly sizeBytes: string;
  readonly sha256?: string;
}
