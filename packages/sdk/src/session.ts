import type {
  ApiError,
  Event,
  MessageAccepted as BrainMessageAccepted,
  Provider,
  Session as SessionData,
  SessionList as SessionListData,
  SessionState,
  CreateSessionRequest,
} from "@aexhq/brain/session";
import { CustomerHand } from "@aexhq/brain";
import type {
  ClientRegistration,
  NetworkPolicy,
  WebSocketFactory,
} from "@aexhq/brain";
import * as z from "zod";

import {
  AbortError,
  OutputRefusalError,
  OutputSchemaError,
  OutputValidationError,
  SessionError,
  abortError,
  errorFromApi,
} from "./errors.js";
import type { OutputValidationIssue } from "./errors.js";
import { canonicalize, jcsSha256, randomIdempotencyKey } from "./json.js";
import type { EventOptions } from "./transport.js";
import { Transport } from "./transport.js";
import { compileTools } from "./tools.js";
import type { Tool } from "./tools.js";
import { encodeBase64, SessionChildren, SessionSandbox, SessionStorage } from "./resources.js";

export type SessionInput = string;

/** Aex adds trusted-output admission identity to Brain's neutral acknowledgement. */
interface MessageAccepted extends BrainMessageAccepted {
  output_id?: string;
  schema_hash?: string;
}

export interface ModelOptions {
  provider: Provider;
  name: string;
  apiKey: string;
  baseUrl?: string;
  maxOutputTokens?: number;
  /** Immutable context capacity used for admission and compaction; Brain never guesses by name. */
  contextWindowTokens?: number;
  temperature?: number;
  reasoningEffort?: "low" | "medium" | "high";
}

/**
 * A built agentloop implementation, as exported by a loop package or produced by
 * `buildLoopBundle` from `@aexhq/agentloop`. Assignment is by import, never by name:
 * the sealed identity is the content digest plus the pinned toolchain.
 */
export interface AgentloopBundle {
  /** The complete deterministic ESM source bundle — the exact bytes sealed and uploaded. */
  source: string;
  /** SHA-256 hex of the UTF-8 source bytes. */
  sha256: string;
  /** The pinned loop-toolchain identity the bundle was built for. */
  toolchain: string;
}

export interface CreateSessionOptions {
  model: ModelOptions;
  /** Omitted or empty grants no tools. A non-empty list is the exact grant. */
  tools?: readonly Tool[];
  /**
   * The agentloop driving this session's turns, assigned by importing its implementation.
   * Sealed at create for the life of the session; children inherit it. Omission seals the
   * official aex loop.
   */
  agentloop?: AgentloopBundle;
  systemPrompt?: string;
  /** Write-only values for environment names declared by managed Tools. */
  secrets?: Record<string, string>;
  /** Maximum direct outbound network authority sealed for managed sandboxes. Omission is deny-all. */
  network?: NetworkPolicy;
  /** Replacement attempts after an unrecoverable provider outcome. Defaults to one. */
  providerRecoveryRetries?: 0 | 1;
  client?: {
    /** Replacement sends to the same customer process and operation. Defaults to one. */
    submitRetries?: 0 | 1;
  };
  /** Optional ceilings for durable child sessions. Omitted fields use the hosted defaults. */
  children?: {
    maxDepth?: number;
    maxDirectChildren?: number;
    maxDescendants?: number;
  };
  metadata?: Record<string, string>;
}

export interface RequestOptions {
  signal?: AbortSignal;
  idempotencyKey?: string;
  metadata?: Record<string, string>;
}

export interface OutputOptions<Schema extends z.ZodType = z.ZodType> extends RequestOptions {
  output: Schema;
  /** Extra attempts after the first invalid candidate. Defaults to 1; maximum 2. */
  outputRetries?: 0 | 1 | 2;
}

export interface ListSessionsOptions {
  limit?: number;
  cursor?: string;
  state?: SessionState;
  signal?: AbortSignal;
}

export interface SessionList {
  data: Session[];
  hasMore: boolean;
  nextCursor?: string;
}

export interface ModelSummary {
  provider: Provider;
  name: string;
  baseUrl?: string;
  contextWindowTokens: number;
}

export interface SessionSummary {
  id: string;
  parentId: string | undefined;
  rootId: string;
  depth: number;
  state: SessionState;
  turnState: SessionData["turn_state"];
  model: ModelSummary;
  createdAt: string;
  updatedAt: string;
  metadata: Readonly<Record<string, string | undefined>>;
}

export class Sessions {
  readonly #transport: Transport;
  readonly #webSocketFactory: WebSocketFactory | undefined;
  readonly #clientId: string | undefined;
  #customerHand: Promise<CustomerHand> | undefined;
  #customerHandInstance: CustomerHand | undefined;
  #closed = false;

