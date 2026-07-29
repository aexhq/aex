import type {
  ApprovalGate,
  AexEventView,
  BuiltinToolName,
  ModelName,
  OutputMode,
  PlatformEnvironmentInput,
  PlatformSubmission,
  ResponseFormat,
  Session,
  SessionCheckpointRevision,
  SessionFile,
  SessionMessage,
  SessionRun,
  SessionRuntime,
  SubmissionAssets,
  TurnOutcome,
  TurnResult,
  TurnTrace,
  WebSocketFactory
} from "@aexhq/contracts";
import type { McpServer } from "./mcp-server.js";
import type { Secret } from "./secret.js";

/**
 * A transcript message as the SDK hands it to a caller.
 *
 * Deliberately NOT an alias of {@link SessionMessage}, which is the WIRE shape
 * returned by `GET /api/sessions/{id}/messages` and always carries `timestamp`,
 * `sequence` and `content`. A `Message` may also be PROJECTED from the event
 * stream by `projectAssistantMessages`, which is building one as the tokens
 * arrive and legitimately has no sequence or timestamp yet, and never has the
 * wire's `content` array.
 *
 * Same distinction as `Session` against `SessionWire`: one shape is what the
 * server sent, the other is what the client assembled. Conflating them made the
 * wire type's guarantees look optional and the projection's gaps look like
 * server behaviour.
 */
export interface Message {
  readonly id: string;
  readonly sender: SessionMessage["sender"];
  readonly text: string;
  readonly timestamp?: string;
  readonly turnSeq?: number;
  readonly sequence?: number;
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

/**
 * Options for creating a resumable session or starting a one-shot run.
 * Reusable bytes are published first through `aex.workspace`, then pinned in
 * `assets` by resource id and immutable version.
 *
 * There is no `idempotencyKey`. The SDK mints the mutation identity itself and
 * reuses it across its own automatic retries, so a submit whose response is lost
 * (the API has a 29-second ceiling — the call can succeed while the
 * acknowledgement never arrives) is de-duplicated server-side instead of
 * creating a second session, a second container, and a second bill.
 */
export interface SessionCreateOptions {
  /**
   * The model to run, as a Vercel AI Gateway `creator/model` slug string
   * (e.g. `"anthropic/claude-haiku-4-5"`, `"deepseek/deepseek-v4-flash"`).
   * The managed gateway routes it — no provider selection and no API key.
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
   * `liveSequence`, and no durable `sequence`) as they arrive. Every model
   * streams through the managed gateway, so `"stream"` is honored for ALL models.
   * A coalesced final `TEXT_MESSAGE_CONTENT` ALWAYS follows the deltas, so a
   * buffered consumer sees the same final text either way; deltas are
   * provisional until that coalesced block.
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
  readonly environment?: SessionEnvironmentOptions;
  /**
   * The execution runtime for the session — grouped as `{ kind, size }`.
   *
   *   - `kind` — which backend runs it: `spot_container` (default; cheapest
   *     capacity that executes every tool, but a reclaim can replay an
   *     interrupted step, so side-effecting tools run AT LEAST ONCE),
   *     `container` (the same host on on-demand capacity, exactly-once), or
   *     `lambda` (serverless: zero idle billing and seconds-not-minutes cold
   *     start, availability-gated and currently unable to execute a tool call).
   *     Prefer the {@link RuntimeKinds} symbol const.
   *   - `size` — the managed box preset ({@link RuntimeSize}); prefer {@link Sizes}.
   *
   * Both optional; the platform applies defaults (`spot_container`, the 1 GB tier).
   * e.g. `runtime: { kind: "container", size: Sizes.CPU_2_8GB }`.
   *
   * Runtimes differ in capability, not only in price. Read what a runtime will
   * actually do from `whoami().runtimeCapabilities.profilesByRuntimeKind` before
   * naming one; a submission that exceeds the selected profile is refused before
   * execution rather than degraded.
   */
  readonly runtime?: SessionRuntime;
  readonly overrides?: SessionOverrides;
  /**
   * Optional callback URL registered on the session. The platform delivers a
   * run-scoped `run.finished` or `run.error` event after every run finalizes,
   * signed Standard-Webhooks style (verify with {@link verifyAexWebhook}). The
   * URL must be https.
   */
  readonly webhook?: { readonly url: string };
}

/** Options for one turn. The mutation identity is SDK-owned; see {@link SessionCreateOptions}. */
export interface SessionSendOptions {
  readonly webSocketFactory?: WebSocketFactory;
  readonly idleTimeoutMs?: number;
  readonly pingIntervalMs?: number;
}

export interface SessionStartOptions extends SessionCreateOptions {
  readonly message: SessionInput;
  readonly deleteAfter?: boolean;
  readonly stream?: SessionSendOptions;
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
