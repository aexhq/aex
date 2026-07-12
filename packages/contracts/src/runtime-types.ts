import type { SessionStatus, SessionTerminalOutcome } from "./status.js";
import type { RuntimeSize } from "./runtime-sizes.js";
import type { SessionCostProviderUsage } from "./session-cost.js";
import type {
  PlatformInlineSecrets,
  PlatformSubmission,
  ProviderName,
  SessionLimits,
  SessionWebhookSpec
} from "./submission.js";

export type SessionRunPhase =
  | "queued"
  | "starting"
  | "running"
  | "finished"
  | "error";

export type SessionRunOutcome = SessionTerminalOutcome;

export interface SessionRun {
  readonly sessionId: string;
  readonly turnSeq: number;
  readonly runId: string;
  readonly phase: SessionRunPhase;
  readonly outcome?: SessionRunOutcome;
  readonly startedAt?: string;
  readonly finishedAt?: string;
  readonly checkpoint?: import("./session-artifacts.js").SessionCheckpointRevision;
  readonly eventCursor?: number;
}

export interface Session {
  readonly id: string;
  readonly status: SessionStatus;
  readonly workspaceId?: string;
  readonly model?: PlatformSubmission["model"];
  /** Managed runtime preset selected when the session was created. */
  readonly runtime?: RuntimeSize;
  readonly runtimeManifest?: import("./runtime-manifest.js").RuntimeManifest;
  readonly acceptsMessages: boolean;
  readonly currentRun?: SessionRun;
  readonly lastRun?: SessionRun;
  /** How long the session may remain idle before it is suspended. */
  readonly idleTtl?: string;
  readonly idleTtlMs?: number;
  readonly createdAt?: string;
  readonly updatedAt?: string;
  readonly idleAt?: string | null;
  readonly suspendedAt?: string | null;
  readonly activeDurationMs?: number;
  readonly retainedStorageBytes?: number;
  readonly usage?: UsageSummary;
  readonly costUsd?: number;
  readonly costTelemetry?: import("./session-cost.js").SessionCostTelemetry;
  readonly errorMessage?: string | null;
  /**
   * Failure taxonomy for the most recent failed run (e.g.
   * `provider-permanent`, `budget_exhausted`) — the class a caller can branch
   * on, complementing the human-readable `errorMessage`.
   */
  readonly failureClass?: string | null;
}