  constructor(transport: Transport, webSocketFactory?: WebSocketFactory, clientId?: string) {
    this.#transport = transport;
    this.#webSocketFactory = webSocketFactory;
    this.#clientId = clientId;
  }

  /** @internal Called by `Aex.close()`. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#customerHandInstance?.close();
    this.#customerHandInstance = undefined;
    this.#customerHand = undefined;
  }

  async create(options: CreateSessionOptions, request: RequestOptions = {}): Promise<Session> {
    if (this.#closed) throw new SessionError("Aex client is closed");
    const compiledTools = await compileTools(options.tools);
    if (options.client?.submitRetries !== undefined && this.#clientId === undefined) {
      throw new TypeError("client.submitRetries requires Aex({ client: { id } })");
    }
    await this.#ensureCustomerHand(compiledTools.clientRegistrations, request.signal);
    const body = {
      model: {
        provider: options.model.provider,
        name: options.model.name,
        api_key: options.model.apiKey,
        ...(options.model.baseUrl === undefined ? {} : { base_url: options.model.baseUrl }),
        ...(options.model.maxOutputTokens === undefined
          ? {}
          : { max_output_tokens: options.model.maxOutputTokens }),
        ...(options.model.contextWindowTokens === undefined
          ? {}
          : { context_window_tokens: options.model.contextWindowTokens }),
        ...(options.model.temperature === undefined ? {} : { temperature: options.model.temperature }),
        ...(options.model.reasoningEffort === undefined
          ? {}
          : { reasoning_effort: options.model.reasoningEffort }),
      },
      tools: {
        items: compiledTools.items,
      },
      ...(compiledTools.bundles.length === 0 ? {} : { tool_bundles: compiledTools.bundles }),
      ...(options.agentloop === undefined
        ? {}
        : {
            agentloop: {
              source_bundle_sha256: options.agentloop.sha256,
              toolchain: options.agentloop.toolchain,
              bundle_base64: encodeBase64(new TextEncoder().encode(options.agentloop.source)),
            },
          }),
      ...(options.secrets === undefined ? {} : { secrets: options.secrets }),
      ...(options.systemPrompt === undefined ? {} : { system_prompt: options.systemPrompt }),
      ...(options.metadata === undefined ? {} : { metadata: options.metadata }),
      ...(options.network === undefined
        ? {}
        : { network: options.network as NonNullable<CreateSessionRequest["network"]> }),
      ...(options.providerRecoveryRetries === undefined
        ? {}
        : { provider_recovery_retries: options.providerRecoveryRetries }),
      ...(this.#clientId === undefined
        ? {}
        : {
            client: {
              id: this.#clientId,
              ...(options.client?.submitRetries === undefined
                ? {}
                : { submit_retries: options.client.submitRetries }),
            },
          }),
      ...(options.children === undefined
        ? {}
        : {
            children: {
              ...(options.children.maxDepth === undefined
                ? {}
                : { max_depth: options.children.maxDepth }),
              ...(options.children.maxDirectChildren === undefined
                ? {}
                : { max_direct_children: options.children.maxDirectChildren }),
              ...(options.children.maxDescendants === undefined
                ? {}
                : { max_descendants: options.children.maxDescendants }),
            },
          }),
    };
    const data = await this.#transport.json<SessionData>("POST", "/v1/sessions", {
      body,
      headers: { "Idempotency-Key": request.idempotencyKey ?? randomIdempotencyKey() },
      signal: request.signal,
      retry: true,
    });
    return new Session(this.#transport, data);
  }

  async get(id: string, options: Pick<RequestOptions, "signal"> = {}): Promise<Session> {
    const data = await this.#transport.json<SessionData>(
      "GET",
      `/v1/sessions/${encodeURIComponent(id)}`,
      { signal: options.signal },
    );
    return new Session(this.#transport, data);
  }

  async list(options: ListSessionsOptions = {}): Promise<SessionList> {
    const query = new URLSearchParams();
    if (options.limit !== undefined) query.set("limit", String(options.limit));
    if (options.cursor !== undefined) query.set("cursor", options.cursor);
    if (options.state !== undefined) query.set("state", options.state);
    const suffix = query.size === 0 ? "" : `?${query}`;
    const list = await this.#transport.json<SessionListData>("GET", `/v1/sessions${suffix}`, {
      signal: options.signal,
    });
    return {
      data: list.data.map((data) => new Session(this.#transport, data)),
      hasMore: list.has_more,
      ...(list.next_cursor === undefined ? {} : { nextCursor: list.next_cursor }),
    };
  }

  async #ensureCustomerHand(
    registrations: readonly ClientRegistration[],
    signal?: AbortSignal,
  ): Promise<void> {
    if (this.#closed) throw new SessionError("Aex client is closed");
    if (registrations.length === 0) return;
    if (this.#clientId === undefined) {
      throw new TypeError("Customer-app Tools require Aex({ client: { id } })");
    }
    if (this.#webSocketFactory === undefined) {
      throw new TypeError("This runtime does not provide WebSocket; pass webSocketFactory to Aex");
    }
    if (this.#customerHand === undefined) {
      let partial: CustomerHand | undefined;
      const starting = (async () => {
        try {
          partial = new CustomerHand(
            async () => {
              const grant = await this.#transport.customerHandGrant(
                this.#clientId!,
              );
              return {
                request: { url: grant.url, protocol: grant.protocol },
                observe: (observation) => this.#transport.customerHandObserve(
                  grant.observationUrl,
                  grant.observationToken,
                  observation,
                ),
              };
            },
            registrations,
            this.#webSocketFactory!,
            { clientId: this.#clientId! },
          );
          this.#customerHandInstance = partial;
          await partial.ready;
          if (this.#closed) {
            partial.close();
            throw new SessionError("Aex client is closed");
          }
          return partial;
        } catch (error) {
          partial?.close();
          throw error;
        }
      })();
      this.#customerHand = starting;
      void starting.catch(() => {
        if (this.#customerHand === starting) {
          this.#customerHand = undefined;
          if (this.#customerHandInstance === partial) this.#customerHandInstance = undefined;
        }
      });
      // The request may stop waiting, but the process-scoped runner remains reconnectable for
      // later sessions. Its grant/reconnect lifetime must never inherit one create signal.
      await waitWithSignal(starting, signal);
      return;
    }
    const hand = await waitWithSignal(this.#customerHand, signal);
    if (this.#closed) throw new SessionError("Aex client is closed");
    await waitWithSignal(hand.register(registrations), signal);
  }
}

function waitWithSignal<T>(promise: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (signal === undefined) return promise;
  if (signal.aborted) return Promise.reject(abortError(signal.reason));
  return new Promise<T>((resolve, reject) => {
    const cleanup = (): void => signal.removeEventListener("abort", onAbort);
    const onAbort = (): void => {
      cleanup();
      reject(abortError(signal.reason));
    };
    signal.addEventListener("abort", onAbort, { once: true });
    promise.then(
      (value) => { cleanup(); resolve(value); },
      (error) => { cleanup(); reject(error); },
    );
  });
}

export class Session implements SessionSummary {
  readonly #transport: Transport;
  #data: SessionData;
  readonly sandbox: SessionSandbox;
  readonly storage: SessionStorage;
  readonly children: SessionChildren;

  constructor(transport: Transport, data: SessionData) {
    this.#transport = transport;
    this.#data = data;
    this.sandbox = new SessionSandbox(transport, data.id);
    this.storage = new SessionStorage(transport, data.id);
    this.children = new SessionChildren(transport, data.id);
  }

  get id(): string {
    return this.#data.id;
  }

  get state(): SessionState {
    return this.#data.state;
  }

  get turnState(): SessionData["turn_state"] {
    return this.#data.turn_state;
  }

  get parentId(): string | undefined {
    return this.#data.parent_id;
  }

  get rootId(): string {
    return this.#data.root_id;
  }

  get depth(): number {
    return this.#data.depth;
  }

  get model(): ModelSummary {
    return {
      provider: this.#data.model.provider,
      name: this.#data.model.name,
      ...(this.#data.model.base_url === undefined ? {} : { baseUrl: this.#data.model.base_url }),
      contextWindowTokens: this.#data.model.context_window_tokens,
    };
  }

  get createdAt(): string {
    return this.#data.created_at;
  }

  get updatedAt(): string {
    return this.#data.updated_at;
  }

  get metadata(): Readonly<Record<string, string | undefined>> {
    return this.#data.metadata;
  }

  async refresh(options: Pick<RequestOptions, "signal"> = {}): Promise<this> {
    this.#data = await this.#transport.json<SessionData>(
      "GET",
      `/v1/sessions/${encodeURIComponent(this.id)}`,
      { signal: options.signal },
    );
    return this;
  }

  send(input: SessionInput, options?: RequestOptions): Promise<string>;
  send<Schema extends z.ZodType>(
    input: SessionInput,
    options: OutputOptions<Schema>,
  ): Promise<z.output<Schema>>;
  async send(
    input: SessionInput,
    options: RequestOptions | OutputOptions = {},
  ): Promise<unknown> {
    const outputOptions = isOutputOptions(options) ? options : undefined;
    const compiled =
      outputOptions === undefined
        ? undefined
        : await compileOutputSchema(outputOptions.output, outputOptions.outputRetries);
    const accepted = await this.#transport.json<MessageAccepted>(
      "POST",
      `/v1/sessions/${encodeURIComponent(this.id)}/messages`,
      {
        body: {
          content: input,
          ...(options.metadata === undefined ? {} : { metadata: options.metadata }),
          ...(compiled === undefined
            ? {}
            : {
                output: {
                  schema: compiled.jsonSchema,
                  schema_hash: compiled.schemaHash,
                  ...(compiled.retries === undefined ? {} : { retries: compiled.retries }),
                },
              }),
        },
        headers: { "Idempotency-Key": options.idempotencyKey ?? randomIdempotencyKey() },
        signal: options.signal,
        retry: true,
      },
    );
    if (compiled !== undefined) {
      if (
        accepted.session_id !== this.id ||
        accepted.output_id === undefined ||
        accepted.schema_hash !== compiled.schemaHash
      ) {
        throw new SessionError("Aex returned an inconsistent typed-output admission");
      }
    }

    let answer = "";
    try {
      for await (const event of this.events({ after: Math.max(0, accepted.seq - 1), signal: options.signal })) {
        if (event.type === "assistant.message" && event.turn_id === accepted.turn_id && event.agent_id === "root") {
          answer = event.text;
        } else if (event.type === "turn.failed" && event.turn_id === accepted.turn_id) {
          throw errorFromApi(event.error, undefined, outputIssues(event.error));
        } else if (event.type === "turn.completed" && event.turn_id === accepted.turn_id) {
          this.markIdle();
          if (event.stop_reason === "cancelled") throw new AbortError();
          if (compiled !== undefined && outputOptions !== undefined) {
            if (event.result === undefined) {
              if (event.stop_reason === "refusal") {
                throw new OutputRefusalError("The model refused the structured-output request");
              }
              throw new OutputValidationError(
                "The model ended the turn without submitting the requested structured output",
                [{
                  path: "",
                  message: "The model did not call aex_submit_output",
                  keyword: "missing_output",
                }],
              );
            }
            if (
              event.result.name !== "aex_submit_output" ||
              event.result.metadata?.output_id !== accepted.output_id ||
              event.result.metadata?.schema_hash !== compiled.schemaHash
            ) {
              throw new SessionError("Aex returned a typed result for a different request");
            }
            const parsed = await outputOptions.output.safeParseAsync(event.result.value);
            if (!parsed.success) {
              throw new OutputValidationError(
                "The output passed the wire schema but failed the original Zod schema",
                parsed.error.issues.map((issue) => ({
                  path: jsonPointer(issue.path),
                  message: issue.message,
                  keyword: issue.code,
                })),
              );
            }
            return parsed.data;
          }
          return answer;
        }
      }
    } catch (error) {
      if (options.signal?.aborted === true) {
        await this.cancel().catch(() => undefined);
        throw abortError(error);
      }
      throw error;
    }
    throw new SessionError("The Aex event stream ended before the session finished its work");
  }

  /**
   * Raw, attempt-aware event stream. Provisional frames have no durable cursor and may later be
   * superseded; consumers rendering them must key by `attempt_id` and process
   * `model.attempt_superseded`. Use `send()` when only the durable winning answer is needed.
   */
  events(options: EventOptions = {}): AsyncGenerator<Event> {
    return this.#transport.events(this.id, options);
  }

