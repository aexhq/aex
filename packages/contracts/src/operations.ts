import { createHash } from "node:crypto";
import { CANONICAL_SHA256_DIGEST_PATTERN } from "./canonical-sha256.js";
import { newId } from "./ids.js";
import { isRecord, isStringLiteral } from "./value-guards.js";
import type { HttpClient } from "./http.js";
import type { AexEvent } from "./event-envelope.js";
import type {
  OtlpExportLogsServiceRequest,
  OtlpExportRequest,
  OtlpExportTraceServiceRequest,
  OtlpSignal
} from "./otlp-projection.js";
import { AexNetworkError, SessionConfigValidationError, SessionStateError } from "./sdk-errors.js";
import {
  type SessionRecordArtifactSummaryV1,
  type SessionRecordDownloadErrorV1
} from "./session-record.js";
import {
  buildSessionArchive,
  buildSessionEventsArchive,
  buildSessionFilesArchive,
  buildSessionMetadataArchive,
  type SessionArchiveEntry
} from "./session-archive.js";
import {
  classifySessionFile,
  filterSessionFiles,
  isPathSelector,
  resolveSessionFileSelector,
  toFilenameMatcher
} from "./session-file-query.js";
import {
  configError,
  idempotencyHeaders,
  resolveIdempotencyKey,
  deriveMessageIdempotencyKey,
  type IdempotencyOptions
} from "./operation-core.js";
import type {
  ChildSessionRef,
  SessionFile,
  SessionFileLink,
  SessionFileLinkWire,
  SessionFileLinkOptions,
  SessionFileDownload,
  SessionFilePathSelector,
  SessionFileSelector,
  SessionFilesQuery,
  SessionFilesSnapshot,
  SessionFileText,
  ReadSessionFileTextOptions,
  Session,
  SessionRun,
  SessionCreateRequest,
  SessionListPage,
  SessionListQuery,
  SessionMessageAccepted,
  SessionMessageRequest,
  SessionMessagesPage,
  SessionMessagesQuery,
  SessionDeleteAccepted,
  SessionStateChangeAccepted,
  SessionWebhookDelivery,
  WhoAmI,
  AccountWhoAmI,
  RuntimeCapabilityName,
  RuntimeCapabilityState,
  RuntimeProfile
} from "./runtime-types.js";
import { BILLING_ADMISSION_STATES } from "./billing-admission.js";
import { RUNTIME_CAPABILITY_NAMES, SESSION_RUN_PHASES } from "./runtime-types.js";
import { RUNTIME_SIZES, parseRuntimeSize, type RuntimeSize } from "./runtime-sizes.js";
import { RUNTIME_KINDS, type RuntimeKind } from "./runtime-kind.js";
import { SESSION_STATUSES, SESSION_TERMINAL_OUTCOMES } from "./status.js";
import { parseProviderFault } from "./provider-fault.js";
import type { ToolInputSchema } from "./session-config.js";
import type {
  WorkspaceFileRecord,
  WorkspaceInstructionRecord,
  WorkspaceResourceListQuery,
  WorkspaceResourcePage,
  WorkspaceSkillRecord,
  WorkspaceToolRecord
} from "./workspace-resources.js";

export {
  classifySessionFile,
  filterSessionFiles,
  resolveSessionFileSelector,
  toFilenameMatcher
} from "./session-file-query.js";

// The idempotency-key policy moved to the `operation-core.js` leaf so
// `account-operations.js` can share it without either module importing the
// other. Named rather than star re-exported: `configError` and
// `idempotencyHeaders` stay module-private, exactly as they were when they
// lived here.
export { IDEMPOTENCY_KEY_MAX_LENGTH, deriveMessageIdempotencyKey, resolveIdempotencyKey } from "./operation-core.js";
export type { IdempotencyOptions } from "./operation-core.js";

// Billing, workspace secrets and the control plane. Star re-exported because
// every name over there is a public operation this module used to declare, and
// `internal.ts` publishes this module as the whole `operations` namespace.
export * from "./account-operations.js";

/**
 * The single source of truth for SDK<->BFF transport. The SDK class
 * AND the CLI subcommands both call these functions; neither
 * surface re-implements HTTP requests against the dashboard.
 *
 * Every function takes an HttpClient (so callers control auth + fetch
 * injection) and returns parsed responses.
 *
 * Workspace identity is derived server-side from the API key on
 * every request; callers do not pass `workspaceId`.
 */

const SESSION_STATUS_SET = new Set<string>(SESSION_STATUSES);
const SESSION_FILE_SHA256_PATTERN = /^[0-9a-f]{64}$/;
const SESSION_FILE_CHECKPOINT_ID_PATTERN = /^[A-Za-z0-9._-]{1,200}$/;

export interface CreateSessionWithMessageOptions extends IdempotencyOptions {
  readonly messageIdempotencyKey?: string;
}

export async function createSession(
  http: HttpClient,
  request: SessionCreateRequest,
  options?: IdempotencyOptions
): Promise<Session> {
  const headers = idempotencyHeaders(options);
  const result = await http.request<{ readonly session: Session }>("/api/sessions", {
    method: "POST",
    ...(headers ? { headers } : {}),
    body: JSON.stringify(request)
  });
  return unwrapSession(result);
}

/** Create a session and enqueue its first message without waiting for the RUN terminal. */
export async function createSessionWithMessage(
  http: HttpClient,
  request: SessionCreateRequest,
  input: SessionMessageRequest["input"],
  options?: CreateSessionWithMessageOptions
): Promise<SessionMessageAccepted> {
  const createKey = resolveIdempotencyKey(options?.idempotencyKey);
  const messageKey = options?.messageIdempotencyKey !== undefined
    ? resolveIdempotencyKey(options.messageIdempotencyKey)
    : deriveMessageIdempotencyKey(createKey);
  if (
    !((typeof input === "string" && input.length > 0) ||
      (Array.isArray(input) && input.length > 0 && input.every((part) => typeof part === "string" && part.length > 0)))
  ) {
    throw configError("input", "session message must be a non-empty string or string array");
  }
  const created = await createSession(http, request, { idempotencyKey: createKey });
  return sendSessionMessage(http, created.id, { input }, { idempotencyKey: messageKey });
}

export async function getSession(http: HttpClient, sessionId: string): Promise<Session> {
  const result = await http.request<{ readonly session: Session }>(
    `/api/sessions/${encodeURIComponent(sessionId)}`
  );
  return unwrapSession(result);
}

export async function listSessions(
  http: HttpClient,
  query?: SessionListQuery
): Promise<SessionListPage> {
  validateSessionListQuery(query);
  const params: Record<string, string> = {};
  if (query?.status !== undefined) params.status = query.status;
  if (query?.since !== undefined) params.since = query.since;
  if (query?.limit !== undefined) params.limit = String(query.limit);
  if (query?.cursor !== undefined) params.cursor = query.cursor;
  const page = await http.request<unknown>("/api/sessions", {}, params);
  if (!isRecord(page) || !Array.isArray(page.sessions)) {
    throw new SessionStateError("sessions.list returned an invalid page: sessions must be an array");
  }
  const sessions = page.sessions.map((value, index) => {
    if (!isRecord(value)) {
      throw new SessionStateError(`sessions.list returned an invalid row at index ${index}`);
    }
    for (const field of ["id", "status", "createdAt", "updatedAt"] as const) {
      if (typeof value[field] !== "string" || value[field].length === 0) {
        throw new SessionStateError(`sessions.list row ${index} has an invalid ${field}`);
      }
    }
    if (!SESSION_STATUS_SET.has(value.status as string)) {
      throw new SessionStateError(`sessions.list row ${index} has an unknown lifecycle status`);
    }
    assertCanonicalSessionWireFields(value, `sessions.list row ${index}`);
    if (typeof value.acceptsMessages !== "boolean") {
      throw new SessionStateError(`sessions.list row ${index} has an invalid acceptsMessages`);
    }
    if (
      value.costUsd !== undefined &&
      (typeof value.costUsd !== "number" || !Number.isFinite(value.costUsd) || value.costUsd < 0)
    ) {
      throw new SessionStateError(`sessions.list row ${index} has an invalid costUsd`);
    }
    const currentRun = normalizeOptionalSessionRun(value.currentRun, value.id as string, `sessions.list row ${index}.currentRun`);
    const lastRun = normalizeOptionalSessionRun(value.lastRun, value.id as string, `sessions.list row ${index}.lastRun`);
    // `providerFault` is detail-only and must never leak onto SessionSummary.
    const { providerFault: _providerFault, ...normalized } = normalizeSessionRuntime(value, `sessions.list row ${index}`);
    return {
      ...normalized,
      ...(currentRun !== undefined ? { currentRun } : {}),
      ...(lastRun !== undefined ? { lastRun } : {})
    } as unknown as SessionListPage["sessions"][number];
  });
  if (page.nextCursor !== undefined && typeof page.nextCursor !== "string") {
    throw new SessionStateError("sessions.list returned an invalid page: nextCursor must be a string");
  }
  return {
    sessions,
    ...(typeof page.nextCursor === "string" ? { nextCursor: page.nextCursor } : {})
  };
}

function validateSessionListQuery(query: SessionListQuery | undefined): void {
  if (query?.limit !== undefined && (!Number.isInteger(query.limit) || query.limit < 1 || query.limit > 100)) {
    throw configError("limit", "sessions.list limit must be an integer between 1 and 100");
  }
  if (query?.status !== undefined && !SESSION_STATUS_SET.has(query.status)) {
    throw configError("status", "sessions.list status must be a session lifecycle status");
  }
  if (query?.since !== undefined && (query.since.length === 0 || !Number.isFinite(Date.parse(query.since)))) {
    throw configError("since", "sessions.list since must be an ISO-8601 timestamp");
  }
  if (query?.cursor !== undefined && query.cursor.length === 0) {
    throw configError("cursor", "sessions.list cursor must be a non-empty opaque string");
  }
}

