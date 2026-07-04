import {
  AexApiError,
  AexError,
  CredentialValidationError,
  DEFAULT_RUN_PROVIDER,
  HttpClient,
  RunConfigValidationError,
  RunStateError,
  SecretString,
  customName,
  isRunSettled,
  operations,
  providersForModel,
  streamCoordinatorEvents,
  type AexEvent,
  type AgentsMdRecord,
  type AgentsMdRef,
  type BillingCheckoutRequest,
  type BillingHostedSession,
  type BillingLedgerPage,
  type BillingLedgerQuery,
  type BillingPortalRequest,
  type BillingSummary,
  type DebugSink,
  type FetchLike,
  type FileRecord,
  type FileRef,
  type McpServerRef,
  type Output,
  type OutputFileType,
  type OutputLink,
  type OutputLinkOptions,
  type OutputQuery,
  type OutputText,
  type OutputMode,
  type ReadOutputTextOptions,
  type OutputSearchQuery,
  type OutputSearchHit,
  type OutputSearchPage,
  type Session,
  type SessionCreateRequest,
  type SessionEvent,
  type SessionListPage,
  type SessionListQuery,
  type SessionMessage,
  type SessionRetentionPolicy,
  type SessionStateChangeAccepted,
  type SessionTurn,
  type PlatformEnvironmentInput,
  type PlatformSubmission,
  type PlatformInlineSecrets,
  type PlatformMcpServerSecret,
  type Run,
  type RunModel,
  type RunEvent,
  type RunTrace,
  type UsageSummary,
  type RunLimits,
  parseRunLimits,
  parseRuntimeSize,
  parseRunTimeout,
  parseRunWebhook,
  type RunWebhookDelivery,
  type RunProvider,
  type SecretRecord,
  type RunUnit,
  BUILTIN_TOOL_NAMES,
  type BuiltinToolName,
  type RuntimeSize,
  type SkillToolRef,
  type ToolRef,
  type WebhookSigningSecret,
  type WebSocketFactory,
  type WhoAmI,
  TERMINAL_RUN_STATUSES
} from "@aexhq/contracts";
import { AgentsMd } from "./agents-md.js";
import { uploadAsset, type AssetFetch, type UploadedAsset } from "./asset-upload.js";
import { File } from "./file.js";
import { McpServer } from "./mcp-server.js";
import {
  AexRateLimitError,
  isThrottleFault,
  parseProviderFault,
  withRetry,
  type ProviderFault,
  type RetryOptions
} from "./retry.js";
import { Secret, splitSecretEnv } from "./secret.js";
import { SkillTool } from "./skill-tool.js";
import { Tool } from "./tool.js";

export interface AexOptions {
  /** Workspace-scoped SDK API key. */
  readonly apiKey?: string;
  /**
   * API plane root, e.g. `https://aex.example.com`. Optional —
   * defaults to the canonical hosted URL (`https://api.aex.dev`).
   * Override for local, staging, or other hosted aex API planes.
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
   * Built-in transport retry policy. Every BFF request is retried on transient
   * failures (HTTP 429/500/502/503/504/529 and network errors) with bounded
   * exponential backoff + jitter, honoring `Retry-After`. Billable submits carry
   * a stable idempotency key, so a retry never creates a duplicate billable run.
   *
   * Omit for sensible defaults (4 attempts, ~2 min budget); pass an object to
   * tune `maxAttempts` / delays / `maxElapsedMs`; pass `false` to disable.
   */
  readonly retry?: RetryOptions | false;
}

/**
 * The settle-consistent result of {@link Aex.run}:
 * the one-shot session record plus its events, decoded trace, assistant text,
 * and captured outputs — everything a "do it and give me the result" caller
 * needs without hand-rolling a session/message/stream loop.
 */
export interface RunResult {
  readonly runId: string;
  /** The session id used as the run-compatible handle. */
  readonly sessionId?: string;
  /** Run-compatible view of the underlying session record. */
  readonly run: Run;
  /** The underlying resumable session record. */
  readonly session?: Session;
  /** The turn accepted for this one-shot run. */
  readonly turn?: SessionTurn;
  readonly status: string;
  /** `true` when the one-shot turn parked the session cleanly (`idle` or `suspended`). */
  readonly ok: boolean;
  /** The assistant's final text. */
  readonly text: string;
  /** Assistant messages projected from the settled event stream. */
  readonly messages: readonly Message[];
  /** The session turn event stream. */
  readonly events: readonly RunEvent[];
  /** Decoded view of the events: tool calls + usage + assistant text. */
  readonly trace: RunTrace;
  /** The run's captured output files. */
  readonly outputs: readonly Output[];
  /** Aggregate token usage when the deployment exposes it on the record. */
  readonly usage?: UsageSummary;
  /**
   * Settle-time showback estimate (USD), from the settle-stamped session
   * record's `costUsd` (the full `costTelemetry` block is served on
   * `GET /api/runs/:id`, not on the session projection). The settle
   * write lands tens of seconds AFTER the turn parks, so by default this is
   * usually absent on a fresh run — pass `settleConsistent: true` to wait for
   * it, or read `sessions.get(runId).costUsd` later.
   */
  readonly costUsd?: number;
  /** The run's error message when `!ok`. */
  readonly error?: string;
}

/** Options for {@link Aex.run}. */
export interface RunCollectOptions {
  /** Overall wait budget (ms) for the one-shot session turn to park. */
  readonly timeoutMs?: number;
  readonly webSocketFactory?: WebSocketFactory;
  readonly idleTimeoutMs?: number;
  readonly pingIntervalMs?: number;
  /** Throw a {@link RunStateError} when the run does not succeed. Default false. */
  readonly throwOnFailure?: boolean;
  /**
   * Wait (bounded, ~60s) for the settle write after the turn parks, so the
   * result carries the settle-stamped `costUsd`/`usage`/`errorMessage`. The
   * settle lambda lands tens of seconds after the park event, so this trades
   * latency for a complete record. Default false: return at park; read
   * `sessions.get(runId)` later for the showback.
   */
  readonly settleConsistent?: boolean;
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
}

/**
 * Options for opening a session (the low-level API) or a one-shot `run`.
 * Everything the agent needs is spelled out at the call site:
 *
 *   - `model` / `system` — the agent's brief.
 *   - `tools` — custom `Tool` bundles, skill-tools
 *     (`Tools.fromSkillDir` / `Tools.fromSkillUrl`), and builtin tool-name
 *     references; local composition instances are materialized to the hosted
 *     asset store before the session lands.
 *   - `agentsMd` / `files` — local composition instances
 *     (`AgentsMd.fromContent` / `File.fromBytes`, …), materialized to the
 *     hosted asset store before the session lands.
 *   - `mcpServers` — instances whose secrets are split into the vaulted secrets
 *     channel server-side; the public submission carries only the declarations.
 *   - `apiKeys` — the BYOK provider key(s), keyed by provider. A key for the
 *     selected provider is REQUIRED. The platform never holds a long-lived
 *     provider key on your behalf.
 */
export interface SessionCreateOptions {
  /**
   * Upstream provider selector. Prefer naming it explicitly with the
   * {@link Providers} symbol const, e.g. `provider: Providers.DEEPSEEK`. When
   * omitted it is derived from `model`; if supplied it MUST serve the model.
   */
  readonly provider?: RunProvider;
  /**
   * Closed public model id. Prefer the {@link Models} symbol const, e.g.
   * `Models.CLAUDE_HAIKU_4_5`.
   */
  readonly model: RunModel;
  readonly system?: string;
  /**
   * Tools available to the agent. Each entry is a custom {@link Tool} bundle, a
   * skill-tool ({@link SkillTool} from `Tools.fromSkillDir` / `Tools.fromSkillUrl`),
   * or a BUILTIN tool reference — a bare name string, preferably
   * `BuiltinTools.<name>` so a typo is a compile error.
   */
  readonly tools?: readonly (Tool | SkillTool | BuiltinToolName)[];
  readonly agentsMd?: readonly AgentsMd[];
  readonly files?: readonly File[];
  readonly mcpServers?: readonly McpServer[];
  /**
   * Output capture policy for the session's output files. `allowedDirs` omitted
   * captures every regular file the session creates or modifies; the listed
   * roots narrow capture; `deniedDirs` subtracts noise.
   */
  readonly outputs?: {
    readonly allowedDirs?: readonly string[];
    readonly deniedDirs?: readonly string[];
    readonly captureTimeoutMs?: number;
    readonly maxFileBytes?: number;
    readonly maxTotalBytes?: number;
    readonly maxFiles?: number;
  };
  /**
   * Whether to inject the standard builtin tool set
   * ({@link DEFAULT_BUILTIN_TOOLS}). Omitted / `true` injects the standard
   * builtins; `false` injects none. Cherry-pick a subset back via `tools`.
   */
  readonly includeBuiltinTools?: boolean;
  /**
   * Assistant-output granularity. `"buffered"` (default) delivers one event per
   * assistant message; `"stream"` delivers per-token text deltas.
   */
  readonly outputMode?: OutputMode;
  readonly metadata?: PlatformSubmission["metadata"];
  readonly idempotencyKey?: string;
  /** BYOK provider key(s), keyed by provider. */
  readonly apiKeys?: Partial<Record<RunProvider, string>>;
  readonly environment?: SessionEnvironmentOptions;
  /**
   * Managed runtime size. One of the closed {@link RuntimeSize} preset tokens.
   * Prefer the {@link Sizes} symbol const.
   */
  readonly runtime?: RuntimeSize;
  readonly overrides?: SessionOverrides;
  /**
   * Optional per-session callback URL. The platform delivers the terminal
   * event to `webhook.url` at the settle-consistent barrier, signed
   * Standard-Webhooks style (verify with {@link verifyAexWebhook}). The URL
   * must be https.
   */
  readonly webhook?: { readonly url: string };
}

