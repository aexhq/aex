import type { SessionStatus } from "./status.js";
import type {
  PlatformRunSubmissionInput,
  PlatformSubmission
} from "./submission.js";

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

export type SessionTurnStatus =
  | "none"
  | "launching"
  | "running"
  | "parking"
  | "idle"
  | "suspended"
  | "failed";

export interface SessionTurn {
  readonly sessionId: string;
  readonly turnSeq: number;
  readonly turnId?: string;
  readonly status?: SessionTurnStatus;
  readonly startedAt?: string;
  readonly endedAt?: string | null;
  readonly eventCursor?: number;
}

export interface Session {
  readonly id: string;
  readonly sessionId?: string;
  readonly status: SessionStatus | string;
  readonly turnSeq?: number;
  readonly turnStatus?: SessionTurnStatus;
  /** How long the session may remain idle before it is suspended. */
  readonly idleTtl?: string;
  readonly idleTtlMs?: number;
  readonly createdAt?: string;
  readonly updatedAt?: string;
  readonly idleAt?: string | null;
  readonly suspendedAt?: string | null;
  readonly activeDurationMs?: number;
  readonly lastTurnDurationMs?: number;
  readonly retainedStorageBytes?: number;
  readonly usage?: UsageSummary;
  readonly costUsd?: number;
  readonly errorMessage?: string | null;
  /**
   * Settle-written failure taxonomy for an `error` session (e.g.
   * `provider-permanent`, `budget_exhausted`) — the class a caller can branch
   * on, complementing the human-readable `errorMessage`.
   */
  readonly failureClass?: string | null;
  readonly [key: string]: unknown;
}

export interface SessionSummary {
  readonly id: string;
  readonly sessionId?: string;
  readonly status: SessionStatus | string;
  readonly turnSeq?: number;
  /** How long the session may remain idle before it is suspended. */
  readonly idleTtl?: string;
  readonly idleTtlMs?: number;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly idleAt?: string | null;
  readonly suspendedAt?: string | null;
  readonly activeDurationMs?: number;
  readonly retainedStorageBytes?: number;
  readonly costUsd?: number;
}

export interface SessionListQuery {
  readonly status?: string;
  readonly since?: string;
  readonly limit?: number;
  readonly cursor?: string;
}

export interface SessionListPage {
  readonly sessions: readonly SessionSummary[];
  readonly nextCursor?: string;
}

export interface SessionRetentionPolicy {
  /** How long the session may remain idle before it is suspended. */
  readonly idleTtl?: string;
}

export type SessionSubmission = Omit<PlatformSubmission, "prompt"> & {
  readonly prompt?: readonly string[];
};

export type SessionCreateRequest = Omit<
  PlatformRunSubmissionInput,
  "idempotencyKey" | "submission"
> & {
  readonly submission: SessionSubmission;
  readonly input?: string | readonly string[];
  readonly retention?: SessionRetentionPolicy;
};

export interface SessionMessageRequest {
  readonly input: string | readonly string[];
  readonly metadata?: Readonly<Record<string, unknown>>;
}

export interface SessionMessageAccepted {
  readonly session: Session;
  readonly turn: SessionTurn;
  readonly eventCursor?: number;
}

export type SessionMessageSender = "user" | "assistant" | "system" | "tool";

export interface SessionMessage {
  readonly id: string;
  readonly sender: SessionMessageSender;
  readonly text: string;
  readonly timestamp?: string;
  readonly turnSeq?: number;
  readonly sequence?: number;
  readonly messageId?: string;
  readonly content?: unknown;
  readonly [key: string]: unknown;
}

export interface SessionMessagesQuery {
  readonly limit?: number;
  readonly cursor?: string;
  readonly since?: string;
}

export interface SessionMessagesPage {
  readonly messages: readonly SessionMessage[];
  readonly nextCursor?: string;
}

export interface SessionStateChangeAccepted {
  readonly session: Session;
  readonly turn?: SessionTurn;
  readonly eventCursor?: number;
}

export type SessionEvent = import("./event-envelope.js").AexEvent;

