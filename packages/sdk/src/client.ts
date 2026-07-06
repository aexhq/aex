import {
  AexApiError,
  CredentialValidationError,
  DEFAULT_RUN_PROVIDER,
  HttpClient,
  PLANE_BASE_URLS,
  RunConfigValidationError,
  RunStateError,
  SecretString,
  asAexEventView,
  customName,
  isSessionParked as isSessionParkedEvent,
  isRunSettled,
  assertStreamableOutputMode,
  operations,
  parseApiKey,
  resolveModelProvider,
  streamCoordinatorEvents,
  usageFromProviderUsage,
  type AexEvent,
  type AexEventView,
  type AgentsMdRecord,
  type AgentsMdRef,
  type ApprovalGate,
  type BatchItemResult,
  type BatchResult,
  type BillingCheckoutRequest,
  type BillingHostedSession,
  type BillingLedgerPage,
  type BillingLedgerQuery,
  type BillingPortalRequest,
  type BillingSummary,
  type ChildRunRef,
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
  type ResponseFormat,
  type OutputSearchQuery,
  type OutputSearchHit,
  type OutputSearchPage,
  type RunCostProviderUsage,
  type RunOutcome,
  type Session,
  type SessionCreateRequest,
  type SessionEvent,
  type SessionListPage,
  type SessionListQuery,
  type SessionMessage,
  type SessionMessageAccepted,
  type SessionRetentionPolicy,
  type SessionStateChangeAccepted,
  type SessionTerminalOutcome,
  type SessionTurn,
  type SettledResult,
  type PlatformEnvironmentInput,
  type PlatformSubmission,
  type PlatformInlineSecrets,
  type PlatformMcpServerSecret,
  type Run,
  type RunModel,
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
  SKILLS_MAX,
  type SkillRecord,
  type SkillRef,
  type ToolRef,
  type WebhookSigningSecret,
  type WebSocketFactory,
  type WhoAmI,
  TERMINAL_RUN_STATUSES
} from "@aexhq/contracts";
import { AgentsMd } from "./agents-md.js";
import { uploadAsset, uploadAssetMultipart, type AssetFetch, type UploadedAsset } from "./asset-upload.js";
import { File, type ZipStreamDriver } from "./file.js";
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
import { Skill, type SkillUploader } from "./skill.js";
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
 * The unified SETTLED result of {@link Aex.run}. Extends the contracts
 * {@link SettledResult} (the ONE settled shape `run()` and `done()` share), so
 * the terminal `status` (a {@link SessionTerminalOutcome}), `ok`, `costUsd`
 * (`number`, `>= 0`), and `usage` are ALWAYS present — `run()` awaits the settle
 * commit by default. Adds the one-shot conveniences: the run-compatible record,
 * events, decoded trace, assistant text, and captured outputs.
 *
 * `T` is the `responseFormat` decode type: when the run was submitted with a
 * `json_schema` `responseFormat`, {@link outcome} carries the typed decoded
 * value or a typed refusal.
 */
export interface RunResult<T = unknown> extends SettledResult {
  readonly runId: string;
  /** The session id used as the run-compatible handle. */
  readonly sessionId?: string;
  /** Run-compatible view of the underlying session record. */
  readonly run: Run;
  /** The underlying resumable session record (its lifecycle `status` is idle/suspended when resumable). */
  readonly session?: Session;
  /** The turn accepted for this one-shot run. */
  readonly turn?: SessionTurn;
  /** The assistant's final text. */
  readonly text: string;
  /** Assistant messages projected from the settled event stream. */
  readonly messages: readonly Message[];
  /** The session turn event stream — each event carries the `is*()` type-guard methods. */
  readonly events: readonly AexEventView[];
  /** Decoded view of the events: tool calls + usage + assistant text. */
  readonly trace: RunTrace;
  /** The run's captured output files. */
  readonly outputs: readonly Output[];
  /**
   * The typed schema-decode outcome — present only when the run was submitted
   * with a `json_schema` `responseFormat`: `{ kind:'decoded', value }` or
   * `{ kind:'refused', reason }`. There is no untyped path that yields a
   * hallucinated object.
   */
  readonly outcome?: RunOutcome<T>;
}

/** How a one-shot / turn resolves: at the render-complete park, or (default) at the settle commit. */
export type SettleAwait = "park" | "settle";

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
   * When the result resolves. `'settle'` (DEFAULT) waits (bounded) for the
   * settle commit after the turn parks, so `costUsd`/`usage`/terminal `status`
   * are always present. `'park'` returns at the render-complete park event for
   * latency-sensitive streaming — cost/usage are then best-effort (the settle
   * write lands tens of seconds later).
   */
  readonly await?: SettleAwait;
}

/** The result of {@link Aex.submit}: the run id + a resumable session handle. */
export interface SubmitResult {
  readonly runId: string;
  readonly session: SessionHandle;
}

/** Options for {@link Aex.batch}. */
export interface BatchOptions {
  /** Max concurrent items, clamped to `[1, 10]` (below the workspace tier cap). */
  readonly concurrency?: number;
  /** Reserved — the cost/usage rollup is always computed on the result. */
  readonly rollup?: boolean;
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
   * Per-run iteration cap (agent loop turns). Defaults + ceiling are enforced
   * server-side; omit to accept the platform default. A positive integer.
   */
  readonly maxTurns?: number;
}

/**
 * Options for opening a session (the low-level API) or a one-shot `run`.
 * Everything the agent needs is spelled out at the call site:
 *
 *   - `model` / `system` — the agent's brief.
 *   - `tools` — custom `Tool` bundles and builtin tool-name references; local
 *     custom-tool instances are materialized to the hosted asset store before
 *     the session lands.
 *   - `skills` — workspace skill bundles from `Skill.fromDir` / `Skill.fromUrl`
 *     / `Skill.fromFiles` / …; the SDK upserts them by name before the session
 *     lands, and the wire submission references only those names.
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
   * Tools available to the agent. Each entry is a custom {@link Tool} bundle or
   * a BUILTIN tool reference — a bare name string, preferably `BuiltinTools.<name>`
   * so a typo is a compile error.
   */
  readonly tools?: readonly (Tool | BuiltinToolName)[];
  /**
   * Workspace skills available to the agent. Build them with the `Skill.from*`
   * factories; the SDK upserts each bundle into the workspace skill registry by
   * name, then sends only `{ kind:"skill", name }` in the submission.
   */
  readonly skills?: readonly Skill[];
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
   * Assistant-output granularity. `"buffered"` (default) delivers ONE coalesced
   * `TEXT_MESSAGE_CONTENT` per assistant message. `"stream"` delivers per-token
   * `TEXT_MESSAGE_CONTENT` DELTAS (each `event.isTextMessage()` with
   * `event.data.delta === true`) as they arrive — but this is CAPABILITY-GATED:
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
   * `run<T>()`'s `result.outcome` (`{ kind:'decoded', value }` or
   * `{ kind:'refused', reason }`) — never an untyped hallucinated object.
   */
  readonly responseFormat?: ResponseFormat;
  /**
   * Declarative HITL write-gate: the platform parks the session
   * `awaiting_approval` BEFORE dispatching any tool in `tools`, independent of
   * model prose. Resume with `session.approve()` or reject with
   * `session.deny()`.
   */
  readonly approvalGate?: ApprovalGate;
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
  /**
   * When the turn resolves. `'settle'` (DEFAULT) awaits the settle commit so
   * `costUsd`/`usage`/terminal `status` are present on the result; `'park'`
   * returns at the render-complete park event (cost/usage then best-effort).
   */
  readonly await?: SettleAwait;
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

/**
 * The unified SETTLED result of one turn (`session.send(...).done()`). Extends
 * the contracts {@link SettledResult}, so `done()` returns the SAME shape as
 * `run()`: the terminal `status` (a {@link SessionTerminalOutcome}), `ok`,
 * `costUsd`, and `usage` are always present (the turn awaits settle by default).
 */
export interface SessionTurnResult<T = unknown> extends SettledResult {
  readonly sessionId: string;
  readonly session: Session;
  readonly turn: SessionTurn;
  readonly text: string;
  readonly events: readonly AexEventView[];
  readonly outputs: readonly Output[];
  readonly messages: readonly Message[];
  /** The typed schema-decode outcome when a `json_schema` `responseFormat` was set. */
  readonly outcome?: RunOutcome<T>;
}

export interface SessionRunResult extends SessionTurnResult {}

