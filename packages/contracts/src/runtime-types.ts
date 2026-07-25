import type * as z from "zod/mini";
import type { SessionStatus, SessionTerminalOutcome } from "./status.js";
import type { RuntimeSize } from "./runtime-sizes.js";
import type { RuntimeKind } from "./runtime-kind.js";
import type { SessionCostProviderUsage } from "./session-cost.js";
import type { ProviderFault } from "./provider-fault.js";
import type {
  PlatformInlineSecrets,
  PlatformSubmission,
  SessionLimits,
  SessionWebhookSpec
} from "./submission.js";
// TYPE-ONLY, and deliberately so: `schemas/response-sessions.ts` imports
// `SESSION_RUN_PHASES` from this module, so a value import would close a runtime
// cycle. `import type` is erased, and `typeof Schema` is a type position, so the
// wire types below are derived from the schemas that validate the actual bytes
// without either module depending on the other at run time.
import type {
  CoordinatorTicketResponseSchema,
  SessionDeleteResponseSchema,
  SessionFileSchema,
  SessionFileLinkResponseSchema,
  SessionWireSchema
} from "./schemas/response-sessions.js";
import type {
  ChildFinalizeResponseSchema,
  ChildResultResponseSchema
} from "./schemas/response-sessions-internal.js";
import type {
  McpServerRecordSchema,
  McpServerListResponseSchema
} from "./schemas/response-mcp-servers.js";
import type { WorkspaceWebhookDeliverySchema } from "./schemas/response-webhooks.js";
import type { WorkspaceEraseResponseSchema } from "./schemas/response-workspace.js";
import type {
  AdminBillingAccountTypeResponseSchema,
  AdminBillingPaymentMethodResponseSchema,
  AdminBillingTopupResponseSchema
} from "./schemas/response-billing.js";

// ===========================================================================
// Wire-vs-client drift guards
//
// Several types below are the CLIENT shape: what the SDK hands a caller after
// `operations.ts` has normalised the response. A client type is legitimately
// hand-written (it carries `readonly`, richer nested types, and the fields the
// SDK synthesises), but hand-written is exactly how it drifted from the wire in
// the first place. So every hand-written client shape that mirrors a response
// schema is pinned to it by a KEY-SET equality check: adding or removing a field
// on either side is a compile error, while the deliberate differences
// (`readonly`, nested type identity, optionality) stay expressible.
// ===========================================================================

/** `true` only when `A` and `B` are the same set of keys. */
type KeysEqual<A extends PropertyKey, B extends PropertyKey> =
  [Exclude<A, B>] extends [never] ? ([Exclude<B, A>] extends [never] ? true : false) : false;

/** Fails to compile unless `T` is `true`. */
type Assert<T extends true> = T;

/** Public run phases. Internal finalization remains behind the consistency barrier. */
export const SESSION_RUN_PHASES = [
  "queued",
  "starting",
  "running",
  "finished",
  "error"
] as const;

export type SessionRunPhase = (typeof SESSION_RUN_PHASES)[number];

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

/**
 * The execution runtime a session runs on. Both fields are optional (the
 * platform applies defaults): `kind` selects the backend
 * (`lambda` (default) | `spot_container` | `container`), `size` selects the
 * managed box preset. Grouped so the SDK surface reads
 * `runtime: { kind: "lambda", size: Sizes.CPU_2_8GB }`.
 */
export interface SessionRuntime {
  readonly kind?: RuntimeKind;
  readonly size?: RuntimeSize;
}

/**
 * The session projection **exactly as the data plane serves it**.
 *
 * Derived from `SessionWireSchema`, the schema the C4 harness validates real
 * bytes against, so this type and that schema cannot disagree. The wire carries
 * FLAT `runtimeKind` + `runtimeSize`; there is no grouped `runtime` object on
 * it. `normalizeSessionRuntime` in `operations.ts` folds the pair into
 * {@link Session} before any SDK caller sees a session, which is why both shapes
 * are declared: they are two different things, and calling them both `Session`
 * is what let three fields disagree with the server unnoticed.
 *
 * Nobody outside `operations.ts` should consume this. It exists so the
 * transform is a named, checkable step instead of an undocumented mutation.
 */