export interface UsageSummary {
  readonly inputTokens?: number;
  readonly outputTokens?: number;
  readonly cacheReadInputTokens?: number;
  readonly cacheCreationInputTokens?: number;
  readonly totalTokens?: number;
}

/**
 * Filters for {@link import("./operations.js").listRuns} / `Aex.runs.list`.
 * Every field is optional; omitting all of them lists the most recent runs in the
 * token's workspace. Workspace identity is derived server-side from the API token,
 * so there is no `workspaceId` here — a token can only ever enumerate its own runs.
 */
export interface RunListQuery {
  /** Restrict to a single run status, e.g. `"succeeded"`. */
  readonly status?: string;
  /** ISO-8601 lower bound on `createdAt` (inclusive). */
  readonly since?: string;
  /** Page size. Defaults to 25, clamped server-side to [1, 100]. */
  readonly limit?: number;
  /** Opaque keyset cursor from a prior page's `nextCursor`. */
  readonly cursor?: string;
}

/**
 * A public-safe run summary as returned by `GET /api/runs` (the workspace run
 * list). DELIBERATELY omits the submission snapshot (model/prompt/env) — the full,
 * redaction-scanned submission is only reachable through `getRunUnit(runId)`.
 */
export interface RunSummary {
  readonly id: string;
  readonly status: string;
  readonly createdAt: string;
  readonly updatedAt: string;
  /** Settled showback estimate (USD), present once the run has cost telemetry. */
  readonly costUsd?: number;
}

/** One page of the workspace run list. `nextCursor` absent ⇒ last page. */
export interface RunListPage {
  readonly runs: readonly RunSummary[];
  readonly nextCursor?: string;
}

/**
 * Cross-run output search query (`Aex.outputs.search`). Restrict to a
 * corpus with `runIds`; filter by filename substring / extension / content type.
 * The MVP composes this client-side (per-run `listOutputs` + filter) — a future
 * server-side `GET /api/outputs/search` can back the same contract with a real
 * cross-run index, body-only swap.
 */
export interface OutputSearchQuery {
  /** Restrict the search to these runs (the chat corpus allow-list). */
  readonly runIds?: readonly string[];
  /** Case-insensitive substring match on the output filename. */
  readonly filename?: string;
  /** File extension, with or without a leading dot. Case-insensitive. */
  readonly extension?: string;
  /** Exact content type or a prefix wildcard such as `image/*`. */
  readonly contentType?: string;
  /** Cap the number of hits returned (default 100). */
  readonly limit?: number;
}

/** One output-search hit — a reference only (no bytes); read with `readOutputText`. */
export interface OutputSearchHit {
  readonly runId: string;
  readonly outputId: string;
  readonly filename?: string;
  readonly sizeBytes?: number;
  readonly contentType?: string;
}

