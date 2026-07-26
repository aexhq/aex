import {
  CredentialValidationError,
  HttpClient,
  PLANE_BASE_URLS,
  SessionStateError,
  SecretString,
  asAexEventView,
  asAexStreamEventView,
  customName,
  isReplayableEvent,
  tryParseApiKey,
  parseModelSlug,
  resolveBuiltinToolNames,
  streamCoordinatorEvents,
  type AexEvent,
  type AexEventView,
  type AexStreamEvent,
  type AexStreamEventView,
  type BillingAutoTopupRequest,
  type BillingAutoTopupUpdate,
  type BillingHostedSession,
  type BillingLedgerPage,
  type BillingLedgerQuery,
  type BillingPortalRequest,
  type BillingSummary,
  type BillingTopupCheckoutRequest,
  type ChildSessionRef,
  type DebugSink,
  type FetchLike,
  type McpServerRef,
  type OtlpExportLogsServiceRequest,
  type OtlpExportTraceServiceRequest,
  type SessionFile,
  type SessionFileType,
  type SessionFileLink,
  type SessionFileLinkOptions,
  type SessionFileQuery,
  type SessionFilesQuery,
  type SessionFilesSnapshot,
  type SessionFileText,
  type ReadSessionFileTextOptions,
  type TurnOutcome,
  type Session,
  type SessionCreateRequest,
  type SessionListPage,
  type SessionListQuery,
  type SessionMessageAccepted,
  type SessionStateChangeAccepted,
  type SessionRun,
  type PlatformEnvironmentInput,
  type PlatformSubmission,
  type PlatformInlineSecrets,
  type SessionLimits,
  parseSessionLimits,
  parseApprovalGate,
  parseResponseFormat,
  parseRuntimeSize,
  parseRuntimeKind,
  parseSessionTimeout,
  parseSessionWebhook,
  type SessionWebhookDelivery,
  type SecretRecord,
  type OrgRecord,
  type CreateOrgRequest,
  type WorkspaceRecord,
  type CreateWorkspaceRequest,
  type NewWorkspace,
  type ApiKeyRecord,
  type CreateApiKeyRequest,
  type NewApiKey,
  type OrgMemberRecord,
  type CreateOrgInviteRequest,
  type OrgInvite,
  type WorkspaceFileRef,
  type WorkspaceFileRecord,
  type WorkspaceInstructionRef,
  type WorkspaceInstructionRecord,
  type WorkspaceResourceListQuery,
  type WorkspaceResourcePage,
  type WorkspaceSkillRef,
  type WorkspaceSkillRecord,
  type WorkspaceToolRef,
  type WorkspaceToolRecord,
  type WebhookSigningSecret,
  type WebSocketFactory,
  type WhoAmI
} from "@aexhq/contracts";
import { hasRunTerminalType, operations, type AssetUploadRetryOptions } from "@aexhq/contracts/internal";
import { Instructions } from "./instructions.js";
import { uploadAsset, uploadAssetMultipart, type AssetFetch, type UploadedAsset } from "./asset-upload.js";
import { File, type ZipStreamDriver } from "./file.js";
import {
  AexRateLimitError,
  isThrottleFault,
  resolveRetryConfig,
  type ProviderFault,
  type RetryOptions
} from "./retry.js";
import { legacySessionProviderFault } from "./legacy-session-provider-fault.js";
import { splitSecretEnv } from "./secret.js";
import { Skill } from "./skill.js";
import { Tool } from "./tool.js";
import type {
  IdempotencyOptions,
  Message,
  SessionCreateOptions,
  SessionInput,
  SessionResult,
  SessionRunResult,
  SessionSendOptions,
  SessionStartOptions,
  StartSessionOptions
} from "./client-types.js";
import {
  assertRunCheckpoint,
  assertSessionCommittedAfterRun,
  buildTurnResult,
  isSessionRunTerminalEvent,
  messageFromWire,
  PROGRESSING_SESSION_STATUSES,
  projectAssistantMessages,
  terminalCheckpointId,
  terminalSessionStatusFromEvents,
  turnTraceFromEvents
} from "./event-projection.js";
import {
  assertStartSessionOptions,
  assertSupportedSessionFields,
  assertSupportedSessionSendOptions,
  configError,
  normaliseSessionInput,
  validatedSessionConfig
} from "./session-validate.js";
import {
  fileCaptureForWire,
  mergeMcpServers,
  sessionEnvironmentForWire,
  sessionRetentionForWire
} from "./submission-wire.js";

export type {
  IdempotencyOptions,
  Message,
  SessionCreateOptions,
  SessionEnvironmentOptions,
  SessionInput,
  SessionOverrides,
  SessionResult,
  SessionRunResult,
  SessionSendOptions,
  SessionStartOptions,
  StartSessionOptions
} from "./client-types.js";

export interface AexOptions {
  /** Workspace-scoped SDK API key. */
  readonly apiKey?: string;
  /**
   * API root, e.g. `https://aex.example.com`. Optional — defaults to the
   * canonical prd URL (`https://api.aex.dev`). Dev keys select
   * `https://dev-api.aex.dev`; override this for a localhost development stack
   * or another hosted aex API endpoint.
   */
  readonly baseUrl?: string;
  /** Optional `fetch` override for testing. */
  readonly fetch?: FetchLike;
  /**
   * Local debug output. `true` prints a redacted one-line trace per HTTP
   * request to stderr (method, path, status, elapsed); pass a function to
   * route the traces elsewhere. Purely local — nothing is uploaded.
   */
  readonly debug?: boolean | DebugSink;
  /**
   * Built-in transport retry policy for hosted API requests and direct asset
   * uploads. Reads, idempotent HTTP methods, and mutations carrying a stable
   * Idempotency-Key are retried on transient failures with bounded exponential
   * backoff + jitter. Unsafe mutations are attempted once.
   *
   * Omit for sensible defaults (4 attempts, ~2 min budget); pass an object to
   * tune `maxAttempts` / delays / `maxElapsedMs`; pass `false` to disable.
   */
  readonly retry?: RetryOptions | false;
}

interface InternalSessionSendOptions extends SessionSendOptions {
  readonly signal?: AbortSignal;
}

interface InternalSessionRunStreamOptions extends InternalSessionSendOptions {
  readonly from: number;
}

export class SessionRunStream implements AsyncIterable<AexStreamEventView> {
  readonly #stream: () => AsyncGenerator<AexStreamEventView, SessionRunResult, void>;
  readonly #events: AexStreamEventView[] = [];
  readonly #waiters = new Set<() => void>();
  #outcome:
    | { readonly ok: true; readonly value: SessionRunResult }
    | { readonly ok: false; readonly error: unknown }
    | undefined;
  #pump: Promise<SessionRunResult> | undefined;
  #hasIterator = false;

  constructor(stream: () => AsyncGenerator<AexStreamEventView, SessionRunResult, void>) {
    this.#stream = stream;
  }