  async cancel(options: Pick<RequestOptions, "signal"> = {}): Promise<this> {
    this.#data = await this.#transport.json<SessionData>(
      "POST",
      `/v1/sessions/${encodeURIComponent(this.id)}/cancel`,
      { signal: options.signal, retry: true },
    );
    return this;
  }

  async end(options: Pick<RequestOptions, "signal"> = {}): Promise<this> {
    this.#data = await this.#transport.json<SessionData>(
      "POST",
      `/v1/sessions/${encodeURIComponent(this.id)}/end`,
      { signal: options.signal, retry: true },
    );
    return this;
  }

  async delete(options: Pick<RequestOptions, "signal"> & { queue?: boolean } = {}): Promise<void> {
    await this.#transport.deleteSession(this.id, options.queue !== true, options.signal);
    this.#data = {
      ...this.#data,
      state: options.queue === true ? "deleting" : "deleted",
    };
  }

  private markIdle(): void {
    this.#data = { ...this.#data, turn_state: "idle" };
  }
}

function isOutputOptions(options: RequestOptions | OutputOptions): options is OutputOptions {
  return "output" in options;
}

async function compileOutputSchema(
  schema: z.ZodType,
  retries: 0 | 1 | 2 | undefined,
): Promise<{ jsonSchema: Record<string, unknown>; schemaHash: string; retries: 0 | 1 | 2 | undefined }> {
  let jsonSchema: Record<string, unknown>;
  try {
    assertPortableOutputSchema(schema);
    jsonSchema = z.toJSONSchema(schema, {
      target: "draft-2020-12",
      unrepresentable: "throw",
    }) as Record<string, unknown>;
  } catch (cause) {
    throw new OutputSchemaError(messageOf(cause, "The Zod schema cannot be represented as JSON Schema"), {
      cause,
    });
  }
  if (jsonSchema.type !== "object") {
    throw new OutputSchemaError("session.send() output requires a Zod object schema");
  }
  assertSupportedJsonSchema(jsonSchema);
  return { jsonSchema, schemaHash: await jcsSha256(jsonSchema), retries };
}