/** A page of output-search hits. */
export interface OutputSearchPage {
  readonly hits: readonly OutputSearchHit[];
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
  /** Coarse origin classifier. See {@link AexEventSource}. */
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

/** Status of a per-run webhook delivery. Terminal: delivered/exhausted/invalid. */
export type RunWebhookDeliveryStatus =
  | "pending"
  | "delivering"
  | "retrying"
  | "delivered"
  | "exhausted"
  | "invalid";

/**
 * One row of a run's webhook delivery ledger, as returned by
 * `GET /api/runs/:id/webhook-deliveries`. `id` is the stable `webhook-id`
 * header the consumer dedupes on across retries; the optional fields are
 * populated only once a delivery attempt has been made.
 */
export interface RunWebhookDelivery {
  readonly id: string;
  readonly eventType: string;
  readonly status: RunWebhookDeliveryStatus;
  readonly attemptCount: number;
  readonly lastStatusCode?: number;
  readonly lastError?: string;
  readonly nextAttemptAt?: string;
  readonly createdAt: string;
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
 * `outputLink` / `createOutputLink` to get a temporary direct URL for download.
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

export type OutputFileType =
  | "text"
  | "json"
  | "image"
  | "audio"
  | "video"
  | "pdf"
  | "archive"
  | "binary"
  | "unknown";

export interface OutputQuery {
  /** Exact normalized output path. Leading `/` and `outputs/` are ignored. */
  readonly path?: string;
  /** Basename match. A RegExp is tested against the basename only. */
  readonly filename?: string | RegExp;
  /**
   * Directory prefix. Leading `/` and `outputs/` are ignored.
   * `recursive` defaults to true.
   */
  readonly dir?: string;
  readonly recursive?: boolean;
  /** File extension, with or without a leading dot. Case-insensitive. */
  readonly extension?: string;
  /** Exact content type or a prefix wildcard such as `image/*`. */
  readonly contentType?: string;
  /** High-level type inferred from content type first, then extension. */
  readonly type?: OutputFileType;
}

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

/** Options for `Aex.outputs.read` / {@link import("./operations.js").readOutputText}. */
export interface ReadOutputTextOptions {
  /**
   * Stop reading after this many bytes. Defaults to 50_000; clamped server-side
   * of the SDK to [1, 10_000_000]. The read streams and cancels once the cap is
   * reached, so the remainder of a large file is never transferred.
   */
  readonly maxBytes?: number;
  /**
   * When set, return only the lines of the (capped) text matching this pattern.
   * A string is matched literally (case-insensitive); a RegExp is used as given.
   */
  readonly grep?: string | RegExp;
}

/**
 * A byte-capped, decoded text read of one output file, as returned by
 * `Aex.outputs.read`. Built for feeding run deliverables to an LLM
 * without loading the whole (possibly very large) file into memory or context:
 * the read streams and stops at `maxBytes`, so `text` is at most that many bytes
 * decoded as UTF-8. Check {@link truncated} before treating `text` as complete.
 */
export interface OutputText {
  readonly output: Output;
  /** Decoded UTF-8, capped to the requested `maxBytes`. */
  readonly text: string;
  /** True when the file is larger than `maxBytes` (so `text` is a prefix). */
  readonly truncated: boolean;
  /**
   * Full size of the file in bytes when the server reports it (`content-length`);
   * otherwise the number of bytes actually read.
   */
  readonly totalBytes: number;
}

export type OutputLinkExpiresIn = number | "15m" | "1h" | "1d";

export interface OutputLinkOptions {
  /** Seconds or one of the documented presets. Defaults to `"1h"`. */
  readonly expiresIn?: OutputLinkExpiresIn;
}

export interface OutputLink {
  readonly url: string;
  readonly expiresAt?: string;
  readonly expiresInSeconds?: number;
  readonly output?: Output;
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
   * snapshots at the time of the `whoami` call; `null` means no app-visible
   * cap is applied for that field.
   */
  readonly caps?: {
    /** Token-bucket cap on POST /api/runs per minute, per workspace. */
    readonly runSubmitPerMinute?: number;
    /** Hard cap on concurrent non-terminal runs the workspace may hold. */
    readonly maxConcurrentRuns?: number;
    /** Storage cap (bytes) on captured output objects, workspace-wide. `null` means unlimited. */
    readonly storageCapBytes?: number | null;
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
  /**
   * ADDITIVE effective per-workspace limits returned by `GET /whoami` on
   * current platform deployments — everything a caller needs to anticipate a
   * `429` / `402` submit rejection before hitting it. Every value is the same
   * one the platform's admission gates enforce. Optional: older deployments
   * omit the field entirely.
   */
  readonly limits?: {
    /** Effective live-run concurrency cap. One more live run past it fails with `429 workspace_concurrency_exceeded`. */
    readonly maxConcurrentRuns: number;
    /** Effective submit-velocity cap per minute; `0` = unlimited (disabled). Past it: `429 workspace_submit_rate_exceeded`. */
    readonly submitRatePerMinute: number;
    /** Effective monthly spend cap in USD; `0` = unlimited. Once `monthSpendUsd` reaches it: `402 workspace_spend_cap_exceeded`. */
    readonly spendCapUsd: number;
    /** Accrued spend in the current UTC calendar month — the value the spend gate compares. */
    readonly monthSpendUsd: number;
    /** Prepaid balance read-model — the value the balance gate compares. */
    readonly balanceUsd: number;
    /** Effective (payment-method-aware) submit floor: submits fail with `402 insufficient_balance` once `balanceUsd` is at or below it. */
    readonly balanceGraceFloorUsd: number;
    /** `"active"` means a bounded card overdraft is already folded into `balanceGraceFloorUsd`. */
    readonly paymentMethodStatus: "none" | "active";
  };
  readonly [key: string]: unknown;
}

/**
 * Workspace skill bundle as the dashboard BFF returns it. Mirrors a row
 * of `skill_bundles` joined with its computed manifest. `state` is the
 * upload lifecycle (`pending` -> `ready`); only `ready` rows are
 * referenceable from a run. Delete is hard; historical runs keep their
 * submit-time snapshots rather than depending on this row.
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
 * Value-bearing result of an audited `aex.secrets.get_value`. The ONLY wire shape
 * that carries a workspace secret value back to the caller. Value read is a logged
 * action (POST, not GET) so a value read is always attributable.
 */
export interface SecretReveal {
  readonly name: string;
  readonly value: string;
}

/**
 * Customer-facing billing summary — `GET /api/billing` (scope `billing:read`).
 * All money fields are USD numbers. The index signature keeps the shape tolerant
 * of ADDITIVE server fields (e.g. a deployment newer than this SDK reporting
 * extra plan attributes) — unknown keys are preserved, never rejected.
 */
export interface BillingSummary {
  /** Prepaid balance (authoritative ledger sum). */
  readonly balanceUsd: number;
  /** Accrued spend for the current calendar month. */
  readonly monthSpendUsd: number;
  /** Monthly spend cap enforced on new runs. */
  readonly spendCapUsd: number;
  readonly planKey: string;
  readonly subscriptionStatus: string;
  /** `"active"` once a payment method is bound; older deployments omit it. */
  readonly paymentMethodStatus?: string;
  readonly [key: string]: unknown;
}

/** Self-serve paid plans exposed through hosted checkout. */
export type BillingCheckoutPlanKey = "pro" | "team";

export interface BillingCheckoutRequest {
  readonly planKey: BillingCheckoutPlanKey;
  /** Optional return URL after successful hosted checkout. */
  readonly successUrl?: string;
  /** Optional return URL after checkout cancellation. */
  readonly cancelUrl?: string;
  /** Optional caller-stable key so a retry does not create a second hosted session. */
  readonly idempotencyKey?: string;
}

export interface BillingPortalRequest {
  /** Optional return URL after leaving the hosted billing portal. */
  readonly returnUrl?: string;
}

/** Hosted checkout/portal session. The client should open `url`. */
export interface BillingHostedSession {
  readonly url: string;
  readonly [key: string]: unknown;
}

/**
 * One row of the workspace credit ledger as returned by
 * `GET /api/billing/ledger` (newest first). `amountUsd` is signed: top-ups are
 * positive, run charges negative. Tolerant of additive server fields.
 */
export interface BillingLedgerEntry {
  readonly id: string;
  /** e.g. `top_up`, `run_charge`. Open server vocabulary. */
  readonly entryType: string;
  readonly amountUsd: number;
  readonly currency: string;
  /** The run this entry charges, `null` for non-run entries. */
  readonly runId?: string | null;
  readonly description?: string | null;
  readonly createdBy?: string;
  readonly createdAt: string;
  readonly [key: string]: unknown;
}

/** Query for the billing ledger read. `limit` is clamped server-side to [1, 100] (default 25). */
export interface BillingLedgerQuery {
  readonly limit?: number;
}

/** One page of recent ledger rows (newest first). Not cursor-paged — `limit` bounds the read. */
export interface BillingLedgerPage {
  readonly entries: readonly BillingLedgerEntry[];
}

/**
 * The workspace webhook signing secret reveal — `POST /api/webhook/signing-secret`.
 * `whsec` is the Standard-Webhooks style `whsec_<base64>` string that
 * `verifyAexWebhook` accepts as `secret`. The endpoint reveals the current
 * secret, CREATING one on first use; it does not rotate (a repeat call returns
 * the same value). POST (not GET) so every reveal is a logged action.
 */
export interface WebhookSigningSecret {
  readonly whsec: string;
}