export interface SessionSendOptions {
  readonly idempotencyKey?: string;
  readonly from?: number;
  readonly webSocketFactory?: WebSocketFactory;
  readonly idleTimeoutMs?: number;
  readonly pingIntervalMs?: number;
}

interface InternalSessionSendOptions extends SessionSendOptions {
  readonly signal?: AbortSignal;
}

export interface SessionRunOptions extends SessionCreateOptions {
  readonly message: SessionInput;
  readonly deleteAfter?: boolean;
  readonly messageIdempotencyKey?: string;
  readonly stream?: Omit<SessionSendOptions, "idempotencyKey">;
}

export interface SessionTurnResult {
  readonly sessionId: string;
  readonly session: Session;
  readonly turn: SessionTurn;
  readonly status: string;
  readonly text: string;
  readonly events: readonly SessionEvent[];
  readonly outputs: readonly Output[];
  readonly messages: readonly Message[];
}

export interface SessionRunResult extends SessionTurnResult {}

export class SessionTurnStream implements AsyncIterable<SessionEvent> {
  readonly #run: () => AsyncGenerator<SessionEvent, SessionTurnResult, void>;
  #generator: AsyncGenerator<SessionEvent, SessionTurnResult, void> | undefined;
  #outcome:
    | { readonly ok: true; readonly value: SessionTurnResult }
    | { readonly ok: false; readonly error: unknown }
    | undefined;
  #done: Promise<SessionTurnResult> | undefined;

  constructor(run: () => AsyncGenerator<SessionEvent, SessionTurnResult, void>) {
    this.#run = run;
  }

  /**
   * ONE underlying send per turn: iterating the stream and calling `done()`
   * (the documented `for await … ; await turn.done()` pattern) must share a
   * single generator — a fresh generator per consumer would re-POST the
   * message as a second billable turn (or 409 `session_busy` mid-turn).
   */
  #shared(): AsyncGenerator<SessionEvent, SessionTurnResult, void> {
    this.#generator ??= this.#capture(this.#run());
    return this.#generator;
  }

  async *#capture(
    generator: AsyncGenerator<SessionEvent, SessionTurnResult, void>
  ): AsyncGenerator<SessionEvent, SessionTurnResult, void> {
    try {
      let next = await generator.next();
      while (!next.done) {
        yield next.value;
        next = await generator.next();
      }
      this.#outcome = { ok: true, value: next.value };
      return next.value;
    } catch (error) {
      this.#outcome = { ok: false, error };
      throw error;
    }
  }

  [Symbol.asyncIterator](): AsyncIterator<SessionEvent> {
    const generator = this.#shared();
    // Deliberately do NOT forward `return()`: `break`-ing out of a
    // `for await` loop must not close the in-flight turn — `done()` can
    // still drain it to completion afterwards.
    return {
      next: () => generator.next(),
      return: async () => ({ done: true as const, value: undefined })
    };
  }

  done(): Promise<SessionTurnResult> {
    this.#done ??= (async () => {
      const generator = this.#shared();
      let next = await generator.next();
      while (!next.done) {
        next = await generator.next();
      }
      // A prior consumer may have drained the generator already: its return
      // value is then gone from `next.value`, so replay the captured outcome.
      if (next.value !== undefined) {
        return next.value;
      }
      if (this.#outcome !== undefined) {
        if (this.#outcome.ok) {
          return this.#outcome.value;
        }
        throw this.#outcome.error;
      }
      return next.value;
    })();
    return this.#done;
  }
}

type InternalSessionSender = (input: SessionInput, options?: InternalSessionSendOptions) => SessionTurnStream;
const internalSessionSenders = new WeakMap<SessionHandle, InternalSessionSender>();
type CallableSessionMessages = SessionMessages & (() => SessionMessages);

function sendSessionInternal(
  session: SessionHandle,
  input: SessionInput,
  options: InternalSessionSendOptions = {}
): SessionTurnStream {
  const sender = internalSessionSenders.get(session);
  if (sender === undefined) {
    throw new Error("Aex: invalid session handle");
  }
  return sender(normaliseSessionInput(input, "SessionHandle.send", "input"), options);
}

export type Message = SessionMessage;

/**
 * Accessor over the session's assistant messages. `session.messages` returns
 * this synchronously; each method fetches on call.
 */
export interface SessionMessages {
  all(): Promise<readonly Message[]>;
  /** Compatibility alias for {@link SessionMessages.all}. */
  list(): Promise<readonly Message[]>;
  last(): Promise<Message | undefined>;
  first(): Promise<Message | undefined>;
}

/**
 * Accessor over the session's event stream (`session.events()`): the buffered
 * `SessionEvent` snapshots, the polling `RunEvent` iterator, the live
 * coordinator envelope iterator, and the events-namespace archive.
 */
export interface SessionEvents {
  list(): Promise<readonly SessionEvent[]>;
  last(): Promise<SessionEvent | undefined>;
  first(): Promise<SessionEvent | undefined>;
  stream(options?: StreamEventsOptions): AsyncIterable<RunEvent>;
  streamEnvelopes(options?: StreamEnvelopesOptions): AsyncIterable<AexEvent>;
  archiveLink(options?: OutputLinkOptions): Promise<OutputLink>;
  /** Download the events-namespace archive as a zip. */
  download(options?: OutputDownloadOptions): Promise<Uint8Array>;
}

/**
 * Accessor over the session's captured output files (`session.outputs()`):
 * enumerate, read one as capped text, locate/resolve, and download.
 */
export interface SessionOutputs {
  list(query?: OutputQuery): Promise<readonly Output[]>;
  last(): Promise<Output | undefined>;
  first(): Promise<Output | undefined>;
  read(selector: OutputFileSelector, options?: ReadOutputTextOptions): Promise<OutputText>;
  find(query: OutputQuery): Promise<readonly Output[]>;
  findOne(query: OutputQuery): Promise<Output | null>;
  link(selectorOrQuery: OutputLinkSelector, options?: OutputLinkOptions): Promise<OutputLink>;
  fetch(selectorOrQuery: OutputLinkSelector, options?: OutputLinkOptions): Promise<Response>;
  /** No selector = outputs-namespace zip; with selector = one file's raw bytes. */
  download(selector?: OutputFileSelector, options?: OutputDownloadOptions): Promise<Uint8Array>;
}

/**
 * Accessor over the session's webhook delivery ledger (`session.webhooks()`).
 */
export interface SessionWebhooks {
  list(): Promise<readonly RunWebhookDelivery[]>;
  redeliver(deliveryId: string): Promise<void>;
}

export class SessionHandle {
  readonly #http: HttpClient;
  readonly #fetch: FetchLike | undefined;
  #session: Session;
  /** The last message sent on this handle, for {@link SessionHandle.replayLast}. */
  #lastSend: { readonly input: SessionInput; readonly idempotencyKey: string } | undefined;