export type SessionWire = z.infer<typeof SessionWireSchema>;

/**
 * A session as the SDK hands it to a caller — {@link SessionWire} with the flat
 * `runtimeKind`/`runtimeSize` pair folded into {@link SessionRuntime}. Nothing
 * else is added, removed or renamed by the fold.
 */
export interface Session {
  readonly id: string;
  readonly status: SessionStatus;
  readonly workspaceId?: string;
  readonly model?: PlatformSubmission["model"];
  /** The upstream model provider the session ran on, when the server records one. */
  readonly provider?: string;
  /**
   * Execution runtime: the backend (`kind`) and box size (`size`) it runs on.
   * CLIENT-SIDE SHAPE — the server sends `runtimeKind` and `runtimeSize` as two
   * top-level fields (see {@link SessionWire}); the SDK groups them here.
   */
  readonly runtime?: SessionRuntime;
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
  readonly costUsd?: number;
  /**
   * Cost and usage telemetry for the settled session. **This is where token
   * counts live** — `costTelemetry.providerUsage`, projected by
   * {@link usageFromProviderUsage}. There is no top-level `usage` field; the
   * server has never sent one.
   */
  readonly costTelemetry?: import("./session-cost.js").SessionCostTelemetry;
  readonly errorMessage?: string | null;
  /**
   * Failure taxonomy for the most recent failed run (e.g.
   * `provider-permanent`, `budget_exhausted`) — the class a caller can branch
   * on, complementing the human-readable `errorMessage`.
   */
  readonly failureClass?: string | null;
  /** Structured, redaction-safe provider detail for the most recent failed run. */
  readonly providerFault?: ProviderFault;
  /**
   * Content-retention lifecycle (WS4). `"active"` — the default when absent —
   * means the session's CONTENT (events, messages, files, manifest, archive) is
   * still readable. `"metadata_only"` means the content-retention window elapsed
   * and the content was tombstoned: this session RECORD still reads, but every
   * content endpoint now answers HTTP 410 `content_deleted` (surfaced by the SDK
   * as `ContentDeletedError`). Optional and forward-compatible: an older
   * deployment omits it and the session parses as active.
   */
  readonly dataState?: "active" | "metadata_only";
  /** When the content was purged (ISO 8601). Present once `dataState` is `"metadata_only"`. */
  readonly contentPurgedAt?: string;
  /** What triggered the content purge: `"retention"` (window elapsed) or `"user"` (explicit delete). */
  readonly contentDeletedBy?: "retention" | "user";
}

/**
 * The fold is exactly `runtimeKind` + `runtimeSize` → `runtime`, and nothing
 * else. A field appearing on one side only is a compile error here.
 */
type _SessionFoldIsTotal = Assert<
  KeysEqual<Exclude<keyof SessionWire, "runtimeKind" | "runtimeSize"> | "runtime", keyof Session>
>;