  #start(): Promise<SessionRunResult> {
    this.#pump ??= this.#drain();
    return this.#pump;
  }

  async #drain(): Promise<SessionRunResult> {
    const generator = this.#stream();
    try {
      let next = await generator.next();
      while (!next.done) {
        this.#events.push(next.value);
        this.#notify();
        next = await generator.next();
      }
      this.#outcome = { ok: true, value: next.value };
      this.#notify();
      return next.value;
    } catch (error) {
      this.#outcome = { ok: false, error };
      this.#notify();
      throw error;
    }
  }

  #notify(): void {
    for (const resolve of this.#waiters) resolve();
    this.#waiters.clear();
  }

  [Symbol.asyncIterator](): AsyncIterator<AexStreamEventView> {
    let cursor = this.#hasIterator ? this.#events.length : 0;
    this.#hasIterator = true;
    void this.#start().catch(() => {});
    // Deliberately do NOT forward `return()`: `break`-ing out of a
    // `for await` loop must not close the in-flight turn — `finished()` can
    // still drain it to completion afterwards.
    return {
      next: async () => {
        while (true) {
          if (cursor < this.#events.length) {
            return { done: false as const, value: this.#events[cursor++]! };
          }
          if (this.#outcome !== undefined) {
            if (!this.#outcome.ok) throw this.#outcome.error;
            return { done: true as const, value: undefined };
          }
          await new Promise<void>((resolve) => this.#waiters.add(resolve));
        }
      },
      return: async () => ({ done: true as const, value: undefined })
    };
  }

  finished(): Promise<SessionRunResult> {
    return this.#start();
  }
}

type InternalSessionSender = (input: SessionInput, options?: InternalSessionSendOptions) => SessionRunStream;
const internalSessionSenders = new WeakMap<SessionHandle, InternalSessionSender>();
function sendSessionInternal(
  session: SessionHandle,
  input: SessionInput,
  options: InternalSessionSendOptions = {}
): SessionRunStream {
  const sender = internalSessionSenders.get(session);
  if (sender === undefined) {
    throw new Error("Aex: invalid session handle");
  }
  return sender(normaliseSessionInput(input, "session.messages.send", "input"), options);
}

/**
 * Accessor over the session's assistant messages. `session.messages` returns
 * this synchronously; each method fetches on call.
 */
export interface SessionMessages {
  send(input: SessionInput, options?: SessionSendOptions): SessionRunStream;
  replayLast(options?: SessionSendOptions): SessionRunStream;
  /** Return the full transcript, transparently following bounded API pages. */
  list(): Promise<readonly Message[]>;
  last(): Promise<Message | undefined>;
  first(): Promise<Message | undefined>;
}

/**
 * Accessor over the session's events. Snapshot/polling reads yield durable
 * {@link AexEventView}s with a replay cursor. The live coordinator may also
 * yield provisional {@link AexStreamEventView}s with `replayable:false` and a
 * per-run `liveSequence` instead of a durable `sequence`.
 */
export interface SessionEvents {
  /** Lazily traverse durable history one bounded API page at a time. */
  iterate(options?: IterateEventsOptions): AsyncIterable<AexEventView>;
  list(): Promise<readonly AexEventView[]>;
  last(): Promise<AexEventView | undefined>;
  first(): Promise<AexEventView | undefined>;
  stream(options?: StreamEventsOptions): AsyncIterable<AexEventView>;
  streamEnvelopes(options?: StreamEnvelopesOptions): AsyncIterable<AexStreamEventView>;
  archiveLink(options?: SessionFileLinkOptions): Promise<SessionFileLink>;
  /** Download the events-namespace archive as a zip. */
  download(options?: DownloadOptions): Promise<Uint8Array>;
}

/** Standards-pure OTLP/HTTP JSON pages derived from a session's journal. */
export interface SessionOtel {
  traces(): AsyncIterable<OtlpExportTraceServiceRequest>;
  logs(): AsyncIterable<OtlpExportLogsServiceRequest>;
}

/**
 * Accessor over the session's captured files (`session.files`):
 * enumerate, read one as capped text, locate/resolve, and download.
 */
export interface SessionFiles {
  list(query?: SessionFilesQuery): Promise<SessionFilesSnapshot>;
  last(): Promise<SessionFile | undefined>;
  first(): Promise<SessionFile | undefined>;
  read(selector: SessionFileSelector, options?: ReadSessionFileTextOptions): Promise<SessionFileText>;
  find(query: SessionFilesQuery): Promise<readonly SessionFile[]>;
  findOne(query: SessionFilesQuery): Promise<SessionFile | null>;
  link(selectorOrQuery: SessionFileLinkSelector, options?: SessionFileLinkOptions): Promise<SessionFileLink>;
  fetch(selectorOrQuery: SessionFileLinkSelector, options?: SessionFileLinkOptions): Promise<Response>;
  /** No selector = files-namespace zip; with selector = one file's raw bytes. */
  download(selector?: SessionFileSelector, options?: DownloadOptions): Promise<Uint8Array>;
}

/**
 * Accessor over the session's run-scoped webhook ledger (`session.webhooks`).
 */
export interface SessionWebhooks {
  list(): Promise<readonly SessionWebhookDelivery[]>;
  redeliver(deliveryId: string): Promise<void>;
}

export class SessionHandle {
  readonly #http: HttpClient;
  readonly #fetch: FetchLike | undefined;
  #session: Session;
  readonly messages: SessionMessages;
  readonly events: SessionEvents;
  readonly otel: SessionOtel;
  readonly files: SessionFiles;
  readonly webhooks: SessionWebhooks;
  /** The last message sent on this handle, for `session.messages.replayLast()`. */
  #lastSend: { readonly input: SessionInput; readonly idempotencyKey: string } | undefined;

  constructor(http: HttpClient, session: Session, fetch?: FetchLike) {
    this.#http = http;
    this.#session = session;
    this.#fetch = fetch;
    const id = session.id;
    this.messages = sessionMessages(
      http,
      id,
      (input, options = {}) => {
        assertSupportedSessionSendOptions(options, "session.messages.send");
        return sendSessionInternal(this, input, options);
      },
      (options = {}) => {
        assertSupportedSessionSendOptions(options, "session.messages.replayLast");
        const last = this.#lastSend;
        if (last === undefined) {
          throw new SessionStateError("session.messages.replayLast: no message has been sent on this session yet");
        }
        return sendSessionInternal(this, last.input, {
          ...options,
          idempotencyKey: options.idempotencyKey ?? last.idempotencyKey
        });
      }
    );
    this.events = sessionEvents(http, id);
    this.otel = sessionOtel(http, id);
    this.files = sessionFiles(http, id, fetch);
    this.webhooks = {
      list: () => operations.getSessionWebhookDeliveries(http, id),
      redeliver: (deliveryId) => operations.redeliverSessionWebhook(http, id, deliveryId)
    };
    internalSessionSenders.set(this, (input, options = {}) => new SessionRunStream(() => this.#send(input, options)));
  }

  get id(): string {
    return this.#session.id;
  }

  get record(): Session {
    return this.#session;
  }

  async *#send(input: SessionInput, options: InternalSessionSendOptions): AsyncGenerator<AexStreamEventView, SessionRunResult, void> {
    const idempotencyKey = operations.resolveIdempotencyKey(options.idempotencyKey);
    this.#lastSend = { input, idempotencyKey };
    const accepted = await operations.sendSessionMessage(this.#http, this.id, { input }, { idempotencyKey });
    this.#session = accepted.session;
    const run = accepted.run;
    const eventCursor = accepted.eventCursor ?? run.eventCursor ?? 0;
    const streamEvents: AexStreamEventView[] = [];
    for await (const event of streamSessionRunEvents(this.#http, this.id, run, {
      ...options,
      from: eventCursor
    })) {
      streamEvents.push(event);
      yield event;
    }
    const events = streamEvents.filter(isDurableEventView);
    // RUN_FINISHED/RUN_ERROR is the consistency barrier: all reads below must
    // already observe the same committed run and checkpoint.
    const read = terminalSessionStatusFromEvents(events, run.runId);
    this.#session = await operations.getSession(this.#http, this.id);
    assertSessionCommittedAfterRun(this.#session, run, read);
    const checkpointId = terminalCheckpointId(events, run.runId);
    const snapshot = checkpointId === undefined
      ? undefined
      : await operations.listSessionFiles(this.#http, this.id, { checkpointId });
    if (snapshot !== undefined) {
      assertRunCheckpoint(events, run.runId, snapshot.revision);
    }
    const messages = projectAssistantMessages(events);
    return buildTurnResult(
      this.id,
      this.#session,
      run,
      events,
      snapshot?.files ?? [],
      snapshot?.revision,
      messages,
      read
    );
  }

  async suspend(): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.suspendSession(this.#http, this.id);
    this.#session = accepted.session;
    return accepted;
  }

  async cancel(): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.cancelSession(this.#http, this.id);
    this.#session = accepted.session;
    return accepted;
  }

  async resume(): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.resumeSession(this.#http, this.id);
    this.#session = accepted.session;
    return accepted;
  }

  async delete(): Promise<void> {
    const accepted = await operations.deleteSession(this.#http, this.id);
    if (accepted && typeof accepted === "object" && "session" in accepted) {
      this.#session = accepted.session;
    }
  }

  /**
   * Request the HITL write-gate: park this session `awaiting_approval` before
   * its next gated action. Imperative counterpart to the declarative
   * `approvalGate` submission option. Resume with {@link approve} / reject with
   * {@link deny}.
   */
  async requestApproval(): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.requestApproval(this.#http, this.id);
    this.#session = accepted.session;
    return accepted;
  }

  /** Approve an `awaiting_approval` session so the held turn resumes (→ running). */
  async approve(): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.approveSession(this.#http, this.id);
    this.#session = accepted.session;
    return accepted;
  }

  /** Deny an `awaiting_approval` session; its run is cancelled and the session returns to `idle`. */
  async deny(): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.denySession(this.#http, this.id);
    this.#session = accepted.session;
    return accepted;
  }

  /**
   * Enumerate this session's subagent CHILD sessions (`GET /api/sessions/:id/children`). Each is
   * a read-only {@link ChildSessionHandle} with observable events, checkpointed
   * files, descendants, lineage, and lifecycle state.
   */
  async children(): Promise<readonly ChildSessionHandle[]> {
    const refs = await operations.listSessionChildren(this.#http, this.id);
    return refs.map((ref) => new ChildSessionHandle(this.#http, ref, this.#fetch));
  }

  /** Re-read the session record from the server and store it as the current record. */
  async refresh(): Promise<Session> {
    this.#session = await operations.getSession(this.#http, this.id);
    return this.#session;
  }

  /**
   * Download EVERYTHING public about this session as one zip, assembled
   * client-side from the public read endpoints. Organised into `metadata/`,
   * `events/`, and `files/` folders, plus a `manifest.json`. Pass `to` to
   * also write the bytes to a file path while still returning them.
   */
  async download(options?: DownloadOptions): Promise<Uint8Array> {
    return writeOptionalFile(await operations.download(this.#http, this.id), options?.to);
  }

  /** Download only the session record (the `metadata` namespace) as a zip. */
  async downloadMetadata(options?: DownloadOptions): Promise<Uint8Array> {
    return writeOptionalFile(await operations.downloadMetadata(this.#http, this.id), options?.to);
  }
}

export class SessionClient {
  readonly #http: HttpClient;
  readonly #fetch: FetchLike | undefined;
  readonly #buildCreateRequest: (options: SessionCreateOptions) => Promise<SessionCreateRequest>;

  constructor(
    http: HttpClient,
    buildCreateRequest: (options: SessionCreateOptions) => Promise<SessionCreateRequest>,
    fetch?: FetchLike
  ) {
    this.#http = http;
    this.#buildCreateRequest = buildCreateRequest;
    this.#fetch = fetch;
  }

  async create(options: SessionCreateOptions): Promise<SessionHandle> {
    const request = await this.#buildCreateRequest(options);
    const session = await operations.createSession(
      this.#http,
      request,
      { idempotencyKey: operations.resolveIdempotencyKey(options.idempotencyKey) }
    );
    return new SessionHandle(this.#http, session, this.#fetch);
  }

  async open(sessionId: string): Promise<SessionHandle> {
    return new SessionHandle(this.#http, await operations.getSession(this.#http, sessionId), this.#fetch);
  }

  get(sessionId: string): Promise<Session> {
    return operations.getSession(this.#http, sessionId);
  }

  async delete(sessionId: string): Promise<void> {
    await operations.deleteSession(this.#http, sessionId);
  }

  list(query?: SessionListQuery): Promise<SessionListPage> {
    return operations.listSessions(this.#http, query);
  }

}

/** Child-session events accessor (list + polling stream) keyed on a session id. */
export interface ChildSessionEvents {
  iterate(options?: IterateEventsOptions): AsyncIterable<AexEventView>;
  list(): Promise<readonly AexEventView[]>;
  stream(options?: StreamEventsOptions): AsyncIterable<AexEventView>;
}

/**
 * Read-only observation handle for a lineage-discoverable subagent child.
 * Child ids address events, checkpointed files, and descendants. They are not
 * top-level resumable sessions, so this handle deliberately has no get, send,
 * cancel, suspend, resume, or delete controls.
 */
export class ChildSessionHandle {
  readonly #http: HttpClient;
  readonly #fetch: FetchLike | undefined;
  readonly #ref: ChildSessionRef;
  readonly events: ChildSessionEvents;
  readonly files: SessionFiles;

  constructor(http: HttpClient, ref: ChildSessionRef, fetch?: FetchLike) {
    this.#http = http;
    this.#ref = ref;
    this.#fetch = fetch;
    this.events = childSessionEventsAccessor(http, ref);
    this.files = sessionFiles(http, ref.id, fetch);
  }

  get id(): string {
    return this.#ref.id;
  }

  get parentSessionId(): string {
    return this.#ref.parentSessionId;
  }

  get depth(): number | undefined {
    return this.#ref.depth;
  }

  /** The lifecycle status captured by the parent lineage read. */
  get status(): ChildSessionRef["status"] {
    return this.#ref.status;
  }

  get ref(): ChildSessionRef {
    return this.#ref;
  }

  /** This child's own subagent children (recursive lineage). */
  async children(): Promise<readonly ChildSessionHandle[]> {
    const refs = await operations.listSessionChildren(this.#http, this.id);
    return refs.map((ref) => new ChildSessionHandle(this.#http, ref, this.#fetch));
  }
}

/** Session-record events accessor (list + polling stream) used by {@link ChildSessionHandle}. */
function childSessionEventsAccessor(http: HttpClient, ref: ChildSessionRef): ChildSessionEvents {
  return {
    iterate: (options?: IterateEventsOptions) => iterateSessionEventViews(http, ref.id, options ?? {}),
    list: async () => (await operations.listSessionEvents(http, ref.id)).map(asAexEventView),
    stream: (options?: StreamEventsOptions) => streamChildSessionEventsPolling(http, ref, options ?? {})
  };
}

async function* iterateSessionEventViews(
  http: HttpClient,
  id: string,
  options: IterateEventsOptions
): AsyncIterable<AexEventView> {
  for await (const event of operations.iterateSessionEvents(http, id, options)) {
    yield asAexEventView(event);
  }
}

interface ResolvedSessionEventPolling {
  readonly boundedRunlessSnapshot: boolean;
  readonly isTerminal: (event: AexEvent) => boolean;
}

type ResolveSessionEventPolling = (
  from: number
) => ResolvedSessionEventPolling | PromiseLike<ResolvedSessionEventPolling>;

/** One snapshot-polling engine shared by root and child session event streams. */
async function* pollSessionEventViews(
  http: HttpClient,
  id: string,
  options: StreamEventsOptions,
  resolvePolling: ResolveSessionEventPolling
): AsyncIterable<AexEventView> {
  const from = validateStreamEventsFrom(options.from);
  if (options.signal?.aborted) return;
  const seenIds = new Set<string>();
  const intervalMs = options.intervalMs ?? 1_000;
  const signal = options.signal;
  const { boundedRunlessSnapshot, isTerminal } = await resolvePolling(from);
  while (!signal?.aborted) {
    const events = await operations.listSessionEvents(http, id);
    let terminalSeen = false;
    for (const event of events) {
      if (event.sequence >= from && !seenIds.has(event.id)) {
        seenIds.add(event.id);
        yield asAexEventView(event);
      }
      if (isTerminal(event)) terminalSeen = true;
    }
    if (terminalSeen || boundedRunlessSnapshot) return;
    // `sleep` rejects on abort — treat that as a graceful stop.
    try {
      await sleep(intervalMs, signal);
    } catch {
      return;
    }
  }
}

/**
 * Poll a child session's events until a committed RUN terminal is visible, the
 * signal aborts, or the caller breaks the iterator.
 */
function streamChildSessionEventsPolling(
  http: HttpClient,
  ref: ChildSessionRef,
  options: StreamEventsOptions
): AsyncIterable<AexEventView> {
  return pollSessionEventViews(http, ref.id, options, (from) => {
    const progressing = PROGRESSING_SESSION_STATUSES.has(ref.status);
    const targetRunId = progressing ? undefined : ref.lastRun?.runId;
    const priorRunId = progressing ? ref.lastRun?.runId : undefined;
    return {
      boundedRunlessSnapshot: !progressing && targetRunId === undefined,
      isTerminal: targetRunId !== undefined
        ? (event) => isSessionRunTerminalEvent(event, targetRunId)
        : (event) => hasRunTerminalType(event) && event.sequence >= from && event.runId !== priorRunId
    };
  });
}

async function* streamSessionRunEvents(
  http: HttpClient,
  sessionId: string,
  run: SessionRun,
  options: InternalSessionRunStreamOptions
): AsyncGenerator<AexStreamEventView, void, void> {
  const first = await operations.getSessionCoordinatorTicket(http, sessionId);
  for await (const event of streamCoordinatorEvents({
    wsUrl: first.wsUrl,
    from: options.from ?? 0,
    fetchTicket: async () => (await operations.getSessionCoordinatorTicket(http, sessionId)).ticket,
    isTerminal: (event: AexEvent) => isSessionRunTerminalEvent(event, run.runId),
    ...(options.signal ? { signal: options.signal } : {}),
    ...(options.webSocketFactory ? { webSocketFactory: options.webSocketFactory } : {}),
    ...(options.idleTimeoutMs !== undefined ? { idleTimeoutMs: options.idleTimeoutMs } : {}),
    ...(options.pingIntervalMs !== undefined ? { pingIntervalMs: options.pingIntervalMs } : {})
  })) {
    if (event.runId !== run.runId) continue;
    yield asAexStreamEventView(event);
  }
}

function isDurableEventView(event: AexStreamEventView): event is AexEventView {
  return isReplayableEvent(event as AexStreamEvent);
}

/**
 * Poll the session's event snapshots until a durable run terminal, the signal
 * aborts, or the caller breaks the iterator, deduping by event id. Yields the
 * one canonical guard-bearing {@link AexEventView} (same shape as every other
 * event surface). Module-level so `SessionHandle.events` can hand it to its
 * accessor object literal.
 */
function streamSessionEventsPolling(
  http: HttpClient,
  id: string,
  options: StreamEventsOptions
): AsyncIterable<AexEventView> {
  return pollSessionEventViews(http, id, options, async (from) => {
    const initial = await operations.getSession(http, id);
    const targetRunId = initial.currentRun?.runId ?? initial.lastRun?.runId;
    return {
      boundedRunlessSnapshot: targetRunId === undefined && !PROGRESSING_SESSION_STATUSES.has(initial.status),
      isTerminal: targetRunId === undefined
        ? (event) => hasRunTerminalType(event) && event.sequence >= from
        : (event) => isSessionRunTerminalEvent(event, targetRunId)
    };
  });
}

/**
 * Stream the unified {@link AexEvent} envelope live over the session's
 * coordinator WebSocket. The ticket is re-minted on each (re)connect so a long
 * session never outlives it. Module-level so `SessionHandle.events` can hand
 * it to its accessor object literal.
 */
async function* streamSessionEnvelopes(
  http: HttpClient,
  id: string,
  options: StreamEnvelopesOptions
): AsyncIterable<AexStreamEventView> {
  const first = await operations.getSessionCoordinatorTicket(http, id);
  for await (const event of streamCoordinatorEvents({
    wsUrl: first.wsUrl,
    from: options.from ?? 0,
    fetchTicket: async () => (await operations.getSessionCoordinatorTicket(http, id)).ticket,
    isTerminal: hasRunTerminalType,
    ...(options.signal ? { signal: options.signal } : {}),
    ...(options.idleTimeoutMs !== undefined ? { idleTimeoutMs: options.idleTimeoutMs } : {}),
    ...(options.pingIntervalMs !== undefined ? { pingIntervalMs: options.pingIntervalMs } : {}),
    ...(options.eventQuietRecheckMs !== undefined ? { eventQuietRecheckMs: options.eventQuietRecheckMs } : {})
  })) {
    yield asAexStreamEventView(event);
  }
}

/**
 * Download captured files. No selector → the full files namespace as a
 * zip; a selector → one file's raw bytes. Module-level so
 * `SessionHandle.files` can hand it to its accessor object literal.
 */
async function downloadSessionFile(
  http: HttpClient,
  id: string,
  selector?: SessionFileSelector,
  options?: DownloadOptions
): Promise<Uint8Array> {
  // One selector-resolution path: contracts resolve every selector against the
  // authoritative checkpoint, then download and verify the committed bytes.
  const transferOptions = {
    ...(options?.timeoutMs !== undefined ? { timeoutMs: options.timeoutMs } : {}),
    ...(options?.checkpointId !== undefined ? { checkpointId: options.checkpointId } : {})
  };
  const bytes =
    selector === undefined
      ? await operations.downloadSessionFiles(http, id, transferOptions)
      : (await operations.downloadSessionFile(http, id, selector, transferOptions)).bytes;
  return writeOptionalFile(bytes, options?.to);
}

/**
 * Build the files accessor for a session id. Shared by
 * `SessionHandle.files` and child-session handles, so both expose the identical
 * rich {@link SessionFiles} surface.
 */
function sessionFiles(http: HttpClient, id: string, fetchLike: FetchLike | undefined): SessionFiles {
  const list = (query?: SessionFilesQuery): Promise<SessionFilesSnapshot> =>
    operations.listSessionFiles(http, id, query);
  return {
    list,
    last: async () => (await list()).files.at(-1),
    first: async () => (await list()).files[0],
    read: (selector, options) => operations.readSessionFileText(http, id, selector, options),
    find: (query) => operations.findSessionFiles(http, id, query),
    findOne: (query) => operations.findSessionFile(http, id, query),
    link: (selectorOrQuery, options) => operations.sessionFileLink(http, id, selectorOrQuery, options),
    fetch: async (selectorOrQuery, options) => {
      const link = await operations.sessionFileLink(http, id, selectorOrQuery, options);
      return (fetchLike ?? globalThis.fetch)(link.url);
    },
    download: (selector, options) => downloadSessionFile(http, id, selector, options)
  };
}

function sessionMessages(
  http: HttpClient,
  id: string,
  send: (input: SessionInput, options?: SessionSendOptions) => SessionRunStream,
  replayLast: (options?: SessionSendOptions) => SessionRunStream
): SessionMessages {
  const list = (): Promise<readonly Message[]> => listAllSessionMessages(http, id);
  return {
    send,
    replayLast,
    list,
    last: async () => (await list()).at(-1),
    first: async () => (await list())[0]
  };
}

const LIST_MESSAGES_PAGE_BUDGET = 1000;

async function listAllSessionMessages(http: HttpClient, id: string): Promise<readonly Message[]> {
  const messages: Message[] = [];
  const seenCursors = new Set<string>();
  let cursor: string | undefined;
  for (let pageIndex = 0; pageIndex < LIST_MESSAGES_PAGE_BUDGET; pageIndex += 1) {
    const page = await operations.listSessionMessages(http, id, cursor === undefined ? undefined : { cursor });
    messages.push(...page.messages.map(messageFromWire));
    if (page.nextCursor === undefined) return messages;
    if (typeof page.nextCursor !== "string" || page.nextCursor.length === 0) {
      throw new SessionStateError("session messages response contains an invalid nextCursor", { sessionId: id });
    }
    if (seenCursors.has(page.nextCursor)) {
      throw new SessionStateError("session messages pagination repeated a cursor", {
        sessionId: id,
        cursor: page.nextCursor
      });
    }
    seenCursors.add(page.nextCursor);
    cursor = page.nextCursor;
  }
  throw new SessionStateError("session messages pagination exceeded its page budget", {
    sessionId: id,
    pageBudget: LIST_MESSAGES_PAGE_BUDGET
  });
}

function sessionEvents(http: HttpClient, id: string): SessionEvents {
  const iterate = (options?: IterateEventsOptions): AsyncIterable<AexEventView> =>
    iterateSessionEventViews(http, id, options ?? {});
  const list = async (): Promise<readonly AexEventView[]> =>
    (await operations.listSessionEvents(http, id)).map(asAexEventView);
  return {
    iterate,
    list,
    last: async () => {
      let last: AexEventView | undefined;
      for await (const event of iterate()) last = event;
      return last;
    },
    first: async () => {
      for await (const event of iterate()) return event;
      return undefined;
    },
    stream: (options?: StreamEventsOptions) => streamSessionEventsPolling(http, id, options ?? {}),
    streamEnvelopes: (options?: StreamEnvelopesOptions) => streamSessionEnvelopes(http, id, options ?? {}),
    archiveLink: (options?: SessionFileLinkOptions) => operations.eventArchiveLink(http, id, options),
    download: async (options?: DownloadOptions) =>
      writeOptionalFile(await operations.downloadEvents(http, id), options?.to)
  };
}

function sessionOtel(http: HttpClient, id: string): SessionOtel {
  return {
    traces: () => operations.iterateSessionOtlpPages(http, id, "traces") as AsyncIterable<OtlpExportTraceServiceRequest>,
    logs: () => operations.iterateSessionOtlpPages(http, id, "logs") as AsyncIterable<OtlpExportLogsServiceRequest>
  };
}

export interface StreamEventsOptions {
  /** Starting cursor; only events with `sequence >= from` are considered. Default 0. */
  readonly from?: number;
  /** Poll interval in ms for the event snapshot loop. Default 1000. */
  readonly intervalMs?: number;
  readonly signal?: AbortSignal;
}

export interface IterateEventsOptions {
  /** Number of durable events requested per API page. Default and maximum: 1000. */
  readonly pageSize?: number;
  /** Stop before requesting another page, or abort the active page request. */
  readonly signal?: AbortSignal;
}

function validateStreamEventsFrom(value: number | undefined): number {
  if (value === undefined) return 0;
  if (!Number.isSafeInteger(value) || value < 0) {
    throw configError(
      "session.events.stream",
      "from",
      "from must be a non-negative safe integer"
    );
  }
  return value;
}

export interface StreamEnvelopesOptions {
  /** Starting cursor — events with `sequence >= from` are delivered. Default 0. */
  readonly from?: number;
  readonly signal?: AbortSignal;
  /**
   * Half-open watchdog window passed to the coordinator stream. If no frame
   * arrives within this many ms, the SDK reconnects and resumes from the
   * current cursor. Default 45s. Set 0 to disable.
   */
  readonly idleTimeoutMs?: number;
  /**
   * Client keep-alive ping cadence passed to the coordinator stream. Default
   * 15s. Set 0 to disable.
   */
  readonly pingIntervalMs?: number;
  /**
   * Event-quiet replay self-heal window passed to the coordinator stream. If no
   * real event frame arrives within this many ms, the SDK reconnects and replays
   * from the current cursor. Default 90s. Set 0 to disable.
   */
  readonly eventQuietRecheckMs?: number;
}

export type SessionFilePathMatch = "exact" | "suffix";

export interface SessionFilePathSelector {
  readonly path: string;
  readonly match?: SessionFilePathMatch;
}

export interface SessionFileIdSelector {
  readonly id: string;
  readonly checkpointId: string;
}

export type SessionFileSelector = SessionFile | SessionFileIdSelector | SessionFilePathSelector;

export type SessionFileLinkSelector = SessionFileSelector | SessionFilesQuery;

export interface DownloadOptions {
  readonly to?: string;
  /** Immutable checkpoint to read. */
  readonly checkpointId?: string;
  /**
   * Per-attempt timeout for fetching and reading the selected file body.
   * Defaults to 30_000ms; idempotent file downloads retry once on timeout.
   */
  readonly timeoutMs?: number;
}

/**
 * Workspace secret management exposed under `client.workspace.secrets`.
 *
 * Lifecycle parity with assets: a `Secret.value(...)` is per-session and
 * gone at terminal; `set` persists a named, searchable workspace secret you can `get` (metadata),
 * `rotate`, `list`, and `delete`. The identity is the `name`; the value rotates
 * under that stable name.
 *
 * Values are write-only through the public SDK: `set`/`rotate` send the value in
 * the request BODY (never the URL); `get`/`list` return metadata only.
 */
export class SecretsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  /** Create a named workspace secret. Accepts a raw string or a `SecretString`. */
  set(args: { readonly name: string; readonly value: string | SecretString }): Promise<SecretRecord> {
    return operations.createSecret(this.#http, { name: args.name, value: unwrapSecretValue(args.value) });
  }

  /** List workspace secret metadata (searchable by name). Never returns values. */
  list(): Promise<readonly SecretRecord[]> {
    return operations.listSecrets(this.#http);
  }

  /** Metadata for one workspace secret by name. Never returns the value. */
  get(name: string): Promise<SecretRecord> {
    return operations.getSecret(this.#http, name);
  }

  /** Replace the value of an existing workspace secret; bumps its version. */
  rotate(args: { readonly name: string; readonly value: string | SecretString }): Promise<SecretRecord> {
    return operations.rotateSecret(this.#http, { name: args.name, value: unwrapSecretValue(args.value) });
  }

  delete(name: string): Promise<void> {
    return operations.deleteSecret(this.#http, name);
  }
}

/** Accept a raw string or a `SecretString`; return the raw value for the wire. */
function unwrapSecretValue(value: string | SecretString): string {
  const raw = value instanceof SecretString ? value.unwrap() : value;
  if (typeof raw !== "string" || !raw) {
    throw new Error("secrets: value must be a non-empty string");
  }
  return raw;
}

/**
 * The one-time reveal of a newly created workspace + its first workspace-scoped
 * API key. The key is shown exactly once at creation; it is wrapped in a
 * redacted {@link SecretString} so it never lands in a log via string coercion —
 * call `apiKey.unwrap()` to read the raw value (e.g. to construct a new `Aex`
 * client bound to the workspace).
 */
export interface NewWorkspaceResult {
  readonly workspaceId: string;
  /** The workspace-scoped API key, revealed ONCE. Redacted on `toString`/JSON. */
  readonly apiKey: SecretString;
  readonly slug?: string;
  readonly orgId?: string;
}

/** The one-time reveal of a newly minted API key (workspace key or account PAT). */
export interface NewApiKeyResult {
  readonly id: string;
  /** The freshly minted key value, revealed ONCE. Redacted on `toString`/JSON. */
  readonly apiKey: SecretString;
  readonly name?: string;
  readonly kind?: string;
  readonly workspaceId?: string;
  readonly scopes?: readonly string[];
}

function wrapNewWorkspace(wire: NewWorkspace): NewWorkspaceResult {
  return {
    workspaceId: wire.workspaceId,
    apiKey: new SecretString(wire.apiKey, "workspace api key"),
    ...(typeof wire.slug === "string" ? { slug: wire.slug } : {}),
    ...(typeof wire.orgId === "string" ? { orgId: wire.orgId } : {})
  };
}

function wrapNewApiKey(wire: NewApiKey): NewApiKeyResult {
  return {
    id: wire.id,
    apiKey: new SecretString(wire.apiKey, "api key"),
    ...(typeof wire.name === "string" ? { name: wire.name } : {}),
    ...(typeof wire.kind === "string" ? { kind: wire.kind } : {}),
    ...(typeof wire.workspaceId === "string" ? { workspaceId: wire.workspaceId } : {}),
    ...(Array.isArray(wire.scopes) ? { scopes: wire.scopes as readonly string[] } : {})
  };
}

/**
 * Control-plane management of the ORGS the account principal belongs to. Reached
 * with an account credential (PAT / device session), NOT a data-plane workspace
 * key. An org owns workspaces and is the billing / roles / cap boundary.
 *
 * Naming: `client.orgs` (plural) is a collection across the account, mirroring
 * `client.sessions` / `client.workspaces`.
 */
export class OrgsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  /** Create an org; the caller becomes its admin. */
  create(args: CreateOrgRequest): Promise<OrgRecord> {
    return operations.createOrg(this.#http, args);
  }

  /** List the orgs the caller belongs to. */
  list(): Promise<readonly OrgRecord[]> {
    return operations.listOrgs(this.#http);
  }

  /** List an org's members (pending invites appear with `status: "pending"`). */
  members(orgId: string): Promise<readonly OrgMemberRecord[]> {
    return operations.listOrgMembers(this.#http, orgId);
  }

  /** Invite an email to the org at a role (`member` by default). */
  invite(orgId: string, args: CreateOrgInviteRequest): Promise<OrgInvite> {
    return operations.createOrgInvite(this.#http, orgId, args);
  }
}

/**
 * Control-plane management of WORKSPACES across the account's orgs. This is the
 * PLURAL collection (`client.workspaces`) — distinct from the SINGULAR
 * `client.workspace`, which is the data-plane context bound to the current key
 * (its files/skills/tools/instructions/secrets). Creating a workspace returns
 * its first workspace-scoped key ONCE.
 */
export class WorkspacesClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  /**
   * Create a workspace under an org and reveal its first workspace-scoped API
   * key ONCE (wrapped in a redacted {@link SecretString}). The free tier caps at
   * 3 workspaces per org.
   */
  async create(args: CreateWorkspaceRequest): Promise<NewWorkspaceResult> {
    return wrapNewWorkspace(await operations.createWorkspace(this.#http, args));
  }

  /** List the workspaces the caller can manage across their orgs. */
  list(): Promise<readonly WorkspaceRecord[]> {
    return operations.listWorkspaces(this.#http);
  }

  /** Delete a workspace by id. Idempotent. */
  delete(workspaceId: string): Promise<void> {
    return operations.deleteWorkspace(this.#http, workspaceId);
  }
}

/**
 * Control-plane management of API KEYS — both data-plane workspace keys and
 * account PATs. Creating a key reveals its value ONCE (wrapped in a redacted
 * {@link SecretString}); list/delete operate on metadata only.
 */
export class KeysClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  /**
   * Mint an API key and reveal its value ONCE. Pass `workspaceId` for a
   * data-plane workspace key, or `account: true` for an account PAT. A PAT
   * cannot mint another PAT (anti-escalation, enforced server-side).
   */
  async create(args: CreateApiKeyRequest = {}): Promise<NewApiKeyResult> {
    return wrapNewApiKey(await operations.createApiKey(this.#http, args));
  }

  /** List API keys (metadata only; never values). */
  list(): Promise<readonly ApiKeyRecord[]> {
    return operations.listApiKeys(this.#http);
  }

  /** Revoke/delete an API key by id. Idempotent. */
  delete(keyId: string): Promise<void> {
    return operations.deleteApiKey(this.#http, keyId);
  }
}

/**
 * Unified user-facing client for aex. The same class powers the published
 * `@aexhq/sdk` SDK and, under the hood, the bundled `aex` CLI. All remote
 * operations use the hosted aex API and operate on durable session records.
 *
 * The SDK never asks the caller for a workspace id — workspace identity
 * is derived server-side from the API key on every request. Use
 * `client.whoami()` if you want to introspect which workspace the
 * token resolves to.
 */
export class Aex {
  readonly #http: HttpClient;
  readonly #debug: DebugSink | undefined;
  /** The same fetch the HttpClient uses, threaded into direct asset uploads. */
  readonly #fetch: FetchLike | undefined;
  readonly #assetRetry: AssetUploadRetryOptions;
  /**
   * The SINGULAR data-plane context bound to the current key: this workspace's
   * files, skills, tools, instructions, and secrets. (Contrast {@link workspaces},
   * the plural control-plane collection.)
   */
  readonly workspace: WorkspaceClient;
  readonly sessions: SessionClient;
  /** Control-plane: the orgs the account principal belongs to (roles/members/invites). */
  readonly orgs: OrgsClient;
  /**
   * Control-plane: manage WORKSPACES across your orgs (create/list/delete). The
   * PLURAL collection — not to be confused with {@link workspace} (singular), the
   * data-plane context of the current key.
   */
  readonly workspaces: WorkspacesClient;
  /** Control-plane: manage API keys — workspace keys and account PATs (create/list/delete). */
  readonly keys: KeysClient;

  constructor(apiKey: string, options?: Omit<AexOptions, "apiKey">);
  constructor(options: AexOptions);
  constructor(options: string | AexOptions, overrides: Omit<AexOptions, "apiKey"> = {}) {
    const resolved = typeof options === "string" ? { ...overrides, apiKey: options } : options;
    const apiKey = resolved.apiKey;
    if (!apiKey) {
      // Typed so a caller catching AexError (the SDK's error base) catches a
      // missing credential too, instead of a bare Error slipping the taxonomy.
      throw new CredentialValidationError("Aex: apiKey is required");
    }
    // Self-describing key ⇒ plane-aware routing, checked ZERO-network in the
    // constructor: derive the baseUrl from the key's plane when omitted, and
    // fail fast on a plane/baseUrl mismatch instead of a bare 401 after a full
    // round-trip. A non-self-describing key (no `aex_` shape) skips this and keeps
    // the HttpClient default.
    const baseUrl = resolveBaseUrlForKey(apiKey, resolved.baseUrl);
    // The transport applies the ONE shared retry policy — `HTTP_RETRY_POLICY`
    // from `@aexhq/contracts`, the same object the `aex` CLI hands its own
    // HttpClient — so an `aex` command and an SDK call react identically to a
    // 429. The raw `#fetch` below stays unwrapped for object-storage traffic.
    // Asset uploads apply the same resolved policy in their transfer helper;
    // `session.files.fetch()` intentionally returns the raw one-shot response.
    const baseFetch: FetchLike = resolved.fetch ?? ((input: Parameters<FetchLike>[0], init: Parameters<FetchLike>[1]) => fetch(input, init));
    this.#debug = resolved.debug
      ? typeof resolved.debug === "function"
        ? resolved.debug
        : (line: string) => console.error(line)
      : undefined;
    this.#assetRetry = resolved.retry === false
      ? { maxAttempts: 1 }
      : resolveRetryConfig(resolved.retry);
    this.#http = new HttpClient({
      ...(baseUrl ? { baseUrl } : {}),
      apiKey,
      fetch: baseFetch,
      retry: resolved.retry === false ? false : resolveRetryConfig(resolved.retry),
      // Opt-in local diagnostics: emit a redacted per-request trace to
      // stderr. Uploads nothing. A caller wanting a custom sink can pass
      // a function instead of `true`.
      ...(this.#debug ? { debug: this.#debug } : {})
    });
    this.#fetch = resolved.fetch;
    this.workspace = new WorkspaceClient(this.#http, {
      upload: (args) => this.#uploadAsset(args),
      uploadStream: (args) => this.#uploadAssetStream(args)
    });
    this.sessions = new SessionClient(this.#http, (options) => this.#buildSessionCreateRequest(options), this.#fetch);
    this.orgs = new OrgsClient(this.#http);
    this.workspaces = new WorkspacesClient(this.#http);
    this.keys = new KeysClient(this.#http);
  }

  /**
   * Internal: materialize raw bytes to the content-addressable asset store
   * (`/api/assets/presign` -> PUT -> `/api/assets/finalize`). Used by the session-create
   * prepare step to upload draft skill / tool / instructions / file bundles so
   * the wire submission carries only plain `kind:"asset"` refs (skills resolve
   * to name-only `kind:"skill"` refs after their bytes upload).
   * NOT part of the public API.
   */
  async #uploadAsset(args: {
    readonly bytes: Uint8Array;
    readonly hash: string;
    readonly contentType?: string;
  }): Promise<UploadedAsset> {
    return uploadAsset({
      http: this.#http,
      bytes: args.bytes,
      hash: args.hash,
      ...(args.contentType ? { contentType: args.contentType } : {}),
      retry: this.#assetRetry,
      ...(this.#fetch ? { fetch: this.#fetch as unknown as AssetFetch } : {})
    });
  }

  /**
   * Internal: materialize a LARGE draft (a `File.fromPath` over the streaming
   * threshold) to the content store via the two-pass streaming multipart flow —
   * hash the deterministic canonical-zip stream, presign by hash (dedup still
   * short-circuits), then upload it in parts. Bounded memory (one entry + one
   * part). NOT part of the public API.
   */
  async #uploadAssetStream(args: {
    readonly drive: ZipStreamDriver;
    readonly contentType?: string;
  }): Promise<UploadedAsset> {
    return uploadAssetMultipart({
      http: this.#http,
      drive: args.drive,
      ...(args.contentType ? { contentType: args.contentType } : {}),
      retry: this.#assetRetry,
      ...(this.#fetch ? { fetch: this.#fetch as unknown as AssetFetch } : {})
    });
  }

  /**
   * Convenience one-shot on top of the canonical session API:
   * open a session, send `message` as the first run, stream through its durable
   * terminal event, then return the collected text,
   * events, files, and session record. The returned `sessionId` is the session id,
   * so callers can resume later with `aex.sessions.open(sessionId)`.
   */
  async start<T = unknown>(options: SessionStartOptions, opts: StartSessionOptions = {}): Promise<SessionResult<T>> {
    if (!options || typeof options !== "object" || Array.isArray(options)) {
      throw configError("Aex.start", "options", "options are required");
    }
    assertStartSessionOptions(opts, "Aex.start");
    const scopedSignal = scopedAbortSignal(opts.timeoutMs);
    try {
      const { message, deleteAfter, messageIdempotencyKey, stream, ...createOptions } = options;
      assertSupportedSessionFields(options, "Aex.start", true);
      const input = normaliseSessionInput(message, "Aex.start", "message");
      assertSupportedSessionSendOptions(stream, "Aex.start stream", false);
      const sendOptions: InternalSessionSendOptions = {
        ...(stream ?? {}),
        ...(scopedSignal?.signal ? { signal: scopedSignal.signal } : {}),
        ...(opts.webSocketFactory ? { webSocketFactory: opts.webSocketFactory } : {}),
        ...(opts.idleTimeoutMs !== undefined ? { idleTimeoutMs: opts.idleTimeoutMs } : {}),
        ...(opts.pingIntervalMs !== undefined ? { pingIntervalMs: opts.pingIntervalMs } : {})
      };
      // Derive the message key from the create key (like the CLI) so a retried
      // session with the same `idempotencyKey` de-duplicates BOTH the create and the
      // billable turn server-side — never a duplicate billable session turn (sdk-dx-3).
      const createKey = operations.resolveIdempotencyKey(createOptions.idempotencyKey);
      const messageKey =
        messageIdempotencyKey !== undefined
          ? operations.resolveIdempotencyKey(messageIdempotencyKey)
          : operations.deriveMessageIdempotencyKey(createKey);
      const session = await this.sessions.create({ ...createOptions, idempotencyKey: createKey });
      // One terminal boundary: RUN_FINISHED/RUN_ERROR carries the run outcome,
      // cost, and usage. `start()` only reshapes the same finished result.
      let turnResult: SessionRunResult;
      try {
        turnResult = await sendSessionInternal(session, input, { ...sendOptions, idempotencyKey: messageKey }).finished();
      } catch (err) {
        if (scopedSignal?.signal.aborted) {
          // The client-side wait budget expired. Throw rather than returning a
          // misleading partial result; the session continues server-side.
          throw new SessionStateError(
            `Aex.start: timed out after ${opts.timeoutMs}ms waiting for run completion in session ${session.id}; the session ` +
              `continues server-side — cancel via session.cancel() or reopen with aex.sessions.open(${JSON.stringify(session.id)})`
          );
        }
        throw err;
      }
      const sessionId = turnResult.sessionId;
      if (deleteAfter) {
        await session.delete();
      }
      const sessionState = turnResult.session;
      const trace = turnTraceFromEvents(turnResult.events);
      const outcome = turnResult.outcome as TurnOutcome<T> | undefined;
      const { outcome: _untypedOutcome, ...baseResult } = turnResult;
      const result: SessionResult<T> = {
        ...baseResult,
        trace,
        ...(outcome !== undefined ? { outcome } : {})
      };
      if (opts.throwOnFailure && !turnResult.ok) {
        // A turn that failed because the upstream provider throttled us surfaces
        // as a structured, non-leaky AexRateLimitError carrying the provider
        // fault, so callers can branch on `isRateLimited(err)` and replay.
        const throttle = throttleFromSession(sessionState, this.#debug);
        if (throttle) {
          throw new AexRateLimitError({
            status: throttle.status ?? 429,
            attempts: 1,
            source: "provider",
            providerFault: throttle,
            ...(throttle.retryAfterMs !== undefined ? { retryAfterMs: throttle.retryAfterMs } : {})
          });
        }
        throw new SessionStateError(
          `Aex.start: session ${sessionId} ended ${turnResult.status}${turnResult.error ? `: ${turnResult.error}` : ""}`,
          { sessionId, status: turnResult.status }
        );
      }
      return result;
    } finally {
      scopedSignal?.clear();
    }
  }

  async #buildSessionCreateRequest(options: SessionCreateOptions): Promise<SessionCreateRequest> {
    if (!options || typeof options !== "object" || Array.isArray(options)) {
      throw configError("aex.sessions.create", "options", "options are required");
    }
    assertSupportedSessionFields(options, "aex.sessions.create", false);
    // Model is REQUIRED and validated as a gateway `creator/model` slug string.
    // This is a structural boundary gate only — a well-formed slug the gateway
    // catalog doesn't yet serve is allowed through (the server arbitrates), so a
    // slightly-old SDK can still run a newly-launched model.
    if (typeof options.model !== "string" || !options.model) {
      throw configError("aex.sessions.create", "model", "model is required");
    }
    const selectedModel = options.model;
    validatedSessionConfig(
      "aex.sessions.create",
      "model",
      "model must be a gateway model slug of the form \"creator/model\"",
      () => parseModelSlug(selectedModel),
      { kind: "scalar", rejectedValues: [selectedModel] }
    );
    // Fast client-side validation via the contract parsers (the SSoT). runtimeSize
    // and timeout are STABLE closed sets whose invalid values the create endpoint
    // otherwise SILENTLY defaults (no error ever — pre-launch edge-sweep F11/F12);
    // reject them synchronously with a typed error instead. webhook shape is
    // re-checked here for a fast local fail (the server enforces it too). Model is
    // deliberately NOT hard-rejected here to preserve forward-compat with models
    // added server-side before an SDK upgrade (an unknown model still fails on the
    // server).
    const runtimeSize = options.runtime?.size;
    validatedSessionConfig(
      "aex.sessions.create",
      "runtime.size",
      "runtime.size must be a supported size preset",
      () => parseRuntimeSize(runtimeSize),
      { kind: "scalar", rejectedValues: [runtimeSize] }
    );
    const runtimeKind = options.runtime?.kind;
    validatedSessionConfig(
      "aex.sessions.create",
      "runtime.kind",
      "runtime.kind must be one of: container, spot_container, lambda",
      () => parseRuntimeKind(runtimeKind),
      { kind: "scalar", rejectedValues: [runtimeKind] }
    );
    const sessionTimeout = options.overrides?.timeout;
    validatedSessionConfig(
      "aex.sessions.create",
      "overrides.timeout",
      "overrides.timeout must be a supported duration",
      () => parseSessionTimeout(sessionTimeout),
      { kind: "scalar", rejectedValues: [sessionTimeout] }
    );
    if (options.webhook !== undefined) {
      validatedSessionConfig(
        "aex.sessions.create",
        "webhook.url",
        "webhook.url must be a valid HTTPS URL",
        () => parseSessionWebhook(options.webhook),
        { kind: "redacted" }
      );
    }
    if (options.responseFormat !== undefined) {
      validatedSessionConfig(
        "aex.sessions.create",
        "responseFormat",
        "responseFormat is invalid",
        () => parseResponseFormat(options.responseFormat),
        { kind: "redacted" }
      );
    }
    if (options.approvalGate !== undefined) {
      validatedSessionConfig(
        "aex.sessions.create",
        "approvalGate",
        "approvalGate is invalid",
        () => parseApprovalGate(options.approvalGate),
        { kind: "redacted" }
      );
    }
    const splitEnvironmentSecrets = validatedSessionConfig(
      "aex.sessions.create",
      "environment.secrets",
      "environment.secrets is invalid",
      () => splitSecretEnv(options.environment?.secrets),
      { kind: "redacted" }
    );
    const secretEnvDeclarations = splitEnvironmentSecrets.declarations;
    const envSecretValues = splitEnvironmentSecrets.values;

    let limits: SessionLimits | undefined;
    const limitsInput: { maxSpendUsd?: number; maxTurns?: number } = {};
    if (options.overrides?.maxSpendUsd !== undefined) {
      const maxSpendUsd = options.overrides.maxSpendUsd;
      validatedSessionConfig(
        "aex.sessions.create",
        "overrides.maxSpendUsd",
        "overrides.maxSpendUsd must be valid",
        () => parseSessionLimits({ maxSpendUsd }),
        { kind: "scalar", rejectedValues: [maxSpendUsd] }
      );
      limitsInput.maxSpendUsd = maxSpendUsd;
    }
    if (options.overrides?.maxTurns !== undefined) {
      const maxTurns = options.overrides.maxTurns;
      validatedSessionConfig(
        "aex.sessions.create",
        "overrides.maxTurns",
        "overrides.maxTurns must be valid",
        () => parseSessionLimits({ maxTurns }),
        { kind: "scalar", rejectedValues: [maxTurns] }
      );
      limitsInput.maxTurns = maxTurns;
    }
    limits = parseSessionLimits(Object.keys(limitsInput).length > 0 ? limitsInput : undefined);

    const { submissionMcpServers, mergedMcpSecrets } = mergeMcpServers(
      options.mcpServers ?? [],
      []
    );
    const fileCapture: PlatformSubmission["fileCapture"] | undefined = validatedSessionConfig(
      "aex.sessions.create",
      "fileCapture",
      "fileCapture is invalid",
      () => fileCaptureForWire(options.fileCapture),
      { kind: "redacted" }
    );
    const environment: PlatformEnvironmentInput | undefined = validatedSessionConfig(
      "aex.sessions.create",
      "environment",
      "environment is invalid",
      () => sessionEnvironmentForWire(options.environment),
      { kind: "redacted" }
    );
    let builtinTools = options.builtinTools ?? "default";
    if (Array.isArray(builtinTools)) {
      const selectedBuiltinTools = builtinTools;
      builtinTools = validatedSessionConfig(
        "aex.sessions.create",
        "builtinTools",
        "builtinTools contains an unsupported tool name",
        () => resolveBuiltinToolNames(selectedBuiltinTools),
        { kind: "scalar", rejectedValues: selectedBuiltinTools }
      );
    }

    const submission: SessionCreateRequest["submission"] = {
      model: options.model,
      ...(options.system ? { system: options.system } : {}),
      assets: {
        files: options.assets?.files ?? [],
        skills: options.assets?.skills ?? [],
        tools: options.assets?.tools ?? [],
        instructions: options.assets?.instructions ?? []
      },
      builtinTools,
      mcpServers: submissionMcpServers as readonly McpServerRef[],
      ...(Object.keys(secretEnvDeclarations).length > 0 ? { secretEnv: secretEnvDeclarations } : {}),
      ...(environment ? { environment: environment as NonNullable<PlatformSubmission["environment"]> } : {}),
      ...(options.metadata ? { metadata: options.metadata } : {}),
      ...(fileCapture ? { fileCapture } : {}),
      ...(options.outputMode !== undefined ? { outputMode: options.outputMode } : {}),
      ...(options.responseFormat !== undefined ? { responseFormat: options.responseFormat } : {}),
      ...(options.approvalGate !== undefined ? { approvalGate: options.approvalGate } : {})
    };

    const secrets: PlatformInlineSecrets = {
      ...(mergedMcpSecrets.length > 0 ? { mcpServers: mergedMcpSecrets } : {}),
      ...(Object.keys(envSecretValues).length > 0 ? { envSecrets: envSecretValues } : {})
    };

    const retention = sessionRetentionForWire(options);

    return {
      submission,
      ...(options.runtime?.size ? { runtimeSize: options.runtime.size } : {}),
      ...(options.runtime?.kind ? { runtimeKind: options.runtime.kind } : {}),
      ...(options.overrides?.timeout ? { timeout: options.overrides.timeout } : {}),
      ...(limits ? { limits } : {}),
      retention,
      // Operational/delivery concern — sibling of secrets, NOT part of the
      // hashed submission. Delivered at the committed RUN terminal.
      ...(options.webhook ? { webhook: options.webhook } : {}),
      secrets
    };
  }

  whoami(): Promise<WhoAmI> {
    return operations.whoami(this.#http);
  }

  /**
   * Read the workspace billing summary: prepaid `balanceUsd`, current-month
   * `monthSpendUsd`, the enforced `spendCapUsd`, this period's free
   * `allowances`, the `autoTopup` settings and the saved `paymentMethod`. Backed
   * by `GET /api/billing` (scope `billing:read`).
   */
  billing(): Promise<BillingSummary> {
    return operations.getBilling(this.#http);
  }

  /**
   * Buy prepaid credit through hosted checkout. Open the returned `url`; the
   * same flow saves the card on first use, and the balance moves once the charge
   * settles. An amount below `billing().autoTopup.minimumAmountUsd` is refused.
   */
  billingTopup(request: BillingTopupCheckoutRequest, options?: IdempotencyOptions): Promise<BillingHostedSession> {
    return operations.createBillingTopupCheckout(this.#http, request, options);
  }

  /**
   * Set auto-recharge. OFF by default, and a saved card does not enable it —
   * that is the difference between consenting to one charge and granting a
   * standing authority. Omitted fields keep their stored value; enabling needs a
   * saved card and `thresholdUsd` must stay strictly below `amountUsd`.
   */
  billingAutoTopup(request: BillingAutoTopupRequest): Promise<BillingAutoTopupUpdate> {
    return operations.updateBillingAutoTopup(this.#http, request);
  }

  /**
   * Create a hosted billing-portal session for the workspace customer.
   * Open the returned `url` in a browser.
   */
  billingPortal(request: BillingPortalRequest = {}, options?: IdempotencyOptions): Promise<BillingHostedSession> {
    return operations.createBillingPortal(this.#http, request, options);
  }

  /**
   * Read recent workspace credit-ledger rows, newest first — top-ups, run
   * charges, and redemptions with signed `amountUsd`. Backed by
   * `GET /api/billing/ledger`; `limit` is clamped server-side to [1, 100]
   * (default 25). Not cursor-paged.
   */
  billingLedger(query?: BillingLedgerQuery): Promise<BillingLedgerPage> {
    return operations.getBillingLedger(this.#http, query);
  }

  /**
   * Reveal the workspace webhook signing secret (creating one on first use) —
   * the `whsec_<base64>` value `verifyAexWebhook` takes as `secret`. Backed by
   * `POST /api/webhook/signing-secret`; repeat calls return the SAME value (the
   * hosted API does not rotate it). Treat the reveal as sensitive: never log it.
   */
  webhookSigningSecret(): Promise<WebhookSigningSecret> {
    return operations.getWebhookSigningSecret(this.#http);
  }
}

/**
 * Resolve the effective `baseUrl` from a self-describing API key (ZERO-network):
 *   - non-self-describing key: return the caller's `baseUrl`
 *     unchanged (HttpClient falls back to the prd default).
 *   - `baseUrl` omitted: DERIVE it from the key's plane.
 *   - `baseUrl` supplied but its plane DISAGREES with the key's plane: throw
 *     {@link CredentialValidationError} BEFORE any request.
 */
function resolveBaseUrlForKey(apiKey: string, baseUrl: string | undefined): string | undefined {
  const parsed = tryParseApiKey(apiKey);
  if (parsed === null) return baseUrl;
  const planeUrl = PLANE_BASE_URLS[parsed.plane];
  if (baseUrl === undefined) {
    return planeUrl;
  }
  const canonicalPlane =
    baseUrl === PLANE_BASE_URLS.dev ? "dev" : baseUrl === PLANE_BASE_URLS.prd ? "prd" : undefined;
  if (canonicalPlane !== undefined && canonicalPlane !== parsed.plane) {
    throw new CredentialValidationError(
      `Aex: this API key is for the ${parsed.plane} plane but baseUrl targets the ${canonicalPlane} plane (${baseUrl}) — ` +
        `pass the ${parsed.plane} plane baseUrl, or omit baseUrl to auto-route.`,
      { plane: parsed.plane, baseUrl }
    );
  }
  return baseUrl;
}

function scopedAbortSignal(timeoutMs: number | undefined): { readonly signal: AbortSignal; clear(): void } | undefined {
  if (timeoutMs === undefined) {
    return undefined;
  }
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), Math.max(0, timeoutMs));
  return {
    signal: controller.signal,
    clear() {
      clearTimeout(timer);
    }
  };
}

async function writeOptionalFile(bytes: Uint8Array, to?: string): Promise<Uint8Array> {
  if (to !== undefined) {
    const { writeFile } = await import("node:fs/promises");
    await writeFile(to, bytes);
  }
  return bytes;
}

function sleep(ms: number, signal: AbortSignal | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new Error("aborted"));
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(timer);
      reject(new Error("aborted"));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

/**
 * Extract a throttle-class {@link ProviderFault} from a failed session record.
 * Reads the contract-owned structured field. Only when that field is absent,
 * the narrow historical adapter may recognize an exact legacy terminal.
 */
function throttleFromSession(session: Session, debug?: DebugSink): ProviderFault | undefined {
  const fault = session.providerFault ?? legacySessionProviderFault(session, debug);
  return fault && isThrottleFault(fault) ? fault : undefined;
}

type WorkspaceAssetPublisher = {
  upload(args: { readonly bytes: Uint8Array; readonly hash: string; readonly contentType: string }): Promise<UploadedAsset>;
  uploadStream(args: { readonly drive: ZipStreamDriver; readonly contentType: string }): Promise<UploadedAsset>;
};

export class WorkspaceFilesClient {
  constructor(private readonly http: HttpClient, private readonly publisher: WorkspaceAssetPublisher) {}

  async publish(file: File): Promise<WorkspaceFileRecord> {
    const bundle = file._takeDraftBundle();
    const stream = file._takeDraftStream();
    if (!bundle && !stream) throw new Error("workspace.files.publish requires a draft File");
    const uploaded = bundle
      ? await this.publisher.upload({ bytes: bundle.bytes, hash: bundle.contentHash, contentType: "application/zip" })
      : await this.publisher.uploadStream({ drive: stream!.drive, contentType: "application/zip" });
    return operations.publishWorkspaceFile(this.http, {
      assetId: uploaded.assetId,
      contentHash: uploaded.contentHash,
      sizeBytes: uploaded.sizeBytes,
      contentType: uploaded.contentType,
      name: bundle?.name ?? stream!.name,
      mountPath: bundle?.mountPath ?? stream!.mountPath
    });
  }

  list(query?: WorkspaceResourceListQuery): Promise<WorkspaceResourcePage<WorkspaceFileRecord>> {
    return operations.listWorkspaceFiles(this.http, query);
  }
  get(resourceId: string, version?: number): Promise<WorkspaceFileRecord> { return operations.getWorkspaceFile(this.http, resourceId, version); }
  delete(resourceId: string): Promise<void> { return operations.deleteWorkspaceFile(this.http, resourceId); }
}

export class WorkspaceSkillsClient {
  constructor(private readonly http: HttpClient, private readonly publisher: WorkspaceAssetPublisher) {}

  async publish(skill: Skill): Promise<WorkspaceSkillRecord> {
    const bundle = skill._takeDraftBundle();
    if (!bundle) throw new Error("workspace.skills.publish requires a draft Skill");
    const uploaded = await this.publisher.upload({ bytes: bundle.bytes, hash: bundle.contentHash, contentType: "application/zip" });
    return operations.publishWorkspaceSkill(this.http, {
      assetId: uploaded.assetId,
      contentHash: uploaded.contentHash,
      sizeBytes: uploaded.sizeBytes,
      contentType: uploaded.contentType,
      name: bundle.name,
      description: bundle.description
    });
  }

  list(query?: WorkspaceResourceListQuery): Promise<WorkspaceResourcePage<WorkspaceSkillRecord>> {
    return operations.listWorkspaceSkills(this.http, query);
  }
  get(resourceId: string, version?: number): Promise<WorkspaceSkillRecord> { return operations.getWorkspaceSkill(this.http, resourceId, version); }
  delete(resourceId: string): Promise<void> { return operations.deleteWorkspaceSkill(this.http, resourceId); }
}

export class WorkspaceToolsClient {
  constructor(private readonly http: HttpClient, private readonly publisher: WorkspaceAssetPublisher) {}

  async publish(tool: Tool): Promise<WorkspaceToolRecord> {
    const bundle = tool._takeDraftBundle();
    if (!bundle) throw new Error("workspace.tools.publish requires a draft Tool");
    const uploaded = await this.publisher.upload({ bytes: bundle.bytes, hash: bundle.contentHash, contentType: "application/zip" });
    return operations.publishWorkspaceTool(this.http, {
      assetId: uploaded.assetId,
      contentHash: uploaded.contentHash,
      sizeBytes: uploaded.sizeBytes,
      contentType: uploaded.contentType,
      name: bundle.ref.name,
      description: bundle.ref.description,
      input_schema: bundle.ref.input_schema,
      entry: bundle.ref.entry
    });
  }

  list(query?: WorkspaceResourceListQuery): Promise<WorkspaceResourcePage<WorkspaceToolRecord>> {
    return operations.listWorkspaceTools(this.http, query);
  }
  get(resourceId: string, version?: number): Promise<WorkspaceToolRecord> { return operations.getWorkspaceTool(this.http, resourceId, version); }
  delete(resourceId: string): Promise<void> { return operations.deleteWorkspaceTool(this.http, resourceId); }
}

export class WorkspaceInstructionsClient {
  constructor(private readonly http: HttpClient, private readonly publisher: WorkspaceAssetPublisher) {}

  async publish(instructions: Instructions): Promise<WorkspaceInstructionRecord> {
    const bundle = instructions._takeDraftBundle();
    if (!bundle) throw new Error("workspace.instructions.publish requires draft instructions");
    const uploaded = await this.publisher.upload({ bytes: bundle.bytes, hash: bundle.contentHash, contentType: "application/zip" });
    return operations.publishWorkspaceInstruction(this.http, {
      assetId: uploaded.assetId,
      contentHash: uploaded.contentHash,
      sizeBytes: uploaded.sizeBytes,
      contentType: uploaded.contentType,
      name: bundle.name
    });
  }

  list(query?: WorkspaceResourceListQuery): Promise<WorkspaceResourcePage<WorkspaceInstructionRecord>> {
    return operations.listWorkspaceInstructions(this.http, query);
  }
  get(resourceId: string, version?: number): Promise<WorkspaceInstructionRecord> { return operations.getWorkspaceInstruction(this.http, resourceId, version); }
  delete(resourceId: string): Promise<void> { return operations.deleteWorkspaceInstruction(this.http, resourceId); }
}

export class WorkspaceClient {
  readonly files: WorkspaceFilesClient;
  readonly skills: WorkspaceSkillsClient;
  readonly tools: WorkspaceToolsClient;
  readonly instructions: WorkspaceInstructionsClient;
  readonly secrets: SecretsClient;

  constructor(http: HttpClient, publisher: WorkspaceAssetPublisher) {
    this.files = new WorkspaceFilesClient(http, publisher);
    this.skills = new WorkspaceSkillsClient(http, publisher);
    this.tools = new WorkspaceToolsClient(http, publisher);
    this.instructions = new WorkspaceInstructionsClient(http, publisher);
    this.secrets = new SecretsClient(http);
  }
}

export type { SessionFileType, SessionFileLink, SessionFileLinkOptions, SessionFileQuery };
