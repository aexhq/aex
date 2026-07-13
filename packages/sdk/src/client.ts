import {
  CredentialValidationError,
  DEFAULT_PROVIDER,
  HttpClient,
  PLANE_BASE_URLS,
  SessionConfigValidationError,
  SessionStateError,
  SecretString,
  asAexEventView,
  asAexStreamEventView,
  customName,
  isReplayableEvent,
  assertStreamableOutputMode,
  parseApiKey,
  resolveModelProvider,
  resolveBuiltinToolNames,
  streamCoordinatorEvents,
  usageFromProviderUsage,
  type AexEvent,
  type AexEventView,
  type AexStreamEvent,
  type AexStreamEventView,
  type ApprovalGate,
  type BillingCheckoutRequest,
  type BillingHostedSession,
  type BillingLedgerPage,
  type BillingLedgerQuery,
  type BillingPortalRequest,
  type BillingSummary,
  type ChildSessionRef,
  type DebugSink,
  type FetchLike,
  type McpServerRef,
  type SessionFile,
  type SessionFileType,
  type SessionFileLink,
  type SessionFileLinkOptions,
  type SessionFileQuery,
  type SessionFilesQuery,
  type SessionFilesSnapshot,
  type SessionCheckpointRevision,
  type SessionFileText,
  type OutputMode,
  type ReadSessionFileTextOptions,
  type ResponseFormat,
  type SessionCostProviderUsage,
  type TurnOutcome,
  type Session,
  type SessionCreateRequest,
  type SessionListPage,
  type SessionListQuery,
  type SessionMessage,
  type SessionMessageAccepted,
  type SessionRetentionPolicy,
  type SessionStateChangeAccepted,
  type SessionStatus,
  type SessionRunOutcome,
  type SessionRun,
  type TurnResult,
  type PlatformEnvironmentInput,
  type PlatformSubmission,
  type PlatformInlineSecrets,
  type PlatformMcpServerSecret,
  type ModelName,
  type TurnTrace,
  type UsageSummary,
  type SessionLimits,
  parseSessionLimits,
  parseApprovalGate,
  parseResponseFormat,
  parseRuntimeSize,
  parseSessionTimeout,
  parseSessionWebhook,
  type SessionWebhookDelivery,
  type ProviderName,
  type SecretRecord,
  type BuiltinToolName,
  type RuntimeSize,
  type SubmissionAssets,
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
import { operations, type AssetUploadRetryOptions } from "@aexhq/contracts/internal";
import { Instructions } from "./instructions.js";
import { uploadAsset, uploadAssetMultipart, type AssetFetch, type UploadedAsset } from "./asset-upload.js";
import { File, type ZipStreamDriver } from "./file.js";
import { McpServer } from "./mcp-server.js";
import {
  AexRateLimitError,
  isThrottleFault,
  parseProviderFault,
  resolveRetryConfig,
  withRetry,
  type ProviderFault,
  type RetryOptions
} from "./retry.js";
import { Secret, splitSecretEnv } from "./secret.js";
import { Skill } from "./skill.js";
import { Tool } from "./tool.js";

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

/**
 * The unified finished result of {@link Aex.start}. Extends the contracts
 * {@link TurnResult} (the one shape `start()` and `finished()` share), so
 * the terminal `status` (a {@link SessionTerminalOutcome}), `ok`, `costUsd`
 * (`number`, `>= 0`), and `usage` are always present. RUN_FINISHED is emitted
 * only after the checkpoint commit. It is the same result returned by
 * `session.messages.send(...).finished()`, plus the decoded event trace.
 *
 * `T` is the `responseFormat` decode type: when the session was submitted with a
 * `json_schema` `responseFormat`, {@link outcome} carries the typed decoded
 * value or a typed refusal.
 */
export interface SessionResult<T = unknown> extends SessionRunResult<T> {
  /** Decoded view of the events: tool calls, usage, and assistant text. */
  readonly trace: TurnTrace;
}

/** Options for {@link Aex.start}. */
export interface StartSessionOptions {
  /** Overall wait budget (ms) for the one-shot run to finish. */
  readonly timeoutMs?: number;
  readonly webSocketFactory?: WebSocketFactory;
  readonly idleTimeoutMs?: number;
  readonly pingIntervalMs?: number;
  /** Throw a {@link SessionStateError} when the session does not succeed. Default false. */
  readonly throwOnFailure?: boolean;
}

export type SessionInput = string | readonly string[];

export interface SessionEnvironmentOptions extends Omit<PlatformEnvironmentInput, "envVars"> {
  readonly variables?: Readonly<Record<string, string>>;
  readonly secrets?: Readonly<Record<string, Secret>>;
}

export interface SessionOverrides {
  readonly idleTtl?: string;
  readonly timeout?: string;
  readonly maxSpendUsd?: number;
  /**
   * Per-session iteration cap (agent loop turns). Defaults + ceiling are enforced
   * server-side; omit to accept the platform default. A positive integer.
   */
  readonly maxTurns?: number;
}

/** Stable mutation identity used for server-side deduplication and safe transport retries. */
export interface IdempotencyOptions {
  readonly idempotencyKey?: string;
}

/**
 * Options for creating a resumable session or starting a one-shot run.
 * Reusable bytes are published first through `aex.workspace`, then pinned in
 * `assets` by resource id and immutable version.
 */
export interface SessionCreateOptions extends IdempotencyOptions {
  /**
   * Upstream provider selector. Prefer naming it explicitly with the
   * {@link Providers} symbol const, e.g. `provider: Providers.DEEPSEEK`. When
   * omitted it is derived from `model`; if supplied it MUST serve the model.
   */
  readonly provider?: ProviderName;
  /**
   * Closed public model id. Prefer the {@link Models} symbol const, e.g.
   * `Models.CLAUDE_HAIKU_4_5`.
   */
  readonly model: ModelName;
  readonly system?: string;
  /** Immutable workspace resources to materialize for the session. */
  readonly assets?: Partial<SubmissionAssets>;
  readonly mcpServers?: readonly McpServer[];
  /**
   * File capture policy for the session's captured files. Omit `allowedDirs`
   * to expose regular workspace files from the latest complete checkpoint;
   * listed roots narrow capture, and `deniedDirs` subtracts noise.
   */
  readonly fileCapture?: {
    readonly allowedDirs?: readonly string[];
    readonly deniedDirs?: readonly string[];
    readonly captureTimeoutMs?: number;
    readonly maxFileBytes?: number;
    readonly maxTotalBytes?: number;
    readonly maxFiles?: number;
  };
  /** Builtin capabilities, kept separate from uploaded custom-tool assets. */
  readonly builtinTools?: "default" | "none" | readonly BuiltinToolName[];
  /**
   * Assistant-output granularity. `"buffered"` (default) delivers ONE coalesced
   * `TEXT_MESSAGE_CONTENT` per assistant message. `"stream"` delivers provisional,
   * non-replayable per-token `TEXT_MESSAGE_CONTENT` deltas (`replayable:false`,
   * `liveSequence`, and no durable `sequence`) as they arrive. This is capability-gated:
   * `"stream"` is only honored for a streamable provider (submitting `"stream"`
   * against a non-streamable one is rejected at submit, never silently
   * downgraded). A coalesced final `TEXT_MESSAGE_CONTENT` ALWAYS follows the
   * deltas, so a buffered consumer sees the same final text either way; deltas
   * are provisional until that coalesced block.
   */
  readonly outputMode?: OutputMode;
  /**
   * Structured-output policy. `{ kind:'text' }` (default) is free-form;
   * `{ kind:'json_schema', schema, strict?, name? }` requests provider-native
   * constrained decode against `schema`. The typed outcome is read from
   * `start<T>()`'s `result.outcome` (`{ kind:'decoded', value }` or
   * `{ kind:'refused', reason }`) — never an untyped hallucinated object.
   */
  readonly responseFormat?: ResponseFormat;
  /**
   * Declarative HITL write-gate: the platform puts the session
   * `awaiting_approval` BEFORE dispatching any tool in `tools`, independent of
   * model prose. Resume with `session.approve()` or reject with
   * `session.deny()`.
   */
  readonly approvalGate?: ApprovalGate;
  readonly metadata?: PlatformSubmission["metadata"];
  /** BYOK provider key(s), keyed by provider. */
  readonly apiKeys?: Partial<Record<ProviderName, string>>;
  readonly environment?: SessionEnvironmentOptions;
  /**
   * Managed runtime size. One of the closed {@link RuntimeSize} preset tokens.
   * Prefer the {@link Sizes} symbol const.
   */
  readonly runtime?: RuntimeSize;
  readonly overrides?: SessionOverrides;
  /**
    * Optional callback URL registered on the session. The platform delivers a
    * run-scoped `run.finished` or `run.error` event after every run finalizes,
    * signed Standard-Webhooks style (verify with {@link verifyAexWebhook}). The
    * URL must be https.
   */
  readonly webhook?: { readonly url: string };
}

export interface SessionSendOptions extends IdempotencyOptions {
  readonly webSocketFactory?: WebSocketFactory;
  readonly idleTimeoutMs?: number;
  readonly pingIntervalMs?: number;
}

interface InternalSessionSendOptions extends SessionSendOptions {
  readonly signal?: AbortSignal;
}

interface InternalSessionRunStreamOptions extends InternalSessionSendOptions {
  readonly from: number;
}

export interface SessionStartOptions extends SessionCreateOptions {
  readonly message: SessionInput;
  readonly deleteAfter?: boolean;
  readonly messageIdempotencyKey?: string;
  readonly stream?: Omit<SessionSendOptions, "idempotencyKey">;
}

/**
 * The unified finished result of one run (`session.messages.send(...).finished()`). Extends
 * the contracts {@link TurnResult}, so `finished()` returns the SAME shape as
 * `start()`: the terminal `status` (a {@link SessionTerminalOutcome}), `ok`,
 * `costUsd`, and `usage` come from that run's terminal event and are always present.
 */
export interface SessionRunResult<T = unknown> extends TurnResult {
  readonly sessionId: string;
  readonly session: Session;
  readonly run: SessionRun;
  readonly text: string;
  readonly events: readonly AexEventView[];
  readonly files: readonly SessionFile[];
  /** Committed checkpoint for a finished run; absent when a run errors before one exists. */
  readonly checkpoint?: SessionCheckpointRevision;
  readonly messages: readonly Message[];
  /** The typed schema-decode outcome when a `json_schema` `responseFormat` was set. */
  readonly outcome?: TurnOutcome<T>;
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

export type Message = SessionMessage;

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

/**
 * Poll a child session's events until a committed RUN terminal is visible, the
 * signal aborts, or the caller breaks the iterator.
 */
async function* streamChildSessionEventsPolling(
  http: HttpClient,
  ref: ChildSessionRef,
  options: StreamEventsOptions
): AsyncIterable<AexEventView> {
  const id = ref.id;
  const from = validateStreamEventsFrom(options.from);
  if (options.signal?.aborted) return;
  const seenIds = new Set<string>();
  const intervalMs = options.intervalMs ?? 1_000;
  const signal = options.signal;
  const progressing = PROGRESSING_SESSION_STATUSES.has(ref.status);
  const targetRunId = progressing ? undefined : ref.lastRun?.runId;
  const priorRunId = progressing ? ref.lastRun?.runId : undefined;
  const boundedRunlessSnapshot = !progressing && targetRunId === undefined;
  while (!signal?.aborted) {
    const events = await operations.listSessionEvents(http, id);
    let terminalSeen = false;
    for (const event of events) {
      if (event.sequence >= from && !seenIds.has(event.id)) {
        seenIds.add(event.id);
        yield asAexEventView(event);
      }
      if (
        (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR") &&
        (targetRunId !== undefined
          ? event.runId === targetRunId
          : event.sequence >= from && event.runId !== priorRunId)
      ) {
        terminalSeen = true;
      }
    }
    if (terminalSeen || boundedRunlessSnapshot) return;
    try {
      await sleep(intervalMs, signal);
    } catch {
      return;
    }
  }
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
async function* streamSessionEventsPolling(
  http: HttpClient,
  id: string,
  options: StreamEventsOptions
): AsyncIterable<AexEventView> {
  const from = validateStreamEventsFrom(options.from);
  if (options.signal?.aborted) return;
  const seenIds = new Set<string>();
  const intervalMs = options.intervalMs ?? 1_000;
  const signal = options.signal;
  const initial = await operations.getSession(http, id);
  const targetRunId = initial.currentRun?.runId ?? initial.lastRun?.runId;
  const boundedRunlessSnapshot = targetRunId === undefined && !PROGRESSING_SESSION_STATUSES.has(initial.status);
  while (!signal?.aborted) {
    const events = await operations.listSessionEvents(http, id);
    let terminalSeen = false;
    for (const event of events) {
      if (event.sequence >= from && !seenIds.has(event.id)) {
        seenIds.add(event.id);
        yield asAexEventView(event);
      }
      if (
        (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR") &&
        (targetRunId === undefined ? event.sequence >= from : event.runId === targetRunId)
      ) {
        terminalSeen = true;
      }
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
    isTerminal: isSessionEnvelopeTerminal,
    ...(options.signal ? { signal: options.signal } : {}),
    ...(options.idleTimeoutMs !== undefined ? { idleTimeoutMs: options.idleTimeoutMs } : {}),
    ...(options.pingIntervalMs !== undefined ? { pingIntervalMs: options.pingIntervalMs } : {}),
    ...(options.eventQuietRecheckMs !== undefined ? { eventQuietRecheckMs: options.eventQuietRecheckMs } : {})
  })) {
    yield asAexStreamEventView(event);
  }
}

function isSessionEnvelopeTerminal(event: AexEvent): boolean {
  return event.type === "RUN_FINISHED" || event.type === "RUN_ERROR";
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

function messageFromWire(message: SessionMessage): Message {
  return {
    id: message.id,
    sender: message.sender,
    text: message.text,
    ...(message.timestamp !== undefined ? { timestamp: message.timestamp } : {}),
    ...(message.turnSeq !== undefined ? { turnSeq: message.turnSeq } : {}),
    ...(message.sequence !== undefined ? { sequence: message.sequence } : {})
  };
}

function projectAssistantMessages(events: readonly AexEvent[]): readonly Message[] {
  const out: Message[] = [];
  const byMessageId = new Map<string, number>();
  for (let i = 0; i < events.length; i++) {
    const event = events[i] as MessageEventLike;
    if (event.type !== "TEXT_MESSAGE_CONTENT") continue;
    const data = asRecord(event.data);
    if (data.delta === true) continue;
    const text = typeof data.text === "string" ? data.text : undefined;
    if (text === undefined) continue;
    const messageId = typeof data.messageId === "string" && data.messageId ? data.messageId : undefined;
    const sequence = event.sequence ?? event.seq;
    const timestamp = event.time ?? event.recordedAt ?? timestampFromEpochMs(event.receivedAt);
    const turnSeq = typeof data.turnSeq === "number" ? data.turnSeq : undefined;
    if (messageId !== undefined) {
      const existing = byMessageId.get(messageId);
      if (existing !== undefined) {
        const current = out[existing]!;
        out[existing] = {
          ...current,
          text: `${current.text}${text}`,
          ...(timestamp !== undefined ? { timestamp } : {}),
          ...(sequence !== undefined ? { sequence } : {}),
          ...(turnSeq !== undefined ? { turnSeq } : {})
        };
        continue;
      }
      byMessageId.set(messageId, out.length);
    }
    out.push({
      id: messageId ?? (typeof event.id === "string" && event.id ? event.id : `message-${i}`),
      sender: "assistant",
      text,
      ...(timestamp !== undefined ? { timestamp } : {}),
      ...(sequence !== undefined ? { sequence } : {}),
      ...(turnSeq !== undefined ? { turnSeq } : {})
    });
  }
  return out;
}

function assistantTextFromEvents(events: readonly AexEvent[]): string {
  return assistantTextEntriesFromEvents(events).map((entry) => entry.text).join("");
}

function turnTraceFromEvents(events: readonly AexEvent[]): TurnTrace {
  return {
    toolCalls: toolCallsFromEvents(events),
    usage: usageFromEvents(events),
    text: assistantTextEntriesFromEvents(events)
  };
}

function assistantTextEntriesFromEvents(
  events: readonly AexEvent[]
): TurnTrace["text"] {
  const out: Array<Mutable<TurnTrace["text"][number]>> = [];
  for (const event of events) {
    if (event.type !== "TEXT_MESSAGE_CONTENT") continue;
    const data = asRecord(event.data);
    if (data.delta === true) continue;
    const text = typeof data.text === "string" ? data.text : undefined;
    if (text === undefined) continue;
    const entry: Mutable<TurnTrace["text"][number]> = { text };
    const messageId = typeof data.messageId === "string" ? data.messageId : undefined;
    if (messageId !== undefined) entry.messageId = messageId;
    if (typeof event.sequence === "number") entry.seq = event.sequence;
    if (typeof event.time === "string") entry.recordedAt = event.time;
    out.push(entry);
  }
  return out;
}

function toolCallsFromEvents(events: readonly AexEvent[]): TurnTrace["toolCalls"] {
  const order: string[] = [];
  const byId = new Map<string, Mutable<TurnTrace["toolCalls"][number]>>();
  for (const event of events) {
    const data = asRecord(event.data);
    if (event.type === "TOOL_CALL_START") {
      const id = typeof data.id === "string" ? data.id : undefined;
      if (id === undefined) continue;
      const trace: Mutable<TurnTrace["toolCalls"][number]> = {
        id,
        name: typeof data.name === "string" ? data.name : "",
        args: asRecord(data.arguments)
      };
      const messageId = typeof data.messageId === "string" ? data.messageId : undefined;
      if (messageId !== undefined) trace.messageId = messageId;
      if (typeof event.sequence === "number") trace.startSeq = event.sequence;
      if (typeof event.time === "string") trace.startedAt = event.time;
      if (!byId.has(id)) order.push(id);
      byId.set(id, trace);
      continue;
    }
    if (event.type === "TOOL_CALL_RESULT") {
      const id = typeof data.id === "string" ? data.id : undefined;
      if (id === undefined) continue;
      const result: Mutable<NonNullable<TurnTrace["toolCalls"][number]["result"]>> = {
        isError: data.isError === true,
        content: data.content ?? null
      };
      if (typeof event.sequence === "number") result.seq = event.sequence;
      if (typeof event.time === "string") result.recordedAt = event.time;
      let trace = byId.get(id);
      if (trace === undefined) {
        trace = { id, name: "", args: {} };
        order.push(id);
        byId.set(id, trace);
      }
      trace.result = result;
      const duration = durationMs(trace.startedAt, result.recordedAt);
      if (duration !== undefined) trace.durationMs = duration;
    }
  }
  return order.map((id) => byId.get(id)!);
}

function usageFromEvents(events: readonly AexEvent[]): UsageSummary {
  const totals = { inputTokens: 0, outputTokens: 0, cacheReadInputTokens: 0, cacheCreationInputTokens: 0 };
  let seen = false;
  for (const event of events) {
    if (event.type !== "CUSTOM") continue;
    const data = asRecord(event.data);
    if (data.name !== "aex.usage") continue;
    const value = asRecord(data.value);
    const fields = [
      ["input_tokens", "inputTokens"],
      ["output_tokens", "outputTokens"],
      ["cache_read_input_tokens", "cacheReadInputTokens"],
      ["cache_creation_input_tokens", "cacheCreationInputTokens"]
    ] as const;
    for (const [wireName, apiName] of fields) {
      const n = value[wireName];
      if (typeof n === "number" && Number.isFinite(n)) {
        totals[apiName] += n;
        seen = true;
      }
    }
  }
  if (!seen) return {};
  return {
    inputTokens: totals.inputTokens,
    outputTokens: totals.outputTokens,
    cacheReadInputTokens: totals.cacheReadInputTokens,
    cacheCreationInputTokens: totals.cacheCreationInputTokens,
    totalTokens: totals.inputTokens + totals.outputTokens
  };
}

interface MessageEventLike {
  readonly id?: string;
  readonly type?: string;
  readonly seq?: number;
  readonly sequence?: number;
  readonly recordedAt?: string;
  readonly time?: string;
  readonly receivedAt?: number;
  readonly data?: unknown;
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function timestampFromEpochMs(value: unknown): string | undefined {
  return typeof value === "number" && Number.isFinite(value)
    ? new Date(value).toISOString()
    : undefined;
}

type Mutable<T> = { -readonly [K in keyof T]: T[K] };

function durationMs(start: string | undefined, end: string | undefined): number | undefined {
  if (start === undefined || end === undefined) return undefined;
  const a = Date.parse(start);
  const b = Date.parse(end);
  if (!Number.isFinite(a) || !Number.isFinite(b)) return undefined;
  const delta = b - a;
  return delta >= 0 ? delta : undefined;
}

/**
 * The verdict carried by a run terminal event. Session lifecycle states never
 * appear here; a suspension or approval hold interrupts the run.
 */
const SESSION_TERMINAL_READS = new Set<string>([
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "interrupted"
]);

/** The terminal event's explicit outcome. */
function carriedOutcome(event: AexEvent): SessionRunOutcome | undefined {
  const outcome = event.data.outcome;
  return typeof outcome === "string" && SESSION_TERMINAL_READS.has(outcome)
    ? (outcome as SessionRunOutcome)
    : undefined;
}

function isSessionRunTerminalEvent(event: AexEvent, runId: string): boolean {
  return event.runId === runId && (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR");
}

/** Read and validate the committed terminal event's explicit run outcome. */
function terminalSessionStatusFromEvents(events: readonly AexEvent[], runId: string): SessionRunOutcome {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (!isSessionRunTerminalEvent(event, runId)) continue;
    const carried = carriedOutcome(event);
    if (carried === undefined) {
      throw new SessionStateError("RUN terminal is missing a valid explicit outcome", { runId });
    }
    if (event.type === "RUN_ERROR" && carried !== "failed") {
      throw new SessionStateError("RUN_ERROR must carry outcome=failed", { runId });
    }
    if (event.type === "RUN_FINISHED" && carried === "failed") {
      throw new SessionStateError("a failed run must terminate with RUN_ERROR", { runId });
    }
    return carried;
  }
  throw new SessionStateError(`run ${runId} ended without a matching RUN_FINISHED or RUN_ERROR event`, { runId });
}

/** True only for a completed successful run. */
function isTerminalReadOk(read: SessionRunOutcome): boolean {
  return read === "succeeded";
}

const PROGRESSING_SESSION_STATUSES = new Set<SessionStatus>([
  "creating",
  "running",
  "suspending",
  "cancelling",
  "deleting"
]);

/** Validate the session projection observed immediately after a durable terminal. */
function assertSessionCommittedAfterRun(
  session: Session,
  run: SessionRun,
  outcome: SessionRunOutcome
): void {
  if (PROGRESSING_SESSION_STATUSES.has(session.status) || session.currentRun?.runId === run.runId) {
    throw new SessionStateError("RUN terminal was observed before the session state was committed", {
      sessionId: session.id,
      runId: run.runId,
      status: session.status
    });
  }
  if (session.lastRun?.runId !== run.runId) {
    throw new SessionStateError("RUN terminal does not match the session's lastRun", {
      sessionId: session.id,
      runId: run.runId,
      lastRunId: session.lastRun?.runId
    });
  }
  if (outcome === "succeeded" && (session.status !== "idle" || session.acceptsMessages !== true)) {
    throw new SessionStateError("a succeeded RUN_FINISHED must leave the session idle and accepting messages", {
      sessionId: session.id,
      runId: run.runId,
      status: session.status,
      acceptsMessages: session.acceptsMessages
    });
  }
}

function runBillingFromEvents(
  events: readonly AexEvent[],
  runId: string
): { readonly costUsd: number; readonly usage: UsageSummary } {
  const terminal = [...events].reverse().find(
    (event) => event.runId === runId && (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR")
  );
  if (terminal === undefined) {
    throw new SessionStateError(`run ${runId} ended without a matching terminal event`, { runId });
  }
  const data = asRecord(terminal.data);
  const costUsd = data.costUsd;
  const providerUsage = data.providerUsage;
  if (typeof costUsd !== "number" || !Number.isFinite(costUsd) || costUsd < 0 || !Array.isArray(providerUsage)) {
    throw new SessionStateError("RUN terminal is missing valid per-run cost and provider usage", { runId });
  }
  return {
    costUsd,
    usage: usageFromProviderUsage(providerUsage as readonly SessionCostProviderUsage[])
  };
}

/**
 * The IMMEDIATE authoritative failure text — the terminal `RUN_ERROR` event's
 * `data.failureMessage`. `result.error` reads this FIRST so a failed (e.g.
 * bad-BYOK) session's error is never empty even before the session-record mirror
 * exposes `errorMessage`.
 */
function failureFromEvents(events: readonly AexEvent[]): string | undefined {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (event.type !== "RUN_ERROR") continue;
    const data = asRecord(event.data);
    for (const key of ["failureMessage", "message", "error"]) {
      const value = data[key];
      if (typeof value === "string" && value.length > 0) return value;
    }
  }
  return undefined;
}

/** The typed schema-decode outcome from the terminal `aex.result.*` event, if any. */
function outcomeFromEvents<T = unknown>(events: readonly AexEventView[]): TurnOutcome<T> | undefined {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (event.isResultDecoded()) {
      const value = asRecord(event.data).value;
      const decoded =
        value && typeof value === "object" && !Array.isArray(value) && "value" in (value as object)
          ? (value as { readonly value: unknown }).value
          : value;
      return { kind: "decoded", value: decoded as T };
    }
    if (event.isResultRefused()) {
      const payload = asRecord(asRecord(event.data).value);
      const reason = payload.reason;
      const detail = typeof payload.detail === "string" ? payload.detail : undefined;
      return {
        kind: "refused",
        reason: reason === "schema_violation" || reason === "uncertain" || reason === "refused" ? reason : "refused",
        ...(detail !== undefined ? { detail } : {})
      };
    }
  }
  return undefined;
}

/**
 * Build the unified finished run result (shared by `session.messages.send().finished()`
 * and `Aex.start`): the terminal outcome `status`, `ok`, `costUsd` (>= 0),
 * `usage` (from the run terminal), and event-first `error`.
 */
function buildTurnResult(
  sessionId: string,
  session: Session,
  run: SessionRun,
  events: readonly AexEventView[],
  files: readonly SessionFile[],
  checkpoint: SessionCheckpointRevision | undefined,
  messages: readonly Message[],
  read: SessionRunOutcome
): SessionRunResult {
  const status = read;
  const ok = isTerminalReadOk(read);
  const { usage, costUsd } = runBillingFromEvents(events, run.runId);
  const error =
    failureFromEvents(events) ??
    (!ok && typeof session.errorMessage === "string" && session.errorMessage ? session.errorMessage : undefined);
  const outcome = outcomeFromEvents(events);
  return {
    sessionId,
    session,
    run,
    status,
    ok,
    costUsd,
    usage,
    ...(error !== undefined ? { error } : {}),
    text: assistantTextFromEvents(events),
    events,
    files,
    ...(checkpoint !== undefined ? { checkpoint } : {}),
    messages,
    ...(outcome !== undefined ? { outcome } : {})
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

function assertRunCheckpoint(
  events: readonly AexEvent[],
  runId: string,
  revision: SessionCheckpointRevision
): void {
  const terminal = [...events].reverse().find(
    (event) => event.runId === runId && (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR")
  );
  const checkpoint = terminal ? asRecord(terminal.data.checkpoint) : {};
  if (
    terminal === undefined ||
    revision.runId !== runId ||
    checkpoint.checkpointId !== revision.checkpointId
  ) {
    throw new SessionStateError("RUN terminal and session files resolved to different checkpoints", {
      runId,
      terminalCheckpointId: checkpoint.checkpointId,
      fileCheckpointId: revision.checkpointId,
      fileCheckpointRunId: revision.runId
    });
  }
}

function terminalCheckpointId(events: readonly AexEvent[], runId: string): string | undefined {
  const terminal = [...events].reverse().find(
    (event) => event.runId === runId && (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR")
  );
  const checkpointId = terminal ? asRecord(terminal.data.checkpoint).checkpointId : undefined;
  if (typeof checkpointId !== "string" || checkpointId.length === 0) {
    if (terminal?.type === "RUN_ERROR") return undefined;
    throw new SessionStateError("RUN_FINISHED is missing its committed checkpoint", { runId });
  }
  return checkpointId;
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
  /** The same fetch the HttpClient uses, threaded into direct asset uploads. */
  readonly #fetch: FetchLike | undefined;
  readonly #assetRetry: AssetUploadRetryOptions;
  readonly workspace: WorkspaceClient;
  readonly sessions: SessionClient;

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
    // Wrap the transport fetch (the caller's override, or global `fetch`) with
    // the bounded-retry layer so every BFF request gets default resilience.
    // The raw `#fetch` below stays unwrapped for object-storage traffic. Asset
    // uploads apply the resolved client policy in their transfer helper;
    // `session.files.fetch()` intentionally returns the raw one-shot response.
    const baseFetch: FetchLike = resolved.fetch ?? ((input: Parameters<FetchLike>[0], init: Parameters<FetchLike>[1]) => fetch(input, init));
    const retryingFetch = withRetry(baseFetch, resolved.retry);
    this.#assetRetry = resolved.retry === false
      ? { maxAttempts: 1 }
      : resolveRetryConfig(resolved.retry);
    this.#http = new HttpClient({
      ...(baseUrl ? { baseUrl } : {}),
      apiKey,
      fetch: retryingFetch,
      // Opt-in local diagnostics: emit a redacted per-request trace to
      // stderr. Uploads nothing. A caller wanting a custom sink can pass
      // a function instead of `true`.
      ...(resolved.debug
        ? { debug: typeof resolved.debug === "function" ? resolved.debug : (line: string) => console.error(line) }
        : {})
    });
    this.#fetch = resolved.fetch;
    this.workspace = new WorkspaceClient(this.#http, {
      upload: (args) => this.#uploadAsset(args),
      uploadStream: (args) => this.#uploadAssetStream(args)
    });
    this.sessions = new SessionClient(this.#http, (options) => this.#buildSessionCreateRequest(options), this.#fetch);
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
    assertAllowedObjectFields(
      opts,
      "Aex.start",
      "options",
      ["timeoutMs", "webSocketFactory", "idleTimeoutMs", "pingIntervalMs", "throwOnFailure"]
    );
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
        const throttle = throttleFromSession(sessionState);
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
    // Model is REQUIRED and checked BEFORE the provider key, so omitting `model`
    // reports "model is required" rather than a misleading provider-key message.
    if (typeof options.model !== "string" || !options.model) {
      throw configError("aex.sessions.create", "model", "model is required");
    }
    // One model→provider resolver (SSoT), shared with the CLI: it honors an
    // explicit provider (forward-compat: an unknown model is allowed through so a
    // slightly-old SDK can still run a newly-launched model), infers the default
    // provider for a known model, and rejects an unknown model without a provider.
    let provider: ProviderName;
    try {
      provider = resolveModelProvider(options.model, options.provider);
    } catch (err) {
      void err;
      throw configError(
        "aex.sessions.create",
        options.provider === undefined ? "model" : "provider",
        options.provider === undefined
          ? "model must be recognized unless provider is supplied explicitly"
          : "provider cannot serve the selected model"
      );
    }
    validateApiKeys(options.apiKeys, provider, "aex.sessions.create");
    // WS9 fail-closed: `outputMode:'stream'` on a NON-streamable provider is a
    // hard reject at the earliest seam (no silent downgrade to buffered).
    if (options.outputMode !== undefined) {
      try {
        assertStreamableOutputMode(options.outputMode, provider);
      } catch (err) {
        void err;
        throw configError("aex.sessions.create", "outputMode", "outputMode is not supported for the selected provider");
      }
    }
    // Fast client-side validation via the contract parsers (the SSoT). runtimeSize
    // and timeout are STABLE closed sets whose invalid values the create endpoint
    // otherwise SILENTLY defaults (no error ever — pre-launch edge-sweep F11/F12);
    // reject them synchronously with a typed error instead. webhook shape is
    // re-checked here for a fast local fail (the server enforces it too). Model is
    // deliberately NOT hard-rejected here to preserve forward-compat with models
    // added server-side before an SDK upgrade (an unknown model still fails on the
    // server).
    try {
      parseRuntimeSize(options.runtime);
    } catch (err) {
      void err;
      throw configError("aex.sessions.create", "runtime", "runtime must be a supported size preset");
    }
    try {
      parseSessionTimeout(options.overrides?.timeout);
    } catch (err) {
      void err;
      throw configError("aex.sessions.create", "overrides.timeout", "overrides.timeout must be a supported duration");
    }
    if (options.webhook !== undefined) {
      try {
        parseSessionWebhook(options.webhook);
      } catch (err) {
        void err;
        throw configError("aex.sessions.create", "webhook.url", "webhook.url must be a valid HTTPS URL");
      }
    }
    if (options.responseFormat !== undefined) {
      try {
        parseResponseFormat(options.responseFormat);
      } catch (err) {
        void err;
        throw configError("aex.sessions.create", "responseFormat", "responseFormat is invalid");
      }
    }
    if (options.approvalGate !== undefined) {
      try {
        parseApprovalGate(options.approvalGate);
      } catch (err) {
        void err;
        throw configError("aex.sessions.create", "approvalGate", "approvalGate is invalid");
      }
    }
    let secretEnvDeclarations: ReturnType<typeof splitSecretEnv>["declarations"];
    let envSecretValues: ReturnType<typeof splitSecretEnv>["values"];
    try {
      const split = splitSecretEnv(options.environment?.secrets);
      secretEnvDeclarations = split.declarations;
      envSecretValues = split.values;
    } catch (err) {
      void err;
      throw configError("aex.sessions.create", "environment.secrets", "environment.secrets is invalid");
    }

    let limits: SessionLimits | undefined;
    const limitsInput: { maxSpendUsd?: number; maxTurns?: number } = {};
    if (options.overrides?.maxSpendUsd !== undefined) {
      try {
        parseSessionLimits({ maxSpendUsd: options.overrides.maxSpendUsd });
      } catch (err) {
        void err;
        throw configError("aex.sessions.create", "overrides.maxSpendUsd", "overrides.maxSpendUsd must be valid");
      }
      limitsInput.maxSpendUsd = options.overrides.maxSpendUsd;
    }
    if (options.overrides?.maxTurns !== undefined) {
      try {
        parseSessionLimits({ maxTurns: options.overrides.maxTurns });
      } catch (err) {
        void err;
        throw configError("aex.sessions.create", "overrides.maxTurns", "overrides.maxTurns must be valid");
      }
      limitsInput.maxTurns = options.overrides.maxTurns;
    }
    limits = parseSessionLimits(Object.keys(limitsInput).length > 0 ? limitsInput : undefined);

    const { submissionMcpServers, mergedMcpSecrets } = mergeMcpServers(
      options.mcpServers ?? [],
      []
    );
    let fileCapture: PlatformSubmission["fileCapture"] | undefined;
    try {
      fileCapture = fileCaptureForWire(options.fileCapture);
    } catch (err) {
      void err;
      throw configError("aex.sessions.create", "fileCapture", "fileCapture is invalid");
    }
    let environment: PlatformEnvironmentInput | undefined;
    try {
      environment = sessionEnvironmentForWire(options.environment);
    } catch (err) {
      void err;
      throw configError("aex.sessions.create", "environment", "environment is invalid");
    }
    let builtinTools = options.builtinTools ?? "default";
    if (Array.isArray(builtinTools)) {
      try {
        builtinTools = resolveBuiltinToolNames(builtinTools);
      } catch (err) {
        void err;
        throw configError("aex.sessions.create", "builtinTools", "builtinTools contains an unsupported tool name");
      }
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
      ...(options.apiKeys ? { apiKeys: options.apiKeys } : {}),
      ...(mergedMcpSecrets.length > 0 ? { mcpServers: mergedMcpSecrets } : {}),
      ...(Object.keys(envSecretValues).length > 0 ? { envSecrets: envSecretValues } : {})
    };

    const retention = sessionRetentionForWire(options);

    return {
      provider,
      submission,
      ...(options.runtime ? { runtimeSize: options.runtime } : {}),
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
   * `monthSpendUsd`, the enforced `spendCapUsd`, and plan fields. Backed by
   * `GET /api/billing` (scope `billing:read`). The result is additive-tolerant:
   * fields a newer deployment reports that this SDK does not know yet pass
   * through on the returned object.
   */
  billing(): Promise<BillingSummary> {
    return operations.getBilling(this.#http);
  }

  /**
   * Create a hosted checkout session for a paid plan (`pro` or `team`).
   * Open the returned `url` in a browser. Plan activation happens after
   * checkout completes.
   */
  billingCheckout(request: BillingCheckoutRequest, options?: IdempotencyOptions): Promise<BillingHostedSession> {
    return operations.createBillingCheckout(this.#http, request, options);
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
  const parsed = parseApiKey(apiKey);
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
 * Module-private factory for SDK validation errors. `details.field` is the only
 * stable machine-readable payload; messages are human guidance and may change.
 */
function configError(surface: string, field: string, message: string): SessionConfigValidationError {
  return new SessionConfigValidationError(`${surface}: ${message}`, { field });
}

/**
 * Extract a throttle-class {@link ProviderFault} from a failed session record.
 * Reads a structured `providerFault` / `error` field first (the shape the
 * runtime is expected to emit on a throttled turn), then falls back to a
 * heuristic scan of `errorMessage`. Returns `undefined` when the failure is not
 * a throttle.
 */
function throttleFromSession(session: Session): ProviderFault | undefined {
  const fault =
    parseProviderFault((session as { readonly providerFault?: unknown }).providerFault) ??
    parseProviderFault((session as { readonly error?: unknown }).error) ??
    faultFromErrorMessage(typeof session.errorMessage === "string" ? session.errorMessage : undefined);
  return fault && isThrottleFault(fault) ? fault : undefined;
}

/** Last-resort throttle detection from a free-text turn error message. */
function faultFromErrorMessage(message: string | undefined): ProviderFault | undefined {
  if (message === undefined || message.length === 0) return undefined;
  const lower = message.toLowerCase();
  if (/\b429\b|rate.?limit|too many requests/.test(lower)) {
    return { kind: "rate_limit", message };
  }
  if (/\b529\b|overloaded/.test(lower)) {
    return { kind: "overloaded", message };
  }
  return undefined;
}

function normaliseSessionInput(
  input: string | readonly string[],
  surface: string,
  field: string
): string | readonly string[] {
  if (typeof input === "string") {
    if (!input) {
      throw configError(surface, field, `${field} must be a non-empty string`);
    }
    if (!input.trim()) {
      throw configError(surface, field, `${field} must contain non-whitespace text`);
    }
    return input;
  }
  if (!Array.isArray(input) || input.length === 0) {
    throw configError(surface, field, `${field} must be a non-empty string or string array`);
  }
  for (const segment of input) {
    if (typeof segment !== "string" || !segment) {
      throw configError(surface, field, `${field} segments must be non-empty strings`);
    }
  }
  if (input.every((segment) => !segment.trim())) {
    throw configError(surface, field, `${field} must contain non-whitespace text`);
  }
  return [...input];
}

function assertSupportedSessionFields(
  options: SessionCreateOptions,
  surface: string,
  allowStartFields: boolean
): void {
  const record = options as unknown as Record<string, unknown>;
  const allowed = new Set([
    "provider", "model", "system", "assets", "mcpServers", "fileCapture",
    "builtinTools", "outputMode", "responseFormat", "approvalGate", "metadata",
    "idempotencyKey", "apiKeys", "environment", "runtime", "overrides", "webhook",
    ...(allowStartFields ? ["message", "deleteAfter", "messageIdempotencyKey", "stream"] : [])
  ]);
  const guidance: Readonly<Record<string, string>> = {
    runtimeSize: "use runtime",
    secretEnv: "use environment.secrets",
    parentSessionId: "subagent lineage is assigned by the platform",
    message: "sessions are created without a first message; use Aex.start or session.messages.send",
    prompt: "use message",
    instructions: "publish Instructions through aex.workspace.instructions and pass the returned ref in assets.instructions"
  };
  for (const field of Object.keys(record)) {
    if (!allowed.has(field)) {
      const detail = guidance[field];
      throw configError(
        surface,
        field,
        `${field} is not a supported option${detail === undefined ? "" : `; ${detail}`}`
      );
    }
  }
  const overrides = record.overrides;
  if (overrides && typeof overrides === "object" && !Array.isArray(overrides)) {
    const overrideRecord = overrides as Record<string, unknown>;
    if (Object.prototype.hasOwnProperty.call(overrideRecord, "idleSuspendAfter")) {
      throw configError(
        surface,
        "overrides.idleSuspendAfter",
        "overrides.idleSuspendAfter is not a supported option; use overrides.idleTtl."
      );
    }
  }
  assertStructuredSessionFields(record, surface, allowStartFields);
}

function assertSupportedSessionSendOptions(
  options: unknown,
  surface: string,
  allowIdempotencyKey = true
): void {
  const record = options as Record<string, unknown> | undefined;
  if (!record || typeof record !== "object") return;
  const allowed = new Set([
    "webSocketFactory",
    "idleTimeoutMs",
    "pingIntervalMs",
    ...(allowIdempotencyKey ? ["idempotencyKey"] : [])
  ]);
  for (const field of Object.keys(record)) {
    if (allowed.has(field)) continue;
    const guidance = field === "from"
      ? "use session.events.list(), stream(), or streamEnvelopes() for replay"
      : field === "signal"
        ? "use session.cancel() / session.suspend() for remote control"
        : undefined;
    throw configError(
      surface,
      field,
      `${field} is not a supported option${guidance === undefined ? "" : `; ${guidance}`}`
    );
  }
}

function assertStructuredSessionFields(
  record: Record<string, unknown>,
  surface: string,
  allowStartFields: boolean
): void {
  const overrides = assertAllowedObjectFields(
    record.overrides,
    surface,
    "overrides",
    ["idleTtl", "timeout", "maxSpendUsd", "maxTurns"]
  );
  void overrides;
  assertAssetsFields(record.assets, surface);
  assertAllowedObjectFields(
    record.fileCapture,
    surface,
    "fileCapture",
    ["allowedDirs", "deniedDirs", "captureTimeoutMs", "maxFileBytes", "maxTotalBytes", "maxFiles"]
  );
  assertEnvironmentFields(record.environment, surface);
  assertAllowedObjectFields(record.webhook, surface, "webhook", ["url"]);

  const responseFormat = assertRecord(record.responseFormat, surface, "responseFormat");
  if (responseFormat !== undefined) {
    const allowed = responseFormat.kind === "text"
      ? ["kind"]
      : ["kind", "schema", "strict", "name"];
    assertAllowedKeys(responseFormat, surface, "responseFormat", allowed);
  }
  assertAllowedObjectFields(record.approvalGate, surface, "approvalGate", ["tools"]);
  if (allowStartFields) {
    assertAllowedObjectFields(
      record.stream,
      surface,
      "stream",
      ["webSocketFactory", "idleTimeoutMs", "pingIntervalMs"]
    );
  }
}

function assertAssetsFields(value: unknown, surface: string): void {
  const assets = assertAllowedObjectFields(
    value,
    surface,
    "assets",
    ["files", "skills", "tools", "instructions"]
  );
  if (assets === undefined) return;
  // Workspace publish methods return records that extend the reusable ref with
  // immutable metadata. Accept those records directly so publish -> session is ergonomic.
  const common = [
    "kind", "resourceId", "version", "assetId", "contentHash",
    "createdAt", "updatedAt", "sizeBytes", "contentType"
  ];
  const fields: Readonly<Record<string, readonly string[]>> = {
    files: [...common, "name", "mountPath"],
    skills: [...common, "name", "description"],
    tools: [...common, "name", "description", "input_schema", "entry"],
    instructions: [...common, "name"]
  };
  for (const [category, allowed] of Object.entries(fields)) {
    const entries = assets[category];
    if (entries === undefined) continue;
    if (!Array.isArray(entries)) {
      throw configError(surface, `assets.${category}`, `assets.${category} must be an array`);
    }
    entries.forEach((entry, index) => {
      const field = `assets.${category}[${index}]`;
      const item = assertRecord(entry, surface, field);
      if (item !== undefined) assertAllowedKeys(item, surface, field, allowed);
    });
  }
}

function assertEnvironmentFields(value: unknown, surface: string): void {
  const environment = assertAllowedObjectFields(
    value,
    surface,
    "environment",
    ["networking", "packages", "variables", "secrets"]
  );
  if (environment === undefined) return;
  assertAllowedObjectFields(
    environment.networking,
    surface,
    "environment.networking",
    ["mode", "allowedHosts"]
  );
  for (const field of ["variables", "secrets"] as const) {
    assertRecord(environment[field], surface, `environment.${field}`);
  }
  const packages = environment.packages;
  if (packages === undefined) return;
  if (!Array.isArray(packages)) {
    throw configError(surface, "environment.packages", "environment.packages must be an array");
  }
  packages.forEach((entry, index) => {
    const field = `environment.packages[${index}]`;
    const item = assertRecord(entry, surface, field);
    if (item !== undefined) assertAllowedKeys(item, surface, field, ["name", "version"]);
  });
}

function assertAllowedObjectFields(
  value: unknown,
  surface: string,
  field: string,
  allowed: readonly string[]
): Record<string, unknown> | undefined {
  const record = assertRecord(value, surface, field);
  if (record !== undefined) assertAllowedKeys(record, surface, field, allowed);
  return record;
}

function assertRecord(
  value: unknown,
  surface: string,
  field: string
): Record<string, unknown> | undefined {
  if (value === undefined) return undefined;
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw configError(surface, field, `${field} must be an object`);
  }
  return value as Record<string, unknown>;
}

function assertAllowedKeys(
  record: Record<string, unknown>,
  surface: string,
  field: string,
  allowed: readonly string[]
): void {
  const allowedSet = new Set(allowed);
  for (const key of Object.keys(record)) {
    if (allowedSet.has(key)) continue;
    const nested = `${field}.${key}`;
    throw configError(surface, nested, `${nested} is not a supported option`);
  }
}

function validateApiKeys(
  apiKeys: Partial<Record<ProviderName, string>> | undefined,
  provider: ProviderName,
  surface: string
): void {
  const key = apiKeys?.[provider];
  if (typeof key !== "string" || key.length === 0) {
    throw configError(surface, `apiKeys.${provider}`, "a provider API key is required in apiKeys");
  }
}

function fileCaptureForWire(fileCapture: SessionCreateOptions["fileCapture"]): PlatformSubmission["fileCapture"] | undefined {
  if (fileCapture === undefined) {
    return undefined;
  }
  const allowedDirs = fileCapture.allowedDirs?.filter((dir) => dir.length > 0);
  const deniedDirs = fileCapture.deniedDirs?.filter((dir) => dir.length > 0);
  const hasNumericOverride =
    fileCapture.captureTimeoutMs !== undefined ||
    fileCapture.maxFileBytes !== undefined ||
    fileCapture.maxTotalBytes !== undefined ||
    fileCapture.maxFiles !== undefined;
  if ((allowedDirs?.length ?? 0) === 0 && (deniedDirs?.length ?? 0) === 0 && !hasNumericOverride) {
    return undefined;
  }
  return {
    ...(allowedDirs && allowedDirs.length > 0 ? { allowedDirs } : {}),
    ...(deniedDirs && deniedDirs.length > 0 ? { deniedDirs } : {}),
    ...(fileCapture.captureTimeoutMs !== undefined ? { captureTimeoutMs: fileCapture.captureTimeoutMs } : {}),
    ...(fileCapture.maxFileBytes !== undefined ? { maxFileBytes: fileCapture.maxFileBytes } : {}),
    ...(fileCapture.maxTotalBytes !== undefined ? { maxTotalBytes: fileCapture.maxTotalBytes } : {}),
    ...(fileCapture.maxFiles !== undefined ? { maxFiles: fileCapture.maxFiles } : {})
  };
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

const DEFAULT_SESSION_IDLE_TTL = "3m";

function sessionRetentionForWire(options: SessionCreateOptions): SessionRetentionPolicy {
  return {
    idleTtl: options.overrides?.idleTtl ?? DEFAULT_SESSION_IDLE_TTL
  };
}

function sessionEnvironmentForWire(
  environment: SessionEnvironmentOptions | undefined
): PlatformEnvironmentInput | undefined {
  if (environment === undefined) {
    return undefined;
  }
  const { variables, secrets: _secrets, ...rest } = environment;
  void _secrets;
  const out: PlatformEnvironmentInput = {
    ...rest,
    ...(variables !== undefined ? { envVars: variables } : {})
  };
  return Object.keys(out).length === 0 ? undefined : out;
}

function mergeMcpServers(
  inputs: readonly McpServer[],
  explicitSecrets: readonly PlatformMcpServerSecret[]
): {
  submissionMcpServers: ReadonlyArray<McpServerRef | { readonly kind: "workspace"; readonly id: string }>;
  mergedMcpSecrets: readonly PlatformMcpServerSecret[];
} {
  const submissionMcpServers: Array<McpServerRef | { readonly kind: "workspace"; readonly id: string }> = [];
  const secretByName = new Map<string, PlatformMcpServerSecret>();
  for (const secret of explicitSecrets) {
    secretByName.set(secret.name, secret);
  }
  for (let i = 0; i < inputs.length; i++) {
    const entry = inputs[i];
    if (!(entry instanceof McpServer)) {
      throw configError("aex", `mcpServers[${i}]`, `mcpServers[${i}] must be an McpServer instance`);
    }
    submissionMcpServers.push(entry.toSubmissionEntry());
    const secret = entry.toSecretEntry();
    if (secret) {
      const existing = secretByName.get(secret.name);
      if (existing && existing.url !== secret.url) {
        throw configError(
          "aex",
          `mcpServers[${i}].url`,
          `mcpServers[${i}].url conflicts with another MCP declaration`
        );
      }
      secretByName.set(secret.name, secret);
    }
  }
  return {
    submissionMcpServers,
    mergedMcpSecrets: Array.from(secretByName.values())
  };
}

export type { SessionFileType, SessionFileLink, SessionFileLinkOptions, SessionFileQuery };