export class SessionTurnStream implements AsyncIterable<AexEventView> {
  readonly #run: () => AsyncGenerator<AexEventView, SessionTurnResult, void>;
  #generator: AsyncGenerator<AexEventView, SessionTurnResult, void> | undefined;
  #outcome:
    | { readonly ok: true; readonly value: SessionTurnResult }
    | { readonly ok: false; readonly error: unknown }
    | undefined;
  #done: Promise<SessionTurnResult> | undefined;

  constructor(run: () => AsyncGenerator<AexEventView, SessionTurnResult, void>) {
    this.#run = run;
  }

  /**
   * ONE underlying send per turn: iterating the stream and calling `done()`
   * (the documented `for await … ; await turn.done()` pattern) must share a
   * single generator — a fresh generator per consumer would re-POST the
   * message as a second billable turn (or 409 `session_busy` mid-turn).
   */
  #shared(): AsyncGenerator<AexEventView, SessionTurnResult, void> {
    this.#generator ??= this.#capture(this.#run());
    return this.#generator;
  }

  async *#capture(
    generator: AsyncGenerator<AexEventView, SessionTurnResult, void>
  ): AsyncGenerator<AexEventView, SessionTurnResult, void> {
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

  [Symbol.asyncIterator](): AsyncIterator<AexEventView> {
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
 * Accessor over the session's event stream (`session.events()`). EVERY surface
 * yields the one canonical {@link AexEventView} (guard-bearing, non-optional
 * populated `sequence`): the buffered snapshot `list()`, the polling `stream()`
 * iterator, the live coordinator `streamEnvelopes()` iterator, and the
 * events-namespace archive.
 */
export interface SessionEvents {
  list(): Promise<readonly AexEventView[]>;
  last(): Promise<AexEventView | undefined>;
  first(): Promise<AexEventView | undefined>;
  stream(options?: StreamEventsOptions): AsyncIterable<AexEventView>;
  streamEnvelopes(options?: StreamEnvelopesOptions): AsyncIterable<AexEventView>;
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
  /**
   * Search THIS session's captured outputs by filename (`string | RegExp`) /
   * extension / content type. Metadata-only (reference hits, no bytes). A
   * content-shaped query throws a typed "content search unsupported" rather than
   * silently returning 0 hits.
   */
  search(query?: PerSessionOutputSearchQuery): Promise<OutputSearchPage>;
  link(selectorOrQuery: OutputLinkSelector, options?: OutputLinkOptions): Promise<OutputLink>;
  fetch(selectorOrQuery: OutputLinkSelector, options?: OutputLinkOptions): Promise<Response>;
  /** No selector = outputs-namespace zip; with selector = one file's raw bytes. */
  download(selector?: OutputFileSelector, options?: OutputDownloadOptions): Promise<Uint8Array>;
}

/** A per-session output search — {@link OutputSearchQuery} without the cross-run `runIds` corpus. */
export type PerSessionOutputSearchQuery = Omit<OutputSearchQuery, "runIds">;

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

  async *#send(input: SessionInput, options: InternalSessionSendOptions): AsyncGenerator<AexEventView, SessionTurnResult, void> {
    const idempotencyKey = operations.resolveIdempotencyKey(options.idempotencyKey);
    this.#lastSend = { input, idempotencyKey };
    const accepted = await this.#acceptTurn(input, idempotencyKey, options.signal);
    this.#session = accepted.session;
    const turn = accepted.turn;
    const events: AexEventView[] = [];
    for await (const event of streamSessionTurnEvents(this.#http, this.id, turn, {
      ...options,
      from: options.from ?? accepted.eventCursor ?? turn.eventCursor ?? 0
    })) {
      events.push(event);
      yield event;
    }
    // Read the CARRIED terminal outcome from the events (never re-derive a lossy idle).
    const read = terminalSessionStatusFromEvents(events, turn.turnSeq);
    // Await the settle commit by DEFAULT so cost/usage + the terminal outcome are
    // always present; `await: 'park'` returns at the render-complete park event.
    const readSession = await operations.getSession(this.#http, this.id).catch(() => this.#session);
    const settled =
      (options.await ?? "settle") === "park"
        ? readSession
        : (await settledSessionRecord(this.#http, this.id, readSession, options.signal)) ?? readSession;
    this.#session = withTerminalSessionStatus(settled, read);
    const outputs = await operations.listSessionOutputs(this.#http, this.id).catch(() => [] as readonly Output[]);
    const messages = projectAssistantMessages(events);
    return settledTurnResult(this.id, this.#session, turn, events, outputs, messages, read);
  }

  /**
   * POST the next turn, reconciling the settle-lag race. A turn stream ends on
   * the idle park EVENT, but the session RECORD can lag at `running` for a short
   * window before the platform commits it — so an immediate follow-up `send()`
   * 409s `session_busy` even though, from the caller's view, the previous turn
   * already parked. When that happens we poll the record until it leaves
   * `running`, then retry the SAME idempotent POST (a replay de-duplicates, so
   * this never creates a second billable turn). A session that stays busy past
   * {@link SESSION_BUSY_RECONCILE_DEADLINE_MS} is a genuinely in-flight turn: we
   * surface a clear, bounded {@link RunStateError} rather than masking it or
   * waiting forever. Any non-`running` busy status (suspended / cancelling /
   * deleted) is a real rejection and passes straight through.
   */
  async #acceptTurn(
    input: SessionInput,
    idempotencyKey: string,
    signal: AbortSignal | undefined
  ): Promise<SessionMessageAccepted> {
    const deadline = Date.now() + SESSION_BUSY_RECONCILE_DEADLINE_MS;
    for (;;) {
      try {
        return await operations.sendSessionMessage(this.#http, this.id, { input }, { idempotencyKey });
      } catch (err) {
        if (!isSettlingSessionBusy(err) || signal?.aborted) throw err;
        if (Date.now() >= deadline) {
          throw new RunStateError(
            `SessionHandle.send: session ${this.id} was still running ${SESSION_BUSY_RECONCILE_DEADLINE_MS}ms after the previous turn parked — a turn is still in flight`,
            { sessionId: this.id, status: "running", cause: err }
          );
        }
        await this.#awaitLeftRunning(deadline, signal);
      }
    }
  }

  /**
   * Poll the session record until it is no longer `running`/`creating`, or the
   * reconcile deadline elapses. Updates the stored record so the eventual retry
   * (or the terminal error) reflects the freshest status.
   */
  async #awaitLeftRunning(deadline: number, signal: AbortSignal | undefined): Promise<void> {
    for (;;) {
      try {
        await sleep(SESSION_BUSY_RECONCILE_INTERVAL_MS, signal);
      } catch {
        return; // aborted — let the retry POST surface the real state
      }
      const record = await operations.getSession(this.#http, this.id).catch(() => undefined);
      if (record !== undefined) {
        this.#session = record;
        if (record.status !== "running" && record.status !== "creating") return;
      }
      if (Date.now() >= deadline) return;
    }
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
   * Request the HITL write-gate: park this session `awaiting_approval` before
   * its next gated action. Imperative counterpart to the declarative
   * `approvalGate` submission option. Resume with {@link approve} / reject with
   * {@link deny}.
   */
  async requestApproval(options: Pick<SessionSendOptions, "idempotencyKey"> = {}): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.requestApproval(this.#http, this.id, options);
    this.#session = accepted.session;
    return accepted;
  }

  /** Approve an `awaiting_approval` session so the held turn resumes (→ running). */
  async approve(options: Pick<SessionSendOptions, "idempotencyKey"> = {}): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.approveSession(this.#http, this.id, options);
    this.#session = accepted.session;
    return accepted;
  }

  /** Deny an `awaiting_approval` session so the held turn is cancelled (→ cancelled). */
  async deny(options: Pick<SessionSendOptions, "idempotencyKey"> = {}): Promise<SessionStateChangeAccepted> {
    const accepted = await operations.denySession(this.#http, this.id, options);
    this.#session = accepted.session;
    return accepted;
  }

  /**
   * Enumerate this run's subagent CHILD runs (`GET /runs/:id/children`). Each is
   * a {@link ChildRunHandle} backed by the RUN facade (getRun/events/outputs) —
   * NOT `openSession` — so every child the platform hands you is resolvable, with
   * its lineage (`parentRunId`/`depth`) and terminal outcome exposed.
   */
  async children(): Promise<readonly ChildRunHandle[]> {
    const refs = await operations.listRunChildren(this.#http, this.id);
    return refs.map((ref) => new ChildRunHandle(this.#http, ref, this.#fetch));
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
   * snapshots (`list`/`last`/`first`), the polling `AexEventView` iterator
   * (`stream`), the live coordinator envelope iterator (`streamEnvelopes`), and
   * the events-namespace archive (`archiveLink`/`download`).
   */
  events(): SessionEvents {
    const http = this.#http;
    const id = this.id;
    const list = async (): Promise<readonly AexEventView[]> =>
      (await operations.listSessionEvents(http, id)).map(asAexEventView);
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

  async run(options: SessionRunOptions): Promise<SessionRunResult> {
    const { message, deleteAfter, messageIdempotencyKey, stream, ...createOptions } = options;
    assertNoLegacySessionFields(options, "Aex.sessions.run");
    const input = normaliseSessionInput(message, "Aex.sessions.run", "message");
    // Derive the message key from the create key (like the CLI) so a retried run
    // with the same `idempotencyKey` de-duplicates BOTH the create and the
    // billable turn — never a duplicate billable run.
    const createKey = operations.resolveIdempotencyKey(createOptions.idempotencyKey);
    const messageKey =
      messageIdempotencyKey !== undefined ? operations.resolveIdempotencyKey(messageIdempotencyKey) : deriveMessageKey(createKey);
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

/**
 * Cross-run output search (`aex.outputs`). Composed client-side (per-run
 * `listSessionOutputs` + the contracts output filter): scope a corpus with
 * `query.runIds`, or omit it to scan every run in the workspace. Metadata-only
 * (reference hits, no bytes); a content-shaped query throws a typed
 * "content search unsupported".
 */
export class OutputsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  async search(query: OutputSearchQuery = {}): Promise<OutputSearchPage> {
    assertMetadataOnlyOutputSearch(query, "aex.outputs.search");
    // Dedup the caller-supplied allow-list so a run repeated in `runIds` (from
    // concatenating corpora) isn't scanned twice and doesn't inflate hit count.
    const unscoped = query.runIds === undefined;
    const runIds = unscoped ? await this.#allRunIds() : [...new Set(query.runIds)];
    const limit = query.limit ?? 100;
    const hits: OutputSearchHit[] = [];
    for (const runId of runIds) {
      let outputs: readonly Output[];
      try {
        outputs = await searchRunOutputs(this.#http, runId, query);
      } catch (err) {
        if (unscoped && isMissingOutputsSession(err)) continue;
        throw err;
      }
      for (const hit of outputHits(runId, outputs, limit - hits.length)) {
        hits.push(hit);
        if (hits.length >= limit) return { hits };
      }
    }
    return { hits };
  }

  /** Enumerate every run id in the workspace by paging `listSessions`. */
  async #allRunIds(): Promise<readonly string[]> {
    const ids: string[] = [];
    const seenCursors = new Set<string>();
    let cursor: string | undefined;
    do {
      if (cursor !== undefined) {
        if (seenCursors.has(cursor)) {
          throw new Error("aex.outputs.search: listSessions returned a repeated cursor");
        }
        seenCursors.add(cursor);
      }
      const page = await operations.listSessions(this.#http, cursor ? { cursor } : {});
      for (const session of page.sessions) ids.push(session.id);
      cursor = page.nextCursor;
    } while (cursor);
    return ids;
  }
}

/** A run-facade events accessor (list + polling stream) keyed on a run id. */
export interface RunEvents {
  list(): Promise<readonly AexEventView[]>;
  stream(options?: StreamEventsOptions): AsyncIterable<AexEventView>;
}

/** A run-facade outputs accessor (a subset of {@link SessionOutputs}) keyed on a run id. */
export interface RunOutputs {
  list(query?: OutputQuery): Promise<readonly Output[]>;
  find(query: OutputQuery): Promise<readonly Output[]>;
  findOne(query: OutputQuery): Promise<Output | null>;
  read(selector: OutputFileSelector, options?: ReadOutputTextOptions): Promise<OutputText>;
  link(selectorOrQuery: OutputLinkSelector, options?: OutputLinkOptions): Promise<OutputLink>;
  download(selector?: OutputFileSelector, options?: OutputDownloadOptions): Promise<Uint8Array>;
}

/**
 * A first-class, lineage-discoverable SUBAGENT CHILD run — handed out by
 * `session.children()` / `run.children()`, backed by the RUN facade
 * (getRun/events/outputs), NOT `openSession`. Every child the platform hands you
 * is resolvable through this handle; its lineage (`parentRunId`/`depth`) and
 * terminal outcome (`status`) are first-class.
 */
export class ChildRunHandle {
  readonly #http: HttpClient;
  readonly #fetch: FetchLike | undefined;
  readonly #ref: ChildRunRef;

  constructor(http: HttpClient, ref: ChildRunRef, fetch?: FetchLike) {
    this.#http = http;
    this.#ref = ref;
    this.#fetch = fetch;
  }

  get id(): string {
    return this.#ref.id;
  }

  get parentRunId(): string {
    return this.#ref.parentRunId;
  }

  get depth(): number | undefined {
    return this.#ref.depth;
  }

  /** The child's run status (a real run terminal outcome once settled). */
  get status(): string {
    return this.#ref.status;
  }

  get ref(): ChildRunRef {
    return this.#ref;
  }

  /** Re-read the child run record (status, lineage, costTelemetry). */
  get(): Promise<Run> {
    return operations.getRun(this.#http, this.id);
  }

  /** The child's events over the RUN facade (`/runs/:id/events`). */
  events(): RunEvents {
    return runEventsAccessor(this.#http, this.id);
  }

  /** The child's captured outputs over the RUN facade (`/runs/:id/outputs`). */
  outputs(): RunOutputs {
    return runOutputsAccessor(this.#http, this.id, this.#fetch);
  }

  /** This child's own subagent children (recursive lineage). */
  async children(): Promise<readonly ChildRunHandle[]> {
    const refs = await operations.listRunChildren(this.#http, this.id);
    return refs.map((ref) => new ChildRunHandle(this.#http, ref, this.#fetch));
  }

  /** Cancel the child run (run facade `POST /runs/:id/cancel`). */
  cancel(): Promise<void> {
    return operations.cancelRun(this.#http, this.id);
  }
}

/** Run-facade events accessor (list + polling stream) — used by {@link ChildRunHandle}. */
function runEventsAccessor(http: HttpClient, id: string): RunEvents {
  return {
    list: async () => (await operations.listRunEvents(http, id)).map(asAexEventView),
    stream: (options?: StreamEventsOptions) => streamRunEventsPolling(http, id, options ?? {})
  };
}

/** Run-facade outputs accessor — used by {@link ChildRunHandle}. */
function runOutputsAccessor(http: HttpClient, id: string, fetchLike: FetchLike | undefined): RunOutputs {
  void fetchLike;
  return {
    list: (query?: OutputQuery) => operations.listOutputs(http, id, query),
    find: (query) => operations.findOutputs(http, id, query),
    findOne: (query) => operations.findOutput(http, id, query),
    read: (selector, options) => operations.readOutputText(http, id, selector, options),
    link: (selectorOrQuery, options) => operations.outputLink(http, id, selectorOrQuery, options),
    download: (selector, options) => downloadSessionOutput(http, id, selector, options)
  };
}

/**
 * Poll a RUN's events (via the run facade) until it reaches a terminal status,
 * the signal aborts, or the caller breaks the iterator. Uses `getRun` (not
 * `getSession`) so it resolves for a child run that has no session facade.
 */
async function* streamRunEventsPolling(
  http: HttpClient,
  id: string,
  options: StreamEventsOptions
): AsyncIterable<AexEventView> {
  if (options.signal?.aborted) return;
  const seenIds = new Set<string>();
  const intervalMs = options.intervalMs ?? 1_000;
  const signal = options.signal;
  while (!signal?.aborted) {
    const events = await operations.listRunEvents(http, id);
    for (const event of events) {
      if (!seenIds.has(event.id)) {
        seenIds.add(event.id);
        yield asAexEventView(event);
      }
    }
    const run = await operations.getRun(http, id);
    if (isSessionParked(run.status)) return;
    try {
      await sleep(intervalMs, signal);
    } catch {
      return;
    }
  }
}

/** Default / max concurrent items for {@link Aex.batch} (bounded below the tier cap). */
const DEFAULT_BATCH_CONCURRENCY = 4;
const BATCH_MAX_CONCURRENCY = 10;

/** Sum the per-item settled cost/usage into a real {@link BatchResult} rollup. */
function rollupBatch<T>(results: readonly BatchItemResult<T>[]): BatchResult<T> {
  let totalCostUsd = 0;
  let okCount = 0;
  const failed: BatchItemResult<T>[] = [];
  for (const result of results) {
    totalCostUsd += result.costUsd;
    if (result.ok) okCount += 1;
    else failed.push(result);
  }
  return {
    results,
    totalCostUsd,
    totalUsage: sumUsageSummaries(results.map((result) => result.usage)),
    okCount,
    failed
  };
}

/** Field-wise sum of {@link UsageSummary} objects; a field is present iff some item carried it. */
function sumUsageSummaries(usages: readonly UsageSummary[]): UsageSummary {
  const keys = ["inputTokens", "outputTokens", "cacheReadInputTokens", "cacheCreationInputTokens", "totalTokens"] as const;
  const out: Mutable<UsageSummary> = {};
  for (const key of keys) {
    let sum: number | undefined;
    for (const usage of usages) {
      const value = usage[key];
      if (typeof value === "number") sum = (sum ?? 0) + value;
    }
    if (sum !== undefined) out[key] = sum;
  }
  return out;
}

async function* streamSessionTurnEvents(
  http: HttpClient,
  sessionId: string,
  turn: SessionTurn,
  options: InternalSessionSendOptions
): AsyncGenerator<AexEventView, void, void> {
  const first = await operations.getSessionCoordinatorTicket(http, sessionId);
  for await (const event of streamCoordinatorEvents({
    wsUrl: first.wsUrl,
    from: options.from ?? 0,
    fetchTicket: async () => (await operations.getSessionCoordinatorTicket(http, sessionId)).ticket,
    isTerminal: (event: SessionEvent) => isSessionTurnTerminalEvent(event, turn.turnSeq),
    ...(options.signal ? { signal: options.signal } : {}),
    ...(options.webSocketFactory ? { webSocketFactory: options.webSocketFactory } : {}),
    ...(options.idleTimeoutMs !== undefined ? { idleTimeoutMs: options.idleTimeoutMs } : {}),
    ...(options.pingIntervalMs !== undefined ? { pingIntervalMs: options.pingIntervalMs } : {})
  })) {
    yield asAexEventView(event);
  }
}

/**
 * Poll the session's event snapshots until the session parks, the signal
 * aborts, or the caller breaks the iterator, deduping by event id. Yields the
 * one canonical guard-bearing {@link AexEventView} (same shape as every other
 * event surface). Module-level so `SessionHandle.events()` can hand it to its
 * accessor object literal.
 */
async function* streamSessionEventsPolling(
  http: HttpClient,
  id: string,
  options: StreamEventsOptions
): AsyncIterable<AexEventView> {
  if (options.signal?.aborted) return;
  const seenIds = new Set<string>();
  const intervalMs = options.intervalMs ?? 1_000;
  const signal = options.signal;
  while (!signal?.aborted) {
    const events = await operations.listRunEvents(http, id);
    for (const event of events) {
      if (!seenIds.has(event.id)) {
        seenIds.add(event.id);
        yield asAexEventView(event);
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
): AsyncIterable<AexEventView> {
  const first = await operations.getSessionCoordinatorTicket(http, id);
  for await (const event of streamCoordinatorEvents({
    wsUrl: first.wsUrl,
    from: options.from ?? 0,
    fetchTicket: async () => (await operations.getSessionCoordinatorTicket(http, id)).ticket,
    // settleConsistent ends the stream on the post-mirror barrier instead of
    // the earlier RUN_FINISHED UX signal.
    isTerminal: options.settleConsistent ? isRunSettled : isSessionEnvelopeTerminal,
    ...(options.signal ? { signal: options.signal } : {})
  })) {
    yield asAexEventView(event);
  }
}

function isSessionEnvelopeTerminal(event: AexEvent): boolean {
  return event.type === "RUN_FINISHED" || event.type === "RUN_ERROR" || isSessionParkedEvent(event);
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
  // One selector-resolution path: the contracts `downloadOutput` lists-if-path
  // then downloads, throwing with PUBLIC verb names — no duplicated resolver.
  const bytes =
    selector === undefined
      ? await operations.downloadOutputs(http, id)
      : (await operations.downloadOutput(http, id, selector)).bytes;
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
    search: async (query: PerSessionOutputSearchQuery = {}) => {
      assertMetadataOnlyOutputSearch(query, "outputs().search");
      const outputs = await searchRunOutputs(http, id, query);
      return { hits: outputHits(id, outputs, query.limit ?? 100) };
    },
    link: (selectorOrQuery, options) => operations.outputLink(http, id, selectorOrQuery, options),
    fetch: async (selectorOrQuery, options) => {
      const link = await operations.outputLink(http, id, selectorOrQuery, options);
      return (fetchLike ?? globalThis.fetch)(link.url);
    },
    download: (selector, options) => downloadSessionOutput(http, id, selector, options)
  };
}

/**
 * List one run's outputs matching a metadata search: `extension`/`contentType`
 * via the contracts filter, and `filename` via {@link operations.toFilenameMatcher}
 * — a case-insensitive SUBSTRING for a string, `.test` for a RegExp (RegExp-safe;
 * no `escapeRegExp` footgun on a reused pattern).
 */
async function searchRunOutputs(
  http: HttpClient,
  runId: string,
  query: Omit<OutputSearchQuery, "runIds">
): Promise<readonly Output[]> {
  const listQuery: OutputQuery = {
    ...(query.extension !== undefined ? { extension: query.extension } : {}),
    ...(query.contentType !== undefined ? { contentType: query.contentType } : {})
  };
  const outputs = await operations.listSessionOutputs(
    http,
    runId,
    Object.keys(listQuery).length > 0 ? listQuery : undefined
  );
  if (query.filename === undefined) return outputs;
  const match = operations.toFilenameMatcher(query.filename);
  return outputs.filter((output) => typeof output.filename === "string" && match(output.filename));
}

/**
 * Fail-fast on a CONTENT-shaped search: the search surface is metadata-only, so
 * a `content`/`text`/`query` needle throws a typed error rather than silently
 * returning 0 hits (which reads as "no matches" for a query that was never run).
 */
function assertMetadataOnlyOutputSearch(query: object, surface: string): void {
  for (const key of ["content", "text", "query", "grep", "body"]) {
    if (Object.prototype.hasOwnProperty.call(query, key)) {
      throw new RunConfigValidationError(
        `${surface}: content search is not supported — search matches on filename/extension/contentType metadata only`,
        { field: key }
      );
    }
  }
}

/** Project a run's output files to reference-only {@link OutputSearchHit}s, capped. */
function outputHits(runId: string, outputs: readonly Output[], limit: number): OutputSearchHit[] {
  const hits: OutputSearchHit[] = [];
  for (const o of outputs) {
    hits.push({
      runId,
      outputId: o.id,
      ...(o.filename !== undefined ? { filename: o.filename } : {}),
      ...(o.sizeBytes !== undefined ? { sizeBytes: o.sizeBytes } : {}),
      ...(o.contentType !== undefined ? { contentType: o.contentType } : {})
    });
    if (hits.length >= limit) break;
  }
  return hits;
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

function projectAssistantMessages(events: readonly AexEvent[]): readonly Message[] {
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

function assistantTextFromEvents(events: readonly AexEvent[]): string {
  return assistantTextEntriesFromEvents(events).map((entry) => entry.text).join("");
}

function runTraceFromEvents(events: readonly AexEvent[]): RunTrace {
  return {
    toolCalls: toolCallsFromEvents(events),
    usage: usageFromEvents(events),
    text: assistantTextEntriesFromEvents(events)
  };
}

function assistantTextEntriesFromEvents(
  events: readonly AexEvent[]
): RunTrace["text"] {
  const out: Array<Mutable<RunTrace["text"][number]>> = [];
  for (const event of events) {
    if (event.type !== "TEXT_MESSAGE_CONTENT") continue;
    const data = asRecord(event.data);
    const text = typeof data.text === "string" ? data.text : undefined;
    if (text === undefined) continue;
    const entry: Mutable<RunTrace["text"][number]> = { text };
    const messageId = typeof data.messageId === "string" ? data.messageId : undefined;
    if (messageId !== undefined) entry.messageId = messageId;
    if (typeof event.sequence === "number") entry.seq = event.sequence;
    if (typeof event.time === "string") entry.recordedAt = event.time;
    out.push(entry);
  }
  return out;
}

function toolCallsFromEvents(events: readonly AexEvent[]): RunTrace["toolCalls"] {
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
      if (typeof event.sequence === "number") trace.startSeq = event.sequence;
      if (typeof event.time === "string") trace.startedAt = event.time;
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
 * The terminal READ a turn's terminal event carries: either a
 * {@link SessionTerminalOutcome} (succeeded/failed/timed_out/cancelled) or a
 * resumable/held lifecycle park (idle/suspended/awaiting_approval). The SDK
 * READS this from the carried event — it never re-derives a lossy `idle`.
 */
export type SessionTerminalRead = SessionTerminalOutcome | "idle" | "suspended" | "awaiting_approval";

const SESSION_TERMINAL_READS = new Set<string>([
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "idle",
  "suspended",
  "awaiting_approval"
]);

/** The CUSTOM `aex.session.<name>` terminal event names → the carried read. */
const SESSION_TERMINAL_EVENT_READS: Readonly<Record<string, SessionTerminalRead>> = {
  "aex.session.succeeded": "succeeded",
  "aex.session.failed": "failed",
  // Clean-cut: the bare `error` park is now `failed` (one terminal vocabulary).
  "aex.session.error": "failed",
  "aex.session.timed_out": "timed_out",
  "aex.session.cancelled": "cancelled",
  "aex.session.idle": "idle",
  "aex.session.suspended": "suspended",
  "aex.session.awaiting_approval": "awaiting_approval"
};

/** An explicit `data.value.outcome` a park/settle event may carry (the authoritative outcome). */
function carriedOutcome(event: AexEvent): SessionTerminalRead | undefined {
  const value = event.data.value;
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const outcome = (value as { readonly outcome?: unknown }).outcome;
  return typeof outcome === "string" && SESSION_TERMINAL_READS.has(outcome)
    ? (outcome as SessionTerminalRead)
    : undefined;
}

function isSessionTurnTerminalEvent(event: SessionEvent, turnSeq: number): boolean {
  if (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR") {
    return true;
  }
  const name = customName(event);
  if (name === null || !(name in SESSION_TERMINAL_EVENT_READS)) {
    return false;
  }
  const value = event.data.value;
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return true;
  }
  const eventTurnSeq = (value as { readonly turnSeq?: unknown }).turnSeq;
  return typeof eventTurnSeq !== "number" || eventTurnSeq === turnSeq;
}

/**
 * Read the CARRIED terminal outcome/park from the turn's terminal event — never
 * a lossy re-derivation. Prefers an explicit `data.value.outcome`, then the
 * `aex.session.<name>` name, then RUN_ERROR→failed / RUN_FINISHED→succeeded.
 */
function terminalSessionStatusFromEvents(events: readonly SessionEvent[], turnSeq: number): SessionTerminalRead | undefined {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (!isSessionTurnTerminalEvent(event, turnSeq)) continue;
    const carried = carriedOutcome(event);
    if (carried !== undefined) return carried;
    if (event.type === "RUN_ERROR") return "failed";
    if (event.type === "RUN_FINISHED") return "succeeded";
    const name = customName(event);
    if (name !== null && name in SESSION_TERMINAL_EVENT_READS) return SESSION_TERMINAL_EVENT_READS[name]!;
  }
  return undefined;
}

function withTerminalSessionStatus(session: Session, read: SessionTerminalRead | undefined): Session {
  if (read === undefined || session.status === read) return session;
  if (
    session.status !== "creating" &&
    session.status !== "running" &&
    session.status !== "suspending" &&
    session.status !== "cancelling"
  ) {
    return session;
  }
  return { ...session, status: read };
}

/** Map a terminal READ to the 4-value {@link SessionTerminalOutcome} for the result `status`. */
function readToOutcome(read: SessionTerminalRead): SessionTerminalOutcome {
  return read === "failed" || read === "timed_out" || read === "cancelled" ? read : "succeeded";
}

/** `true` for an OK terminal read (succeeded / resumable idle|suspended / held awaiting_approval). */
function isTerminalReadOk(read: SessionTerminalRead): boolean {
  return read !== "failed" && read !== "timed_out" && read !== "cancelled";
}

/** Best-effort READ off a settled record when the events carried none. */
function sessionRecordRead(session: Session): SessionTerminalRead {
  const outcome = session.lastTurnOutcome;
  if (outcome !== undefined && SESSION_TERMINAL_READS.has(outcome)) return outcome;
  const status = session.status;
  if (status === "idle" || status === "suspended" || status === "awaiting_approval") return status;
  if (status === "succeeded" || status === "failed" || status === "timed_out" || status === "cancelled") return status;
  if (status === "error") return "failed";
  return "succeeded";
}

/** The settle-written provider usage entries (the SINGLE token-usage source). */
function providerUsageOf(record: Session | Run): readonly RunCostProviderUsage[] | undefined {
  const telemetry = (record as {
    readonly costTelemetry?: { readonly providerUsage?: readonly RunCostProviderUsage[] };
  }).costTelemetry;
  return telemetry?.providerUsage;
}

/**
 * The IMMEDIATE authoritative failure text — the terminal `RUN_ERROR` event's
 * `data.failureMessage`. `result.error` reads this FIRST so a failed (e.g.
 * bad-BYOK) run's error is never empty even before the settle-lagged
 * `sessionRecord.errorMessage` lands.
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
function outcomeFromEvents<T = unknown>(events: readonly AexEventView[]): RunOutcome<T> | undefined {
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
 * Build the ONE unified settled turn result (shared by `session.send().done()`
 * and `Aex.run`): the terminal outcome `status`, `ok`, `costUsd` (>= 0),
 * `usage` (from `costTelemetry.providerUsage`), and event-first `error`.
 */
function settledTurnResult(
  sessionId: string,
  session: Session,
  turn: SessionTurn,
  events: readonly AexEventView[],
  outputs: readonly Output[],
  messages: readonly Message[],
  read: SessionTerminalRead | undefined
): SessionTurnResult {
  const effectiveRead = read ?? sessionRecordRead(session);
  const status = readToOutcome(effectiveRead);
  const ok = isTerminalReadOk(effectiveRead);
  const usage = usageFromProviderUsage(providerUsageOf(session));
  const costUsd = typeof session.costUsd === "number" ? session.costUsd : 0;
  const error =
    failureFromEvents(events) ??
    (!ok && typeof session.errorMessage === "string" && session.errorMessage ? session.errorMessage : undefined);
  const outcome = outcomeFromEvents(events);
  return {
    sessionId,
    session,
    turn,
    status,
    ok,
    costUsd,
    usage,
    ...(error !== undefined ? { error } : {}),
    text: assistantTextFromEvents(events),
    events,
    outputs,
    messages,
    ...(outcome !== undefined ? { outcome } : {})
  };
}

export interface StreamEventsOptions {
  /** Poll interval in ms for the event snapshot loop. Default 1000. */
  readonly intervalMs?: number;
  readonly signal?: AbortSignal;
}

export interface StreamEnvelopesOptions {
  /** Starting cursor — events with `sequence >= from` are delivered. Default 0. */
  readonly from?: number;
  readonly signal?: AbortSignal;
  /**
   * End the stream settle-consistently. By default the iterator ends on the
   * AG-UI terminal event (RUN_FINISHED / RUN_ERROR) or the managed-session
   * terminal park (`aex.session.succeeded` / failed / timed_out / cancelled /
   * idle / suspended). These are render-complete UX signals, emitted before the
   * platform commit is necessarily read-consistent, so a `getRun` immediately
   * after can still read `running`. With `settleConsistent: true` the iterator
   * keeps reading PAST the render terminal until the post-mirror
   * `aex.run.settled` barrier, so when it ends a subsequent `getRun` is
   * guaranteed terminal and `listOutputs` is complete.
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
 * Workspace skill registry operations exposed under `client.skills`.
 *
 * Session creation normally passes `Skill.from*(...)` instances directly in the
 * `skills` option (auto-upserted by name). This namespace is the metadata
 * read/delete surface for the named workspace skill registry.
 */
export class SkillsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(): Promise<readonly SkillRecord[]> {
    return operations.listSkills(this.#http);
  }

  get(name: string): Promise<SkillRecord> {
    return operations.getSkill(this.#http, name);
  }

  delete(name: string): Promise<void> {
    return operations.deleteSkill(this.#http, name);
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
  readonly skills: SkillsClient;
  readonly secrets: SecretsClient;
  readonly sessions: SessionClient;
  /** Cross-run output search (`aex.outputs.search(...)`). */
  readonly outputs: OutputsClient;

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
    // round-trip. An opaque/legacy key (no `aex_` shape) skips this and keeps
    // the HttpClient default.
    const baseUrl = resolveBaseUrlForKey(apiKey, resolved.baseUrl);
    // Wrap the transport fetch (the caller's override, or global `fetch`) with
    // the bounded-retry layer so every BFF request gets default resilience.
    // The raw `#fetch` below stays unwrapped for the direct-to-storage asset PUT
    // and presigned output GETs, which target object storage, not the API plane.
    const baseFetch: FetchLike = resolved.fetch ?? ((input: Parameters<FetchLike>[0], init: Parameters<FetchLike>[1]) => fetch(input, init));
    const retryingFetch = withRetry(baseFetch, resolved.retry);
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
    this.agentsMd = new AgentsMdClient(this.#http);
    this.files = new FilesClient(this.#http);
    this.skills = new SkillsClient(this.#http);
    this.secrets = new SecretsClient(this.#http);
    this.sessions = new SessionClient(this.#http, (options) => this.#buildSessionCreateRequest(options), this.#fetch);
    this.outputs = new OutputsClient(this.#http);
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
   * prepare step to upload draft skill / tool / agentsMd / file bundles so
   * the wire submission carries only plain `kind:"asset"` refs (skills resolve
   * to name-only `kind:"skill"` refs after their bytes upload).
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
   * Internal: materialize a LARGE draft (a `File.fromPath` over the streaming
   * threshold) to the content store via the two-pass streaming multipart flow —
   * hash the deterministic canonical-zip stream, presign by hash (dedup still
   * short-circuits), then upload it in parts. Bounded memory (one entry + one
   * part). NOT part of the public API.
   */
  async _uploadAssetStream(args: {
    readonly drive: ZipStreamDriver;
    readonly contentType?: string;
  }): Promise<UploadedAsset> {
    return uploadAssetMultipart({
      http: this.#http,
      drive: args.drive,
      ...(args.contentType ? { contentType: args.contentType } : {}),
      ...(this.#fetch ? { fetch: this.#fetch as unknown as AssetFetch } : {})
    });
  }

  /**
   * Internal: upsert already-uploaded skill metadata into the workspace registry.
   * The bytes are staged through `_uploadAsset`; this call binds the content hash
   * to a mutable workspace skill name before the run references that name.
   */
  async _upsertSkill(args: {
    readonly name: string;
    readonly contentHash: string;
    readonly description: string;
    readonly sizeBytes: number;
  }): Promise<{ readonly updated: boolean }> {
    return operations.upsertSkill(this.#http, args);
  }

  /**
   * Convenience one-shot on top of the canonical session API:
   * open a session, send `message` as the first turn, stream until the session
   * parks (`idle` / `suspended` / `error`), then return the collected text,
   * events, outputs, and session record. The returned `runId` is the session id,
   * so callers can resume later with `openSession(runId)`.
   */
  async run<T = unknown>(options: SessionRunOptions, opts: RunCollectOptions = {}): Promise<RunResult<T>> {
    const scopedSignal = scopedAbortSignal(opts.timeoutMs);
    try {
      const { message, deleteAfter, messageIdempotencyKey, stream, ...createOptions } = options;
      assertNoLegacySessionFields(options, "Aex.run");
      const input = normaliseSessionInput(message, "Aex.run", "message");
      assertNoSessionSendSignal(stream, "Aex.run stream");
      const sendOptions: InternalSessionSendOptions = {
        ...(stream ?? {}),
        ...(scopedSignal?.signal ? { signal: scopedSignal.signal } : {}),
        ...(opts.webSocketFactory ? { webSocketFactory: opts.webSocketFactory } : {}),
        ...(opts.idleTimeoutMs !== undefined ? { idleTimeoutMs: opts.idleTimeoutMs } : {}),
        ...(opts.pingIntervalMs !== undefined ? { pingIntervalMs: opts.pingIntervalMs } : {}),
        // run()/done() await settle by DEFAULT; opt out with `await: 'park'`.
        ...(opts.await !== undefined ? { await: opts.await } : {})
      };
      // Derive the message key from the create key (like the CLI) so a retried
      // run with the same `idempotencyKey` de-duplicates BOTH the create and the
      // billable turn server-side — never a duplicate billable run (sdk-dx-3).
      const createKey = operations.resolveIdempotencyKey(createOptions.idempotencyKey);
      const messageKey =
        messageIdempotencyKey !== undefined ? operations.resolveIdempotencyKey(messageIdempotencyKey) : deriveMessageKey(createKey);
      const session = await this.sessions.create({ ...createOptions, idempotencyKey: createKey });
      // ONE terminal boundary: `done()` awaits settle by default, so the turn
      // result already carries the terminal outcome + cost + usage. `run()` just
      // reshapes it — `done()` == `run()` (WS3).
      let turnResult: SessionTurnResult;
      try {
        turnResult = await sendSessionInternal(session, input, { ...sendOptions, idempotencyKey: messageKey }).done();
      } catch (err) {
        if (scopedSignal?.signal.aborted) {
          // The client-side wait budget (opts.timeoutMs) expired. Parity with
          // SessionHandle.wait(): THROW rather than a misleading silent result;
          // the run continues server-side.
          throw new RunStateError(
            `Aex.run: timed out after ${opts.timeoutMs}ms waiting for run ${session.id} to park; the run ` +
              `continues server-side — cancel via session.cancel() or resume with openSession(${JSON.stringify(session.id)})`
          );
        }
        throw err;
      }
      const runId = turnResult.sessionId;
      if (deleteAfter) {
        await session.delete();
      }
      const sessionRecord = turnResult.session;
      const run = sessionToRun(sessionRecord);
      const trace = runTraceFromEvents(turnResult.events);
      const outcome = turnResult.outcome as RunOutcome<T> | undefined;
      const result: RunResult<T> = {
        runId,
        run,
        sessionId: runId,
        session: sessionRecord,
        turn: turnResult.turn,
        status: turnResult.status,
        ok: turnResult.ok,
        costUsd: turnResult.costUsd,
        usage: turnResult.usage,
        text: turnResult.text,
        messages: turnResult.messages,
        events: turnResult.events,
        trace,
        outputs: turnResult.outputs,
        ...(turnResult.error !== undefined ? { error: turnResult.error } : {}),
        ...(outcome !== undefined ? { outcome } : {})
      };
      if (opts.throwOnFailure && !turnResult.ok) {
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
          `Aex.run: session ${runId} ended ${turnResult.status}${turnResult.error ? `: ${turnResult.error}` : ""}`,
          { runId, status: turnResult.status }
        );
      }
      return result;
    } finally {
      scopedSignal?.clear();
    }
  }

  /**
   * Fire-and-forget: create the session and POST its first turn WITHOUT awaiting
   * settle (the honest counterpart to await-settle `run()`). Resolves with the
   * `runId` + a resumable {@link SessionHandle} immediately; observe the run via
   * a `webhook`, the event stream, or `openSession(runId)`.
   */
  async submit(options: SessionRunOptions): Promise<SubmitResult> {
    const { message, deleteAfter: _deleteAfter, messageIdempotencyKey, stream: _stream, ...createOptions } = options;
    assertNoLegacySessionFields(options, "Aex.submit");
    const input = normaliseSessionInput(message, "Aex.submit", "message");
    const request = await this.#buildSessionCreateRequest(createOptions);
    const { runId, session } = await operations.submit(
      this.#http,
      { ...request, input },
      {
        idempotencyKey: operations.resolveIdempotencyKey(createOptions.idempotencyKey),
        ...(messageIdempotencyKey !== undefined ? { messageIdempotencyKey } : {})
      }
    );
    return { runId, session: new SessionHandle(this.#http, session, this.#fetch) };
  }

  /**
   * Run a batch of one-shot items with a bounded worker pool, returning every
   * item's settled result PLUS a REAL cost/usage rollup. The rollup is honest
   * because each `run()` awaits settle, so each item's `costUsd`/`usage` is
   * populated — a failed item lands in `failed[]`, never a silent `$0` success.
   * Concurrency is clamped below the workspace tier cap.
   */
  async batch<T = unknown>(
    items: readonly SessionRunOptions[],
    options: BatchOptions = {}
  ): Promise<BatchResult<T>> {
    if (!Array.isArray(items)) {
      throw new RunConfigValidationError("Aex.batch: items must be an array of run options");
    }
    const concurrency = Math.min(Math.max(1, Math.floor(options.concurrency ?? DEFAULT_BATCH_CONCURRENCY)), BATCH_MAX_CONCURRENCY);
    const results = (await mapWithConcurrency(items, concurrency, async (item): Promise<BatchItemResult<T>> => {
      const result = await this.run<T>(item);
      return {
        runId: result.runId,
        status: result.status,
        ok: result.ok,
        costUsd: result.costUsd,
        usage: result.usage,
        ...(result.error !== undefined ? { error: result.error } : {}),
        ...(result.outcome !== undefined ? { outcome: result.outcome } : {})
      };
    })) as BatchItemResult<T>[];
    return rollupBatch(results);
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
    // Model is REQUIRED and checked BEFORE the provider key, so omitting `model`
    // reports "model is required" rather than a misleading provider-key message.
    if (typeof options.model !== "string" || !options.model) {
      throw configError("Aex.openSession", "model", "model is required");
    }
    // One model→provider resolver (SSoT), shared with the CLI: it honors an
    // explicit provider (forward-compat: an unknown model is allowed through so a
    // slightly-old SDK can still run a newly-launched model), infers the default
    // provider for a known model, and — for an UNKNOWN model with no provider —
    // throws a shared `did you mean?` suggestion.
    let provider: RunProvider;
    try {
      provider = resolveModelProvider(options.model, options.provider);
    } catch (err) {
      throw configError(
        "Aex.openSession",
        options.provider === undefined ? "model" : "provider",
        err instanceof Error ? err.message : String(err),
        options.provider ?? options.model
      );
    }
    validateApiKeys(options.apiKeys, provider, "Aex.openSession");
    // WS9 fail-closed: `outputMode:'stream'` on a NON-streamable provider is a
    // hard reject at the earliest seam (no silent downgrade to buffered).
    if (options.outputMode !== undefined) {
      try {
        assertStreamableOutputMode(options.outputMode, provider);
      } catch (err) {
        throw configError("Aex.openSession", "outputMode", err instanceof Error ? err.message : String(err), options.outputMode);
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
    const limitsInput: { maxSpendUsd?: number; maxTurns?: number } = {};
    if (options.overrides?.maxSpendUsd !== undefined) limitsInput.maxSpendUsd = options.overrides.maxSpendUsd;
    if (options.overrides?.maxTurns !== undefined) limitsInput.maxTurns = options.overrides.maxTurns;
    try {
      limits = parseRunLimits(Object.keys(limitsInput).length > 0 ? limitsInput : undefined);
    } catch (err) {
      // One `configError` factory for every client-side validation throw, so
      // maxSpendUsd/maxTurns are `RunConfigValidationError` (not a base AexError).
      throw configError("Aex.openSession", "limits", err instanceof Error ? err.message : String(err));
    }

    const uploader: AssetUploader = (args) => this._uploadAsset(args);
    const streamUploader: AssetStreamUploader = (args) => this._uploadAssetStream(args);
    // The four prepare passes touch disjoint instance sets, so run them
    // concurrently; each internally uploads its drafts with bounded concurrency.
    const [preparedTools, preparedSkills, preparedAgentsMd, preparedFiles] = await Promise.all([
      prepareTools(options.tools ?? [], uploader),
      prepareSkills(options.skills ?? [], this),
      prepareAgentsMd(options.agentsMd ?? [], uploader),
      prepareFiles(options.files ?? [], uploader, streamUploader)
    ]);
    const { submissionMcpServers, mergedMcpSecrets } = mergeMcpServers(
      options.mcpServers ?? [],
      []
    );
    const outputCapture = outputsForWire(options.outputs);
    const environment = sessionEnvironmentForWire(options.environment);

    const submission: SessionCreateRequest["submission"] = {
      model: options.model,
      ...(options.system ? { system: options.system } : {}),
      // Builtin name strings + custom tool refs ride the `tools` union; skills
      // are first-class name refs in `submission.skills`.
      tools: [
        ...preparedTools.builtinNames,
        ...preparedTools.refs
      ] as unknown as readonly ToolRef[],
      ...(preparedSkills.length > 0 ? { skills: preparedSkills } : {}),
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

/**
 * Resolve the effective `baseUrl` from a self-describing API key (ZERO-network):
 *   - key omitted-of-shape (opaque/legacy): return the caller's `baseUrl`
 *     unchanged (HttpClient falls back to the prd default).
 *   - `baseUrl` omitted: DERIVE it from the key's plane; a plane with no default
 *     host (dev) requires an explicit `baseUrl` — throw with that guidance.
 *   - `baseUrl` supplied but its plane DISAGREES with the key's plane: throw
 *     {@link CredentialValidationError} BEFORE any request (the exact `dev key
 *     against the prd default → bare token_invalid` trap).
 */
function resolveBaseUrlForKey(apiKey: string, baseUrl: string | undefined): string | undefined {
  const parsed = parseApiKey(apiKey);
  if (parsed === null) return baseUrl;
  const planeUrl = PLANE_BASE_URLS[parsed.plane];
  if (baseUrl === undefined) {
    if (planeUrl === null) {
      throw new CredentialValidationError(
        `Aex: this API key is for the ${parsed.plane} plane, which has no default host — pass baseUrl explicitly.`,
        { plane: parsed.plane }
      );
    }
    return planeUrl;
  }
  // The only client-detectable mismatch: a non-prd key pointed at the canonical
  // prd host (dev has no canonical host to compare a prd key against).
  if (baseUrl === PLANE_BASE_URLS.prd && parsed.plane !== "prd") {
    throw new CredentialValidationError(
      `Aex: this API key is for the ${parsed.plane} plane but baseUrl targets the prd plane (${PLANE_BASE_URLS.prd}) — ` +
        `pass the ${parsed.plane} plane baseUrl, or omit baseUrl to auto-route.`,
      { plane: parsed.plane, baseUrl }
    );
  }
  return baseUrl;
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
 * A settle marker on a record: the settle commit stamps `settledAt` (and always
 * `costUsd`, default 0) plus the terminal outcome / cost telemetry. The SAME
 * predicate gates the early-return and the loop-return so a $0 turn (costUsd:0
 * present) resolves as settled — never a hang, never an undefined cost.
 */
function isSettledRecord(record: Session): boolean {
  if (!isSessionParked(record.status)) return false;
  return (
    (record as { readonly settledAt?: unknown }).settledAt !== undefined ||
    typeof record.costUsd === "number" ||
    record.lastTurnOutcome !== undefined ||
    providerUsageOf(record) !== undefined
  );
}

/**
 * Poll for the session RECORD to reach a SETTLED state — i.e. for the settle
 * write (which stamps `settledAt` + `costUsd` + the terminal outcome) to land.
 * Returns the settled record, or `undefined` on timeout/abort/read-failure so
 * the caller can fall back to the record it already holds. The first read is
 * immediate, so a fast settle costs one extra GET and no added latency.
 */
async function settledSessionRecord(
  http: HttpClient,
  sessionId: string,
  lastSeen: Session,
  signal: AbortSignal | undefined
): Promise<Session | undefined> {
  if (isSettledRecord(lastSeen)) return lastSeen;
  const deadline = Date.now() + SETTLE_POLL_DEADLINE_MS;
  while (signal?.aborted !== true && Date.now() < deadline) {
    const record = await operations.getSession(http, sessionId).catch(() => undefined);
    if (record !== undefined && isSettledRecord(record)) return record;
    try {
      await sleep(SETTLE_POLL_INTERVAL_MS, signal);
    } catch {
      return undefined;
    }
  }
  return undefined;
}

/**
 * How long a follow-up `send()` waits for the session RECORD to catch up after
 * our own turn parked idle before treating the `session_busy` as a genuinely
 * in-flight turn (ms). The idle park EVENT ends the turn stream, but the record
 * commit lags briefly behind it.
 */
const SESSION_BUSY_RECONCILE_DEADLINE_MS = 30_000;
/** Interval between record reads while reconciling a settle-lag `session_busy` (ms). */
const SESSION_BUSY_RECONCILE_INTERVAL_MS = 500;

/**
 * A 409 `session_busy` whose CURRENT status is `running` — the one case that is
 * transient: the platform is still catching up from our own just-parked turn.
 * The 409 body carries `{ error:"session_busy", status:<current> }`; any other
 * busy status (suspended / cancelling / deleted) is a real, non-retryable
 * rejection that must surface immediately.
 */
function isSettlingSessionBusy(err: unknown): boolean {
  if (!(err instanceof AexApiError) || err.status !== 409) return false;
  const body = asRecord(err.body);
  return body.error === "session_busy" && body.status === "running";
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
    status === "awaiting_approval" ||
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
 * ONE factory for every client-side validation throw: a
 * {@link RunConfigValidationError} carrying structured `details: { field, value? }`
 * so callers branch on `err.details.field` instead of string-parsing the message.
 */
function configError(surface: string, field: string, message: string, value?: unknown): RunConfigValidationError {
  return new RunConfigValidationError(
    `${surface}: ${message}`,
    value === undefined ? { field } : { field, value }
  );
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
  const removedProxyField = ["proxy", "Endpoints"].join("");
  const messages: Record<string, string> = {
    input: "send user messages with session.send(...) or use run({ message }).",
    prompt: "use message for one-shot run input or session.send(...) for follow-up messages.",
    instructions: "use system.",
    idleSuspendAfter: "use overrides.idleTtl.",
    idleTtl: "use overrides.idleTtl.",
    retention: "use overrides.idleTtl.",
    secretEnv: "use environment.secrets.",
    secrets: "use top-level apiKeys for provider keys and environment.secrets for run secrets.",
    runtimeSize: "use runtime.",
    limits: "use overrides.",
    timeout: "use overrides.timeout.",
    parentRunId: "subagent lineage is assigned by the platform.",
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
 * Stages a LARGE draft's canonical-zip STREAM to the content store via the
 * two-pass multipart flow and returns the resulting asset id. Satisfied by
 * `Aex._uploadAssetStream`.
 */
type AssetStreamUploader = (args: {
  readonly drive: ZipStreamDriver;
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
 * Max concurrent asset uploads within a single prepare pass. Bounded so a large
 * tools/files/skills list can't stampede the presign endpoint (the transport
 * already carries bounded retry); 5 is a comfortable overlap without a burst.
 */
const UPLOAD_CONCURRENCY = 5;

/**
 * Map `items` through `fn` with at most `limit` in flight, PRESERVING ORDER (the
 * result at index `i` is `fn(items[i], i)` regardless of completion order). A
 * rejection from any call propagates (the first to reject wins) once the
 * in-flight batch settles. Exported for direct unit testing; not part of the
 * public SDK surface (index.ts controls that).
 */
export async function mapWithConcurrency<T, R>(
  items: readonly T[],
  limit: number,
  fn: (item: T, index: number) => Promise<R>
): Promise<R[]> {
  const out = new Array<R>(items.length);
  let next = 0;
  const lanes = Array.from({ length: Math.min(Math.max(1, limit), items.length) }, async () => {
    for (let i = next++; i < items.length; i = next++) {
      out[i] = await fn(items[i]!, i);
    }
  });
  await Promise.all(lanes);
  return out;
}

/**
 * Split the `tools` union into custom tool refs (drafts eagerly uploaded as
 * assets) and builtin tool-name references (bare strings, validated against the
 * closed {@link BUILTIN_TOOL_NAMES} set). Builtin names are deduped, in input
 * order.
 */
async function prepareTools(
  tools: readonly (Tool | BuiltinToolName)[],
  uploader: AssetUploader
): Promise<{
  readonly refs: readonly ToolRef[];
  readonly builtinNames: readonly BuiltinToolName[];
}> {
  // Map with bounded concurrency (order preserved). Each entry resolves to
  // either a builtin-name marker or an uploaded custom-tool ref; the two groups
  // are folded back apart afterwards, preserving input order + builtin dedup.
  type Prepared = { readonly kind: "builtin"; readonly name: BuiltinToolName } | { readonly kind: "ref"; readonly ref: ToolRef };
  const prepared = await mapWithConcurrency(tools, UPLOAD_CONCURRENCY, async (entry, i): Promise<Prepared> => {
    // A bare string is a builtin tool reference.
    if (typeof entry === "string") {
      if (!(BUILTIN_TOOL_NAMES as readonly string[]).includes(entry)) {
        throw new RunConfigValidationError(
          `aex: tools[${i}] (${JSON.stringify(entry)}) is not a builtin tool name; ` +
            `expected a Tool or one of: ${BUILTIN_TOOL_NAMES.join(", ")}`
        );
      }
      return { kind: "builtin", name: entry };
    }
    if (!(entry instanceof Tool)) {
      const maybeEntry: unknown = entry;
      if (maybeEntry instanceof Skill) {
        throw new RunConfigValidationError(`aex: tools[${i}] is a Skill; pass skills via the top-level skills option`);
      }
      throw new RunConfigValidationError(`aex: tools[${i}] must be a Tool or a builtin tool name`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new RunConfigValidationError(`aex: tools[${i}] is draft but has no bytes`);
      }
      const assetId = await resolveAssetId(entry, bundle, uploader);
      return { kind: "ref", ref: { ...bundle.ref, assetId } };
    }
    return { kind: "ref", ref };
  });
  const refs: ToolRef[] = [];
  const seenBuiltins = new Set<BuiltinToolName>();
  const builtinNames: BuiltinToolName[] = [];
  for (const item of prepared) {
    if (item.kind === "builtin") {
      if (!seenBuiltins.has(item.name)) {
        seenBuiltins.add(item.name);
        builtinNames.push(item.name);
      }
    } else {
      refs.push(item.ref);
    }
  }
  return { refs, builtinNames };
}

/**
 * Upload/upsert workspace skills and return public name-only refs. Draft skills
 * auto-upsert (bytes → asset store, then registry PUT) by name; already-uploaded
 * skills pass through. Duplicate names are pre-checked (uploads run in parallel,
 * so the dedup cannot be a running set inside the map).
 */
async function prepareSkills(
  skills: readonly Skill[],
  uploader: SkillUploader
): Promise<readonly SkillRef[]> {
  if (skills.length > SKILLS_MAX) {
    throw new RunConfigValidationError(`aex: skills exceeds the ${SKILLS_MAX}-skill limit (got ${skills.length})`);
  }
  const seen = new Set<string>();
  for (let i = 0; i < skills.length; i++) {
    const entry = skills[i];
    if (!(entry instanceof Skill)) {
      throw new RunConfigValidationError(
        `aex: skills[${i}] must be a Skill (Skill.fromDir / fromUrl / fromFiles / fromContent / fromBytes)`
      );
    }
    if (seen.has(entry.name)) {
      throw new RunConfigValidationError(`aex: skills duplicate name: ${entry.name}`);
    }
    seen.add(entry.name);
  }
  return mapWithConcurrency(skills, UPLOAD_CONCURRENCY, async (entry, i): Promise<SkillRef> => {
    const uploaded = entry.isDraft ? await entry.upload(uploader) : entry;
    const ref = uploaded.ref;
    if (ref.kind !== "skill") {
      throw new RunConfigValidationError(`aex: skills[${i}] did not resolve to a workspace skill ref`);
    }
    return ref;
  });
}

/** Walk AgentsMd[], eagerly upload drafts as assets (bounded concurrency), and return plain asset refs. */
async function prepareAgentsMd(
  agentsMds: readonly AgentsMd[],
  uploader: AssetUploader
): Promise<readonly AgentsMdRef[]> {
  return mapWithConcurrency(agentsMds, UPLOAD_CONCURRENCY, async (entry, i): Promise<AgentsMdRef> => {
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
      return { kind: "asset", assetId, name: bundle.name };
    }
    return ref;
  });
}

/** Walk File[], eagerly upload drafts as assets (bounded concurrency), and return plain asset refs. */
async function prepareFiles(
  files: readonly File[],
  uploader: AssetUploader,
  streamUploader: AssetStreamUploader
): Promise<readonly FileRef[]> {
  return mapWithConcurrency(files, UPLOAD_CONCURRENCY, async (entry, i): Promise<FileRef> => {
    if (!(entry instanceof File)) {
      throw new RunConfigValidationError(`aex: files[${i}] must be a File instance`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      // A large draft carries a streaming driver instead of in-memory bytes.
      const stream = entry._takeDraftStream();
      if (stream) {
        const cached = entry._cachedAssetId;
        if (cached !== undefined) {
          return { kind: "asset", assetId: cached, name: stream.name, mountPath: stream.mountPath };
        }
        const uploaded = await streamUploader({ drive: stream.drive, contentType: "application/zip" });
        entry._rememberAsset(uploaded.assetId);
        return { kind: "asset", assetId: uploaded.assetId, name: stream.name, mountPath: stream.mountPath };
      }
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new RunConfigValidationError(`aex: files[${i}] is draft but has no bytes`);
      }
      const assetId = await resolveAssetId(entry, bundle, uploader);
      return { kind: "asset", assetId, name: bundle.name, mountPath: bundle.mountPath };
    }
    return ref;
  });
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