function assertSupportedJsonSchema(schema: Record<string, unknown>): void {
  const bytes = new TextEncoder().encode(canonicalize(schema));
  if (bytes.byteLength > 64 * 1024) {
    throw new OutputSchemaError("The output schema exceeds the 65536-byte service limit");
  }
  let nodes = 0;
  const visit = (value: unknown, depth: number): void => {
    if (depth > 64) throw new OutputSchemaError("The output schema exceeds the service depth limit");
    nodes += 1;
    if (nodes > 4096) throw new OutputSchemaError("The output schema exceeds the service node limit");
    if (Array.isArray(value)) {
      for (const child of value) visit(child, depth + 1);
      return;
    }
    if (value === null || typeof value !== "object") return;
    const object = value as Record<string, unknown>;
    if ("pattern" in object || "patternProperties" in object) {
      throw new OutputSchemaError(
        "Regular-expression JSON Schema keywords are not supported for Aex structured output",
      );
    }
    for (const keyword of ["$ref", "$dynamicRef", "$recursiveRef"] as const) {
      const reference = object[keyword];
      if (typeof reference === "string" && !reference.startsWith("#")) {
        throw new OutputSchemaError("Remote JSON Schema references are not supported for Aex structured output");
      }
    }
    for (const child of Object.values(object)) visit(child, depth + 1);
  };
  visit(schema, 0);
}