export async function sendSessionMessage(
  http: HttpClient,
  sessionId: string,
  request: SessionMessageRequest,
  options?: IdempotencyOptions
): Promise<SessionMessageAccepted> {
  const headers = idempotencyHeaders(options);
  const accepted = await http.request<SessionMessageAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/messages`,
    {
      method: "POST",
      ...(headers ? { headers } : {}),
      body: JSON.stringify(request)
    }
  );
  return normalizeSessionMessageAccepted(accepted, sessionId);
}

export async function listSessionMessages(
  http: HttpClient,
  sessionId: string,
  query?: SessionMessagesQuery
): Promise<SessionMessagesPage> {
  const params: Record<string, string> = {};
  if (query?.limit !== undefined) params.limit = String(query.limit);
  if (query?.cursor !== undefined) params.cursor = query.cursor;
  if (query?.since !== undefined) params.since = query.since;
  return http.request<SessionMessagesPage>(
    `/api/sessions/${encodeURIComponent(sessionId)}/messages`,
    {},
    params
  );
}

export async function suspendSession(
  http: HttpClient,
  sessionId: string
): Promise<SessionStateChangeAccepted> {
  const accepted = await http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/suspend`,
    { method: "POST" }
  );
  return normalizeSessionAccepted(accepted, "session suspend response");
}

export async function cancelSession(
  http: HttpClient,
  sessionId: string
): Promise<SessionStateChangeAccepted> {
  const accepted = await http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/cancel`,
    { method: "POST" }
  );
  return normalizeSessionAccepted(accepted, "session cancel response");
}

export async function resumeSession(
  http: HttpClient,
  sessionId: string
): Promise<SessionStateChangeAccepted> {
  const accepted = await http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/resume`,
    { method: "POST" }
  );
  return normalizeSessionAccepted(accepted, "session resume response");
}

/**
 * Request the HITL write-gate: park the session `awaiting_approval` before its
 * next gated action (mirrors {@link suspendSession}). Imperative counterpart to
 * the declarative submission-time `approvalGate`.
 */
export async function requestApproval(
  http: HttpClient,
  sessionId: string
): Promise<SessionStateChangeAccepted> {
  const accepted = await http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/request-approval`,
    { method: "POST" }
  );
  return normalizeSessionAccepted(accepted, "session approval-request response");
}

/** Approve an `awaiting_approval` session so the held turn resumes (→ running). */
export async function approveSession(
  http: HttpClient,
  sessionId: string
): Promise<SessionStateChangeAccepted> {
  const accepted = await http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/approve`,
    { method: "POST" }
  );
  return normalizeSessionAccepted(accepted, "session approve response");
}

