import type * as z from "zod/mini";
import type { SessionStatus, SessionTerminalOutcome } from "./status.js";
import type { RuntimeSize } from "./runtime-sizes.js";
import type { RuntimeKind } from "./runtime-kind.js";
import type { SessionCostProviderUsage } from "./session-cost.js";
import type { ProviderFault } from "./provider-fault.js";
import type { BillingAdmissionState } from "./billing-admission.js";
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
 * (`spot_container` (default — see
 * {@link import("./runtime-kind.js").DEFAULT_RUNTIME_KIND}) | `container` |
 * `lambda`), `size` selects the managed box preset. Grouped so the SDK surface
 * reads `runtime: { kind: "spot_container", size: Sizes.CPU_2_8GB }`.
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
    /** Prepaid credit balance in USD — one of the two things the credit gate compares. */
    readonly balanceUsd: number;
    /** Submit floor the balance is compared against; only meaningful when {@link creditGateActive}. */
    readonly balanceGraceFloorUsd: number;
    /**
     * The OTHER half of the credit predicate: free model-usage allowance left
     * this UTC month, in USD. A submit is admitted while EITHER this or
     * {@link balanceUsd} is positive; both at zero is `402 insufficient_credits`.
     */
    readonly llmTokenAllowanceRemainingUsd: number;
    /** Whether the prepaid credit gate applies to this workspace. */
    readonly creditGateActive: boolean;
    readonly paymentMethodStatus: "none" | "active";
    /** What the gates sized this workspace at — card presence is the lever, not a plan. */
    readonly admissionState: BillingAdmissionState;
    /** True when auto-recharge is on, i.e. exhaustion triggers a top-up instead of a 402. */
    readonly autoTopupEnabled: boolean;
    readonly accountType: "standard" | "internal";
  };
}

/** The closed capability vocabulary a runtime profile declares. */
export const RUNTIME_CAPABILITY_NAMES = [
  "toolExecution",
  "workspaceCheckpoint",
  "workspaceFileCapture",
  "streamingDeltas",
  "approvalGate",
  "postHook",
  "mcpTools",
  "scheduledWait",
  "customerSecrets",
  "containedEgress"
] as const;

export type RuntimeCapabilityName = (typeof RUNTIME_CAPABILITY_NAMES)[number];

/** Two states only. There is no partial state: a runtime performs a capability or refuses it. */
export type RuntimeCapabilityState = "supported" | "unsupported";

/**
 * What one runtime will actually do. Runtime choice is not free of behavioral
 * consequences and this contract does not pretend otherwise: a submission that
 * exceeds the selected profile is refused before execution, never degraded.
 */
export interface RuntimeProfile {
  readonly schemaVersion: 1;
  readonly runtimeKind: RuntimeKind;
  readonly capabilities: Readonly<Record<RuntimeCapabilityName, RuntimeCapabilityState>>;
  readonly limits: {
    /** Hard ceiling on one session's lifetime, in ms. */
    readonly maxSessionMs: number;
    /** Hard ceiling on ONE atomic LLM call or tool call, in ms. */
    readonly maxSingleEffectMs: number;
    /** Hard ceiling on the sandbox workspace, in bytes. */
    readonly maxWorkspaceBytes: number;
    readonly maxConcurrentToolCalls: number;
  };
  readonly delivery: {
    /**
     * `at-least-once` means a reclaim can replay an interrupted step, so a tool
     * with an external side effect may run more than once.
     */
    readonly toolExecution: "at-least-once" | "exactly-once";
    readonly coldStartClass: "warm" | "cold-seconds" | "cold-tens-of-seconds";
    /** `zero` bills nothing while a session is parked or waiting. */
    readonly idleBilling: "wall-clock" | "zero";
  };
  readonly computeBasis: "wall_clock" | "microvm_running";
}

export interface RuntimeCapabilities {
  readonly schemaVersion: 1;
  readonly capabilityVersion: string;
  readonly capabilityHash: `sha256:${string}`;
  readonly availableRuntimeKinds: readonly RuntimeKind[];
  readonly sizesByRuntimeKind: Partial<Record<RuntimeKind, readonly RuntimeSize[]>>;
  readonly unavailable: Partial<Record<RuntimeKind, { readonly code: string }>>;
  /**
   * TOTAL over every runtime kind, including ones this workspace may not name.
   * Availability and capability are different questions: deciding whether to ask
   * for access to a runtime requires knowing what it would do for you.
   */
  readonly profilesByRuntimeKind: Record<RuntimeKind, RuntimeProfile>;
}

// The ACCOUNT and WORKSPACE-MANAGEMENT record types — workspace secret
// metadata, billing, the control-plane org/workspace/API-key surface, and the
// response-derived route families — live in `account-types.ts`. Same public
// barrel, different file: this module stays about the session lifecycle.