function outputIssues(error: ApiError): readonly OutputValidationIssue[] {
  const details = error.details;
  if (details === undefined || details === null || typeof details !== "object") return [];
  const issues = (details as { issues?: unknown }).issues;
  if (!Array.isArray(issues)) return [];
  return issues.flatMap((issue): OutputValidationIssue[] => {
    if (issue === null || typeof issue !== "object") return [];
    const value = issue as { path?: unknown; message?: unknown; keyword?: unknown };
    if (typeof value.path !== "string" || typeof value.message !== "string") return [];
    return [{
      path: value.path,
      message: value.message,
      ...(typeof value.keyword === "string" ? { keyword: value.keyword } : {}),
    }];
  });
}

function jsonPointer(path: readonly PropertyKey[]): string {
  if (path.length === 0) return "";
  return `/${path
    .map((part) => String(part).replaceAll("~", "~0").replaceAll("/", "~1"))
    .join("/")}`;
}

function messageOf(error: unknown, fallback: string): string {
  return error instanceof Error && error.message !== "" ? error.message : fallback;
}

/**
 * A server cannot execute user-defined Zod functions. Reject them before admission instead of
 * committing a value that only the calling process can later discover was invalid.
 */
function assertPortableOutputSchema(schema: z.ZodType): void {
  const seen = new WeakSet<object>();
  const visit = (value: unknown): void => {
    if (value === null || typeof value !== "object" || seen.has(value)) return;
    seen.add(value);
    const internal = value as { _zod?: { def?: unknown } };
    if (internal._zod?.def !== undefined) {
      const definition = internal._zod.def as { type?: unknown; check?: unknown };
      const unsupported =
        definition.type === "custom" ||
        definition.type === "transform" ||
        definition.type === "file" ||
        definition.check === "overwrite";
      if (unsupported) {
        throw new OutputSchemaError(
          `This Zod schema contains ${String(definition.type ?? definition.check)} behavior that cannot be enforced by the Aex output service`,
        );
      }
      visit(definition);
      return;
    }
    if (Array.isArray(value)) {
      for (const item of value) visit(item);
      return;
    }
    for (const child of Object.values(value)) visit(child);
  };
  visit(schema);
}