/** Deny an `awaiting_approval` session so the held turn is cancelled (→ cancelled). */
export async function denySession(
  http: HttpClient,
  sessionId: string
): Promise<SessionStateChangeAccepted> {
  const accepted = await http.request<SessionStateChangeAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}/deny`,
    { method: "POST" }
  );
  return normalizeSessionAccepted(accepted, "session deny response");
}

/**
 * Delete a session and purge its stored footprint.
 *
 * Answers 200 with a BODY, not 204 — the deleted session plus the counters from
 * a best-effort cleanup. This used to be typed `SessionStateChangeAccepted |
 * void`, which said the body might not arrive and hid the two cleanup fields
 * entirely; it always arrives, and `cleanupComplete: false` is the only signal
 * that some stored objects survived the delete.
 */
export async function deleteSession(
  http: HttpClient,
  sessionId: string
): Promise<SessionDeleteAccepted> {
  const accepted = await http.request<SessionDeleteAccepted>(
    `/api/sessions/${encodeURIComponent(sessionId)}`,
    { method: "DELETE" }
  );
  const normalized = normalizeSessionAccepted(accepted, "session delete response");
  if (typeof normalized.purgedSessionFileObjects !== "number") {
    throw new SessionStateError("session delete response is missing purgedSessionFileObjects");
  }
  if (typeof normalized.cleanupComplete !== "boolean") {
    throw new SessionStateError("session delete response is missing cleanupComplete");
  }
  return normalized;
}

export async function listSessionEvents(
  http: HttpClient,
  sessionId: string
): Promise<readonly AexEvent[]> {
  const events: AexEvent[] = [];
  for await (const event of iterateSessionEvents(http, sessionId)) events.push(event);
  return events;
}

export interface IterateSessionEventsOptions {
  /** Number of events requested per API page. Default and maximum: 1000. */
  readonly pageSize?: number;
  readonly signal?: AbortSignal;
}

/**
 * Lazily traverse a session's durable event history. At most one bounded API
 * page is retained by this iterator, and breaking the loop prevents later
 * pages from being requested.
 */
export async function* iterateSessionEvents(
  http: HttpClient,
  sessionId: string,
  options: IterateSessionEventsOptions = {}
): AsyncIterable<AexEvent> {
  const pageSize = options.pageSize;
  if (pageSize !== undefined && (!Number.isSafeInteger(pageSize) || pageSize < 1 || pageSize > 1000)) {
    throw configError("pageSize", "session event pageSize must be an integer between 1 and 1000");
  }

  const path = `/api/sessions/${encodeURIComponent(sessionId)}/events`;
  const seenCursors = new Set<string>();
  let cursor: string | undefined;
  for (let pageIndex = 0; pageIndex < LIST_EVENTS_PAGE_BUDGET; pageIndex += 1) {
    if (options.signal?.aborted) return;
    const query = {
      ...(pageSize === undefined ? {} : { limit: String(pageSize) }),
      ...(cursor === undefined ? {} : { cursor })
    };
    const result = await http.request<{
      readonly events: readonly AexEvent[];
      readonly nextCursor?: string | null;
    }>(path, options.signal === undefined ? {} : { signal: options.signal }, query);
    for (const event of result.events) yield event;
    if (result.nextCursor === undefined || result.nextCursor === null) return;
    if (typeof result.nextCursor !== "string" || result.nextCursor.length === 0) {
      throw new SessionStateError("session events response contains an invalid nextCursor", { sessionId });
    }
    if (seenCursors.has(result.nextCursor)) {
      throw new SessionStateError("session events pagination repeated a cursor", {
        sessionId,
        cursor: result.nextCursor
      });
    }
    seenCursors.add(result.nextCursor);
    cursor = result.nextCursor;
  }
  throw new SessionStateError("session events pagination exceeded its page budget", {
    sessionId,
    pageBudget: LIST_EVENTS_PAGE_BUDGET
  });
}

export interface SessionOtlpPage<Body extends OtlpExportRequest = OtlpExportRequest> {
  /** The unwrapped, standards-pure OTLP/HTTP JSON request body. */
  readonly body: Body;
  /** Opaque API cursor carried separately from the OTLP body. */
  readonly nextCursor?: string;
}

export interface IterateSessionOtlpOptions {
  readonly signal?: AbortSignal;
}

export async function getSessionOtlpPage(
  http: HttpClient,
  sessionId: string,
  signal: "traces",
  cursor?: string,
  abortSignal?: AbortSignal
): Promise<SessionOtlpPage<OtlpExportTraceServiceRequest>>;
export async function getSessionOtlpPage(
  http: HttpClient,
  sessionId: string,
  signal: "logs",
  cursor?: string,
  abortSignal?: AbortSignal
): Promise<SessionOtlpPage<OtlpExportLogsServiceRequest>>;
export async function getSessionOtlpPage(
  http: HttpClient,
  sessionId: string,
  signal: OtlpSignal,
  cursor?: string,
  abortSignal?: AbortSignal
): Promise<SessionOtlpPage> {
  const path = `/api/sessions/${encodeURIComponent(sessionId)}/otel`;
  const { response } = await http.download(
    path,
    abortSignal === undefined
      ? { headers: { accept: "application/json" } }
      : { headers: { accept: "application/json" }, signal: abortSignal },
    { signal, ...(cursor === undefined ? {} : { cursor }) }
  );
  let body: unknown;
  try {
    body = await response.json();
  } catch (cause) {
    throw new SessionStateError("session OTLP response is not valid JSON", { sessionId, signal }, { cause });
  }
  assertStandardsPureOtlpBody(body, sessionId, signal);
  const responseCursor = response.headers.get("x-aex-next-cursor");
  if (responseCursor !== null && responseCursor.trim().length === 0) {
    throw new SessionStateError("session OTLP response contains an invalid x-aex-next-cursor", {
      sessionId,
      signal
    });
  }
  return {
    body,
    ...(responseCursor === null ? {} : { nextCursor: responseCursor })
  };
}

/**
 * Lazily traverse standards-pure OTLP pages. Pagination metadata remains in
 * `x-aex-next-cursor`; it is never mixed into an OTLP request body.
 */
export async function* iterateSessionOtlpPages(
  http: HttpClient,
  sessionId: string,
  signal: OtlpSignal,
  options: IterateSessionOtlpOptions = {}
): AsyncIterable<OtlpExportRequest> {
  const seenCursors = new Set<string>();
  let cursor: string | undefined;
  for (let pageIndex = 0; pageIndex < LIST_EVENTS_PAGE_BUDGET; pageIndex += 1) {
    if (options.signal?.aborted) return;
    const page = signal === "traces"
      ? await getSessionOtlpPage(http, sessionId, "traces", cursor, options.signal)
      : await getSessionOtlpPage(http, sessionId, "logs", cursor, options.signal);
    if (page.nextCursor !== undefined && seenCursors.has(page.nextCursor)) {
      throw new SessionStateError("session OTLP pagination repeated a cursor", {
        sessionId,
        signal,
        cursor: page.nextCursor
      });
    }
    yield page.body;
    if (page.nextCursor === undefined) return;
    seenCursors.add(page.nextCursor);
    cursor = page.nextCursor;
  }
  throw new SessionStateError("session OTLP pagination exceeded its page budget", {
    sessionId,
    signal,
    pageBudget: LIST_EVENTS_PAGE_BUDGET
  });
}

function assertStandardsPureOtlpBody(
  body: unknown,
  sessionId: string,
  signal: OtlpSignal
): asserts body is OtlpExportRequest {
  const root = signal === "traces" ? "resourceSpans" : "resourceLogs";
  if (!isRecord(body) || Object.keys(body).length !== 1 || !Array.isArray(body[root])) {
    throw new SessionStateError(`session OTLP ${signal} response must contain only ${root}`, {
      sessionId,
      signal
    });
  }
}

export async function listSessionFiles(
  http: HttpClient,
  sessionId: string,
  query?: SessionFilesQuery
): Promise<SessionFilesSnapshot> {
  const requestedCheckpointId = query?.checkpointId === undefined
    ? undefined
    : requireSessionFileCheckpointId(query.checkpointId, "files.list");
  const result = await http.request<SessionFilesSnapshot>(
    `/api/sessions/${encodeURIComponent(sessionId)}/files`,
    {},
    requestedCheckpointId === undefined ? {} : { checkpointId: requestedCheckpointId }
  );
  if (
    !Array.isArray(result.files) ||
    !result.revision ||
    typeof result.revision.checkpointId !== "string" ||
    !SESSION_FILE_CHECKPOINT_ID_PATTERN.test(result.revision.checkpointId) ||
    typeof result.revision.runId !== "string" ||
    result.revision.runId.trim().length === 0 ||
    !Number.isSafeInteger(result.revision.turnSeq) ||
    result.revision.turnSeq < 1 ||
    typeof result.revision.committedAt !== "string" ||
    !Number.isFinite(Date.parse(result.revision.committedAt)) ||
    !Number.isSafeInteger(result.revision.throughSeq) ||
    result.revision.throughSeq < 0
  ) {
    throw new SessionStateError("session files response is missing checkpoint revision metadata", { sessionId });
  }
  if (requestedCheckpointId !== undefined && result.revision.checkpointId !== requestedCheckpointId) {
    throw new SessionStateError("session files response did not resolve the requested checkpoint", {
      sessionId,
      requestedCheckpointId,
      resolvedCheckpointId: result.revision.checkpointId
    });
  }
  for (const file of result.files) {
    if (typeof file.id !== "string" || file.id.length === 0) {
      throw new SessionStateError("session files response contains an invalid file id", { sessionId });
    }
    if (file.checkpointId !== result.revision.checkpointId) {
      throw new SessionStateError("session file is not pinned to the response checkpoint", {
        sessionId,
        fileId: file.id,
        fileCheckpointId: file.checkpointId,
        checkpointId: result.revision.checkpointId
      });
    }
    if (!Number.isSafeInteger(file.sizeBytes) || file.sizeBytes < 0) {
      throw new SessionStateError("session file is missing a valid committed byte length", {
        sessionId,
        fileId: file.id,
        sizeBytes: file.sizeBytes
      });
    }
    if (!SESSION_FILE_SHA256_PATTERN.test(file.sha256)) {
      throw new SessionStateError("session file is missing a valid committed SHA-256 digest", {
        sessionId,
        fileId: file.id
      });
    }
  }
  return {
    revision: result.revision,
    files: query === undefined ? result.files : filterSessionFiles(result.files, query)
  };
}

export async function getSessionCoordinatorTicket(
  http: HttpClient,
  sessionId: string
): Promise<CoordinatorTicket> {
  return http.request<CoordinatorTicket>(
    `/api/sessions/${encodeURIComponent(sessionId)}/events/ticket`,
    { method: "POST" }
  );
}

// Bound the transparent pager: the read route caps each page at 1000, so this
// admits up to ~1e6 events before bailing — past any real session, but bounded so a
// server that never clears `nextCursor` can't loop forever.
const LIST_EVENTS_PAGE_BUDGET = 1000;

/**
 * A coordinator WS connection grant minted by the hosted API's ticket broker.
 *
 * `ok` and `region` are on the wire and were undeclared. `region` is the plane
 * the ticket was minted in, which matters because the grant is only valid
 * against that region's coordinator.
 */
export interface CoordinatorTicket {
  readonly ok: true;
  readonly wsUrl: string;
  readonly ticket: string;
  readonly expiresAtMs: number;
  /** The region whose coordinator this ticket authorises. */
  readonly region: string;
}

export async function findSessionFiles(
  http: HttpClient,
  sessionId: string,
  query: SessionFilesQuery
): Promise<readonly SessionFile[]> {
  return (await listSessionFiles(http, sessionId, query)).files;
}

export async function findSessionFile(
  http: HttpClient,
  sessionId: string,
  query: SessionFilesQuery
): Promise<SessionFile | null> {
  const matches = await findSessionFiles(http, sessionId, query);
  if (matches.length === 0) return null;
  if (matches.length === 1) return matches[0]!;
  throw new SessionStateError("files.findOne: file query matched multiple files", {
    sessionId,
    matches: matches.map((file) => file.filename ?? file.id)
  });
}

export type SessionFileLinkSelector = SessionFileSelector | SessionFilesQuery;

export async function sessionFileLink(
  http: HttpClient,
  sessionId: string,
  selectorOrQuery: SessionFileLinkSelector,
  options?: SessionFileLinkOptions
): Promise<SessionFileLink> {
  const requestedCheckpointId = options?.checkpointId === undefined
    ? undefined
    : requireSessionFileCheckpointId(options.checkpointId, "files.link");
  const file = await resolveSessionFileLinkTarget(http, sessionId, selectorOrQuery, requestedCheckpointId);
  const expiresInSeconds = normalizeSessionFileLinkExpiresIn(options?.expiresIn);
  const checkpointId = requestedCheckpointId ?? file.checkpointId;
  // The WIRE type, not `SessionFileLink`: the server sends no absolute
  // `expiresAt`, and asking for the client type here is what made the synthesis
  // below look like a fallback for a field the server sometimes omits.
  const result = await http.request<SessionFileLinkWire>(
    sessionFileRoute(sessionId, file.id, "link", checkpointId),
    {
      method: "POST",
      body: JSON.stringify({ expiresInSeconds })
    }
  );
  const effectiveExpiresIn = result.expiresInSeconds ?? expiresInSeconds;
  return {
    url: result.url,
    expiresInSeconds: effectiveExpiresIn,
    expiresAt: syntheticExpiresAt(effectiveExpiresIn),
    file
  };
}

/**
 * The hosted API returns `{ url, expiresInSeconds }` without an absolute
 * timestamp; the documented `link.expiresAt` is synthesized client-side from
 * the mint time so it is always present on a returned link.
 */
function syntheticExpiresAt(expiresInSeconds: number): string {
  return new Date(Date.now() + expiresInSeconds * 1000).toISOString();
}

export async function eventArchiveLink(
  http: HttpClient,
  sessionId: string,
  options?: SessionFileLinkOptions
): Promise<SessionFileLink> {
  const expiresInSeconds = normalizeSessionFileLinkExpiresIn(options?.expiresIn);
  // `EventArchiveLinkResponseSchema` — `{ url, expiresInSeconds }` and NO
  // `file`, unlike the per-file link. Hence `SessionFileLink.file` staying
  // optional while `expiresAt` and `expiresInSeconds` are not.
  const result = await http.request<{
    readonly url: string;
    readonly expiresInSeconds?: number;
  }>(`/api/sessions/${encodeURIComponent(sessionId)}/events/link`, {
    method: "POST",
    body: JSON.stringify({ expiresInSeconds })
  });
  const effectiveExpiresIn = result.expiresInSeconds ?? expiresInSeconds;
  return {
    url: result.url,
    expiresInSeconds: effectiveExpiresIn,
    expiresAt: syntheticExpiresAt(effectiveExpiresIn)
  };
}

export async function downloadSessionFile(
  http: HttpClient,
  sessionId: string,
  selector: SessionFileSelector,
  options?: SessionFileTransferOptions
): Promise<SessionFileDownload> {
  const timeoutMs = normalizeSessionFileTransferTimeoutMs(options?.timeoutMs);
  const requestedCheckpointId = options?.checkpointId === undefined
    ? undefined
    : requireSessionFileCheckpointId(options.checkpointId, "files.download");
  const file = await resolveAuthoritativeSessionFile(http, sessionId, selector, requestedCheckpointId);
  const checkpointId = requestedCheckpointId ?? file.checkpointId;
  const path = sessionFileRoute(sessionId, file.id, "download", checkpointId);
  return { file, bytes: await downloadSessionFileBytesWithRetry(http, path, timeoutMs, file) };
}

/** Byte ceiling for {@link readSessionFileText} — a hard cap even if a caller asks for more. */
export const READ_SESSION_FILE_TEXT_MAX_BYTES = 10_000_000;
/** Default `maxBytes` for {@link readSessionFileText} — a chat-sized preview. */
export const READ_SESSION_FILE_TEXT_DEFAULT_BYTES = 50_000;
/** Default per-attempt timeout while fetching or reading one session file body. */
export const SESSION_FILE_TRANSFER_DEFAULT_TIMEOUT_MS = 30_000;
/** Idempotent file GETs retry once on a transfer timeout. */
export const SESSION_FILE_TRANSFER_ATTEMPTS = 2;

export interface SessionFileTransferOptions {
  readonly timeoutMs?: number;
  readonly checkpointId?: string;
}

/**
 * Read ONE session file as byte-capped, decoded UTF-8 text. Built for handing a session
 * deliverable to an LLM tool: it streams the file body and STOPS at `maxBytes`, so
 * a 200 MB artifact never fully buffers in memory or context. `truncated` is true
 * when the file is larger than the cap. Optionally `grep` keeps only matching lines.
 *
 * Selector is the same `{ path }` / `{ id }` shape as `downloadSessionFile`.
 * Both forms resolve against the authoritative checkpoint snapshot first.
 */
export async function readSessionFileText(
  http: HttpClient,
  sessionId: string,
  selector: SessionFileSelector,
  options?: ReadSessionFileTextOptions
): Promise<SessionFileText> {
  const timeoutMs = normalizeSessionFileTransferTimeoutMs(options?.timeoutMs);
  const maxBytes = Math.max(1, Math.min(options?.maxBytes ?? READ_SESSION_FILE_TEXT_DEFAULT_BYTES, READ_SESSION_FILE_TEXT_MAX_BYTES));
  const requestedCheckpointId = options?.checkpointId === undefined
    ? undefined
    : requireSessionFileCheckpointId(options.checkpointId, "files.read");
  const file = await resolveAuthoritativeSessionFile(http, sessionId, selector, requestedCheckpointId);
  const checkpointId = requestedCheckpointId ?? file.checkpointId;
  const path = sessionFileRoute(sessionId, file.id, "download", checkpointId);
  const capped = await readSessionFileTextWithRetry(http, path, maxBytes, timeoutMs, file);
  const text = options?.grep === undefined ? capped.text : grepLines(capped.text, options.grep);
  return { file, text, truncated: capped.truncated, totalBytes: capped.totalBytes };
}

async function downloadSessionFileBytesWithRetry(
  http: HttpClient,
  path: string,
  timeoutMs: number,
  file: SessionFile
): Promise<Uint8Array> {
  return sessionFileTransferWithRetry(path, timeoutMs, async () => {
    const response = await downloadSessionFileResponse(http, path, timeoutMs);
    assertSessionFileResponseLength(file, response);
    const bytes = await readResponseBytes(response, timeoutMs);
    assertSessionFileIntegrity(file, bytes);
    return bytes;
  });
}

async function resolveAuthoritativeSessionFile(
  http: HttpClient,
  sessionId: string,
  selector: SessionFileSelector,
  checkpointOverride?: string
): Promise<SessionFile> {
  const selectorCheckpointId = isPathSelector(selector)
    ? undefined
    : requireSessionFileCheckpointId(
        selector && typeof selector === "object" ? selector.checkpointId : undefined,
        "session file id selector"
      );
  const checkpointId = checkpointOverride === undefined
    ? selectorCheckpointId
    : requireSessionFileCheckpointId(checkpointOverride, "session file operation");
  const snapshot = await listSessionFiles(
    http,
    sessionId,
    checkpointId === undefined ? undefined : { checkpointId }
  );
  if (isPathSelector(selector)) return resolveSessionFileSelector(snapshot.files, selector, sessionId);
  if (typeof selector.id !== "string" || selector.id.length === 0) {
    throw new SessionStateError("files.download: selector must include a file id or path", { sessionId });
  }
  const file = snapshot.files.find((candidate) => candidate.id === selector.id);
  if (file !== undefined) return file;
  throw new SessionStateError(`files.download: file id "${selector.id}" was not found in checkpoint`, {
    sessionId,
    fileId: selector.id,
    checkpointId: snapshot.revision.checkpointId
  });
}

class SessionFileIntegrityError extends SessionStateError {}

function assertSessionFileResponseLength(file: SessionFile, response: Response): void {
  const raw = response.headers.get("content-length");
  if (raw === null) return;
  const declaredSizeBytes = /^\d+$/.test(raw) ? Number(raw) : Number.NaN;
  if (Number.isSafeInteger(declaredSizeBytes) && declaredSizeBytes === file.sizeBytes) return;
  throw new SessionFileIntegrityError("files.download: checkpoint file integrity verification failed", {
    fileId: file.id,
    checkpointId: file.checkpointId,
    expectedSizeBytes: file.sizeBytes,
    declaredSizeBytes: raw
  });
}

function assertSessionFileIntegrity(file: SessionFile, bytes: Uint8Array): void {
  const actualSha256 = createHash("sha256").update(bytes).digest("hex");
  if (bytes.byteLength === file.sizeBytes && actualSha256 === file.sha256) return;
  throw new SessionFileIntegrityError("files.download: checkpoint file integrity verification failed", {
    fileId: file.id,
    checkpointId: file.checkpointId,
    expectedSizeBytes: file.sizeBytes,
    actualSizeBytes: bytes.byteLength,
    expectedSha256: file.sha256,
    actualSha256
  });
}

async function readSessionFileTextWithRetry(
  http: HttpClient,
  path: string,
  maxBytes: number,
  timeoutMs: number,
  file: SessionFile
): Promise<{ readonly text: string; readonly truncated: boolean; readonly totalBytes: number }> {
  return sessionFileTransferWithRetry(path, timeoutMs, async () => {
    const response = await downloadSessionFileResponse(http, path, timeoutMs);
    assertSessionFileResponseLength(file, response);
    return readCappedText(response, maxBytes, timeoutMs, file);
  });
}

async function downloadSessionFileResponse(http: HttpClient, path: string, timeoutMs: number): Promise<Response> {
  const controller = new AbortController();
  const { response } = await withSessionFileTransferTimeout(
    http.download(path, { signal: controller.signal }),
    timeoutMs,
    () => controller.abort(),
    "download-open"
  );
  return response;
}

async function sessionFileTransferWithRetry<T>(
  path: string,
  timeoutMs: number,
  action: () => Promise<T>
): Promise<T> {
  const startedMs = Date.now();
  let lastTimeout: SessionFileTransferTimeoutError | undefined;
  for (let attempt = 1; attempt <= SESSION_FILE_TRANSFER_ATTEMPTS; attempt += 1) {
    try {
      return await action();
    } catch (err) {
      if (!(err instanceof SessionFileTransferTimeoutError)) throw err;
      lastTimeout = err;
    }
  }
  throw new AexNetworkError({
    method: "GET",
    host: "",
    path,
    cause: lastTimeout ?? new SessionFileTransferTimeoutError("unknown", timeoutMs),
    attempts: SESSION_FILE_TRANSFER_ATTEMPTS,
    elapsedMs: Date.now() - startedMs
  });
}

function sessionFileRoute(
  sessionId: string,
  fileId: string,
  action: "download" | "link",
  checkpointId: string
): string {
  if (!checkpointId) {
    throw new SessionStateError("session file operations require checkpointId", { sessionId, fileId });
  }
  return `/api/sessions/${encodeURIComponent(sessionId)}/files/${encodeURIComponent(fileId)}/${action}` +
    `?checkpointId=${encodeURIComponent(checkpointId)}`;
}

function requireSessionFileCheckpointId(value: unknown, context: string): string {
  if (typeof value !== "string" || !SESSION_FILE_CHECKPOINT_ID_PATTERN.test(value)) {
    throw new SessionStateError(
      `${context}: checkpointId must match ${SESSION_FILE_CHECKPOINT_ID_PATTERN.source}`,
      {
        checkpointId: value
      }
    );
  }
  return value;
}

function normalizeSessionFileTransferTimeoutMs(value: number | undefined): number {
  if (value === undefined) return SESSION_FILE_TRANSFER_DEFAULT_TIMEOUT_MS;
  if (!Number.isFinite(value) || value <= 0) {
    throw configError("timeoutMs", "files.download: timeoutMs must be a positive finite number");
  }
  return Math.max(1, Math.floor(value));
}

type SessionFileTransferPhase = "download-open" | "body-read" | "unknown";

class SessionFileTransferTimeoutError extends Error {
  readonly code = "ETIMEDOUT";
  readonly phase: SessionFileTransferPhase;

  constructor(phase: SessionFileTransferPhase, timeoutMs: number) {
    super(`file transfer phase=${phase} timed out after ${timeoutMs}ms`);
    this.name = "SessionFileTransferTimeoutError";
    this.phase = phase;
  }
}

async function withSessionFileTransferTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  abort: () => void,
  phase: SessionFileTransferPhase
): Promise<T> {
  let timedOut = false;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  const timeoutPromise = new Promise<never>((_, reject) => {
    timeout = setTimeout(() => {
      timedOut = true;
      reject(new SessionFileTransferTimeoutError(phase, timeoutMs));
      queueMicrotask(() => {
        try {
          abort();
        } catch {
          // Best effort: the timeout itself is the user-facing failure.
        }
      });
    }, timeoutMs);
  });
  try {
    return await Promise.race([promise, timeoutPromise]);
  } catch (err) {
    if (timedOut && isAbortLikeError(err)) throw new SessionFileTransferTimeoutError(phase, timeoutMs);
    throw err;
  } finally {
    if (timeout !== undefined) clearTimeout(timeout);
  }
}

function isAbortLikeError(err: unknown): boolean {
  const name = (err as { readonly name?: unknown } | null | undefined)?.name;
  return name === "AbortError";
}

async function readResponseBytes(response: Response, timeoutMs: number): Promise<Uint8Array> {
  const body = response.body;
  if (!body) {
    const buffer = await withSessionFileTransferTimeout(
      response.arrayBuffer(),
      timeoutMs,
      () => {},
      "body-read"
    );
    return new Uint8Array(buffer);
  }

  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  try {
    while (true) {
      const { done, value } = await withSessionFileTransferTimeout(
        reader.read(),
        timeoutMs,
        () => {
          void reader.cancel().catch(() => {});
        },
        "body-read"
      );
      if (done) break;
      if (value && value.byteLength > 0) chunks.push(value);
    }
  } finally {
    void reader.cancel().catch(() => {});
  }
  return concatBytes(chunks);
}

/**
 * Read a streamed response body up to `maxBytes` and decode as UTF-8. The
 * checkpoint supplies the authoritative total size. A partial read cancels as
 * soon as the retained prefix reaches the cap; a complete read reaches EOF and
 * verifies both committed size and digest.
 */
async function readCappedText(
  response: Response,
  maxBytes: number,
  timeoutMs: number,
  file: SessionFile
): Promise<{ readonly text: string; readonly truncated: boolean; readonly totalBytes: number }> {
  const decoder = new TextDecoder("utf-8");
  const wholeFile = file.sizeBytes <= maxBytes;
  const targetBytes = wholeFile ? file.sizeBytes : maxBytes;
  const body = response.body;
  if (!body) {
    // No streaming body (some fetch polyfills) — buffer, then slice to the cap.
    const buf = new Uint8Array(
      await withSessionFileTransferTimeout(response.arrayBuffer(), timeoutMs, () => {}, "body-read")
    );
    if (wholeFile) {
      assertSessionFileIntegrity(file, buf);
    } else if (buf.byteLength < targetBytes) {
      throw new SessionFileIntegrityError("files.read: checkpoint file ended before the requested prefix", {
        fileId: file.id,
        checkpointId: file.checkpointId,
        expectedPrefixBytes: targetBytes,
        actualSizeBytes: buf.byteLength
      });
    }
    return {
      text: decoder.decode(buf.subarray(0, targetBytes)),
      truncated: !wholeFile,
      totalBytes: file.sizeBytes
    };
  }
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let retainedBytes = 0;
  try {
    while (wholeFile || retainedBytes < targetBytes) {
      const { done, value } = await withSessionFileTransferTimeout(
        reader.read(),
        timeoutMs,
        () => {
          void reader.cancel().catch(() => {});
        },
        "body-read"
      );
      if (done) break;
      if (value && value.byteLength > 0) {
        const remainingBytes = targetBytes - retainedBytes;
        const retainedFromChunk = Math.min(remainingBytes, value.byteLength);
        if (retainedFromChunk > 0) {
          chunks.push(
            retainedFromChunk === value.byteLength ? value : value.slice(0, retainedFromChunk)
          );
          retainedBytes += retainedFromChunk;
        }
        if (wholeFile && value.byteLength > remainingBytes) {
          throw new SessionFileIntegrityError("files.read: checkpoint file exceeded its committed byte length", {
            fileId: file.id,
            checkpointId: file.checkpointId,
            expectedSizeBytes: file.sizeBytes,
            actualSizeBytesAtLeast: retainedBytes + value.byteLength - retainedFromChunk
          });
        }
        if (!wholeFile && retainedBytes >= targetBytes) break;
      }
    }
  } finally {
    void reader.cancel().catch(() => {});
  }
  const merged = concatBytes(chunks);
  if (wholeFile) {
    assertSessionFileIntegrity(file, merged);
  } else if (retainedBytes < targetBytes) {
    throw new SessionFileIntegrityError("files.read: checkpoint file ended before the requested prefix", {
      fileId: file.id,
      checkpointId: file.checkpointId,
      expectedPrefixBytes: targetBytes,
      actualSizeBytes: retainedBytes
    });
  }
  return { text: decoder.decode(merged), truncated: !wholeFile, totalBytes: file.sizeBytes };
}

function concatBytes(chunks: readonly Uint8Array[]): Uint8Array {
  if (chunks.length === 1) return chunks[0]!;
  const total = chunks.reduce((n, c) => n + c.byteLength, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.byteLength;
  }
  return out;
}

function grepLines(text: string, pattern: string | RegExp): string {
  const test =
    typeof pattern === "string"
      ? (line: string) => line.toLowerCase().includes(pattern.toLowerCase())
      : (line: string) => pattern.test(line);
  return text
    .split("\n")
    .filter((line) => test(line))
    .join("\n");
}

/**
 * List a session's subagent child sessions (`GET /api/sessions/:id/children`). Each
 * {@link ChildSessionRef} is a read-only lineage snapshot whose id addresses
 * events, checkpointed files, and descendants. An empty array means the session
 * spawned no children.
 */
export async function listSessionChildren(
  http: HttpClient,
  sessionId: string
): Promise<readonly ChildSessionRef[]> {
  const result = await http.request<unknown>(`/api/sessions/${encodeURIComponent(sessionId)}/children`);
  if (!isRecord(result) || !Array.isArray(result.children)) {
    throw new SessionStateError("session children response must contain a children array");
  }
  return result.children.map(parseChildSessionRef);
}

function parseChildSessionRef(value: unknown, index: number): ChildSessionRef {
  if (!isRecord(value)) {
    throw new SessionStateError(`session children response has an invalid row at index ${index}`);
  }
  for (const field of ["id", "parentSessionId", "status", "createdAt", "updatedAt"] as const) {
    if (typeof value[field] !== "string" || value[field].length === 0) {
      throw new SessionStateError(`session children row ${index} has an invalid ${field}`);
    }
  }
  if (!SESSION_STATUS_SET.has(value.status as string)) {
    throw new SessionStateError(`session children row ${index} has an unknown lifecycle status`);
  }
  assertCanonicalSessionWireFields(value, `session children row ${index}`);
  if (value.depth !== undefined && (!Number.isSafeInteger(value.depth) || (value.depth as number) < 1)) {
    throw new SessionStateError(`session children row ${index} has an invalid depth`);
  }
  if (
    value.costUsd !== undefined &&
    (typeof value.costUsd !== "number" || !Number.isFinite(value.costUsd) || value.costUsd < 0)
  ) {
    throw new SessionStateError(`session children row ${index} has an invalid costUsd`);
  }
  if (value.terminalAt !== undefined && value.terminalAt !== null && typeof value.terminalAt !== "string") {
    throw new SessionStateError(`session children row ${index} has an invalid terminalAt`);
  }
  if (value.lastRun !== undefined && !isRecord(value.lastRun)) {
    throw new SessionStateError(`session children row ${index} has an invalid lastRun`);
  }
  return value as unknown as ChildSessionRef;
}

/**
 * List a session's run-terminal webhook deliveries. Each finalized run has its
 * own row; an empty array means the session has no webhook or no run has
 * finalized yet.
 */
export async function getSessionWebhookDeliveries(
  http: HttpClient,
  sessionId: string
): Promise<readonly SessionWebhookDelivery[]> {
  const result = await http.request<unknown>(
    `/api/sessions/${encodeURIComponent(sessionId)}/webhook-deliveries`
  );
  if (!isRecord(result) || !Array.isArray(result.deliveries)) {
    throw new SessionStateError("session webhook deliveries response must contain a deliveries array");
  }
  return result.deliveries as readonly SessionWebhookDelivery[];
}

/**
 * Manually re-trigger one run's webhook delivery: resets the row to `pending`
 * and re-sends the frozen payload with the same run-scoped `webhook-id`.
 */
export async function redeliverSessionWebhook(
  http: HttpClient,
  sessionId: string,
  deliveryId: string
): Promise<void> {
  await http.request<unknown>(
    `/api/sessions/${encodeURIComponent(sessionId)}/webhook-deliveries/${encodeURIComponent(deliveryId)}/redeliver`,
    { method: "POST" }
  );
}

/**
 * Delete a workspace asset cache entry. Accepts an `asset_<id>` value,
 * `sha256:<hex>`, or a bare 64-hex digest. Workspace is derived server-side
 * from the token; idempotent.
 * Does NOT affect sessions that already snapshotted the asset.
 */
export async function deleteWorkspaceAsset(http: HttpClient, hash: string): Promise<void> {
  const assetId = hash.startsWith("asset_")
    ? hash
    : `asset_${hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash}`;
  await http.request<unknown>(`/api/assets/${encodeURIComponent(assetId)}`, { method: "DELETE" });
}

export async function whoami(http: HttpClient): Promise<WhoAmI> {
  return parseWhoAmI(await http.request<unknown>("/api/whoami"));
}

/**
 * Validate a CONTROL-plane account token (PAT / device session) against the
 * dashboard BFF `GET /api/whoami`. Same endpoint as {@link whoami}, but the
 * bearer is an account PAT (`aexu_...`) so the server answers with an
 * `account_token` principal (no workspace, no data-plane limits) — a shape the
 * workspace-key {@link whoami} parser rejects. Used by
 * `aex login --api-key <account PAT>` to prove the PAT before persisting it.
 */
export async function accountWhoami(http: HttpClient): Promise<AccountWhoAmI> {
  return parseAccountWhoAmI(await http.request<unknown>("/api/whoami"));
}

function parseAccountWhoAmI(value: unknown): AccountWhoAmI {
  if (!isRecord(value)) {
    throw new SessionStateError("account whoami response must be an object");
  }
  if (value.ok !== true || value.principalType !== "account_token") {
    throw new SessionStateError("account whoami response must identify an account_token principal");
  }
  if (typeof value.appUserId !== "string" || value.appUserId.length === 0) {
    throw new SessionStateError("account whoami response appUserId must be a non-empty string");
  }
  if (!Array.isArray(value.scopes) || !value.scopes.every((scope) => typeof scope === "string")) {
    throw new SessionStateError("account whoami response scopes must be an array of strings");
  }
  return {
    ok: true,
    principalType: "account_token",
    appUserId: value.appUserId,
    scopes: value.scopes as string[],
    ...(typeof value.orgId === "string" ? { orgId: value.orgId } : {}),
    ...(typeof value.tokenId === "string" ? { tokenId: value.tokenId } : {}),
    ...(typeof value.tokenName === "string" ? { tokenName: value.tokenName } : {}),
    ...(typeof value.tokenKind === "string" ? { tokenKind: value.tokenKind } : {})
  };
}

function parseWhoAmI(value: unknown): WhoAmI {
  if (!isRecord(value)) {
    throw new SessionStateError("whoami response must be an object");
  }
  const removed = ["caps", "tokenId", "tokenName"].find((field) => Object.hasOwn(value, field));
  if (removed !== undefined) {
    throw new SessionStateError(`whoami response contains the removed ${removed} field`);
  }
  if (value.ok !== true || value.principalType !== "api_key") {
    throw new SessionStateError("whoami response must identify an api_key principal");
  }
  if (typeof value.workspaceId !== "string" || value.workspaceId.length === 0) {
    throw new SessionStateError("whoami response workspaceId must be a non-empty string");
  }
  if (!Array.isArray(value.scopes) || !value.scopes.every((scope) => typeof scope === "string" && scope.length > 0)) {
    throw new SessionStateError("whoami response scopes must be an array of non-empty strings");
  }
  const limits = parseWhoAmILimits(value.limits);
  const runtimeCapabilities = value.runtimeCapabilities === undefined
    ? undefined
    : parseRuntimeCapabilities(value.runtimeCapabilities);
  return {
    ok: true,
    principalType: "api_key",
    workspaceId: value.workspaceId,
    scopes: value.scopes as string[],
    limits,
    ...(runtimeCapabilities ? { runtimeCapabilities } : {})
  };
}

const RUNTIME_KIND_SET = new Set<string>(RUNTIME_KINDS);
const RUNTIME_SIZE_SET = new Set<string>(RUNTIME_SIZES);

function parseRuntimeCapabilities(value: unknown): WhoAmI["runtimeCapabilities"] {
  const field = "whoami response runtimeCapabilities";
  if (!isRecord(value)) throw new SessionStateError(`${field} must be an object`);
  if (value.schemaVersion !== 1) throw new SessionStateError(`${field}.schemaVersion must be 1`);
  if (typeof value.capabilityVersion !== "string" || value.capabilityVersion.length === 0) {
    throw new SessionStateError(`${field}.capabilityVersion must be a non-empty string`);
  }
  if (typeof value.capabilityHash !== "string" || !CANONICAL_SHA256_DIGEST_PATTERN.test(value.capabilityHash)) {
    throw new SessionStateError(`${field}.capabilityHash must be a canonical SHA-256 digest`);
  }
  if (!Array.isArray(value.availableRuntimeKinds)) {
    throw new SessionStateError(`${field}.availableRuntimeKinds must be an array`);
  }
  const availableRuntimeKinds = value.availableRuntimeKinds as unknown[];
  if (
    !availableRuntimeKinds.every((kind): kind is RuntimeKind => typeof kind === "string" && RUNTIME_KIND_SET.has(kind)) ||
    new Set(availableRuntimeKinds).size !== availableRuntimeKinds.length
  ) {
    throw new SessionStateError(`${field}.availableRuntimeKinds must contain unique supported runtime kinds`);
  }
  if (!isRecord(value.sizesByRuntimeKind)) {
    throw new SessionStateError(`${field}.sizesByRuntimeKind must be an object`);
  }
  if (!isRecord(value.unavailable)) {
    throw new SessionStateError(`${field}.unavailable must be an object`);
  }
  const unknownSizeKind = Object.keys(value.sizesByRuntimeKind).find((kind) => !RUNTIME_KIND_SET.has(kind));
  const unknownUnavailableKind = Object.keys(value.unavailable).find((kind) => !RUNTIME_KIND_SET.has(kind));
  if (unknownSizeKind || unknownUnavailableKind) {
    throw new SessionStateError(`${field} contains an unknown runtime kind`);
  }

  const available = new Set<RuntimeKind>(availableRuntimeKinds);
  const sizesByRuntimeKind: Partial<Record<RuntimeKind, readonly RuntimeSize[]>> = {};
  const unavailable: Partial<Record<RuntimeKind, { readonly code: string }>> = {};
  for (const runtimeKind of RUNTIME_KINDS) {
    const sizes = value.sizesByRuntimeKind[runtimeKind];
    const reason = value.unavailable[runtimeKind];
    if (available.has(runtimeKind)) {
      const parsedSizes = Array.isArray(sizes)
        ? sizes.map((size) => {
          try {
            return parseRuntimeSizeForRead(size);
          } catch {
            return undefined;
          }
        })
        : undefined;
      if (
        !Array.isArray(parsedSizes) || parsedSizes.length === 0 ||
        !parsedSizes.every((size): size is RuntimeSize => typeof size === "string" && RUNTIME_SIZE_SET.has(size)) ||
        new Set(parsedSizes).size !== parsedSizes.length || reason !== undefined
      ) {
        throw new SessionStateError(`${field}.${runtimeKind} must have unique supported sizes and no unavailable reason`);
      }
      sizesByRuntimeKind[runtimeKind] = parsedSizes;
    } else {
      if (sizes !== undefined || !isRecord(reason) || typeof reason.code !== "string" || reason.code.length === 0) {
        throw new SessionStateError(`${field}.${runtimeKind} must have one unavailable reason and no sizes`);
      }
      unavailable[runtimeKind] = { code: reason.code };
    }
  }

  return {
    schemaVersion: 1,
    capabilityVersion: value.capabilityVersion,
    capabilityHash: value.capabilityHash as `sha256:${string}`,
    availableRuntimeKinds,
    sizesByRuntimeKind,
    unavailable,
    profilesByRuntimeKind: parseRuntimeProfiles(value.profilesByRuntimeKind)
  };
}

const RUNTIME_CAPABILITY_STATES = new Set<string>(["supported", "unsupported"]);
const TOOL_EXECUTION_DELIVERY = new Set<string>(["at-least-once", "exactly-once"]);
const COLD_START_CLASSES = new Set<string>(["warm", "cold-seconds", "cold-tens-of-seconds"]);
const IDLE_BILLING_CLASSES = new Set<string>(["wall-clock", "zero"]);
const COMPUTE_BASES = new Set<string>(["wall_clock", "microvm_running"]);

/**
 * The profile set is TOTAL over runtime kinds and is not optional: a response that
 * declares which runtimes exist but not what they do is exactly the gap the public
 * parity claim used to paper over. A missing or partial set is a contract violation.
 */
function parseRuntimeProfiles(value: unknown): Record<RuntimeKind, RuntimeProfile> {
  const field = "whoami response runtimeCapabilities.profilesByRuntimeKind";
  if (!isRecord(value)) throw new SessionStateError(`${field} must be an object`);
  const unknownKind = Object.keys(value).find((kind) => !RUNTIME_KIND_SET.has(kind));
  if (unknownKind !== undefined) throw new SessionStateError(`${field} contains an unknown runtime kind`);
  const profiles = {} as Record<RuntimeKind, RuntimeProfile>;
  for (const runtimeKind of RUNTIME_KINDS) {
    profiles[runtimeKind] = parseRuntimeProfile(value[runtimeKind], `${field}.${runtimeKind}`, runtimeKind);
  }
  return profiles;
}

function parseRuntimeProfile(value: unknown, field: string, runtimeKind: RuntimeKind): RuntimeProfile {
  if (!isRecord(value)) throw new SessionStateError(`${field} must be an object`);
  if (value.schemaVersion !== 1) throw new SessionStateError(`${field}.schemaVersion must be 1`);
  if (value.runtimeKind !== runtimeKind) throw new SessionStateError(`${field}.runtimeKind must be ${runtimeKind}`);
  if (!isRecord(value.capabilities)) throw new SessionStateError(`${field}.capabilities must be an object`);
  const capabilities = {} as Record<RuntimeCapabilityName, RuntimeCapabilityState>;
  for (const capability of RUNTIME_CAPABILITY_NAMES) {
    const state = value.capabilities[capability];
    if (typeof state !== "string" || !RUNTIME_CAPABILITY_STATES.has(state)) {
      throw new SessionStateError(`${field}.capabilities.${capability} must be supported or unsupported`);
    }
    capabilities[capability] = state as RuntimeCapabilityState;
  }
  if (!isRecord(value.limits)) throw new SessionStateError(`${field}.limits must be an object`);
  const limits = value.limits;
  for (const limit of ["maxSessionMs", "maxSingleEffectMs", "maxWorkspaceBytes", "maxConcurrentToolCalls"] as const) {
    const observed = limits[limit];
    if (typeof observed !== "number" || !Number.isFinite(observed) || observed <= 0) {
      throw new SessionStateError(`${field}.limits.${limit} must be a positive number`);
    }
  }
  if (!isRecord(value.delivery)) throw new SessionStateError(`${field}.delivery must be an object`);
  const delivery = value.delivery;
  for (const [key, allowed] of [
    ["toolExecution", TOOL_EXECUTION_DELIVERY],
    ["coldStartClass", COLD_START_CLASSES],
    ["idleBilling", IDLE_BILLING_CLASSES]
  ] as const) {
    const observed = delivery[key];
    if (typeof observed !== "string" || !allowed.has(observed)) {
      throw new SessionStateError(`${field}.delivery.${key} is not a recognized value`);
    }
  }
  if (typeof value.computeBasis !== "string" || !COMPUTE_BASES.has(value.computeBasis)) {
    throw new SessionStateError(`${field}.computeBasis is not a recognized value`);
  }
  return {
    schemaVersion: 1,
    runtimeKind,
    capabilities,
    limits: {
      maxSessionMs: limits.maxSessionMs as number,
      maxSingleEffectMs: limits.maxSingleEffectMs as number,
      maxWorkspaceBytes: limits.maxWorkspaceBytes as number,
      maxConcurrentToolCalls: limits.maxConcurrentToolCalls as number
    },
    delivery: {
      toolExecution: delivery.toolExecution as RuntimeProfile["delivery"]["toolExecution"],
      coldStartClass: delivery.coldStartClass as RuntimeProfile["delivery"]["coldStartClass"],
      idleBilling: delivery.idleBilling as RuntimeProfile["delivery"]["idleBilling"]
    },
    computeBasis: value.computeBasis as RuntimeProfile["computeBasis"]
  };
}

function parseWhoAmILimits(value: unknown): WhoAmI["limits"] {
  if (!isRecord(value)) {
    throw new SessionStateError("whoami response limits must be an object");
  }
  const requiredNumbers = [
    "maxConcurrentSessions",
    "submitRatePerMinute",
    "spendCapUsd",
    "monthSpendUsd",
    "balanceUsd",
    "balanceGraceFloorUsd",
    "llmTokenAllowanceRemainingUsd"
  ] as const;
  for (const field of requiredNumbers) {
    if (typeof value[field] !== "number" || !Number.isFinite(value[field])) {
      throw new SessionStateError(`whoami response limits.${field} must be a finite number`);
    }
  }
  for (const field of ["creditGateActive", "autoTopupEnabled"] as const) {
    if (typeof value[field] !== "boolean") {
      throw new SessionStateError(`whoami response limits.${field} must be a boolean`);
    }
  }
  const paymentMethodStatus = value.paymentMethodStatus;
  const admissionState = value.admissionState;
  const accountType = value.accountType;
  assertOneOf(paymentMethodStatus, ["none", "active"], "limits.paymentMethodStatus");
  assertOneOf(admissionState, BILLING_ADMISSION_STATES, "limits.admissionState");
  assertOneOf(accountType, ["standard", "internal"], "limits.accountType");
  return {
    maxConcurrentSessions: value.maxConcurrentSessions as number,
    submitRatePerMinute: value.submitRatePerMinute as number,
    spendCapUsd: value.spendCapUsd as number,
    monthSpendUsd: value.monthSpendUsd as number,
    balanceUsd: value.balanceUsd as number,
    balanceGraceFloorUsd: value.balanceGraceFloorUsd as number,
    llmTokenAllowanceRemainingUsd: value.llmTokenAllowanceRemainingUsd as number,
    creditGateActive: value.creditGateActive as boolean,
    paymentMethodStatus,
    admissionState,
    autoTopupEnabled: value.autoTopupEnabled as boolean,
    accountType
  };
}

function assertOneOf<const TAllowed extends readonly string[]>(
  value: unknown,
  allowed: TAllowed,
  field: string
): asserts value is TAllowed[number] {
  if (!isStringLiteral(value, allowed)) {
    throw new SessionStateError(`whoami response ${field} is invalid`);
  }
}

/**
 * A session's downloadable content is organised into three public namespaces, each
 * with a matching `download*` verb:
 *
 *   - `files`    — the session's captured files (`sessions/<id>/files/`).
 *   - `events`   — typed event-channel records (`events.jsonl`) plus its namespace manifest.
 *   - `metadata` — the session record (`session.json`) plus its namespace manifest.
 *
 * `download` bundles all three as top-level folders; `downloadSessionFiles` /
 * `downloadEvents` / `downloadMetadata` each bundle one.
 * Every zip is assembled client-side from the public read endpoints —
 * there is no server-side archive route. Callers write the bytes to disk.
 */
type ArtifactNamespace = "files";

interface CollectedArtifacts {
  readonly entries: readonly SessionArchiveEntry[];
  readonly captured: SessionRecordArtifactSummaryV1[];
  readonly errors: SessionRecordDownloadErrorV1[];
}

/**
 * Download each artifact's bytes into a zip-file map keyed by
 * `<zipPrefix><relative-path>`, fetched from the `files`
 * download route. Best-effort: a per-artifact fetch failure records an
 * `errors[]` entry rather than aborting the rest, so the failure is
 * surfaced (never silent) while a partially-available run still yields a
 * usable zip. A committed size or digest contradiction is terminal because
 * returning a partial archive would misrepresent corrupted bytes as absent.
 */
async function collectArtifactBytes(
  http: HttpClient,
  sessionId: string,
  items: readonly SessionFile[],
  zipPrefix: string,
  namespace: ArtifactNamespace,
  timeoutMs = SESSION_FILE_TRANSFER_DEFAULT_TIMEOUT_MS
): Promise<CollectedArtifacts> {
  const entries: SessionArchiveEntry[] = [];
  const captured: SessionRecordArtifactSummaryV1[] = [];
  const errors: SessionRecordDownloadErrorV1[] = [];

  for (const item of items) {
    const rel = item.filename ?? item.id;
    try {
      const path = sessionFileRoute(sessionId, item.id, "download", item.checkpointId);
      entries.push({
        path: `${zipPrefix}${rel}`,
        bytes: await downloadSessionFileBytesWithRetry(http, path, timeoutMs, item),
        ...(item.contentType !== undefined ? { contentType: item.contentType } : {}),
        customerContent: true
      });
      captured.push({
        id: item.id,
        filename: item.filename ?? null,
        ...(item.sizeBytes !== undefined ? { sizeBytes: item.sizeBytes } : {}),
        ...(item.contentType !== undefined ? { contentType: item.contentType } : {})
      });
    } catch (err) {
      if (err instanceof SessionFileIntegrityError) throw err;
      errors.push({ namespace, id: item.id, filename: item.filename ?? null, message: (err as Error).message });
    }
  }

  return { entries: Object.freeze(entries), captured, errors };
}

export function normalizeSessionFileLinkExpiresIn(input: SessionFileLinkOptions["expiresIn"] = "1h"): number {
  if (typeof input === "number") {
    if (!Number.isFinite(input) || input <= 0) {
      throw new SessionStateError("sessionFileLink: expiresIn must be a positive number of seconds", {
        expiresIn: input
      });
    }
    return Math.floor(input);
  }
  if (input === "15m") return 15 * 60;
  if (input === "1h") return 60 * 60;
  if (input === "1d") return 24 * 60 * 60;
  throw new SessionStateError("sessionFileLink: expiresIn must be seconds, \"15m\", \"1h\", or \"1d\"", {
    expiresIn: input
  });
}

async function resolveSessionFileLinkTarget(
  http: HttpClient,
  sessionId: string,
  selectorOrQuery: SessionFileLinkSelector,
  checkpointId?: string
): Promise<SessionFile> {
  if (hasSessionFileIdProperty(selectorOrQuery)) {
    if (typeof selectorOrQuery.id !== "string" || selectorOrQuery.id.length === 0) {
      throw new SessionStateError("sessionFileLink: selector must include a file id or query", { sessionId });
    }
    return resolveAuthoritativeSessionFile(
      http,
      sessionId,
      selectorOrQuery as SessionFileSelector,
      checkpointId
    );
  }
  if (isPathSelector(selectorOrQuery as SessionFileSelector) && (selectorOrQuery as SessionFilePathSelector).match === "suffix") {
    const snapshot = await listSessionFiles(http, sessionId, checkpointId ? { checkpointId } : undefined);
    return resolveSessionFileSelector(snapshot.files, selectorOrQuery as SessionFilePathSelector, sessionId);
  }
  const match = await findSessionFile(http, sessionId, {
    ...(selectorOrQuery as SessionFilesQuery),
    ...(checkpointId ? { checkpointId } : {})
  });
  if (match) return match;
  throw new SessionStateError("sessionFileLink: file query matched no files", { sessionId });
}

function hasSessionFileIdProperty(
  value: SessionFileSelector | SessionFilesQuery
): value is (SessionFileSelector | SessionFilesQuery) & { readonly id: unknown } {
  return Boolean(
    value &&
    typeof value === "object" &&
    "id" in value
  );
}

/**
 * Download EVERYTHING public about a session as one zip, organised into the three
 * namespace folders:
 *
 *   metadata/session.json     — the session record.
 *   events/events.jsonl   — typed event-channel records.
 *   files/<rel>       — the session's captured files.
 *   manifest.json         — `SessionRecordManifestV1`.
 */
export async function download(http: HttpClient, sessionId: string): Promise<Uint8Array> {
  const [session, events, snapshot] = await Promise.all([
    getSession(http, sessionId),
    listSessionEvents(http, sessionId),
    listSessionFiles(http, sessionId)
  ]);

  const collectedFiles = await collectArtifactBytes(http, sessionId, snapshot.files, "files/", "files");
  return buildSessionArchive(sessionId, session, events, collectedFiles);
}

/**
 * Download only the session's captured files (the `files` namespace). Zip
 * layout: `<rel>` per file plus a `manifest.json`
 * (`{ sessionId, namespace: "files", files[], errors[] }`).
 */
export async function downloadSessionFiles(
  http: HttpClient,
  sessionId: string,
  options?: SessionFileTransferOptions
): Promise<Uint8Array> {
  const timeoutMs = normalizeSessionFileTransferTimeoutMs(options?.timeoutMs);
  const requestedCheckpointId = options?.checkpointId === undefined
    ? undefined
    : requireSessionFileCheckpointId(options.checkpointId, "files.download");
  const snapshot = await listSessionFiles(
    http,
    sessionId,
    requestedCheckpointId === undefined ? undefined : { checkpointId: requestedCheckpointId }
  );
  const collectedFiles = await collectArtifactBytes(
    http,
    sessionId,
    snapshot.files,
    "",
    "files",
    timeoutMs
  );
  return buildSessionFilesArchive(sessionId, collectedFiles);
}

/**
 * Download only the event archive (the `events` namespace). Always includes
 * typed `events.jsonl` plus `manifest.json`.
 */
export async function downloadEvents(http: HttpClient, sessionId: string): Promise<Uint8Array> {
  const events = await listSessionEvents(http, sessionId);
  return buildSessionEventsArchive(sessionId, events);
}

/**
 * Download only the session record (the `metadata` namespace) as a zip
 * containing `session.json` plus `manifest.json`.
 */
export async function downloadMetadata(http: HttpClient, sessionId: string): Promise<Uint8Array> {
  const session = await getSession(http, sessionId);
  return buildSessionMetadataArchive(sessionId, session);
}

// ===========================================================================
// Immutable, versioned workspace resources
// ===========================================================================

interface WorkspacePublishBase {
  readonly assetId: string;
  readonly contentHash: string;
  readonly sizeBytes: number;
  readonly contentType: string;
}

export type PublishWorkspaceFileInput = WorkspacePublishBase & {
  readonly name: string;
  readonly mountPath: string;
};

export type PublishWorkspaceSkillInput = WorkspacePublishBase & {
  readonly name: string;
  readonly description: string;
};

export type PublishWorkspaceToolInput = WorkspacePublishBase & {
  readonly name: string;
  readonly description: string;
  readonly input_schema: ToolInputSchema;
  readonly entry: string;
};

/** TEXT, not a {@link WorkspacePublishBase}: nothing is staged through
 *  `/api/assets/presign`, so `assets:write` no longer publishes an instruction. */
export type PublishWorkspaceInstructionInput = { readonly name: string; readonly text: string };

export async function publishWorkspaceFile(
  http: HttpClient,
  input: PublishWorkspaceFileInput
): Promise<WorkspaceFileRecord> {
  return publishWorkspaceResource(http, "files", input);
}

export async function publishWorkspaceSkill(
  http: HttpClient,
  input: PublishWorkspaceSkillInput
): Promise<WorkspaceSkillRecord> {
  return publishWorkspaceResource(http, "skills", input);
}

export async function publishWorkspaceTool(
  http: HttpClient,
  input: PublishWorkspaceToolInput
): Promise<WorkspaceToolRecord> {
  return publishWorkspaceResource(http, "tools", input);
}

export async function publishWorkspaceInstruction(
  http: HttpClient,
  input: PublishWorkspaceInstructionInput
): Promise<WorkspaceInstructionRecord> {
  return publishWorkspaceResource(http, "instructions", input);
}

export function listWorkspaceFiles(
  http: HttpClient,
  query: WorkspaceResourceListQuery = {}
): Promise<WorkspaceResourcePage<WorkspaceFileRecord>> {
  return listWorkspaceResources(http, "files", query);
}

export function listWorkspaceSkills(
  http: HttpClient,
  query: WorkspaceResourceListQuery = {}
): Promise<WorkspaceResourcePage<WorkspaceSkillRecord>> {
  return listWorkspaceResources(http, "skills", query);
}

export function listWorkspaceTools(
  http: HttpClient,
  query: WorkspaceResourceListQuery = {}
): Promise<WorkspaceResourcePage<WorkspaceToolRecord>> {
  return listWorkspaceResources(http, "tools", query);
}

export function listWorkspaceInstructions(
  http: HttpClient,
  query: WorkspaceResourceListQuery = {}
): Promise<WorkspaceResourcePage<WorkspaceInstructionRecord>> {
  return listWorkspaceResources(http, "instructions", query);
}

export function getWorkspaceFile(http: HttpClient, resourceId: string, version?: number): Promise<WorkspaceFileRecord> {
  return getWorkspaceResource(http, "files", resourceId, version);
}

export function getWorkspaceSkill(http: HttpClient, resourceId: string, version?: number): Promise<WorkspaceSkillRecord> {
  return getWorkspaceResource(http, "skills", resourceId, version);
}

export function getWorkspaceTool(http: HttpClient, resourceId: string, version?: number): Promise<WorkspaceToolRecord> {
  return getWorkspaceResource(http, "tools", resourceId, version);
}

export function getWorkspaceInstruction(
  http: HttpClient,
  resourceId: string,
  version?: number
): Promise<WorkspaceInstructionRecord> {
  return getWorkspaceResource(http, "instructions", resourceId, version);
}

export function deleteWorkspaceFile(http: HttpClient, resourceId: string): Promise<void> {
  return deleteWorkspaceResource(http, "files", resourceId);
}

export function deleteWorkspaceSkill(http: HttpClient, resourceId: string): Promise<void> {
  return deleteWorkspaceResource(http, "skills", resourceId);
}

export function deleteWorkspaceTool(http: HttpClient, resourceId: string): Promise<void> {
  return deleteWorkspaceResource(http, "tools", resourceId);
}

export function deleteWorkspaceInstruction(http: HttpClient, resourceId: string): Promise<void> {
  return deleteWorkspaceResource(http, "instructions", resourceId);
}

type WorkspaceResourceKind = "files" | "skills" | "tools" | "instructions";

async function publishWorkspaceResource<T>(
  http: HttpClient,
  kind: WorkspaceResourceKind,
  input: object
): Promise<T> {
  const result = await http.request<{ readonly resource: T }>(`/api/workspace/${kind}`, {
    method: "POST",
    body: JSON.stringify(input)
  });
  return result.resource;
}

async function listWorkspaceResources<T>(
  http: HttpClient,
  kind: WorkspaceResourceKind,
  query: WorkspaceResourceListQuery
): Promise<WorkspaceResourcePage<T>> {
  if (
    query.limit !== undefined &&
    (!Number.isSafeInteger(query.limit) || query.limit < 1 || query.limit > 100)
  ) {
    throw new Error("workspace resource list limit must be an integer from 1 through 100");
  }
  return http.request<WorkspaceResourcePage<T>>(
    `/api/workspace/${kind}`,
    {},
    {
      ...(query.cursor !== undefined ? { cursor: query.cursor } : {}),
      ...(query.limit !== undefined ? { limit: String(query.limit) } : {})
    }
  );
}

async function getWorkspaceResource<T>(
  http: HttpClient,
  kind: WorkspaceResourceKind,
  resourceId: string,
  version?: number
): Promise<T> {
  const result = await http.request<{ readonly resource: T }>(
    `/api/workspace/${kind}/${encodeURIComponent(resourceId)}`,
    {},
    version === undefined ? {} : { version: String(version) }
  );
  return result.resource;
}

async function deleteWorkspaceResource(
  http: HttpClient,
  kind: WorkspaceResourceKind,
  resourceId: string
): Promise<void> {
  await http.request<void>(`/api/workspace/${kind}/${encodeURIComponent(resourceId)}`, { method: "DELETE" });
}

function unwrapSession(result: { readonly session: Session }): Session {
  if (!isRecord(result) || !isRecord(result.session)) {
    throw new SessionStateError("session response must contain a session object");
  }
  const value = result.session;
  if (typeof value.id !== "string" || value.id.length === 0) {
    throw new SessionStateError("session response is missing its canonical id");
  }
  assertCanonicalSessionWireFields(value, "session response");
  if (typeof value.status !== "string" || !SESSION_STATUS_SET.has(value.status)) {
    throw new SessionStateError("session response has an unknown lifecycle status");
  }
  if (typeof value.acceptsMessages !== "boolean") {
    throw new SessionStateError("session response is missing acceptsMessages");
  }
  const currentRun = normalizeOptionalSessionRun(value.currentRun, value.id, "session response currentRun");
  const lastRun = normalizeOptionalSessionRun(value.lastRun, value.id, "session response lastRun");
  let providerFault;
  if (Object.hasOwn(value, "providerFault")) {
    try {
      providerFault = parseProviderFault(value.providerFault);
    } catch (error) {
      throw new SessionStateError(
        `session response has an invalid providerFault: ${error instanceof Error ? error.message : "invalid value"}`
      );
    }
    if (lastRun?.outcome !== "failed") {
      throw new SessionStateError("session response providerFault must belong to a failed lastRun");
    }
  }
  return {
    ...normalizeSessionRuntime(value, "session response"),
    ...(currentRun !== undefined ? { currentRun } : {}),
    ...(lastRun !== undefined ? { lastRun } : {}),
    ...(providerFault !== undefined ? { providerFault } : {})
  } as unknown as Session;
}

const REMOVED_SESSION_WIRE_FIELDS = ["sessionId", "runtime", "turnSeq", "cleanupStatus"] as const;

function assertCanonicalSessionWireFields(value: Record<string, unknown>, context: string): void {
  const removed = REMOVED_SESSION_WIRE_FIELDS.find((field) => Object.hasOwn(value, field));
  if (removed !== undefined) {
    throw new SessionStateError(`${context} contains the removed ${removed} field`);
  }
}

function normalizeSessionAccepted<T extends { readonly session: Session }>(value: T, context: string): T {
  if (!isRecord(value) || !isRecord(value.session)) {
    throw new SessionStateError(`${context} must contain a session object`);
  }
  return {
    ...value,
    session: unwrapSession({ session: value.session })
  };
}

const SESSION_RUN_PHASE_SET = new Set<string>(SESSION_RUN_PHASES);
const SESSION_RUN_OUTCOME_SET = new Set<string>(SESSION_TERMINAL_OUTCOMES);

function normalizeSessionMessageAccepted(value: unknown, requestedSessionId: string): SessionMessageAccepted {
  if (!isRecord(value)) {
    throw new SessionStateError("session message response must be an object");
  }
  if (Object.hasOwn(value, "turn")) {
    throw new SessionStateError("session message response contains the removed turn field; use run");
  }
  if (!isRecord(value.session)) {
    throw new SessionStateError("session message response must contain a session object");
  }
  const session = unwrapSession({ session: value.session as unknown as Session });
  if (session.id !== requestedSessionId) {
    throw new SessionStateError("session message response session.id does not match the requested session", {
      requestedSessionId,
      sessionId: session.id
    });
  }
  if (!isRecord(value.run)) {
    throw new SessionStateError("session message response must contain a run object");
  }
  const run = normalizeSessionRun(value.run, session.id, "session message response run");
  const eventCursor = value.eventCursor;
  if (eventCursor !== undefined && (!Number.isSafeInteger(eventCursor) || (eventCursor as number) < 0)) {
    throw new SessionStateError("session message response eventCursor must be a non-negative safe integer");
  }
  if (
    eventCursor !== undefined &&
    run.eventCursor !== undefined &&
    eventCursor !== run.eventCursor
  ) {
    throw new SessionStateError("session message response eventCursor does not match run.eventCursor");
  }
  return {
    ...value,
    session,
    run,
    ...(typeof eventCursor === "number" ? { eventCursor } : {})
  };
}

function normalizeOptionalSessionRun(value: unknown, sessionId: string, context: string): SessionRun | undefined {
  if (value === undefined) return undefined;
  if (!isRecord(value)) {
    throw new SessionStateError(`${context} must be an object`);
  }
  return normalizeSessionRun(value, sessionId, context);
}

function normalizeSessionRun(value: Record<string, unknown>, sessionId: string, context: string): SessionRun {
  if (Object.hasOwn(value, "executionEndedAt")) {
    throw new SessionStateError(`${context} contains the removed executionEndedAt field`);
  }
  if (typeof value.sessionId !== "string" || value.sessionId.length === 0) {
    throw new SessionStateError(`${context}.sessionId must be a non-empty string`);
  }
  if (value.sessionId !== sessionId) {
    throw new SessionStateError(`${context}.sessionId does not match session.id`, {
      sessionId,
      runSessionId: value.sessionId
    });
  }
  if (typeof value.runId !== "string" || value.runId.length === 0) {
    throw new SessionStateError(`${context}.runId must be a non-empty string`);
  }
  if (!Number.isSafeInteger(value.turnSeq) || (value.turnSeq as number) < 1) {
    throw new SessionStateError(`${context}.turnSeq must be a positive safe integer`);
  }
  if (typeof value.phase !== "string" || !SESSION_RUN_PHASE_SET.has(value.phase)) {
    throw new SessionStateError(`${context}.phase is invalid`);
  }
  if (value.outcome !== undefined && (typeof value.outcome !== "string" || !SESSION_RUN_OUTCOME_SET.has(value.outcome))) {
    throw new SessionStateError(`${context}.outcome is invalid`);
  }
  for (const field of ["startedAt", "finishedAt"] as const) {
    if (value[field] !== undefined && typeof value[field] !== "string") {
      throw new SessionStateError(`${context}.${field} must be a string`);
    }
  }
  if (value.eventCursor !== undefined && (!Number.isSafeInteger(value.eventCursor) || (value.eventCursor as number) < 0)) {
    throw new SessionStateError(`${context}.eventCursor must be a non-negative safe integer`);
  }
  if (value.checkpoint !== undefined && !isRecord(value.checkpoint)) {
    throw new SessionStateError(`${context}.checkpoint must be an object`);
  }
  return {
    sessionId: value.sessionId,
    turnSeq: value.turnSeq as number,
    runId: value.runId,
    phase: value.phase as SessionRun["phase"],
    ...(typeof value.outcome === "string"
      ? { outcome: value.outcome as NonNullable<SessionRun["outcome"]> }
      : {}),
    ...(typeof value.startedAt === "string" ? { startedAt: value.startedAt } : {}),
    ...(typeof value.finishedAt === "string" ? { finishedAt: value.finishedAt } : {}),
    ...(isRecord(value.checkpoint) ? { checkpoint: value.checkpoint as unknown as NonNullable<SessionRun["checkpoint"]> } : {}),
    ...(typeof value.eventCursor === "number" ? { eventCursor: value.eventCursor } : {})
  };
}

function normalizeSessionRuntime(value: Record<string, unknown>, context: string): Record<string, unknown> {
  let size;
  try {
    size = parseRuntimeSizeForRead(value.runtimeSize);
  } catch (error) {
    throw new SessionStateError(`${context} has an invalid runtimeSize: ${error instanceof Error ? error.message : String(error)}`);
  }
  // Fold the flat wire fields (`runtimeKind`, `runtimeSize`) into the grouped
  // client shape `runtime: { kind, size }`. Tolerant on read — an unknown future
  // runtime kind passes through rather than throwing (forward-compat), unlike the
  // strict submit path.
  const kind = typeof value.runtimeKind === "string" ? value.runtimeKind : undefined;
  const runtime =
    kind !== undefined || size !== undefined
      ? { ...(kind !== undefined ? { kind } : {}), ...(size !== undefined ? { size } : {}) }
      : undefined;
  const { runtimeSize: _wireSize, runtimeKind: _wireKind, ...normalized } = value;
  return {
    ...normalized,
    ...(runtime !== undefined ? { runtime } : {})
  };
}

// Session records can outlive a public runtime-size vocabulary migration. Keep
// read-side normalization compatible with records emitted by the previous
// pre-launch platform while retaining strict validation for new submissions.
const LEGACY_RUNTIME_SIZE_ALIASES: Readonly<Record<string, RuntimeSize>> = {
  "shared-0.25x-1gb": "0.25cpu-1gb",
  "shared-0.5x-4gb": "0.5cpu-4gb",
  "shared-1x-6gb": "1cpu-6gb",
  "shared-2x-8gb": "2cpu-8gb",
  "shared-4x-12gb": "4cpu-12gb"
};

function parseRuntimeSizeForRead(input: unknown): RuntimeSize | undefined {
  if (typeof input === "string" && input in LEGACY_RUNTIME_SIZE_ALIASES) {
    return LEGACY_RUNTIME_SIZE_ALIASES[input]!;
  }
  return parseRuntimeSize(input);
}