export interface SessionSummary {
  readonly id: string;
  readonly status: SessionStatus;
  /** Managed runtime preset selected when the session was created. */
  readonly runtime?: RuntimeSize;
  readonly acceptsMessages: boolean;
  readonly currentRun?: SessionRun;
  readonly lastRun?: SessionRun;
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
  readonly status?: SessionStatus;
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

export interface SessionCreateRequest {
  readonly provider: ProviderName;
  readonly submission: SessionSubmission;
  readonly runtimeSize?: RuntimeSize;
  readonly timeout?: string;
  readonly limits?: SessionLimits;
  readonly retention?: SessionRetentionPolicy;
  readonly webhook?: SessionWebhookSpec;
  readonly secrets: PlatformInlineSecrets;
}

export interface SessionMessageRequest {
  readonly input: string | readonly string[];
  readonly metadata?: Readonly<Record<string, unknown>>;
}

export interface SessionMessageAccepted {
  readonly session: Session;
  readonly run: SessionRun;
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
  readonly run?: SessionRun;
  readonly eventCursor?: number;
}

export interface UsageSummary {
  readonly inputTokens?: number;
  readonly outputTokens?: number;
  readonly cacheReadInputTokens?: number;
  readonly cacheCreationInputTokens?: number;
  readonly totalTokens?: number;
}

/**
 * Project a {@link UsageSummary} from the terminal event's
 * {@link SessionCostProviderUsage} entries — the SINGLE server source of token
 * usage ({@link Session.costTelemetry}`.providerUsage`). Sums each field across all
 * provider entries; a field is present only when at least one entry carried it.
 * This retires the dead `session.usage` / `aex.usage`-event usage path: the SDK
 * derives usage from cost telemetry, never re-reads a dual-written top-level
 * `usage`. Pure.
 */
export function usageFromProviderUsage(
  providerUsage: readonly SessionCostProviderUsage[] | undefined
): UsageSummary {
  if (!providerUsage || providerUsage.length === 0) return {};
  let inputTokens: number | undefined;
  let outputTokens: number | undefined;
  let cacheReadInputTokens: number | undefined;
  let cacheCreationInputTokens: number | undefined;
  let totalTokens: number | undefined;
  const add = (acc: number | undefined, value: number | undefined): number | undefined =>
    value === undefined ? acc : (acc ?? 0) + value;
  for (const entry of providerUsage) {
    inputTokens = add(inputTokens, entry.inputTokens);
    outputTokens = add(outputTokens, entry.outputTokens);
    cacheReadInputTokens = add(cacheReadInputTokens, entry.cacheReadInputTokens);
    cacheCreationInputTokens = add(cacheCreationInputTokens, entry.cacheCreationInputTokens);
    totalTokens = add(totalTokens, entry.totalTokens);
  }
  return {
    ...(inputTokens !== undefined ? { inputTokens } : {}),
    ...(outputTokens !== undefined ? { outputTokens } : {}),
    ...(cacheReadInputTokens !== undefined ? { cacheReadInputTokens } : {}),
    ...(cacheCreationInputTokens !== undefined ? { cacheCreationInputTokens } : {}),
    ...(totalTokens !== undefined ? { totalTokens } : {})
  };
}

/** Consistent result exposed after a RUN_FINISHED or RUN_ERROR barrier. */
export interface TurnResult {
  readonly status: SessionRunOutcome;
  /** True only when the run completed successfully. */
  readonly ok: boolean;
  /** AEX showback estimate (USD, >= 0). Excludes BYOK provider spend. */
  readonly costUsd: number;
  /** Aggregate token usage, derived from `costTelemetry.providerUsage`. */
  readonly usage: UsageSummary;
  /** Terminal failure message (from the terminal `RUN_ERROR` event) when `!ok`. */
  readonly error?: string;
}

/**
 * The reason a schema-constrained decode did not yield a value:
 *   - `schema_violation` — the model output failed schema validation.
 *   - `uncertain` — the model signalled low confidence / declined to commit.
 *   - `refused` — the model refused the request.
 */
export type TurnRefusalReason = "schema_violation" | "uncertain" | "refused";

/**
 * The typed outcome of a `start<T>({ responseFormat })`: EITHER a schema-valid
 * decoded value OR a typed refusal — there is no untyped path that silently
 * yields a hallucinated object. `T` is the decoded value type.
 */
export type TurnOutcome<T = unknown> =
  | { readonly kind: "decoded"; readonly value: T }
  | { readonly kind: "refused"; readonly reason: TurnRefusalReason; readonly detail?: string };

/**
 * A subagent child session, enumerated under its parent via `GET /api/sessions/:id/children`.
 * This is a read-only lineage snapshot. Its `id` addresses the child's events,
 * checkpointed files, and descendants, but not the top-level session get/control
 * routes.
 */
export interface ChildSessionRef {
  readonly id: string;
  /** The parent session this child was spawned by. */
  readonly parentSessionId: string;
  /** The child's session status. */
  readonly status: SessionStatus;
  /** Subagent nesting depth (1 = direct child of the top-level session). */
  readonly depth?: number;
  /** Final AEX showback estimate (USD) for the child, when present. */
  readonly costUsd?: number;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly terminalAt?: string | null;
  readonly lastRun?: SessionRun;
}

// The loose `TurnEvent` snapshot shape has been RETIRED. Every event read
// surface — `listSessionEvents`, `list()`/`stream()`/
// `streamEnvelopes()` — now yields the one canonical
// {@link import("./event-envelope.js").AexEvent} (guard-bearing via
// {@link import("./event-view.js").AexEventView}) with a non-optional,
// populated `sequence`. There is no second event identity/shape.

/** Status of a run-terminal webhook delivery. Terminal: delivered/exhausted/invalid. */
export type SessionWebhookDeliveryStatus =
  | "pending"
  | "delivering"
  | "retrying"
  | "delivered"
  | "exhausted"
  | "invalid";

/** CloudEvents type matching the finalized AG-UI run terminal. */
export type SessionRunWebhookEventType = "run.finished" | "run.error";

/** Public run facts frozen into every run-terminal webhook delivery. */
export interface SessionRunWebhookData {
  readonly sessionId: string;
  readonly runId: string;
  readonly turnSeq: number;
  readonly outcome: SessionRunOutcome;
  readonly terminalAt: string;
  readonly checkpoint?: import("./session-artifacts.js").SessionCheckpointRevision;
  readonly reason: string | null;
  readonly failureClass: string | null;
  readonly costTelemetry?: { readonly billedCostUsd: number };
}

interface SessionRunWebhookEnvelope {
  readonly specversion: "1.0";
  /** Deterministic run-scoped delivery id; also sent as the `webhook-id` header. */
  readonly id: string;
  readonly source: "aex";
  /** The run id. Session/thread identity remains available as `data.sessionId`. */
  readonly subject: string;
  readonly time: string;
  readonly data: SessionRunWebhookData;
}

/** Finalized successful run, mapped from AG-UI `RUN_FINISHED`. */
export interface SessionRunFinishedWebhookPayload extends SessionRunWebhookEnvelope {
  readonly type: "run.finished";
}

/** Finalized unsuccessful run, mapped from AG-UI `RUN_ERROR`. */
export interface SessionRunErrorWebhookPayload extends SessionRunWebhookEnvelope {
  readonly type: "run.error";
}

/** The no-alias public webhook envelope for a finalized session run. */
export type SessionRunWebhookPayload =
  | SessionRunFinishedWebhookPayload
  | SessionRunErrorWebhookPayload;

/**
 * One run-scoped row in a session's webhook delivery ledger. `id` is stable
 * across retries and manual redelivery; `runId` + `turnSeq` identify which run
 * produced the frozen payload. Optional attempt fields appear after delivery.
 */
export interface SessionWebhookDelivery {
  readonly id: string;
  readonly runId: string;
  readonly turnSeq: number;
  readonly eventType: SessionRunWebhookEventType;
  readonly status: SessionWebhookDeliveryStatus;
  readonly attemptCount: number;
  readonly lastStatusCode?: number;
  readonly lastError?: string;
  readonly nextAttemptAt?: string;
  readonly createdAt: string;
}

/**
 * One captured session file. Use `session.files.link(...)` to mint a temporary
 * direct download URL.
 */
export interface SessionFile {
  readonly id: string;
  /** Checkpoint this file record is pinned to. */
  readonly checkpointId: string;
  readonly filename?: string;
  /** Exact byte length recorded by the committed checkpoint. */
  readonly sizeBytes: number;
  /** Lowercase SHA-256 digest of the committed bytes. */
  readonly sha256: string;
  readonly contentType?: string;
  readonly createdAt?: string;
  readonly [key: string]: unknown;
}

/** A complete file listing and the immutable checkpoint it was read from. */
export interface SessionFilesSnapshot {
  readonly revision: import("./session-artifacts.js").SessionCheckpointRevision;
  readonly files: readonly SessionFile[];
}

export type SessionFilePathMatch = "exact" | "suffix";

export type SessionFileType =
  | "text"
  | "json"
  | "image"
  | "audio"
  | "video"
  | "pdf"
  | "archive"
  | "binary"
  | "unknown";

export interface SessionFileQuery {
  /** Exact normalized file path. Leading `/` and `files/` are ignored. */
  readonly path?: string;
  /** Basename match. A RegExp is tested against the basename only. */
  readonly filename?: string | RegExp;
  /**
   * Directory prefix. Leading `/` and `files/` are ignored.
   * `recursive` defaults to true.
   */
  readonly dir?: string;
  readonly recursive?: boolean;
  /** File extension, with or without a leading dot. Case-insensitive. */
  readonly extension?: string;
  /** Exact content type or a prefix wildcard such as `image/*`. */
  readonly contentType?: string;
  /** High-level type inferred from content type first, then extension. */
  readonly type?: SessionFileType;
}

/** File filters plus an optional immutable checkpoint selector. */
export interface SessionFilesQuery extends SessionFileQuery {
  readonly checkpointId?: string;
}

export interface SessionFilePathSelector {
  readonly path: string;
  readonly match?: SessionFilePathMatch;
}

export interface SessionFileIdSelector {
  readonly id: string;
  readonly checkpointId: string;
}

export type SessionFileSelector = SessionFile | SessionFileIdSelector | SessionFilePathSelector;

export interface SessionFileDownload {
  readonly file: SessionFile;
  readonly bytes: Uint8Array;
}

/** Options for `session.files.read` / {@link import("./operations.js").readSessionFileText}. */
export interface ReadSessionFileTextOptions {
  /** Checkpoint to read. Defaults to the selected file's checkpoint. */
  readonly checkpointId?: string;
  /**
   * Stop reading after this many bytes. Defaults to 50_000; clamped server-side
   * of the SDK to [1, 10_000_000]. The read streams and cancels once the cap is
   * reached, so the remainder of a large file is never transferred.
   */
  readonly maxBytes?: number;
  /**
   * Per-attempt timeout for fetching and reading the selected file body.
   * Defaults to 30_000ms; idempotent file reads retry once on timeout before
   * surfacing a structured NETWORK_ERROR.
   */
  readonly timeoutMs?: number;
  /**
   * When set, return only the lines of the (capped) text matching this pattern.
   * A string is matched literally (case-insensitive); a RegExp is used as given.
   */
  readonly grep?: string | RegExp;
}

/**
 * A byte-capped, decoded text read of one session file, as returned by
 * `session.files.read`. Built for feeding session deliverables to an LLM
 * without loading the whole (possibly very large) file into memory or context:
 * the read streams and stops at `maxBytes`, so `text` is at most that many bytes
 * decoded as UTF-8. Check {@link truncated} before treating `text` as complete.
 */
export interface SessionFileText {
  readonly file: SessionFile;
  /** Decoded UTF-8, capped to the requested `maxBytes`. */
  readonly text: string;
  /** True when the file is larger than `maxBytes` (so `text` is a prefix). */
  readonly truncated: boolean;
  /** Exact full size recorded by the authoritative checkpoint. */
  readonly totalBytes: number;
}

export type SessionFileLinkExpiresIn = number | "15m" | "1h" | "1d";

export interface SessionFileLinkOptions {
  /** Seconds or one of the documented presets. Defaults to `"1h"`. */
  readonly expiresIn?: SessionFileLinkExpiresIn;
  /** Checkpoint to bind the signed link to. */
  readonly checkpointId?: string;
}

export interface SessionFileLink {
  readonly url: string;
  readonly expiresAt?: string;
  readonly expiresInSeconds?: number;
  readonly file?: SessionFile;
  readonly [key: string]: unknown;
}

export interface WhoAmI {
  readonly ok: true;
  readonly principalType: "api_key";
  readonly workspaceId: string;
  readonly scopes: readonly string[];
  /** Effective workspace limits from the same read models used by admission. */
  readonly limits: {
    /** Effective live-run concurrency cap. One more live run past it fails with `429 workspace_concurrency_exceeded`. */
    readonly maxConcurrentSessions: number;
    /** Effective submit-velocity cap per minute; `0` = unlimited (disabled). Past it: `429 workspace_submit_rate_exceeded`. */
    readonly submitRatePerMinute: number;
    /** Effective monthly spend cap in USD; `0` = unlimited. Once `monthSpendUsd` reaches it: `402 workspace_spend_cap_exceeded`. */
    readonly spendCapUsd: number;
    /** Accrued spend in the current UTC calendar month — the value the spend gate compares. */
    readonly monthSpendUsd: number;
    /** Prepaid balance read-model — the value the balance gate compares. */
    readonly balanceUsd: number;
    /** Submit floor used when {@link balanceGateActive} is true. */
    readonly balanceGraceFloorUsd: number;
    /** Whether the allowance-balance gate applies to this workspace. */
    readonly balanceGateActive: boolean;
    readonly paymentMethodStatus: "none" | "active";
    readonly planKey: "free" | "pro" | "team";
    readonly accountType: "standard" | "internal";
    readonly subscriptionStatus: "none" | "active" | "past_due" | "canceled";
    readonly subscriptionGate: "ok" | "past_due_grace" | "past_due_suspended";
    readonly pastDueAt?: string;
    readonly graceEndsAt?: string;
  };
}

/**
 * Wire-level record for a workspace secret as returned by the BFF.
 *
 * Workspace secrets share the lifecycle semantic of skills/files: a
 * `Secret.value(...)` is per-session and gone at terminal, while
 * `aex.workspace.secrets.set(...)` persists a named reusable value. Use
 * `Secret.ref(name)` to bind that persisted value to a session. The
 * identity is the `name` (the handle a `Secret.ref` points at); the value
 * rotates under that stable name, bumping `version`.
 *
 * This record is METADATA ONLY — it never carries the secret value. Persisted
 * values are write-only through the public workspace-secret API.
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
  /** Monthly spend cap enforced on new sessions. */
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
  /** e.g. `top_up`, `session_charge`. Open server vocabulary. */
  readonly entryType: string;
  readonly amountUsd: number;
  readonly currency: string;
  /** The session this entry charges, `null` for non-run entries. */
  readonly sessionId?: string | null;
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