  constructor(http: HttpClient, session: Session, fetch?: FetchLike) {
    this.#http = http;
    this.#session = session;
    this.#fetch = fetch;
    internalSessionSenders.set(this, (input, options = {}) => new SessionTurnStream(() => this.#send(input, options)));
  }

  get id(): string {
    return this.#session.sessionId ?? this.#session.id;
  }

  get record(): Session {
    return this.#session;
  }

  send(input: SessionInput, options: SessionSendOptions = {}): SessionTurnStream {
    assertNoSessionSendSignal(options, "SessionHandle.send");
    return sendSessionInternal(this, input, options);
  }

  /**
   * Re-send the last message on this session — the clean way to retry a turn a
   * throttle or transient failure interrupted. By default it REUSES the previous
   * message's idempotency key, so if the original turn actually landed
   * server-side the replay de-duplicates instead of creating a second billable
   * turn; pass a fresh `idempotencyKey` to force a brand-new turn.
   */
  replayLast(options: SessionSendOptions = {}): SessionTurnStream {
    assertNoSessionSendSignal(options, "SessionHandle.replayLast");
    const last = this.#lastSend;
    if (last === undefined) {
      throw new RunStateError("SessionHandle.replayLast: no message has been sent on this session yet");
    }
    return sendSessionInternal(this, last.input, {
      ...options,
      idempotencyKey: options.idempotencyKey ?? last.idempotencyKey
    });
  }

  async *#send(input: SessionInput, options: InternalSessionSendOptions): AsyncGenerator<SessionEvent, SessionTurnResult, void> {
    const idempotencyKey = options.idempotencyKey ?? generateIdempotencyKey();
    this.#lastSend = { input, idempotencyKey };
    const accepted = await operations.sendSessionMessage(
      this.#http,
      this.id,
      { input },
      { idempotencyKey }
    );
    this.#session = accepted.session;
    const turn = accepted.turn;
    const events: SessionEvent[] = [];
    for await (const event of streamSessionTurnEvents(this.#http, this.id, turn, {
      ...options,
      from: options.from ?? accepted.eventCursor ?? turn.eventCursor ?? 0
    })) {
      events.push(event);
      yield event;
    }
    const terminalStatus = terminalSessionStatusFromEvents(events, turn.turnSeq);
    const readSession = await operations.getSession(this.#http, this.id).catch(() => this.#session);
    this.#session = withTerminalSessionStatus(readSession, terminalStatus);
    const outputs = await operations.listSessionOutputs(this.#http, this.id).catch(() => [] as readonly Output[]);
    const messages = projectAssistantMessages(events);
    return {
      sessionId: this.id,
      session: this.#session,
      turn,
      status: this.#session.status,
      text: assistantTextFromEvents(events),
      events,
      outputs,
      messages
    };
  }

  async suspend(options: Pick<SessionSendOptions, "idempotencyKey"> = {}): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.suspendSession(this.#http, this.id, options);
    this.#session = accepted.session;
    return accepted;
  }

  async cancel(options: Pick<SessionSendOptions, "idempotencyKey"> = {}): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.cancelSession(this.#http, this.id, options);
    this.#session = accepted.session;
    return accepted;
  }

  async resume(options: Pick<SessionSendOptions, "idempotencyKey"> = {}): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.resumeSession(this.#http, this.id, options);
    this.#session = accepted.session;
    return accepted;
  }

  async delete(options: Pick<SessionSendOptions, "idempotencyKey"> = {}): Promise<void> {
    const accepted = await operations.deleteSession(this.#http, this.id, options);
    if (accepted && typeof accepted === "object" && "session" in accepted) {
      this.#session = accepted.session;
    }
  }

  /**
   * Accessor for the session's assistant messages. `all()` returns them
   * oldest-first; `last()`/`first()` return a single entry or `undefined` when
   * empty. The accessor is callable as a compatibility shim for older
   * `session.messages().list()` callers.
   */
  get messages(): CallableSessionMessages {
    const http = this.#http;
    const id = this.id;
    const fromEvents = async (): Promise<readonly Message[]> =>
      projectAssistantMessages(await operations.listSessionEvents(http, id));
    const all = async (): Promise<readonly Message[]> => {
      try {
        const page = await operations.listSessionMessages(http, id);
        if (!Array.isArray(page.messages)) {
          return fromEvents();
        }
        return page.messages.map(messageFromWire);
      } catch (err) {
        if (!isMissingMessagesEndpoint(err)) {
          throw err;
        }
        return fromEvents();
      }
    };
    let accessor: CallableSessionMessages;
    accessor = (() => accessor) as unknown as CallableSessionMessages;
    Object.assign(accessor, {
      all,
      list: all,
      last: async () => (await all()).at(-1),
      first: async () => (await all())[0]
    });
    return accessor;
  }

  /**
   * Accessor for the session's event stream: the buffered `SessionEvent`
   * snapshots (`list`/`last`/`first`), the polling `RunEvent` iterator
   * (`stream`), the live coordinator envelope iterator (`streamEnvelopes`), and
   * the events-namespace archive (`archiveLink`/`download`).
   */
  events(): SessionEvents {
    const http = this.#http;
    const id = this.id;
    const list = (): Promise<readonly SessionEvent[]> => operations.listSessionEvents(http, id);
    return {
      list,
      last: async () => (await list()).at(-1),
      first: async () => (await list())[0],
      stream: (options?: StreamEventsOptions) => streamSessionEventsPolling(http, id, options ?? {}),
      streamEnvelopes: (options?: StreamEnvelopesOptions) => streamSessionEnvelopes(http, id, options ?? {}),
      archiveLink: (options?: OutputLinkOptions) => operations.eventArchiveLink(http, id, options),
      download: async (options?: OutputDownloadOptions) =>
        writeOptionalFile(await operations.downloadEvents(http, id), options?.to)
    };
  }

  /**
   * Accessor for the session's captured output files: `list`/`last`/`first`
   * enumerate them; `read` streams one as capped text; `find`/`findOne`/`link`/
   * `fetch` locate and resolve them; `download` fetches the outputs-namespace
   * zip (no selector) or one file's raw bytes (with selector).
   */
  outputs(): SessionOutputs {
    return sessionOutputs(this.#http, this.id, this.#fetch);
  }

  /**
   * Accessor for the session's webhook delivery ledger: `list()` returns the
   * delivery attempts; `redeliver(id)` re-sends the frozen payload under the
   * same `webhook-id` so the consumer dedupes.
   */
  webhooks(): SessionWebhooks {
    const http = this.#http;
    const id = this.id;
    return {
      list: () => operations.getRunWebhookDeliveries(http, id),
      redeliver: (deliveryId) => operations.redeliverRunWebhook(http, id, deliveryId)
    };
  }

  /** Re-read the session record from the server and store it as the current record. */
  async refresh(): Promise<Session> {
    this.#session = await operations.getSession(this.#http, this.id);
    return this.#session;
  }

  /**
   * Poll the session record until it reaches a parked/terminal status (idle,
   * suspended, error, or any terminal run status). Throws if `timeoutMs`
   * elapses first. Updates the stored record.
   */
  async wait(options: WaitForRunOptions = {}): Promise<Session> {
    const intervalMs = options.intervalMs ?? 1_500;
    const timeoutMs = options.timeoutMs;
    const signal = options.signal;
    const deadline = typeof timeoutMs === "number" ? Date.now() + timeoutMs : Number.POSITIVE_INFINITY;
    while (!signal?.aborted) {
      const session = await operations.getSession(this.#http, this.id);
      this.#session = session;
      if (isSessionParked(session.status)) return session;
      if (Date.now() >= deadline) {
        throw new Error(`SessionHandle.wait: timeout after ${timeoutMs}ms`);
      }
      await sleep(intervalMs, signal);
    }
    throw new Error("SessionHandle.wait: aborted");
  }

  /**
   * Fetch the self-contained `RunUnit` for this session: parsed submission,
   * attempts, indexed events, outputs, capture failures, proxy-call audit, and
   * resolved skills. Use this when you need fields beyond the session record.
   *
   * On the managed plane this is a LEAN summary — the aggregate collections
   * (`attempts` / `events.entries` / `outputs` / `rawEventPages`) default to
   * empty. For authoritative per-run data use `outputs()` / `events()` /
   * `messages()`. The returned shape is always type-valid (never `undefined`
   * where the type promises an array/page), so array/page access is safe.
   */
  unit(): Promise<RunUnit> {
    return operations.getRunUnit(this.#http, this.id);
  }

  /**
   * Download EVERYTHING public about this session as one zip, assembled
   * client-side from the public read endpoints. Organised into `metadata/`,
   * `events/`, and `outputs/` folders, plus a `manifest.json`. Pass `to` to
   * also write the bytes to a file path while still returning them.
   */
  async download(options?: OutputDownloadOptions): Promise<Uint8Array> {
    return writeOptionalFile(await operations.download(this.#http, this.id), options?.to);
  }

  /** Download only the session record (the `metadata` namespace) as a zip. */
  async downloadMetadata(options?: OutputDownloadOptions): Promise<Uint8Array> {
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
      { idempotencyKey: options.idempotencyKey ?? generateIdempotencyKey() }
    );
    return new SessionHandle(this.#http, session, this.#fetch);
  }

  async open(sessionId: string): Promise<SessionHandle> {
    return new SessionHandle(this.#http, await operations.getSession(this.#http, sessionId), this.#fetch);
  }

  get(sessionId: string): Promise<Session> {
    return operations.getSession(this.#http, sessionId);
  }

  async delete(sessionId: string, options: Pick<SessionSendOptions, "idempotencyKey"> = {}): Promise<void> {
    await operations.deleteSession(this.#http, sessionId, options);
  }

  list(query?: SessionListQuery): Promise<SessionListPage> {
    return operations.listSessions(this.#http, query);
  }

  /**
   * Accessor over one session's captured output files, addressed by id without
   * opening a handle. Returns the SAME rich {@link SessionOutputs} surface as
   * `session.outputs()` — `aex.sessions.outputs(id).list()` /
   * `.read(selector)` / `.download()` / … — so the workspace client and the
   * live handle share one accessor convention.
   */
  outputs(sessionId: string): SessionOutputs {
    return sessionOutputs(this.#http, sessionId, this.#fetch);
  }

  /**
   * Find output files across sessions by filename / extension / content type.
   * Returns lean REFERENCE hits (never bytes; fetch content with `readOutput`).
   * Scope the search to a corpus with `query.runIds` (a session-id allow-list);
   * omit it to scan every session in the workspace. Composed client-side (per-
   * session `listSessionOutputs` + the contracts output filter), bounded by
   * `query.limit` (default 100).
   */
  async searchOutputs(query: OutputSearchQuery = {}): Promise<OutputSearchPage> {
    // Dedup the caller-supplied allow-list so a run repeated in `runIds` (e.g. from
    // concatenating corpora) isn't scanned twice and doesn't inflate the hit count
    // with duplicates (pre-launch edge-sweep F27).
    const unscoped = query.runIds === undefined;
    const sessionIds = unscoped ? await this.#allSessionIds() : [...new Set(query.runIds)];
    const limit = query.limit ?? 100;
    // Translate the search query to an OutputQuery so the contracts output
    // filter does the matching — no re-derived filter logic here.
    const outputQuery: OutputQuery = {
      ...(query.filename ? { filename: new RegExp(escapeRegExp(query.filename), "i") } : {}),
      ...(query.extension ? { extension: query.extension } : {}),
      ...(query.contentType ? { contentType: query.contentType } : {})
    };
    const hasFilter = Object.keys(outputQuery).length > 0;
    const hits: OutputSearchHit[] = [];
    for (const sessionId of sessionIds) {
      let outputs: readonly Output[];
      try {
        outputs = hasFilter
          ? await operations.listSessionOutputs(this.#http, sessionId, outputQuery)
          : await operations.listSessionOutputs(this.#http, sessionId);
      } catch (err) {
        if (unscoped && isMissingOutputsSession(err)) continue;
        throw err;
      }
      for (const o of outputs) {
        hits.push({
          runId: sessionId,
          outputId: o.id,
          ...(o.filename !== undefined ? { filename: o.filename } : {}),
          ...(o.sizeBytes !== undefined ? { sizeBytes: o.sizeBytes } : {}),
          ...(o.contentType !== undefined ? { contentType: o.contentType } : {})
        });
        if (hits.length >= limit) return { hits };
      }
    }
    return { hits };
  }

  /** Enumerate every session id in the workspace by paging `listSessions`. */
  async #allSessionIds(): Promise<readonly string[]> {
    const ids: string[] = [];
    const seenCursors = new Set<string>();
    let cursor: string | undefined;
    do {
      if (cursor !== undefined) {
        if (seenCursors.has(cursor)) {
          throw new Error("Aex.sessions.searchOutputs: listSessions returned a repeated cursor");
        }
        seenCursors.add(cursor);
      }
      const page = await operations.listSessions(this.#http, cursor ? { cursor } : {});
      for (const session of page.sessions) ids.push(session.id);
      cursor = page.nextCursor;
    } while (cursor);
    return ids;
  }

  async run(options: SessionRunOptions): Promise<SessionRunResult> {
    const { message, deleteAfter, messageIdempotencyKey, stream, ...createOptions } = options;
    assertNoLegacySessionFields(options, "Aex.sessions.run");
    const input = normaliseSessionInput(message, "Aex.sessions.run", "message");
    // Derive the message key from the create key (like the CLI) so a retried run
    // with the same `idempotencyKey` de-duplicates BOTH the create and the
    // billable turn — never a duplicate billable run.
    const createKey = createOptions.idempotencyKey ?? generateIdempotencyKey();
    const messageKey = messageIdempotencyKey ?? deriveMessageKey(createKey);
    const session = await this.create({ ...createOptions, idempotencyKey: createKey });
    const result = await session.send(input, {
      ...(stream ?? {}),
      idempotencyKey: messageKey
    }).done();
    if (deleteAfter) {
      await session.delete();
    }
    return result;
  }
}

async function* streamSessionTurnEvents(
  http: HttpClient,
  sessionId: string,
  turn: SessionTurn,
  options: InternalSessionSendOptions
): AsyncGenerator<SessionEvent, void, void> {
  const first = await operations.getSessionCoordinatorTicket(http, sessionId);
  yield* streamCoordinatorEvents({
    wsUrl: first.wsUrl,
    from: options.from ?? 0,
    fetchTicket: async () => (await operations.getSessionCoordinatorTicket(http, sessionId)).ticket,
    isTerminal: (event: SessionEvent) => isSessionTurnTerminalEvent(event, turn.turnSeq),
    ...(options.signal ? { signal: options.signal } : {}),
    ...(options.webSocketFactory ? { webSocketFactory: options.webSocketFactory } : {}),
    ...(options.idleTimeoutMs !== undefined ? { idleTimeoutMs: options.idleTimeoutMs } : {}),
    ...(options.pingIntervalMs !== undefined ? { pingIntervalMs: options.pingIntervalMs } : {})
  });
}

/**
 * Poll the session's `RunEvent` snapshots until the session parks, the signal
 * aborts, or the caller breaks the iterator, deduping by event id. Module-level
 * so `SessionHandle.events()` can hand it to its accessor object literal.
 */
async function* streamSessionEventsPolling(
  http: HttpClient,
  id: string,
  options: StreamEventsOptions
): AsyncIterable<RunEvent> {
  if (options.signal?.aborted) return;
  const seenIds = new Set<string>();
  const intervalMs = options.intervalMs ?? 1_000;
  const signal = options.signal;
  while (!signal?.aborted) {
    const events = await operations.listRunEvents(http, id);
    for (const event of events) {
      if (!seenIds.has(event.id)) {
        seenIds.add(event.id);
        yield event;
      }
    }
    const session = await operations.getSession(http, id);
    if (isSessionParked(session.status)) return;
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
 * session never outlives it. Module-level so `SessionHandle.events()` can hand
 * it to its accessor object literal.
 */
async function* streamSessionEnvelopes(
  http: HttpClient,
  id: string,
  options: StreamEnvelopesOptions
): AsyncIterable<AexEvent> {
  const first = await operations.getSessionCoordinatorTicket(http, id);
  yield* streamCoordinatorEvents({
    wsUrl: first.wsUrl,
    from: options.from ?? 0,
    fetchTicket: async () => (await operations.getSessionCoordinatorTicket(http, id)).ticket,
    // settleConsistent ends the stream on the post-mirror barrier instead of
    // the earlier RUN_FINISHED UX signal.
    ...(options.settleConsistent ? { isTerminal: isRunSettled } : {}),
    ...(options.signal ? { signal: options.signal } : {})
  });
}

/**
 * Download captured deliverables. No selector → the full outputs namespace as a
 * zip; a selector → one file's raw bytes. Module-level so
 * `SessionHandle.outputs()` can hand it to its accessor object literal.
 */
async function downloadSessionOutput(
  http: HttpClient,
  id: string,
  selector?: OutputFileSelector,
  options?: OutputDownloadOptions
): Promise<Uint8Array> {
  let bytes: Uint8Array;
  if (selector === undefined) {
    bytes = await operations.downloadOutputs(http, id);
  } else {
    const output = isOutputPathSelector(selector)
      ? resolveOutputFileSelector(await operations.listOutputs(http, id), selector, id)
      : resolveOutputFileSelector([], selector, id);
    const { response } = await http.download(
      `/api/runs/${encodeURIComponent(id)}/outputs/${encodeURIComponent(output.id)}/download`
    );
    bytes = new Uint8Array(await response.arrayBuffer());
  }
  return writeOptionalFile(bytes, options?.to);
}

/**
 * Build the outputs accessor for a session id. Shared by
 * `SessionHandle.outputs()` (bound to the live handle) and
 * `SessionClient.outputs(id)` (addressed by id without opening a handle), so both
 * expose the identical rich {@link SessionOutputs} surface — one accessor
 * convention, one implementation.
 */
function sessionOutputs(http: HttpClient, id: string, fetchLike: FetchLike | undefined): SessionOutputs {
  const list = (query?: OutputQuery): Promise<readonly Output[]> =>
    operations.listSessionOutputs(http, id, query);
  return {
    list,
    last: async () => (await list()).at(-1),
    first: async () => (await list())[0],
    read: (selector, options) => operations.readOutputText(http, id, selector, options),
    find: (query) => operations.findOutputs(http, id, query),
    findOne: (query) => operations.findOutput(http, id, query),
    link: (selectorOrQuery, options) => operations.outputLink(http, id, selectorOrQuery, options),
    fetch: async (selectorOrQuery, options) => {
      const link = await operations.outputLink(http, id, selectorOrQuery, options);
      return (fetchLike ?? globalThis.fetch)(link.url);
    },
    download: (selector, options) => downloadSessionOutput(http, id, selector, options)
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

function isMissingMessagesEndpoint(err: unknown): boolean {
  return err instanceof AexApiError && (err.status === 404 || err.status === 405 || err.status === 501);
}

function isMissingOutputsSession(err: unknown): boolean {
  return err instanceof AexApiError && err.status === 404;
}

function projectAssistantMessages(events: readonly (SessionEvent | RunEvent)[]): readonly Message[] {
  const out: Message[] = [];
  const byMessageId = new Map<string, number>();
  for (let i = 0; i < events.length; i++) {
    const event = events[i] as MessageEventLike;
    if (event.type !== "TEXT_MESSAGE_CONTENT") continue;
    const data = asRecord(event.data);
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

function assistantTextFromEvents(events: readonly (SessionEvent | RunEvent)[]): string {
  return assistantTextEntriesFromEvents(events).map((entry) => entry.text).join("");
}

function runTraceFromEvents(events: readonly RunEvent[]): RunTrace {
  return {
    toolCalls: toolCallsFromEvents(events),
    usage: usageFromEvents(events),
    text: assistantTextEntriesFromEvents(events)
  };
}

function assistantTextEntriesFromEvents(
  events: readonly (SessionEvent | RunEvent)[]
): RunTrace["text"] {
  const out: Array<Mutable<RunTrace["text"][number]>> = [];
  for (const raw of events) {
    const event = raw as MessageEventLike;
    if (event.type !== "TEXT_MESSAGE_CONTENT") continue;
    const data = asRecord(event.data);
    const text = typeof data.text === "string" ? data.text : undefined;
    if (text === undefined) continue;
    const entry: Mutable<RunTrace["text"][number]> = { text };
    const messageId = typeof data.messageId === "string" ? data.messageId : undefined;
    if (messageId !== undefined) entry.messageId = messageId;
    if (typeof event.seq === "number") entry.seq = event.seq;
    const recordedAt = typeof event.recordedAt === "string" ? event.recordedAt : undefined;
    if (recordedAt !== undefined) entry.recordedAt = recordedAt;
    out.push(entry);
  }
  return out;
}

function toolCallsFromEvents(events: readonly RunEvent[]): RunTrace["toolCalls"] {
  const order: string[] = [];
  const byId = new Map<string, Mutable<RunTrace["toolCalls"][number]>>();
  for (const event of events) {
    const data = asRecord(event.data);
    if (event.type === "TOOL_CALL_START") {
      const id = typeof data.id === "string" ? data.id : undefined;
      if (id === undefined) continue;
      const trace: Mutable<RunTrace["toolCalls"][number]> = {
        id,
        name: typeof data.name === "string" ? data.name : "",
        args: asRecord(data.arguments)
      };
      const messageId = typeof data.messageId === "string" ? data.messageId : undefined;
      if (messageId !== undefined) trace.messageId = messageId;
      if (typeof event.seq === "number") trace.startSeq = event.seq;
      if (typeof event.recordedAt === "string") trace.startedAt = event.recordedAt;
      if (!byId.has(id)) order.push(id);
      byId.set(id, trace);
      continue;
    }
    if (event.type === "TOOL_CALL_RESULT") {
      const id = typeof data.id === "string" ? data.id : undefined;
      if (id === undefined) continue;
      const result: Mutable<NonNullable<RunTrace["toolCalls"][number]["result"]>> = {
        isError: data.isError === true,
        content: data.content ?? null
      };
      if (typeof event.seq === "number") result.seq = event.seq;
      if (typeof event.recordedAt === "string") result.recordedAt = event.recordedAt;
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

/** True when a usage summary actually carries at least one token count. */
function hasUsageCounts(usage: UsageSummary | undefined): usage is UsageSummary {
  return !!usage && Object.values(usage).some((n) => typeof n === "number");
}

function usageFromEvents(events: readonly RunEvent[]): UsageSummary {
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

function isSessionTurnTerminalEvent(event: SessionEvent, turnSeq: number): boolean {
  if (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR") {
    return true;
  }
  const name = customName(event);
  if (
    name !== "aex.session.idle" &&
    name !== "aex.session.suspended" &&
    name !== "aex.session.error"
  ) {
    return false;
  }
  const value = event.data.value;
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return true;
  }
  const eventTurnSeq = (value as { readonly turnSeq?: unknown }).turnSeq;
  return typeof eventTurnSeq !== "number" || eventTurnSeq === turnSeq;
}

function terminalSessionStatusFromEvents(events: readonly SessionEvent[], turnSeq: number): string | undefined {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (!isSessionTurnTerminalEvent(event, turnSeq)) continue;
    if (event.type === "RUN_ERROR") return "error";
    if (event.type === "RUN_FINISHED") return "idle";
    const name = customName(event);
    if (name === "aex.session.idle") return "idle";
    if (name === "aex.session.suspended") return "suspended";
    if (name === "aex.session.error") return "error";
  }
  return undefined;
}

function withTerminalSessionStatus(session: Session, terminalStatus: string | undefined): Session {
  if (terminalStatus === undefined || session.status === terminalStatus) return session;
  if (
    session.status !== "creating" &&
    session.status !== "running" &&
    session.status !== "suspending" &&
    session.status !== "cancelling"
  ) {
    return session;
  }
  return { ...session, status: terminalStatus };
}

export interface StreamEventsOptions {
  /** Poll interval in ms for the `RunEvent` snapshot loop. Default 1000. */
  readonly intervalMs?: number;
  readonly signal?: AbortSignal;
}

export interface StreamEnvelopesOptions {
  /** Starting cursor — events with `sequence >= from` are delivered. Default 0. */
  readonly from?: number;
  readonly signal?: AbortSignal;
  /**
   * End the stream settle-consistently. By default the iterator ends on the
   * AG-UI terminal event (RUN_FINISHED / RUN_ERROR) — the render-complete UX
   * signal, which the runner emits BEFORE the platform commits the run record,
   * so a `getRun` immediately after can still read `running`. With
   * `settleConsistent: true` the iterator keeps reading PAST the terminal event
   * until the post-mirror `aex.run.settled` barrier, so when it ends a
   * subsequent `getRun` is guaranteed terminal and `listOutputs` is complete.
   * Note: outputs are durable at the RUN_FINISHED event already; this only adds
   * the run-RECORD consistency barrier.
   */
  readonly settleConsistent?: boolean;
}

export interface WaitForRunOptions {
  readonly intervalMs?: number;
  readonly timeoutMs?: number;
  readonly signal?: AbortSignal;
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

export type OutputLinkSelector = string | OutputFileSelector | OutputQuery;

export interface OutputDownloadOptions {
  readonly to?: string;
}

/**
 * Workspace AgentsMd admin operations exposed under `client.agentsMd`.
 *
 * New sessions usually use `AgentsMd.fromContent(...)` or
 * `AgentsMd.fromPath(...)` directly on `openSession` / `run`; the SDK
 * materializes those bytes to the hosted asset store before the session starts. This namespace is
 * the read/delete surface for persisted AgentsMd records.
 */
export class AgentsMdClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(): Promise<readonly AgentsMdRecord[]> {
    return operations.listAgentsMd(this.#http);
  }

  get(agentsMdId: string): Promise<AgentsMdRecord> {
    return operations.getAgentsMd(this.#http, agentsMdId);
  }

  delete(agentsMdId: string): Promise<void> {
    return operations.deleteAgentsMd(this.#http, agentsMdId);
  }
}

/**
 * Workspace File admin operations exposed under `client.files`.
 *
 * New sessions usually use `File.fromPath(...)` or
 * `File.fromBytes(...)` directly on `openSession` / `run`; the SDK materializes
 * those bytes to the hosted asset store before the session starts. This namespace is the read/delete
 * surface for persisted file records.
 */
export class FilesClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(): Promise<readonly FileRecord[]> {
    return operations.listFiles(this.#http);
  }

  get(fileId: string): Promise<FileRecord> {
    return operations.getFile(this.#http, fileId);
  }

  delete(fileId: string): Promise<void> {
    return operations.deleteFile(this.#http, fileId);
  }
}

/**
 * Workspace secret management exposed under `client.secrets`, mirroring
 * `client.agentsMd` / `client.files`.
 *
 * Lifecycle parity with assets: a `Secret.value(...)` is per-run and
 * gone at terminal; `set` (or promoting an ephemeral via `secret.upload`)
 * persists a named, searchable workspace secret you can `get` (metadata),
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

  /**
   * Internal: create a workspace secret from an ephemeral `Secret.value(...)`
   * being promoted via `secret.upload(client, { name })`. NOT part of the
   * public API — callers use `set`.
   */
  async _createWorkspaceSecret(args: { readonly name: string; readonly value: string }): Promise<{ readonly name: string }> {
    const record = await operations.createSecret(this.#http, args);
    return { name: record.name };
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
 * Unified user-facing client for the aex platform. The same class
 * powers the published `@aexhq/sdk` SDK and (under the hood) every host-side
 * subcommand of the in-container `aex` CLI. All operations talk to
 * the dashboard BFF and operate on durable run records.
 *
 * The SDK never asks the caller for a workspace id — workspace identity
 * is derived server-side from the API key on every request. Use
 * `client.whoami()` if you want to introspect which workspace the
 * token resolves to.
 */
export class Aex {
  readonly #http: HttpClient;
  /** The same fetch the HttpClient uses, threaded into `_uploadAsset`. */
  readonly #fetch: FetchLike | undefined;
  readonly agentsMd: AgentsMdClient;
  readonly files: FilesClient;
  readonly secrets: SecretsClient;
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
    // Wrap the transport fetch (the caller's override, or global `fetch`) with
    // the bounded-retry layer so every BFF request gets default resilience.
    // The raw `#fetch` below stays unwrapped for the direct-to-storage asset PUT
    // and presigned output GETs, which target object storage, not the API plane.
    const baseFetch: FetchLike = resolved.fetch ?? ((input: Parameters<FetchLike>[0], init: Parameters<FetchLike>[1]) => fetch(input, init));
    const retryingFetch = withRetry(baseFetch, resolved.retry);
    this.#http = new HttpClient({
      ...(resolved.baseUrl ? { baseUrl: resolved.baseUrl } : {}),
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
    this.agentsMd = new AgentsMdClient(this.#http);
    this.files = new FilesClient(this.#http);
    this.secrets = new SecretsClient(this.#http);
    this.sessions = new SessionClient(this.#http, (options) => this.#buildSessionCreateRequest(options), this.#fetch);
  }

  /**
   * Internal: satisfies the `SecretUploader` surface so a
   * `Secret.value(...).upload(client, { name })` promotes an ephemeral secret
   * into the workspace store. Forwarded to `SecretsClient._createWorkspaceSecret`.
   * NOT part of the public API.
   */
  async _createWorkspaceSecret(args: { readonly name: string; readonly value: string }): Promise<{ readonly name: string }> {
    return this.secrets._createWorkspaceSecret(args);
  }

  /**
   * Internal: materialize raw bytes to the content-addressable asset store
   * (`/assets/presign` → PUT → `/assets/finalize`). Used by the session-create
   * prepare step to upload draft skill-tool / tool / agentsMd / file bundles so
   * the wire submission carries only plain `kind:"asset"` / `kind:"skill"` refs.
   * NOT part of the public API.
   */
  async _uploadAsset(args: {
    readonly bytes: Uint8Array;
    readonly hash: string;
    readonly contentType?: string;
  }): Promise<UploadedAsset> {
    return uploadAsset({
      http: this.#http,
      bytes: args.bytes,
      hash: args.hash,
      ...(args.contentType ? { contentType: args.contentType } : {}),
      ...(this.#fetch ? { fetch: this.#fetch as unknown as AssetFetch } : {})
    });
  }

  /**
   * Convenience one-shot on top of the canonical session API:
   * open a session, send `message` as the first turn, stream until the session
   * parks (`idle` / `suspended` / `error`), then return the collected text,
   * events, outputs, and session record. The returned `runId` is the session id,
   * so callers can resume later with `openSession(runId)`.
   */
  async run(options: SessionRunOptions, opts: RunCollectOptions = {}): Promise<RunResult> {
    const scopedSignal = scopedAbortSignal(opts.timeoutMs);
    try {
      const { message, deleteAfter, messageIdempotencyKey, stream, ...createOptions } = options;
      assertNoLegacySessionFields(options, "Aex.run");
      const input = normaliseSessionInput(message, "Aex.run", "message");
      assertNoSessionSendSignal(stream, "Aex.run stream");
      const streamOptions: Omit<InternalSessionSendOptions, "idempotencyKey"> = {
        ...(stream ?? {}),
        ...(scopedSignal?.signal ? { signal: scopedSignal.signal } : {}),
        ...(opts.webSocketFactory ? { webSocketFactory: opts.webSocketFactory } : {}),
        ...(opts.idleTimeoutMs !== undefined ? { idleTimeoutMs: opts.idleTimeoutMs } : {}),
        ...(opts.pingIntervalMs !== undefined ? { pingIntervalMs: opts.pingIntervalMs } : {})
      };
      // Derive the message key from the create key (like the CLI) so a retried
      // run with the same `idempotencyKey` de-duplicates BOTH the create and the
      // billable turn server-side — never a duplicate billable run (sdk-dx-3).
      const createKey = createOptions.idempotencyKey ?? generateIdempotencyKey();
      const messageKey = messageIdempotencyKey ?? deriveMessageKey(createKey);
      const session = await this.sessions.create({ ...createOptions, idempotencyKey: createKey });
      const turnResult = await sendSessionInternal(session, input, {
        ...streamOptions,
        idempotencyKey: messageKey
      }).done();
      const runId = turnResult.sessionId;
      // Settle-consistent enrichment (opt-in): the park EVENT that ends the
      // stream lands tens of seconds BEFORE the settle write that flips the
      // record and stamps costTelemetry/costUsd, so an immediate read misses
      // the showback on virtually every fresh run. `settleConsistent: true`
      // polls for the parked RECORD (bounded; degrades to the immediate read).
      const settledRecord =
        opts.settleConsistent === true
          ? await settledSessionRecord(this.#http, runId, turnResult.session, scopedSignal?.signal)
          : undefined;
      const sessionRecord = settledRecord ?? turnResult.session;
      if (deleteAfter) {
        await session.delete();
      }
      const run = sessionToRun(sessionRecord);
      const events = turnResult.events as unknown as readonly RunEvent[];
      const outputs = turnResult.outputs;
      const ok = turnResult.status === "idle" || turnResult.status === "suspended";
      if (!ok && scopedSignal?.signal.aborted) {
        // The client-side wait budget (opts.timeoutMs) expired before the run
        // reached a terminal park. Parity with SessionHandle.wait(): THROW rather
        // than silently returning a misleading {ok:false,status:"running"} with no
        // error (pre-launch edge-sweep F3). The run continues server-side.
        throw new RunStateError(
          `Aex.run: timed out after ${opts.timeoutMs}ms waiting for run ${runId} to park (last status ` +
            `${JSON.stringify(turnResult.status)}); the run continues server-side — cancel via ` +
            `session.cancel() or resume with openSession(${JSON.stringify(runId)})`
        );
      }
      const trace = runTraceFromEvents(events);
      // Surface the trace-derived usage at the top level when the run record does
      // not carry its own usage (the managed plane doesn't populate session.usage);
      // the per-event trace still yields token counts (pre-launch edge-sweep F5).
      // When NEITHER source carries token counts (trace usage is `{}` — the
      // managed plane emits no `aex.usage` events today), leave `usage` absent so
      // `result.usage` honors its "when the deployment exposes it" contract
      // instead of surfacing a truthy-but-empty object.
      const recordUsage = hasUsageCounts(sessionRecord.usage) ? sessionRecord.usage : undefined;
      const usage = recordUsage ?? (hasUsageCounts(trace.usage) ? trace.usage : undefined);
      const costUsd = typeof sessionRecord.costUsd === "number" ? sessionRecord.costUsd : undefined;
      const errorMessage = typeof sessionRecord.errorMessage === "string" && sessionRecord.errorMessage ? sessionRecord.errorMessage : undefined;
      const result: RunResult = {
        runId,
        run,
        sessionId: runId,
        session: sessionRecord,
        turn: turnResult.turn,
        status: turnResult.status,
        ok,
        text: turnResult.text,
        messages: turnResult.messages,
        events,
        trace,
        outputs,
        ...(usage ? { usage } : {}),
        ...(typeof costUsd === "number" ? { costUsd } : {}),
        ...(!ok && errorMessage ? { error: errorMessage } : {})
      };
      if (opts.throwOnFailure && !ok) {
        // A turn that failed because the upstream provider throttled us surfaces
        // as a structured, non-leaky AexRateLimitError carrying the provider
        // fault, so callers can branch on `isRateLimited(err)` and replay.
        const throttle = throttleFromSession(sessionRecord);
        if (throttle) {
          throw new AexRateLimitError({
            status: throttle.status ?? 429,
            attempts: 1,
            source: "provider",
            providerFault: throttle,
            ...(throttle.retryAfterMs !== undefined ? { retryAfterMs: throttle.retryAfterMs } : {})
          });
        }
        throw new RunStateError(
          `Aex.run: session ${runId} ended ${turnResult.status}${errorMessage ? `: ${errorMessage}` : ""}`,
          { runId, status: turnResult.status }
        );
      }
      return result;
    } finally {
      scopedSignal?.clear();
    }
  }

  openSession(options: SessionCreateOptions): Promise<SessionHandle>;
  openSession(sessionId: string): Promise<SessionHandle>;
  openSession(optionsOrId: SessionCreateOptions | string): Promise<SessionHandle> {
    return typeof optionsOrId === "string"
      ? this.sessions.open(optionsOrId)
      : this.sessions.create(optionsOrId);
  }

  async #buildSessionCreateRequest(options: SessionCreateOptions): Promise<SessionCreateRequest> {
    if (!options || typeof options !== "object") {
      throw new RunConfigValidationError("Aex.openSession: options is required");
    }
    assertNoLegacySessionFields(options, "Aex.openSession");
    // `message` belongs to the one-shot surfaces (`run` / `sessions.run`), which
    // strip it before creating the session. Passing it here used to be SILENTLY
    // dropped — the session was created empty, idled from birth, and auto-
    // suspended at the idle TTL without ever running a turn.
    if (Object.prototype.hasOwnProperty.call(options as unknown as Record<string, unknown>, "message")) {
      throw new RunConfigValidationError(
        "Aex.openSession: message is not a supported option; sessions are created without a first " +
          "message — send it with session.send(...), or use run({ message }) for a one-shot."
      );
    }
    const supportedProviders = providersForModel(options.model);
    if (
      options.provider &&
      supportedProviders.length > 0 &&
      !supportedProviders.includes(options.provider)
    ) {
      throw new RunConfigValidationError(
        `Aex.openSession: provider ${JSON.stringify(options.provider)} is not available for ` +
          `model ${JSON.stringify(options.model)} (supported: ${supportedProviders.join(", ")})`
      );
    }
    const provider: RunProvider = options.provider ?? supportedProviders[0] ?? DEFAULT_RUN_PROVIDER;
    if (
      options.provider === undefined &&
      supportedProviders.length === 0 &&
      typeof options.model === "string" &&
      options.model.length > 0 &&
      (typeof options.apiKeys?.[provider] !== "string" || options.apiKeys[provider]!.length === 0)
    ) {
      // Unknown model with no explicit provider: the DEFAULT_RUN_PROVIDER
      // fallback exists for forward-compat with models added server-side, but
      // without a key for that default the generic missing-key error below
      // would point at the wrong problem (e.g. "pass apiKeys[\"anthropic\"]"
      // when the caller mistyped a deepseek model id). Name the real issue.
      throw new RunConfigValidationError(
        `Aex.openSession: model ${JSON.stringify(options.model)} is not a known model id, so its provider ` +
          `cannot be inferred — pass provider explicitly (with a matching apiKeys entry) to run a model ` +
          `this SDK version does not know about.`
      );
    }
    validateApiKeys(options.apiKeys, provider, "Aex.openSession");
    if (typeof options.model !== "string" || !options.model) {
      throw new RunConfigValidationError("Aex.openSession: model is required");
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
      parseRunTimeout(options.overrides?.timeout);
      if (options.webhook !== undefined) parseRunWebhook(options.webhook);
    } catch (err) {
      throw new RunConfigValidationError(
        `Aex.openSession: ${err instanceof Error ? err.message : String(err)}`
      );
    }
    const { declarations: secretEnvDeclarations, values: envSecretValues } =
      splitSecretEnv(options.environment?.secrets);

    let limits: RunLimits | undefined;
    try {
      limits = parseRunLimits(
        options.overrides?.maxSpendUsd === undefined
          ? undefined
          : { maxSpendUsd: options.overrides.maxSpendUsd }
      );
    } catch (err) {
      throw new AexError(
        "RUN_CONFIG_INVALID",
        `Aex.openSession: ${err instanceof Error ? err.message : String(err)}`
      );
    }

    const uploader: AssetUploader = (args) => this._uploadAsset(args);
    const preparedTools = await prepareTools(options.tools ?? [], uploader);
    const preparedAgentsMd = await prepareAgentsMd(options.agentsMd ?? [], uploader);
    const preparedFiles = await prepareFiles(options.files ?? [], uploader);
    const { submissionMcpServers, mergedMcpSecrets } = mergeMcpServers(
      options.mcpServers ?? [],
      []
    );
    const outputCapture = outputsForWire(options.outputs);
    const environment = sessionEnvironmentForWire(options.environment);

    const submission: SessionCreateRequest["submission"] = {
      model: options.model,
      ...(options.system ? { system: options.system } : {}),
      // Builtin name strings + custom tool refs + skill-tool refs all ride the
      // one `tools` union; the BFF parser splits them back apart by kind.
      tools: [
        ...preparedTools.builtinNames,
        ...preparedTools.refs,
        ...preparedTools.skillToolRefs
      ] as unknown as readonly ToolRef[],
      agentsMd: preparedAgentsMd,
      files: preparedFiles,
      mcpServers: submissionMcpServers as readonly McpServerRef[],
      ...(Object.keys(secretEnvDeclarations).length > 0 ? { secretEnv: secretEnvDeclarations } : {}),
      ...(environment ? { environment: environment as NonNullable<PlatformSubmission["environment"]> } : {}),
      ...(options.metadata ? { metadata: options.metadata } : {}),
      ...(outputCapture ? { outputs: outputCapture } : {}),
      ...(options.includeBuiltinTools !== undefined
        ? { includeBuiltinTools: options.includeBuiltinTools }
        : {}),
      ...(options.outputMode !== undefined ? { outputMode: options.outputMode } : {})
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
      // hashed submission. Delivered at the settle-consistent barrier.
      ...(options.webhook ? { webhook: options.webhook } : {}),
      secrets
    };
  }

  /**
   * Delete a workspace asset blob from the shared content-addressed store
   * (`assets/<workspaceId>/<hash>`). Accepts `sha256:<hex>` or a bare
   * 64-hex digest. Runs that already snapshotted the asset are unaffected.
   */
  deleteWorkspaceAsset(hash: string): Promise<void> {
    return operations.deleteWorkspaceAsset(this.#http, hash);
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
  billingCheckout(request: BillingCheckoutRequest): Promise<BillingHostedSession> {
    return operations.createBillingCheckout(this.#http, request);
  }

  /**
   * Create a hosted billing-portal session for the workspace customer.
   * Open the returned `url` in a browser.
   */
  billingPortal(request: BillingPortalRequest = {}): Promise<BillingHostedSession> {
    return operations.createBillingPortal(this.#http, request);
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

// `Run.status` is a loose `string` on the wire shape, so we membership-test
// against the canonical terminal set rather than re-deriving one (which is how
// `timed_out` got dropped from the old hardcoded list).
const TERMINAL_STATUSES = new Set<string>(TERMINAL_RUN_STATUSES);

function isTerminal(status: string | undefined): boolean {
  return typeof status === "string" && TERMINAL_STATUSES.has(status);
}

/** How long `Aex.run` waits for the settle write after the park event (ms). */
const SETTLE_POLL_DEADLINE_MS = 60_000;
/** Interval between settle-poll reads (ms). */
const SETTLE_POLL_INTERVAL_MS = 750;

/**
 * Poll for the session RECORD to reach a parked status — i.e. for the settle
 * write (which also stamps `costUsd`/`usage`/`errorMessage`) to land. Returns
 * the settled record, or `undefined` on timeout/abort/read-failure so the
 * caller can fall back to the record it already holds. The first read is
 * immediate, so a fast settle costs one extra GET and no added latency.
 */
async function settledSessionRecord(
  http: HttpClient,
  sessionId: string,
  lastSeen: Session,
  signal: AbortSignal | undefined
): Promise<Session | undefined> {
  // `lastSeen.status` may be client-side patched (withTerminalSessionStatus), so
  // only trust it when it is parked AND already carries the settle-stamped cost.
  if (isSessionParked(lastSeen.status) && typeof lastSeen.costUsd === "number") return lastSeen;
  const deadline = Date.now() + SETTLE_POLL_DEADLINE_MS;
  while (signal?.aborted !== true && Date.now() < deadline) {
    const record = await operations.getSession(http, sessionId).catch(() => undefined);
    if (record !== undefined && isSessionParked(record.status)) return record;
    try {
      await sleep(SETTLE_POLL_INTERVAL_MS, signal);
    } catch {
      return undefined;
    }
  }
  return undefined;
}

/**
 * A session is "parked" once it stops making progress: it reached one of the
 * turn-terminal statuses (`idle` / `suspended` / `error`) or a terminal run
 * status. `SessionHandle.wait` / `streamEvents` stop here.
 */
function isSessionParked(status: string | undefined): boolean {
  return (
    status === "idle" ||
    status === "suspended" ||
    status === "error" ||
    status === "deleted" ||
    status === "expired" ||
    isTerminal(status)
  );
}

function sessionToRun(session: Session): Run {
  const id = session.sessionId ?? session.id;
  return {
    id,
    status: String(session.status),
    ...(typeof session.workspaceId === "string" ? { workspaceId: session.workspaceId } : {}),
    ...(typeof session.createdAt === "string" ? { createdAt: session.createdAt } : {}),
    ...(typeof session.updatedAt === "string" ? { updatedAt: session.updatedAt } : {}),
    ...(session.errorMessage !== undefined ? { errorMessage: session.errorMessage } : {}),
    ...(session.usage ? { usage: session.usage } : {})
  };
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

/** Escape a literal string for safe interpolation into a RegExp. */
function escapeRegExp(input: string): string {
  return input.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function isOutputPathSelector(selector: OutputFileSelector): selector is OutputFilePathSelector {
  return Boolean(selector && typeof selector === "object" && "path" in selector);
}

function resolveOutputFileSelector(
  outputs: readonly Output[],
  selector: OutputFileSelector,
  runId: string
): Output {
  if (isOutputPathSelector(selector)) {
    const target = normalizeOutputLookupPath(selector.path);
    if (!target) {
      throw new RunStateError("Aex.downloadOutput: output path must be non-empty", {
        runId,
        path: selector.path
      });
    }
    const matches = outputs.filter((output) => {
      if (typeof output.filename !== "string") return false;
      const filename = normalizeOutputLookupPath(output.filename);
      if (selector.match === "suffix") {
        return filename === target || filename.endsWith(`/${target}`);
      }
      return filename === target;
    });
    if (matches.length === 1) return matches[0]!;
    if (matches.length > 1) {
      throw new RunStateError(
        `Aex.downloadOutput: output path "${selector.path}" matched multiple files`,
        { runId, path: selector.path, matches: matches.map((output) => output.filename ?? output.id) }
      );
    }
    throw new RunStateError(`Aex.downloadOutput: output path "${selector.path}" was not found`, {
      runId,
      path: selector.path
    });
  }
  if (typeof selector.id !== "string" || selector.id.length === 0) {
    throw new RunStateError("Aex.downloadOutput: selector must include an output id or path", { runId });
  }
  return { ...selector, id: selector.id };
}

function normalizeOutputLookupPath(path: string): string {
  return path.replace(/\\/g, "/").replace(/^\/+/, "");
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

function generateIdempotencyKey(): string {
  const cryptoObj = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (cryptoObj?.randomUUID) return cryptoObj.randomUUID();
  return `idem-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

/**
 * Derive the message idempotency key from the session-create key. Mirrors the
 * CLI (`<createKey>:message`) so a retried `run` / `sessions.run` that reuses
 * one `idempotencyKey` de-duplicates BOTH the create and the billable turn.
 */
function deriveMessageKey(createKey: string): string {
  return `${createKey}:message`;
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

/** Last-resort throttle detection from a free-text run error message. */
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
      throw new RunConfigValidationError(`${surface}: ${field} must be a non-empty string`);
    }
    return input;
  }
  if (!Array.isArray(input) || input.length === 0) {
    throw new RunConfigValidationError(`${surface}: ${field} must be a non-empty string or string array`);
  }
  for (const segment of input) {
    if (typeof segment !== "string" || !segment) {
      throw new RunConfigValidationError(`${surface}: ${field} segments must be non-empty strings`);
    }
  }
  return [...input];
}

function assertNoLegacySessionFields(options: SessionCreateOptions, surface: string): void {
  const record = options as unknown as Record<string, unknown>;
  const removedProxyField = "proxy" + "Endpoints";
  const messages: Record<string, string> = {
    input: "send user messages with session.send(...) or use run({ message }).",
    prompt: "use message for one-shot run input or session.send(...) for follow-up messages.",
    instructions: "use system.",
    idleSuspendAfter: "use overrides.idleTtl.",
    idleTtl: "use overrides.idleTtl.",
    retention: "use overrides.idleTtl.",
    secretEnv: "use environment.secrets.",
    skills: "skills are now tools; build one with Tools.fromSkillDir/fromSkillUrl and pass it in tools.",
    secrets: "use top-level apiKeys for provider keys and environment.secrets for run secrets.",
    runtimeSize: "use runtime.",
    parentRunId: "subagents are session-internal; parentRunId is not part of the session API.",
    limits: "use overrides.",
    timeout: "use overrides.timeout.",
    signal: "use session.cancel() / session.suspend() for remote control.",
    postHook: "send a follow-up validation message when the session returns idle.",
    [removedProxyField]: "proxy endpoints are not part of the public SDK session API."
  };
  for (const [field, message] of Object.entries(messages)) {
    if (Object.prototype.hasOwnProperty.call(record, field)) {
      throw new RunConfigValidationError(`${surface}: ${field} is not a supported option; ${message}`);
    }
  }
  const overrides = record.overrides;
  if (overrides && typeof overrides === "object" && !Array.isArray(overrides)) {
    const overrideRecord = overrides as Record<string, unknown>;
    if (Object.prototype.hasOwnProperty.call(overrideRecord, "idleSuspendAfter")) {
      throw new RunConfigValidationError(
        `${surface}: overrides.idleSuspendAfter is not a supported option; use overrides.idleTtl.`
      );
    }
  }
}

function assertNoSessionSendSignal(options: unknown, surface: string): void {
  const record = options as Record<string, unknown> | undefined;
  if (record && typeof record === "object" && Object.prototype.hasOwnProperty.call(record, "signal")) {
    throw new RunConfigValidationError(`${surface}: signal is not a supported option; use session.cancel() / session.suspend() for remote control.`);
  }
}

function validateApiKeys(
  apiKeys: Partial<Record<RunProvider, string>> | undefined,
  provider: RunProvider,
  surface: string
): void {
  const key = apiKeys?.[provider];
  if (typeof key !== "string" || key.length === 0) {
    throw new RunConfigValidationError(
      `${surface}: a provider API key is required — pass apiKeys[${JSON.stringify(provider)}].`
    );
  }
}

function outputsForWire(outputs: SessionCreateOptions["outputs"]): PlatformSubmission["outputs"] | undefined {
  if (outputs === undefined) {
    return undefined;
  }
  const allowedDirs = outputs.allowedDirs?.filter((dir) => dir.length > 0);
  const deniedDirs = outputs.deniedDirs?.filter((dir) => dir.length > 0);
  const hasNumericOverride =
    outputs.captureTimeoutMs !== undefined ||
    outputs.maxFileBytes !== undefined ||
    outputs.maxTotalBytes !== undefined ||
    outputs.maxFiles !== undefined;
  if ((allowedDirs?.length ?? 0) === 0 && (deniedDirs?.length ?? 0) === 0 && !hasNumericOverride) {
    return undefined;
  }
  return {
    ...(allowedDirs && allowedDirs.length > 0 ? { allowedDirs } : {}),
    ...(deniedDirs && deniedDirs.length > 0 ? { deniedDirs } : {}),
    ...(outputs.captureTimeoutMs !== undefined ? { captureTimeoutMs: outputs.captureTimeoutMs } : {}),
    ...(outputs.maxFileBytes !== undefined ? { maxFileBytes: outputs.maxFileBytes } : {}),
    ...(outputs.maxTotalBytes !== undefined ? { maxTotalBytes: outputs.maxTotalBytes } : {}),
    ...(outputs.maxFiles !== undefined ? { maxFiles: outputs.maxFiles } : {})
  };
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

/**
 * Stages a draft bundle's bytes to the content-addressable asset store and
 * returns the resulting asset id. Satisfied by `Aex._uploadAsset`.
 */
type AssetUploader = (args: {
  readonly bytes: Uint8Array;
  readonly hash: string;
  readonly contentType?: string;
}) => Promise<UploadedAsset>;

/**
 * A draft asset instance: yields its bytes once and caches the resolved asset id
 * so reuse across submits skips a re-upload (uploads are content-hash deduped).
 */
interface DraftAsset {
  readonly _cachedAssetId: string | undefined;
  _rememberAsset(assetId: string): void;
}

/**
 * Resolve a draft's asset id: reuse the cached id from a prior submit, otherwise
 * upload the bytes and cache the result on the instance.
 */
async function resolveAssetId(
  entry: DraftAsset,
  bundle: { readonly bytes: Uint8Array; readonly contentHash: string },
  uploader: AssetUploader
): Promise<string> {
  const cached = entry._cachedAssetId;
  if (cached !== undefined) {
    return cached;
  }
  const uploaded = await uploader({
    bytes: bundle.bytes,
    hash: bundle.contentHash,
    contentType: "application/zip"
  });
  entry._rememberAsset(uploaded.assetId);
  return uploaded.assetId;
}

/**
 * Split the `tools` union into custom tool refs (drafts eagerly uploaded as
 * assets), skill-tool refs (skill bundles eagerly uploaded as assets), and
 * builtin tool-name references (bare strings, validated against the closed
 * {@link BUILTIN_TOOL_NAMES} set). Builtin names are deduped, in input order;
 * the three groups are recombined on the wire by the caller (the BFF parser
 * splits them back apart by kind).
 */
async function prepareTools(
  tools: readonly (Tool | SkillTool | BuiltinToolName)[],
  uploader: AssetUploader
): Promise<{
  readonly refs: readonly ToolRef[];
  readonly skillToolRefs: readonly SkillToolRef[];
  readonly builtinNames: readonly BuiltinToolName[];
}> {
  const refs: ToolRef[] = [];
  const skillToolRefs: SkillToolRef[] = [];
  const seenBuiltins = new Set<BuiltinToolName>();
  const builtinNames: BuiltinToolName[] = [];
  for (let i = 0; i < tools.length; i++) {
    const entry = tools[i];
    // A bare string is a builtin tool reference.
    if (typeof entry === "string") {
      if (!(BUILTIN_TOOL_NAMES as readonly string[]).includes(entry)) {
        throw new RunConfigValidationError(
          `aex: tools[${i}] (${JSON.stringify(entry)}) is not a builtin tool name; ` +
            `expected a Tool, a SkillTool, or one of: ${BUILTIN_TOOL_NAMES.join(", ")}`
        );
      }
      if (!seenBuiltins.has(entry)) {
        seenBuiltins.add(entry);
        builtinNames.push(entry);
      }
      continue;
    }
    // A skill-tool: upload its bundle (if a draft) and emit a `kind:"skill"` ref.
    if (entry instanceof SkillTool) {
      const ref = entry.ref;
      if (ref.kind === "draft") {
        const bundle = entry._takeDraftBundle();
        if (!bundle) {
          throw new RunConfigValidationError(`aex: tools[${i}] is a draft skill-tool but has no bytes`);
        }
        const assetId = await resolveAssetId(entry, bundle, uploader);
        skillToolRefs.push({ kind: "skill", assetId, name: bundle.name, description: bundle.description });
        continue;
      }
      skillToolRefs.push(ref);
      continue;
    }
    if (!(entry instanceof Tool)) {
      throw new RunConfigValidationError(`aex: tools[${i}] must be a Tool, a SkillTool, or a builtin tool name`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new RunConfigValidationError(`aex: tools[${i}] is draft but has no bytes`);
      }
      const assetId = await resolveAssetId(entry, bundle, uploader);
      refs.push({ ...bundle.ref, assetId });
      continue;
    }
    refs.push(ref);
  }
  return { refs, skillToolRefs, builtinNames };
}

/** Walk AgentsMd[], eagerly upload drafts as assets, and return plain asset refs. */
async function prepareAgentsMd(
  agentsMds: readonly AgentsMd[],
  uploader: AssetUploader
): Promise<readonly AgentsMdRef[]> {
  const refs: AgentsMdRef[] = [];
  for (let i = 0; i < agentsMds.length; i++) {
    const entry = agentsMds[i];
    if (!(entry instanceof AgentsMd)) {
      throw new RunConfigValidationError(`aex: agentsMd[${i}] must be an AgentsMd instance`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new RunConfigValidationError(`aex: agentsMd[${i}] is draft but has no bytes`);
      }
      const assetId = await resolveAssetId(entry, bundle, uploader);
      refs.push({
        kind: "asset",
        assetId,
        name: bundle.name
      });
      continue;
    }
    refs.push(ref);
  }
  return refs;
}

/** Walk File[], eagerly upload drafts as assets, and return plain asset refs. */
async function prepareFiles(
  files: readonly File[],
  uploader: AssetUploader
): Promise<readonly FileRef[]> {
  const refs: FileRef[] = [];
  for (let i = 0; i < files.length; i++) {
    const entry = files[i];
    if (!(entry instanceof File)) {
      throw new RunConfigValidationError(`aex: files[${i}] must be a File instance`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new RunConfigValidationError(`aex: files[${i}] is draft but has no bytes`);
      }
      const assetId = await resolveAssetId(entry, bundle, uploader);
      refs.push({
        kind: "asset",
        assetId,
        name: bundle.name,
        mountPath: bundle.mountPath
      });
      continue;
    }
    refs.push(ref);
  }
  return refs;
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
      throw new RunConfigValidationError(`aex: mcpServers[${i}] must be an McpServer instance`);
    }
    submissionMcpServers.push(entry.toSubmissionEntry());
    const secret = entry.toSecretEntry();
    if (secret) {
      const existing = secretByName.get(secret.name);
      if (existing && existing.url !== secret.url) {
        throw new RunConfigValidationError(
          `aex: mcpServers[${i}].url conflicts with secrets.mcpServers["${secret.name}"]`
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

export type { OutputFileType, OutputLink, OutputLinkOptions, OutputQuery };