export interface SessionSummary {
  readonly id: string;
  readonly status: SessionStatus;
  /** Execution runtime: the backend (`kind`) and box size (`size`) it runs on. */
  readonly runtime?: SessionRuntime;
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
  readonly submission: SessionSubmission;
  readonly runtimeSize?: RuntimeSize;
  readonly runtimeKind?: RuntimeKind;
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

/**
 * The senders the projected transcript actually carries.
 *
 * Narrowed from a four-member union that also listed `system` and `tool`. The
 * message projection emits only these two — tool activity and system text reach
 * a caller as events, not transcript messages — and
 * `SessionMessagesPageResponseSchema` is strict, so a third value would fail C4
 * rather than pass unnoticed.
 */
export type SessionMessageSender = "user" | "assistant";

/**
 * One projected transcript message.
 *
 * Required-ness follows the wire rather than caution: the projection always
 * emits `timestamp`, `sequence` and `content`, and declaring them optional made
 * callers guard a case the server does not produce. The index signature is gone
 * for the same reason the other twelve went — it let an undeclared field pass
 * unnoticed, which is the thing the response schemas exist to catch.
 */
export interface SessionMessage {
  readonly id: string;
  readonly sender: SessionMessageSender;
  readonly text: string;
  readonly timestamp: string;
  readonly sequence: number;
  readonly content: readonly unknown[];
  readonly turnSeq?: number;
  readonly messageId?: string;
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

/**
 * The body every session state-change route answers with: suspend, cancel,
 * resume, approve, deny and request-approval.
 *
 * The envelope is `{ session }` and NOTHING ELSE. It used to declare an optional
 * `run` and `eventCursor`; no state-change handler has ever emitted either, and
 * a declared-but-never-sent field is indistinguishable from a field the server
 * dropped. The run-bearing envelope is {@link SessionMessageAccepted}, which is
 * a different route.
 */
export interface SessionStateChangeAccepted {
  readonly session: Session;
}

/**
 * `DELETE /api/sessions/{id}` — a 200 **with a body**, not a 204.
 *
 * Beyond the deleted session the server reports the footprint cleanup it
 * attempted. That cleanup is best-effort: `cleanupComplete: false` means the
 * session row is gone but some stored objects were not purged and a later sweep
 * must finish the job, so a caller that needs the bytes actually gone has to
 * read it.
 */
export interface SessionDeleteAccepted {
  readonly session: Session;
  /** How many stored session-file objects the delete purged. */
  readonly purgedSessionFileObjects: number;
  /** False when a cleanup leg failed and the footprint is only partly purged. */
  readonly cleanupComplete: boolean;
}

/** The wire form of {@link SessionDeleteAccepted} (flat runtime fields on `session`). */
export type SessionDeleteResponseWire = z.infer<typeof SessionDeleteResponseSchema>;

type _SessionDeleteMatchesWire = Assert<
  KeysEqual<keyof SessionDeleteResponseWire, keyof SessionDeleteAccepted>
>;

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
  /** AEX showback estimate (USD, >= 0). Includes managed-gateway model tokens. */
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
 * One captured session file, as `GET /api/sessions/{id}/files` and the file-link
 * route project it. Use `session.files.link(...)` to mint a temporary direct
 * download URL.
 *
 * The workspace-relative path is `filename` here. The writer-token
 * {@link ChildResultFile} projection of the same concept calls it `path` — two
 * server projections of one thing that differ in exactly that key name.
 *
 * `filename`, `contentType` and `createdAt` are declared optional but the server
 * sends all three unconditionally. They stay optional because this type doubles
 * as a caller-supplied {@link SessionFileSelector}, where requiring metadata a
 * caller does not have would make the selector unusable.
 */
export interface SessionFile {
  readonly id: string;
  /** Checkpoint this file record is pinned to. */
  readonly checkpointId: string;
  /** Workspace-relative path. Always sent by the server; see the note above. */
  readonly filename?: string;
  /** Exact byte length recorded by the committed checkpoint. */
  readonly sizeBytes: number;
  /** Lowercase SHA-256 digest of the committed bytes. */
  readonly sha256: string;
  readonly contentType?: string;
  readonly createdAt?: string;
}

/** The wire form of {@link SessionFile}: every field unconditional. */
export type SessionFileWire = z.infer<typeof SessionFileSchema>;

type _SessionFileMatchesWire = Assert<KeysEqual<keyof SessionFileWire, keyof SessionFile>>;

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

/**
 * A minted direct-download link, as the SDK returns it.
 *
 * `expiresAt` is NOT on the wire: the server sends `{ url, expiresInSeconds }`
 * (plus `file` on the file-link route) and `sessionFileLink()` synthesises the
 * absolute timestamp from the mint time — which is why it is required here and
 * absent from {@link SessionFileLinkWire}. `file` is optional because the event
 * archive link (`POST .../events/link`) mints a URL for a generated archive that
 * has no file record.
 */
export interface SessionFileLink {
  readonly url: string;
  /** Synthesised client-side from the mint time; the server sends no absolute time. */
  readonly expiresAt: string;
  readonly expiresInSeconds: number;
  /** The file the URL points at. Absent on the event-archive link. */
  readonly file?: SessionFile;
}

/** `POST /api/sessions/{id}/files/{fileId}/link` exactly as served. */
export type SessionFileLinkWire = z.infer<typeof SessionFileLinkResponseSchema>;

/**
 * Identity of a CONTROL-plane account principal (a PAT / device session),
 * as returned by the dashboard BFF `GET /api/whoami` when the bearer is an
 * account token (`aexu_...`). Distinct from {@link WhoAmI} (a data-plane
 * workspace key): an account principal spans orgs and carries no workspace or
 * data-plane limits. Used by `aex login --api-key <account PAT>` to validate a
 * PAT before persisting it, and by any control-plane identity probe.
 */
export interface AccountWhoAmI {
  readonly ok: true;
  readonly principalType: "account_token";
  /** The app user the token authenticates as. */
  readonly appUserId: string;
  readonly scopes: readonly string[];
  /** Org the token is scoped to, when the server reports one. */
  readonly orgId?: string;
  readonly tokenId?: string;
  readonly tokenName?: string;
  /** e.g. `"account"` — the account-token kind reported by the server. */
  readonly tokenKind?: string;
}

export interface WhoAmI {
  readonly ok: true;
  readonly principalType: "api_key";
  /**
   * The PUBLIC workspace id, `wsp_<32 hex>` — the handler renders it through
   * `publicWorkspaceId`. {@link McpServerRecord.workspaceId} matches this;
   * {@link BillingLedgerEntry.workspaceId} and {@link WorkspaceEraseResult} do
   * NOT — they carry the raw id. One concept, two renderings on the same API.
   */
  readonly workspaceId: string;
  readonly scopes: readonly string[];
  /**
   * Authenticated runtime availability for this workspace. Optional only for
   * compatibility with deployments predating capability discovery; when
   * present it is validated as one complete, non-contradictory runtime set.
   */
  readonly runtimeCapabilities?: RuntimeCapabilities;
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
    /**
     * ISO-8601 with a `Z` — this route formats the timestamp. The SAME concept
     * on {@link BillingSummary.pastDueAt} is the Aurora Data API's raw
     * `"YYYY-MM-DD HH:MM:SS"` text. Present only when the subscription gate is
     * not `ok` AND the underlying timestamp exists.
     */
    readonly pastDueAt?: string;
    /** ISO-8601 with a `Z`. Same presence rule as {@link WhoAmI.limits.pastDueAt}. */
    readonly graceEndsAt?: string;
  };
}

export interface RuntimeCapabilities {
  readonly schemaVersion: 1;
  readonly capabilityVersion: string;
  readonly capabilityHash: `sha256:${string}`;
  readonly availableRuntimeKinds: readonly RuntimeKind[];
  readonly sizesByRuntimeKind: Partial<Record<RuntimeKind, readonly RuntimeSize[]>>;
  readonly unavailable: Partial<Record<RuntimeKind, { readonly code: string }>>;
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
  /** ISO-8601 with a `Z` suffix. */
  readonly createdAt?: string;
  /** ISO-8601 with a `Z` suffix. */
  readonly updatedAt?: string;
  readonly deletedAt?: string | null;
}

/**
 * Customer-facing billing summary — `GET /api/billing` (scope `billing:read`).
 * All money fields are USD numbers.
 *
 * This shape used to carry `[key: string]: unknown` as an "additive server
 * fields pass through" promise. That promise is precisely why `accountType` and
 * `pastDueAt` — both sent unconditionally — went undeclared for as long as they
 * did, and why no conformance check could notice: an index signature makes every
 * undeclared field structurally legal. It is gone; the fields are declared.
 */
export interface BillingSummary {
  /** Prepaid balance (authoritative ledger sum). */
  readonly balanceUsd: number;
  /** Accrued spend for the current calendar month. */
  readonly monthSpendUsd: number;
  /** Monthly spend cap enforced on new sessions. */
  readonly spendCapUsd: number;
  /**
   * The raw `plan_key` column. NOT normalised to the `free | pro | team` union
   * that `whoami.limits.planKey` is narrowed to — same concept, two types, and
   * this is the loose one.
   */
  readonly planKey: string;
  readonly subscriptionStatus: "none" | "active" | "past_due" | "canceled";
  /** `"active"` once a payment method is bound. Aurora-authoritative. */
  readonly paymentMethodStatus: "none" | "active";
  /** `"internal"` marks an account exempt from the standard rate card. */
  readonly accountType: "standard" | "internal";
  /**
   * When the subscription entered `past_due`, or `null`.
   *
   * **NOT ISO-8601.** This column is selected raw, so it arrives as the Aurora
   * Data API's text rendering — `"YYYY-MM-DD HH:MM:SS"`, no `T`, no zone. The
   * SAME concept on `whoami.limits.pastDueAt` IS ISO-8601 with a `Z`, because
   * that route formats it. Parse accordingly; do not assume one format.
   */
  readonly pastDueAt: string | null;
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

/** Hosted checkout/portal session. The client should open `url`. One key. */
export interface BillingHostedSession {
  readonly url: string;
}

/**
 * One row of the ORG credit ledger as returned by `GET /api/billing/ledger`
 * (newest first). `amountUsd` is signed: top-ups are positive, run charges
 * negative.
 *
 * Every field below is selected by the handler on every row, so none is
 * optional; the nullable ones are nullable, which is a different statement. The
 * index signature this shape used to carry is gone for the reason given on
 * {@link BillingSummary}.
 */
export interface BillingLedgerEntry {
  readonly id: string;
  /** e.g. `top_up`, `session_charge`. Open server vocabulary. */
  readonly entryType: string;
  readonly amountUsd: number;
  readonly currency: string;
  /** The session this entry charges, `null` for non-run entries. */
  readonly sessionId: string | null;
  /**
   * Cost-attribution tag: the workspace the charge belongs to, `null` for
   * org-level rows such as a top-up.
   *
   * A RAW workspace id (a UUID), not the public `wsp_<hex>` form that
   * `whoami.workspaceId` and the MCP-server records carry. Same concept, two
   * renderings; compare with care.
   */
  readonly workspaceId: string | null;
  readonly description: string | null;
  readonly createdBy: string;
  /**
   * **NOT ISO-8601** — selected raw, so it arrives as the Aurora Data API's
   * `"YYYY-MM-DD HH:MM:SS"` text. See {@link BillingSummary.pastDueAt}.
   */
  readonly createdAt: string;
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

// ===========================================================================
// Control-plane resources (orgs / workspaces / API keys / members)
//
// These describe the ACCOUNT/control-plane surface served by the dashboard BFF
// (distinct from the data-plane, which self-routes on a workspace key). An org
// owns workspaces and is the billing/roles/cap boundary; a workspace stays the
// runtime tenant. Records are metadata-only. The value-bearing one-time reveals
// ({@link NewWorkspace} / {@link NewApiKey}) carry the freshly minted key
// exactly once; the SDK wraps that field in a redacted `SecretString`.
//
// These records used to be "additive-tolerant" — `[key: string]: unknown`, an
// unknown key from a newer deployment passing through rather than being
// rejected — matching the `SecretRecord` / `BillingSummary` precedent. That
// precedent is retired: an index signature makes EVERY undeclared field
// structurally legal, so no conformance check can ever report one, which is
// exactly how `BillingSummary` came to be missing two fields the server always
// sends. The declared key set is now the whole statement.
//
// UNVERIFIED, unlike the data-plane families: the control plane has no route
// table in this package, so these have no response schema and nothing checks
// them against real bytes.
// ===========================================================================

/**
 * One org the caller belongs to — the ownership / billing / roles wrapper ABOVE
 * workspaces. `role` is the caller's own membership role in this org
 * (`admin | member`); billing and the per-org workspace cap live at this level.
 */
export interface OrgRecord {
  readonly id: string;
  readonly name: string;
  /** Globally-unique org slug (`/org/<slug>`); omitted by older deployments. */
  readonly slug?: string;
  /** Plan key that governs billing + the per-org workspace cap (e.g. `free`). */
  readonly planKey?: string;
  /** The caller's role in this org: `admin` or `member`. */
  readonly role?: string;
  readonly createdAt?: string;
}

/** Request body for {@link createOrg} — a display name; the server assigns id/slug. */
export interface CreateOrgRequest {
  readonly name: string;
}

/**
 * A workspace as seen from the CONTROL plane (management view): its id, name,
 * and owning org. Distinct from the data-plane view — this never carries the
 * workspace's files/skills/secrets, only the row a dashboard/CLI lists.
 */
export interface WorkspaceRecord {
  readonly id: string;
  readonly name: string;
  /** Globally-unique workspace slug (`/workspace/<slug>`); omitted by older deployments. */
  readonly slug?: string;
  /** The org that owns this workspace. */
  readonly orgId: string;
  readonly createdAt?: string;
}

/** Request body for {@link createWorkspace}. Free tier caps at 3 workspaces per org. */
export interface CreateWorkspaceRequest {
  /** The org to create the workspace under. */
  readonly orgId: string;
  readonly name: string;
}

/**
 * One-time reveal returned by {@link createWorkspace}: the new workspace's id
 * plus its FIRST workspace-scoped, data-plane API key. The key is shown exactly
 * once at creation — the creating (account) principal has no other data-plane
 * access to it, though the owning user can see/delete it in the dashboard
 * (orphan recovery). The SDK wraps `apiKey` in a redacted `SecretString`.
 */
export interface NewWorkspace {
  readonly workspaceId: string;
  /** The workspace-scoped API key (`aex_<plane>_…`), revealed ONCE. */
  readonly apiKey: string;
  /** Globally-unique workspace slug, when the server assigns one. */
  readonly slug?: string;
  /** The org that owns the new workspace. */
  readonly orgId?: string;
}

/**
 * Metadata for one API key (data-plane workspace key OR account PAT). NEVER
 * carries the secret value — the value is write-only and revealed only once via
 * {@link NewApiKey}. `kind` distinguishes a `workspace` key from an `account`
 * PAT; `workspaceId` is present only for workspace keys.
 */
export interface ApiKeyRecord {
  readonly id: string;
  readonly name?: string;
  /** `workspace` (data-plane) or `account` (control-plane PAT). */
  readonly kind?: string;
  /** Present for workspace keys; absent for account PATs. */
  readonly workspaceId?: string;
  readonly scopes?: readonly string[];
  readonly createdAt?: string;
  readonly lastUsedAt?: string | null;
  readonly revokedAt?: string | null;
}

/**
 * Request body for {@link createApiKey}. Mint EITHER a workspace-scoped
 * data-plane key (pass `workspaceId`) or an account PAT (`account: true`) — the
 * two are mutually exclusive. Anti-escalation: an account PAT can mint workspace
 * keys but not another PAT (enforced server-side).
 */
export interface CreateApiKeyRequest {
  /** Mint a WORKSPACE-scoped data-plane key for this workspace. */
  readonly workspaceId?: string;
  /** Mint an ACCOUNT PAT (control-plane) instead. Mutually exclusive with `workspaceId`. */
  readonly account?: boolean;
  /** Optional human label for the key. */
  readonly name?: string;
  /** Optional scope restriction; defaults server-side. */
  readonly scopes?: readonly string[];
}

/**
 * One-time reveal returned by {@link createApiKey}: the key id plus the freshly
 * minted secret value, shown exactly once. The SDK wraps `apiKey` in a redacted
 * `SecretString`.
 */
export interface NewApiKey {
  readonly id: string;
  /** The freshly minted key value, revealed ONCE. */
  readonly apiKey: string;
  readonly name?: string;
  readonly kind?: string;
  readonly workspaceId?: string;
  readonly scopes?: readonly string[];
}

/** One member of an org (from {@link listOrgMembers}). Never carries credentials. */
export interface OrgMemberRecord {
  /** The member's stable account (app-user) id. */
  readonly appUserId: string;
  readonly email?: string;
  /** `admin` or `member`. */
  readonly role: string;
  /** `active` or `pending` (an unaccepted invite). */
  readonly status?: string;
  readonly createdAt?: string;
}

/** Request body for {@link createOrgInvite} — invite an email at a role. */
export interface CreateOrgInviteRequest {
  readonly email: string;
  /** `admin` or `member`; defaults server-side to `member`. */
  readonly role?: string;
}

/**
 * A pending team invite created by {@link createOrgInvite}. Metadata only — the
 * invite token itself is delivered out-of-band (email), never returned here.
 */
export interface OrgInvite {
  readonly id: string;
  readonly orgId: string;
  readonly email: string;
  readonly role: string;
  /** `pending` until accepted. */
  readonly status?: string;
  readonly expiresAt?: string;
  readonly createdAt?: string;
}

// ===========================================================================
// Route families that had NO declared client type
//
// Six groups of public data-plane routes were reachable only by hand-rolling a
// request and casting the result: every `mcp-servers` route, the workspace
// webhook-delivery list, the three operator billing overrides, the data-plane
// workspace erase, and the two writer-token child hops. Each is DERIVED from its
// response schema (`z.infer`) rather than restated, so the type and the bytes
// the C4 harness validates come from one declaration.
// ===========================================================================

/**
 * One persisted workspace MCP server — `POST/GET /api/mcp-servers`,
 * `GET/DELETE /api/mcp-servers/{id}`.
 *
 * `headerShape` lists header NAMES ONLY. The values live in the secret store and
 * are write-only through this API; a `headers` or `authorization` key appearing
 * on a read is a leak, and the response schema fails the suite if one does.
 *
 * `workspaceId` is the PUBLIC `wsp_<hex>` form (the handler runs it through
 * `publicWorkspaceId`), matching `whoami.workspaceId` — and NOT matching
 * {@link BillingLedgerEntry.workspaceId} or {@link WorkspaceEraseResult}, which
 * are raw ids. `createdAt` / `updatedAt` are ISO-8601 with a `Z`.
 */
export type McpServerRecord = z.infer<typeof McpServerRecordSchema>;

/** `GET /api/mcp-servers` — every workspace MCP server, newest first. Not paged. */
export type McpServerList = z.infer<typeof McpServerListResponseSchema>;

/**
 * One row of `GET /api/webhook/deliveries` — the WORKSPACE view of the delivery
 * ledger, which is {@link SessionWebhookDelivery} plus the `sessionId` and
 * `callbackUrl` it belongs to. Server-capped at the 100 most recent: no `limit`,
 * no cursor.
 */
export type WorkspaceWebhookDelivery = z.infer<typeof WorkspaceWebhookDeliverySchema>;

/**
 * `DELETE /api/workspaces/{workspaceId}` on the DATA plane — the owner's GDPR
 * hard-erase, answering 200 with counters rather than 204. Idempotent: erasing
 * an absent workspace answers 200 with every counter at zero, indistinguishable
 * from erasing an empty one.
 *
 * NOT the same route as {@link deleteWorkspace}, which is the CONTROL plane's
 * `DELETE /api/workspaces/{id}` and returns no body. Same method, same path
 * pattern, different plane and different response.
 *
 * `workspaceId` here is the RAW id, not the public `wsp_<hex>` form.
 */
export type WorkspaceEraseResult = z.infer<typeof WorkspaceEraseResponseSchema>;

/** `POST /api/admin/billing/topup` — operator credit grant. */
export type AdminBillingTopupResult = z.infer<typeof AdminBillingTopupResponseSchema>;

/** `POST /api/admin/billing/payment-method` — operator payment-method override. */
export type AdminBillingPaymentMethodResult = z.infer<
  typeof AdminBillingPaymentMethodResponseSchema
>;

/** `POST /api/admin/billing/account-type` — operator account-type override. */
export type AdminBillingAccountTypeResult = z.infer<typeof AdminBillingAccountTypeResponseSchema>;

/**
 * `GET /api/sessions/{id}/result` — a subagent child's result, read by the
 * in-container runtime with the child's WRITER TOKEN, not a workspace API key.
 * Either still in flight (`settling` / `running` / `queued`) or `finished` with
 * an outcome.
 *
 * Its files key the workspace-relative path as **`path`**, where the public
 * `/files` routes call the same thing `filename` ({@link SessionFile}). Two
 * server projections of one concept differing in one key name.
 */
export type ChildResult = z.infer<typeof ChildResultResponseSchema>;

/**
 * `POST /api/sessions/{id}/finalize` — the writer-token child settle hop. Note
 * the absence of a `session` envelope: this route is not shaped like the public
 * session routes.
 */
export type ChildFinalizeResult = z.infer<typeof ChildFinalizeResponseSchema>;
