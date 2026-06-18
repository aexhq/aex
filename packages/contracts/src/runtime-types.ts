/**
 * Loose record describing a run as the dashboard BFF returns it. Concrete
 * dashboard-managed fields appear in the index signature; the SDK and CLI
 * may surface them without strong typing per-field.
 *
 * `runtimeManifest` is derived from the validated submission + the chosen
 * provider on every read — see `runtime-manifest.ts`. It is `undefined`
 * on responses from BFFs that predate Phase 2 of the runtime-environment
 * rollout; SDK consumers MUST treat it as best-effort and not panic on
 * its absence.
 */
export interface Run {
  readonly id: string;
  readonly status: string;
  readonly workspaceId?: string;
  readonly createdAt?: string;
  readonly updatedAt?: string;
  readonly terminalAt?: string | null;
  /**
   * The run's EXECUTION start (ISO-8601) — when the agent actually began
   * running, distinct from {@link createdAt} (submission/accept time). Present
   * from the moment the run starts executing and throughout its live duration;
   * absent before it starts and after the run's live object is torn down (a
   * terminal run also carries {@link terminalAt} and {@link costTelemetry}
   * durations).
   */
  readonly startedAt?: string;
  readonly errorMessage?: string | null;
  /**
   * Aggregate token usage when a deployment chooses to expose it on the run
   * record. Mid-run this is not populated. Settled provider/runtime usage is
   * exposed through {@link costTelemetry}; per-turn usage breadcrumbs, when a
   * deployment records them internally, are not part of the normal public event
   * stream.
   */
  readonly usage?: UsageSummary;
  readonly costTelemetry?: import("./run-cost.js").RunCostTelemetry;
  readonly runtimeManifest?: import("./runtime-manifest.js").RuntimeManifest;
  readonly [key: string]: unknown;
}

export interface UsageSummary {
  readonly inputTokens?: number;
  readonly outputTokens?: number;
  readonly cacheReadInputTokens?: number;
  readonly cacheCreationInputTokens?: number;
  readonly totalTokens?: number;
}

/**
 * A run event as recorded by the dashboard. Includes the `type` field
 * that the `is*Event` type guards narrow on plus any provider payload.
 *
 * The unified-stream discriminators (`channel`, `source`, `sourceSeq`,
 * `emittedAt`, `receivedAt`, `level`) carry through from the coordinator
 * envelope (see `event-envelope.ts` —
 * {@link import("./event-envelope.js").AexEvent}) so SDK/CLI consumers can
 * split/filter the one stream by channel or source. All are OPTIONAL: archived
 * events from before the unification (and any producer that omits an ordering
 * attribute) lack them, an absent `channel` means `"event"` (see `channelOf`),
 * and `level` is present only on `log`-channel records.
 */
export interface RunEvent {
  readonly id: string;
  readonly type: string;
  readonly runId?: string;
  readonly recordedAt?: string;
  /** Which sub-stream this record rides — `"event"` (typed) or `"log"`. Absent ⇒ `"event"`. */
  readonly channel?: import("./event-envelope.js").AexEventChannel;
  /** Coarse origin classifier (agent/worker/runtime/mcp/aex/workflow/machine). */
  readonly source?: import("./event-envelope.js").AexEventSource;
  /** Per-source monotonic counter assigned at the source (carried, not re-ordered). */
  readonly sourceSeq?: number;
  /** Source wall-clock ms at emit (carried for a best-effort client time view). */
  readonly emittedAt?: number;
  /** The DO's authoritative receive-time (wall-clock ms) stamped at ingest, companion to `seq`. */
  readonly receivedAt?: number;
  /** Log severity, first-class on a `channel: "log"` record ("info" | "warn" | "error"). */
  readonly level?: import("./event-envelope.js").AexLogLevel;
  readonly [key: string]: unknown;
}

/**
 * Provider-emitted event payload. Provider-specific fields are passed through
 * structurally so historical events and managed-runner events can be displayed
 * and downloaded without each client needing provider-specific schemas.
 */
export interface ProviderEvent {
  readonly type: string;
  readonly id?: string | undefined;
  readonly created_at?: string | undefined;
  readonly [key: string]: unknown;
}

/**
 * One captured output file as the dashboard reports it. Use
 * `createOutputLink` to get a short-lived signed URL for download.
 */
export interface Output {
  readonly id: string;
  readonly filename?: string;
  readonly sizeBytes?: number;
  readonly contentType?: string;
  readonly createdAt?: string;
  readonly [key: string]: unknown;
}

export type OutputFilePathMatch = "exact" | "suffix";

export interface OutputFilePathSelector {
  readonly path: string;
  readonly match?: OutputFilePathMatch;
}

export interface OutputFileIdSelector {
  readonly id: string;
}

export type OutputFileSelector = Output | OutputFileIdSelector | OutputFilePathSelector;

export interface OutputFileDownload {
  readonly output: Output;
  readonly bytes: Uint8Array;
}

export interface SignedOutputLink {
  readonly url: string;
  readonly expiresAt?: string;
  readonly [key: string]: unknown;
}

