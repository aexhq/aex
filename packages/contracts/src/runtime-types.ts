import type { SessionStatus, SessionTerminalOutcome } from "./status.js";
import type { SessionCostProviderUsage } from "./session-cost.js";
import type {
  PlatformSessionSubmissionInput,
  PlatformSubmission
} from "./submission.js";

/**
 * Loose record describing a session as the dashboard BFF returns it. Concrete
 * dashboard-managed fields appear in the index signature; the SDK and CLI
 * may surface them without strong typing per-field.
 *
 * `runtimeManifest` is derived from the validated submission + the chosen
 * provider on every read — see `runtime-manifest.ts`. It is `undefined`
 * on responses from BFFs that predate Phase 2 of the runtime-environment
 * rollout; SDK consumers MUST treat it as best-effort and not panic on
 * its absence.
 */
export interface SessionRecord {
  readonly id: string;
  readonly status: string;
  readonly workspaceId?: string;
  readonly createdAt?: string;
  readonly updatedAt?: string;
  readonly terminalAt?: string | null;
  /**
   * The session's EXECUTION start (ISO-8601) — when the agent actually began
   * running, distinct from {@link createdAt} (submission/accept time). Present
   * from the moment the session starts executing and throughout its live duration;
   * absent before it starts and after the session's live object is torn down (a
   * terminal run also carries {@link terminalAt} and {@link costTelemetry}
   * durations).
   */
  readonly startedAt?: string;
  readonly errorMessage?: string | null;
  /**
   * Settle-written failure taxonomy for a failed run (e.g.
   * `provider-permanent`, `invalid_submission`) — the class a caller can branch
   * on, complementing the human-readable `errorMessage`.
   */
  readonly failureClass?: string | null;
  /**
   * Aggregate token usage when a deployment chooses to expose it on the session
   * record. Mid-run this is not populated. Settled provider/runtime usage is
   * exposed through {@link costTelemetry}; per-turn usage breadcrumbs, when a
   * deployment records them internally, are not part of the normal public event
   * stream.
   */
  readonly usage?: UsageSummary;
  readonly costTelemetry?: import("./session-cost.js").SessionCostTelemetry;
  /**
   * The authoritative terminal OUTCOME of the session's last turn — the settle-
   * written outcome (`succeeded`/`failed`/`timed_out`/`cancelled`), distinct
   * from the resumable lifecycle {@link status}. Absent until the session settles.
   */
  readonly lastTurnOutcome?: SessionTerminalOutcome;
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
  /**
   * The authoritative terminal OUTCOME of the session's last turn — the
   * settle-written outcome (`succeeded`/`failed`/`timed_out`/`cancelled`),
   * distinct from the resumable lifecycle {@link status} (`idle`/`suspended`).
   * A cancelled turn reads `cancelled` here even though the session may park
   * `idle`; absent until the turn settles.
   */
  readonly lastTurnOutcome?: SessionTerminalOutcome;
  readonly errorMessage?: string | null;
  /**
   * Settle-written failure taxonomy for a `failed` session (e.g.
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
  PlatformSessionSubmissionInput,
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
 * Project a {@link UsageSummary} from the settle-written
 * {@link SessionCostProviderUsage} entries — the SINGLE server source of token
 * usage ({@link SessionRecord.costTelemetry}`.providerUsage`). Sums each field across all
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

/**
 * The unified SETTLED-RESULT contract shared by `start()` and `done()`. Because
 * both await the settle commit by default, these fields are ALWAYS present at a
 * terminal read — they are NON-optional, so a code path that forgets to
 * populate `costUsd`/`usage`/the terminal `status` fails to typecheck. The SDK
 * `SessionResult`/`SessionTurnResult` extend this one shape so `done()` == `start()`.
 *
 * `costUsd` is the AEX showback estimate in USD (>= 0) and EXCLUDES the
 * customer's BYOK provider spend — price BYOK from `usage` tokens.
 */
export interface SettledResult {
  /** The terminal outcome (never a bare resumable `idle`). */
  readonly status: SessionTerminalOutcome;
  /** `succeeded` ⇒ true; `failed`/`timed_out`/`cancelled` ⇒ false. */
  readonly ok: boolean;
  /** AEX showback estimate (USD, >= 0). Excludes BYOK provider spend. */
  readonly costUsd: number;
  /** Aggregate token usage, derived from `costTelemetry.providerUsage`. */
  readonly usage: UsageSummary;
  /** Terminal failure message (from the terminal `TURN_ERROR` event) when `!ok`. */
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

/** One item's settled result inside a {@link BatchResult}. */
export interface BatchItemResult<T = unknown> extends SettledResult {
  readonly sessionId: string;
  /** Present only for a `responseFormat`-decoded item. */
  readonly outcome?: TurnOutcome<T>;
}

/**
 * The result of `aex.batch(items, …)`: every item's settled result PLUS a real
 * rollup. The rollup is honest because `start()` awaits settle, so every item's
 * `costUsd`/`usage` is populated — a missing cost is a typed absent, not a
 * silent `$0`.
 */
export interface BatchResult<T = unknown> {
  readonly results: readonly BatchItemResult<T>[];
  readonly totalCostUsd: number;
  readonly totalUsage: UsageSummary;
  readonly okCount: number;
  readonly failed: readonly BatchItemResult<T>[];
}

/**
 * Filters for {@link import("./operations.js").listSessionRecords} / the CLI's `aex sessions`.
 * Every field is optional; omitting all of them lists the most recent sessions in the
 * token's workspace. Workspace identity is derived server-side from the API key,
 * so there is no `workspaceId` here — a token can only ever enumerate its own sessions.
 */
export interface SessionRecordListQuery {
  /** Restrict to a single session status, e.g. `"succeeded"`. */
  readonly status?: string;
  /** ISO-8601 lower bound on `createdAt` (inclusive). */
  readonly since?: string;
  /** Page size. Defaults to 25, clamped server-side to [1, 100]. */
  readonly limit?: number;
  /** Opaque keyset cursor from a prior page's `nextCursor`. */
  readonly cursor?: string;
}

/**
 * A public-safe run summary as returned by `GET /api/sessions` (the workspace run
 * list). DELIBERATELY omits the submission snapshot (model/prompt/env) — the full,
 * redaction-scanned submission is only reachable through `getSessionUnit(sessionId)`.
 */
export interface SessionRecordSummary {
  readonly id: string;
  readonly status: string;
  readonly createdAt: string;
  readonly updatedAt: string;
  /** Settled showback estimate (USD), present once the session has cost telemetry. */
  readonly costUsd?: number;
}

/** One page of the workspace session list. `nextCursor` absent ⇒ last page. */
export interface SessionRecordListPage {
  readonly sessions: readonly SessionRecordSummary[];
  readonly nextCursor?: string;
}

/**
 * The minimal capability a value must carry to be RESOLVABLE through the session
 * facade — `getSessionRecord` / `listSessionRecordEvents` / `listSessionFiles` all key on this `id`.
 * Encodes the "handed ⇒ resolvable" invariant at the type level: anything the
 * platform hands you as a session reference exposes a resolvable `id`, so a session can
 * never be surfaced as a bare unresolvable string.
 */
export interface ResolvableSessionRef {
  readonly id: string;
}

/**
 * A subagent child session, enumerated under its parent via `GET /sessions/:id/children`.
 * A first-class, lineage-discoverable run reference: it {@link ResolvableSessionRef}
 * (its `id` resolves through the session record facade — events/files/getSessionRecord), carries the
 * lineage (`parentSessionId`/`depth`) and the terminal outcome/cost, so a child is
 * observable exactly like a top-level session.
 */
export interface ChildSessionRef extends ResolvableSessionRef {
  /** The parent session this child was spawned by. */
  readonly parentSessionId: string;
  /** The child's session status. */
  readonly status: string;
  /** Subagent nesting depth (1 = direct child of the top-level session). */
  readonly depth?: number;
  /** Settled AEX showback estimate (USD) for the child, when present. */
  readonly costUsd?: number;
  readonly createdAt?: string;
  readonly terminalAt?: string | null;
  /** The child's terminal outcome once settled. */
  readonly lastTurnOutcome?: SessionTerminalOutcome;
}

/**
 * Cross-session session-file search query (`Aex.files.search`). Restrict to a
 * corpus with `sessionIds`; filter by filename substring / extension / content type.
 * The MVP composes this client-side (per-session `listSessionFiles` + filter) — a future
 * server-side `GET /api/files/search` can back the same contract with a real
 * cross-session index, body-only swap.
 */
export interface SessionFileSearchQuery {
  /** Restrict the search to these sessions (the chat corpus allow-list). */
  readonly sessionIds?: readonly string[];
  /**
   * Filename match. A string is a case-insensitive SUBSTRING match; a RegExp is
   * tested as given. Unified with {@link SessionFileQuery.filename} (`string | RegExp`)
   * so the same value works on `find()` and `search()` — feed it through
   * {@link import("./operations.js").toFilenameMatcher} rather than assuming a string.
   */
  readonly filename?: string | RegExp;
  /** File extension, with or without a leading dot. Case-insensitive. */
  readonly extension?: string;
  /** Exact content type or a prefix wildcard such as `image/*`. */
  readonly contentType?: string;
  /** Cap the number of hits returned (default 100). */
  readonly limit?: number;
}

/** One session-file search hit — a reference only (no bytes); read with `readSessionFileText`. */
export interface SessionFileSearchHit {
  readonly sessionId: string;
  readonly fileId: string;
  readonly filename?: string;
  readonly sizeBytes?: number;
  readonly contentType?: string;
}

/** A page of session-file search hits. */
export interface SessionFileSearchPage {
  readonly hits: readonly SessionFileSearchHit[];
}

// The loose `TurnEvent` snapshot shape has been RETIRED. Every event read
// surface — `listSessionEvents`/`listSessionRecordEvents`, `list()`/`stream()`/
// `streamEnvelopes()` — now yields the one canonical
// {@link import("./event-envelope.js").AexEvent} (guard-bearing via
// {@link import("./event-view.js").AexEventView}) with a non-optional,
// populated `sequence`. There is no second event identity/shape.

/** Status of a per-session webhook delivery. Terminal: delivered/exhausted/invalid. */
export type SessionWebhookDeliveryStatus =
  | "pending"
  | "delivering"
  | "retrying"
  | "delivered"
  | "exhausted"
  | "invalid";

/**
 * One row of a session's webhook delivery ledger, as returned by
 * `GET /api/sessions/:id/webhook-deliveries`. `id` is the stable `webhook-id`
 * header the consumer dedupes on across retries; the optional fields are
 * populated only once a delivery attempt has been made.
 */
export interface SessionWebhookDelivery {
  readonly id: string;
  readonly eventType: string;
  readonly status: SessionWebhookDeliveryStatus;
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
 * One captured session file as the dashboard reports it. Use
 * `sessionFileLink` / `createSessionFileLink` to get a temporary direct URL for download.
 */
export interface SessionFile {
  readonly id: string;
  readonly filename?: string;
  readonly sizeBytes?: number;
  readonly contentType?: string;
  readonly createdAt?: string;
  readonly [key: string]: unknown;
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

export interface SessionFilePathSelector {
  readonly path: string;
  readonly match?: SessionFilePathMatch;
}

export interface SessionFileIdSelector {
  readonly id: string;
}

export type SessionFileSelector = SessionFile | SessionFileIdSelector | SessionFilePathSelector;

export interface SessionFileDownload {
  readonly file: SessionFile;
  readonly bytes: Uint8Array;
}

/** Options for `Aex.sessions.files(id).read` / {@link import("./operations.js").readSessionFileText}. */
export interface ReadSessionFileTextOptions {
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
 * `Aex.sessions.files(id).read`. Built for feeding session deliverables to an LLM
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
  /**
   * Full size of the file in bytes when the server reports it (`content-length`);
   * otherwise the number of bytes actually read.
   */
  readonly totalBytes: number;
}

export type SessionFileLinkExpiresIn = number | "15m" | "1h" | "1d";

export interface SessionFileLinkOptions {
  /** Seconds or one of the documented presets. Defaults to `"1h"`. */
  readonly expiresIn?: SessionFileLinkExpiresIn;
}

export interface SessionFileLink {
  readonly url: string;
  readonly expiresAt?: string;
  readonly expiresInSeconds?: number;
  readonly file?: SessionFile;
  readonly [key: string]: unknown;
}

export interface WhoAmI {
  /**
   * Kind of principal the bearer resolved to. OPTIONAL IN PRACTICE: current
   * managed deployments do not serve it (`GET /whoami` returns only
   * `workspaceId` + `scopes` + `limits`) — treat `undefined` as "api_key".
   */
  readonly principalType?: "api_key" | "user";
  readonly workspaceId?: string;
  readonly tokenId?: string;
  readonly tokenName?: string | null;
  readonly scopes?: readonly string[];
  /**
   * Workspace-level caps the BFF will enforce on subsequent calls.
   * Surfaced so consumers (e.g. broll's app-side admission gate) can
   * decide whether to keep their own gate or rely on platform headers.
   * All fields optional — older BFFs may omit, and current managed
   * deployments serve the newer {@link limits} block INSTEAD of `caps`.
   * Numbers are concrete snapshots at the time of the `whoami` call;
   * `null` means no app-visible cap is applied for that field.
   */
  readonly caps?: {
    /** Token-bucket cap on POST /api/sessions per minute, per workspace. */
    readonly sessionSubmitPerMinute?: number;
    /** Hard cap on concurrent non-terminal sessions the workspace may hold. */
    readonly maxConcurrentSessions?: number;
    /** Storage cap (bytes) on captured file objects, workspace-wide. `null` means unlimited. */
    readonly storageCapBytes?: number | null;
    /** Current captured-file usage in bytes. */
    readonly storageUsedBytes?: number;
    /**
     * Wall-clock ceiling on a single run before forced termination.
     * `null` means no aex-imposed cap, but this is **not unlimited
     * overall**: the managed runner, infrastructure, or upstream provider may
     * still impose a ceiling, and a session that exceeds it terminates regardless.
     */
    readonly maxSessionDurationMs?: number | null;
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
    readonly maxConcurrentSessions: number;
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
 * referenceable from a session. Delete is hard; historical sessions keep their
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
 * Wire-level metadata record for a workspace skill as returned by the BFF.
 *
 * Workspace skills are named, mutable, by-name-bound bundles: `skill.upload()`
 * upserts one under a stable `name`; a session references it by that name and the
 * platform resolves it to the CURRENT bytes at submit time. This record is
 * METADATA ONLY — the bytes live in the content-addressed asset store keyed by
 * `contentHash`. `version` bumps each time the bytes change under the name.
 */
export interface SkillRecord {
  readonly id?: string;
  readonly kind?: "skill";
  readonly name: string;
  readonly contentHash: string;
  readonly description: string;
  readonly sizeBytes?: number | null;
  readonly version?: number;
  readonly createdAt?: string;
  readonly updatedAt?: string;
  readonly deletedAt?: string | null;
  readonly [key: string]: unknown;
}

/**
 * Wire-level record for a workspace secret as returned by the BFF.
 *
 * Workspace secrets share the lifecycle SEMANTIC of skills/files: a
 * `Secret.value(...)` is per-session and gone at terminal; PROMOTING it (or
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