export interface WhoAmI {
  readonly principalType: "api_token" | "user";
  readonly workspaceId?: string;
  readonly tokenId?: string;
  readonly tokenName?: string | null;
  readonly scopes?: readonly string[];
  /**
   * Workspace-level caps the BFF will enforce on subsequent calls.
   * Surfaced so consumers (e.g. broll's app-side admission gate) can
   * decide whether to keep their own gate or rely on platform headers.
   * All fields optional — older BFFs may omit. Numbers are concrete
   * snapshots at the time of the `whoami` call.
   */
  readonly caps?: {
    /** Token-bucket cap on POST /api/runs per minute, per workspace. */
    readonly runSubmitPerMinute?: number;
    /** Hard cap on concurrent non-terminal runs the workspace may hold. */
    readonly maxConcurrentRuns?: number;
    /** Storage cap (bytes) on captured output objects, workspace-wide. */
    readonly storageCapBytes?: number;
    /** Current captured-output usage in bytes. */
    readonly storageUsedBytes?: number;
    /**
     * Wall-clock ceiling on a single run before forced termination.
     * `null` means no aex-imposed cap, but this is **not unlimited
     * overall**: the managed runner, infrastructure, or upstream provider may
     * still impose a ceiling, and a run that exceeds it terminates regardless.
     */
    readonly maxRunDurationMs?: number | null;
  };
  readonly [key: string]: unknown;
}

/**
 * Workspace skill bundle as the dashboard BFF returns it. Mirrors a row
 * of `skill_bundles` joined with its computed manifest. `state` is the
 * upload lifecycle (`pending` -> `ready`); only `ready` rows are
 * referenceable from a run. `deletedAt` is the soft-delete tombstone
 * (`null` for live bundles).
 *
 * See the public architecture notes and server-side persistence schema for
 * the authoritative shape.
 */
export interface Skill {
  readonly id: string;
  readonly workspaceId?: string;
  readonly name: string;
  readonly state: "pending" | "ready";
  readonly hash?: string | null;
  readonly sizeBytes?: number | null;
  readonly fileCount?: number | null;
  readonly manifest?: ReadonlyArray<{
    readonly path: string;
    readonly size: number;
    readonly mode: number;
  }>;
  readonly createdAt?: string;
  readonly updatedAt?: string;
  readonly finalizedAt?: string | null;
  readonly deletedAt?: string | null;
  readonly [key: string]: unknown;
}

/**
 * Wire-level record for a workspace AgentsMd file as returned by the BFF.
 * Mirrors `PublicWorkspaceFile` from the dashboard service layer.
 */
export interface AgentsMdRecord {
  readonly id: string;
  readonly kind?: "agentsmd";
  readonly name: string;
  readonly state: "pending" | "ready";
  readonly hash?: string | null;
  readonly sizeBytes?: number | null;
  readonly fileCount?: number | null;
  readonly manifest?: ReadonlyArray<{
    readonly path: string;
    readonly size: number;
    readonly mode: number;
  }>;
  readonly createdAt?: string;
  readonly updatedAt?: string;
  readonly finalizedAt?: string | null;
  readonly deletedAt?: string | null;
  readonly [key: string]: unknown;
}

/**
 * Wire-level record for a workspace File as returned by the BFF.
 * Mirrors `PublicWorkspaceFile` from the dashboard service layer
 * with kind='file' and `f_*` ids.
 */
export interface FileRecord {
  readonly id: string;
  readonly kind?: "file";
  readonly name: string;
  readonly state: "pending" | "ready";
  readonly hash?: string | null;
  readonly sizeBytes?: number | null;
  readonly fileCount?: number | null;
  readonly manifest?: ReadonlyArray<{
    readonly path: string;
    readonly size: number;
    readonly mode: number;
  }>;
  readonly createdAt?: string;
  readonly updatedAt?: string;
  readonly finalizedAt?: string | null;
  readonly deletedAt?: string | null;
  readonly [key: string]: unknown;
}

/**
 * Wire-level record for a workspace secret as returned by the BFF.
 *
 * Workspace secrets share the lifecycle SEMANTIC of skills/files: a
 * `Secret.value(...)` is per-run and gone at terminal; PROMOTING it (or
 * `aex.secrets.set`) persists a named, searchable workspace secret. The
 * identity is the `name` (the handle a `Secret.ref` points at); the value
 * rotates under that stable name, bumping `version`.
 *
 * This record is METADATA ONLY — it never carries the secret value. The value
 * is write-only on create/rotate and readable solely via the audited
 * {@link SecretReveal} path.
 */
export interface SecretRecord {
  readonly id: string;
  readonly name: string;
  readonly version: number;
  readonly state: "ready";
  readonly createdAt?: string;
  readonly updatedAt?: string;
  readonly deletedAt?: string | null;
  readonly [key: string]: unknown;
}

/**
 * Value-bearing result of an audited `aex.secrets.reveal`. The ONLY wire shape
 * that carries a workspace secret value back to the caller. Reveal is a logged
 * action (POST, not GET) so a value read is always attributable.
 */
export interface SecretReveal {
  readonly name: string;
  readonly value: string;
}

