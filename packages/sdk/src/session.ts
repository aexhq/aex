import type {
  ApiError,
  Event,
  MessageAccepted as BrainMessageAccepted,
  ToolDefinition,
  Session as BrainSessionData,
  SessionList as SessionListData,
  SessionState,
  CreateSessionRequest,
} from "@aexhq/brain/session";
import {
  CustomerEnvironment,
  prepareComponents,
  type ComponentExtension,
  type SessionTool as ComponentSessionTool,
} from "@aexhq/brain";
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
import { compileCallbacks, type Tool } from "./tools.js";
import { SessionChildren, SessionSandbox, SessionStorage } from "./resources.js";

type SessionData = BrainSessionData & { retain_until: string };

export type SessionInput = string;

/** Aex adds trusted-output admission identity to Brain's neutral acknowledgement. */
interface MessageAccepted extends BrainMessageAccepted {
  output_id?: string;
  schema_hash?: string;
}

export interface ModelOptions {
  /** Imported Model component. */
  component: ComponentExtension<"model">;
  provider: string;
  name: string;
  apiKey: string;
  baseUrl?: string;
  maxOutputTokens?: number;
}

export interface CreateSessionOptions {
  model: ModelOptions;
  /** The imported agent loop that drives this session and every child unless spawn overrides it. */
  agentloop: ComponentExtension<"agentloop">;
  /** Omitted or empty grants no tools. A non-empty list is the exact grant. */
  environments?: Readonly<Record<string, ComponentExtension<"environment">>>;
  tools?: readonly (Tool | ComponentSessionTool)[];
  /** Write-only values for components that explicitly consume session secrets. */
  secrets?: Record<string, string>;
  /** Maximum direct outbound network authority sealed for managed sandboxes. Omission is deny-all. */
  network?: NetworkPolicy;
  /** Replacement attempts after an unrecoverable provider outcome. Defaults to one. */
  providerRecoveryRetries?: 0 | 1;
  /** Optional ceilings for durable child sessions. Omitted fields use the hosted defaults. */
  children?: {
    maxDepth?: number;
    maxDirectChildren?: number;
    maxDescendants?: number;
  };
  metadata?: Record<string, string>;
  /** Finite durable-history deadline. Omit to use the Brain deployment default. */
  retainUntil?: Date | string;
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
  provider: string;
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
  retainUntil: string;
  metadata: Readonly<Record<string, string | undefined>>;
}

export class Sessions {
  readonly #transport: Transport;
  readonly #webSocketFactory: WebSocketFactory | undefined;
  readonly #customerEnvironments = new Map<string, Promise<CustomerEnvironment>>();
  readonly #customerEnvironmentInstances = new Map<string, CustomerEnvironment>();
  readonly #customerEnvironmentReadinessWaiters = new Map<string, number>();
  #closed = false;

  constructor(transport: Transport, webSocketFactory?: WebSocketFactory) {
    this.#transport = transport;
    this.#webSocketFactory = webSocketFactory;
  }

  /** @internal Called by `Aex.close()`. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    for (const hand of this.#customerEnvironmentInstances.values()) hand.close();
    this.#customerEnvironmentInstances.clear();
    this.#customerEnvironments.clear();
  }

  async create(options: CreateSessionOptions, request: RequestOptions = {}): Promise<Session> {
    if (this.#closed) throw new SessionError("Aex client is closed");
    if (options.model?.component === undefined) {
      throw new TypeError("sessions.create requires an imported Model component");
    }
    if (options.agentloop === undefined) {
      throw new TypeError("sessions.create requires an imported Agentloop component");
    }
    const model = options.model.component;
    const agentloop = options.agentloop;
    const tools = [...(options.tools ?? [])];
    const componentTools = tools.filter(isComponentTool);
    const callbackTools = tools.filter((tool): tool is Tool => !isComponentTool(tool));
    const environments = Object.entries(options.environments ?? {});
    if (environments.some(([, environment]) => !isEnvironmentComponent(environment))) {
      throw new TypeError("component sessions accept only precompiled Environment component values");
    }
    const environmentComponents = environments.map(([name, environment]) => {
      if (!isEnvironmentComponent(environment)) {
        throw new TypeError(`Environment ${JSON.stringify(name)} is not a component`);
      }
      return [name, environment] as const;
    });
    const applicationEnvironments = environmentComponents.filter(([, environment]) =>
      isApplicationEnvironment(environment));
    if (callbackTools.length > 0 && applicationEnvironments.length !== 1) {
      throw new TypeError("application callback Tools require exactly one app() Environment");
    }
    const callbacks = await compileCallbacks(callbackTools);
    const allComponentTools = [...componentTools, ...callbacks.components];
    const toolDefinitions = allComponentTools.map(componentToolDefinition);
    const names = new Set<string>();
    for (const definition of toolDefinitions) {
      if (names.has(definition.name)) {
        throw new TypeError(`Tool ${JSON.stringify(definition.name)} was selected twice`);
      }
      names.add(definition.name);
    }
    const callbackEnvironment = applicationEnvironments[0];
    if (callbackEnvironment !== undefined) {
      await this.#ensureCustomerEnvironment(
        applicationEnvironmentId(callbackEnvironment[1]),
        callbacks.registrations,
        request.signal,
      );
    }
    const prepared = await prepareComponents([
      model,
      agentloop,
      ...allComponentTools,
      ...environmentComponents.map(([, environment]) => environment),
    ]);
    const modelBinding = prepared.bindings[0];
    const agentloopBinding = prepared.bindings[1];
    if (modelBinding === undefined || agentloopBinding === undefined) {
      throw new TypeError("component session bindings are incomplete");
    }
    const toolBindings = prepared.bindings.slice(2, 2 + allComponentTools.length);
    const environmentBindings = prepared.bindings.slice(2 + allComponentTools.length);
    const toolItems = allComponentTools.map((tool, index) => {
      const binding = toolBindings[index];
      if (binding === undefined) throw new TypeError("Tool component binding is missing");
      const definition = toolDefinitions[index];
      if (definition === undefined) throw new TypeError("Tool component definition is missing");
      const needsEnvironment = binding.grants.includes("environment");
      const isCallback = index >= componentTools.length;
      if (needsEnvironment && !isCallback && environmentComponents.length !== 1) {
        throw new TypeError(
          "a Tool with the environment grant requires exactly one declared Environment",
        );
      }
      const environmentName = isCallback
        ? callbackEnvironment?.[0]
        : environmentComponents[0]?.[0];
      return {
        definition,
        executor: {
          kind: "component",
          component_digest: binding.component_digest,
          world: binding.world,
          config: binding.config,
          grants: binding.grants,
          ...(needsEnvironment ? { environment: environmentName! } : {}),
        },
      };
    });
    const environmentConfig = Object.fromEntries(environmentComponents.map(([name], index) => {
      const binding = environmentBindings[index];
      if (binding === undefined) {
        throw new TypeError(`Environment component binding ${JSON.stringify(name)} is missing`);
      }
      return [name, {
        component_digest: binding.component_digest,
        world: binding.world,
        config: binding.config,
      }];
    }));
    const body = {
      model: {
        component_digest: modelBinding.component_digest,
        world: modelBinding.world,
        config: modelBinding.config,
        provider: options.model.provider,
        name: options.model.name,
        api_key: options.model.apiKey,
        ...(options.model.baseUrl === undefined ? {} : { base_url: options.model.baseUrl }),
        ...(options.model.maxOutputTokens === undefined
          ? {}
          : { max_output_tokens: options.model.maxOutputTokens }),
      },
      agentloop: {
        component_digest: agentloopBinding.component_digest,
        world: agentloopBinding.world,
        config: agentloopBinding.config,
      },
      component_artifacts: prepared.artifacts,
      tools: { items: toolItems },
      ...(environmentComponents.length === 0 ? {} : { environments: environmentConfig }),
      ...(options.secrets === undefined ? {} : { secrets: options.secrets }),
      ...(options.metadata === undefined ? {} : { metadata: options.metadata }),
      ...(options.retainUntil === undefined
        ? {}
        : { retain_until: normalizeTimestamp(options.retainUntil, "retainUntil") }),
      ...(options.network === undefined ? {} : { network: options.network }),
      ...(options.providerRecoveryRetries === undefined
        ? {}
        : { provider_recovery_retries: options.providerRecoveryRetries }),
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
    } as unknown as CreateSessionRequest;
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

  async #ensureCustomerEnvironment(
    clientId: string | undefined,
    registrations: readonly ClientRegistration[],
    signal?: AbortSignal,
  ): Promise<void> {
    if (this.#closed) throw new SessionError("Aex client is closed");
    if (registrations.length === 0) return;
    if (clientId === undefined) throw new TypeError("Callback Tools require an app() environment");
    if (this.#webSocketFactory === undefined) {
      throw new TypeError("This runtime does not provide WebSocket; pass webSocketFactory to Aex");
    }
    const existing = this.#customerEnvironments.get(clientId);
    if (existing === undefined) {
      let partial: CustomerEnvironment | undefined;
      const starting = (async () => {
        try {
          partial = new CustomerEnvironment(
            async () => {
              const grant = await this.#transport.customerEnvironmentGrant(
                clientId,
              );
              return {
                request: { url: grant.url, protocol: grant.protocol },
                observe: (observation) => this.#transport.customerEnvironmentObserve(
                  grant.observationUrl,
                  grant.observationToken,
                  observation,
                ),
              };
            },
            registrations,
            this.#webSocketFactory!,
            { clientId },
          );
          this.#customerEnvironmentInstances.set(clientId, partial);
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
      this.#customerEnvironments.set(clientId, starting);
      void starting.catch(() => {
        if (this.#customerEnvironments.get(clientId) === starting) {
          this.#customerEnvironments.delete(clientId);
          if (this.#customerEnvironmentInstances.get(clientId) === partial) {
            this.#customerEnvironmentInstances.delete(clientId);
          }
        }
      });
      await this.#waitForCustomerEnvironmentReadiness(clientId, starting, signal);
      return;
    }
    const hand = await this.#waitForCustomerEnvironmentReadiness(clientId, existing, signal);
    if (this.#closed) throw new SessionError("Aex client is closed");
    await waitWithSignal(hand.register(registrations), signal);
  }

  async #waitForCustomerEnvironmentReadiness(
    clientId: string,
    starting: Promise<CustomerEnvironment>,
    signal?: AbortSignal,
  ): Promise<CustomerEnvironment> {
    this.#customerEnvironmentReadinessWaiters.set(
      clientId,
      (this.#customerEnvironmentReadinessWaiters.get(clientId) ?? 0) + 1,
    );
    let settled = false;
    void starting.then(
      () => { settled = true; },
      () => { settled = true; },
    );
    try {
      return await waitWithSignal(starting, signal);
    } catch (error) {
      if (
        signal?.aborted && !settled
        && this.#customerEnvironmentReadinessWaiters.get(clientId) === 1
        && this.#customerEnvironments.get(clientId) === starting
      ) {
        const partial = this.#customerEnvironmentInstances.get(clientId);
        partial?.close();
        this.#customerEnvironments.delete(clientId);
        if (this.#customerEnvironmentInstances.get(clientId) === partial) {
          this.#customerEnvironmentInstances.delete(clientId);
        }
      }
      throw error;
    } finally {
      const waiters = this.#customerEnvironmentReadinessWaiters.get(clientId) ?? 1;
      if (waiters <= 1) this.#customerEnvironmentReadinessWaiters.delete(clientId);
      else this.#customerEnvironmentReadinessWaiters.set(clientId, waiters - 1);
    }
  }
}

function isApplicationEnvironment(
  value: ComponentExtension<"environment">,
): boolean {
  const config = value.config;
  return config !== null && typeof config === "object" && !Array.isArray(config) &&
    (config as { driver?: unknown }).driver === "customer";
}

function applicationEnvironmentId(value: ComponentExtension<"environment">): string {
  const config = value.config as { configuration?: { registration?: unknown } };
  const id = config.configuration?.registration;
  if (typeof id !== "string" || !/^[A-Za-z0-9_.:-]{1,128}$/u.test(id)) {
    throw new TypeError("app() Environment registration is invalid");
  }
  return id;
}

type ComponentToolConfig = Readonly<Record<string, unknown>> & {
  readonly definition: ToolDefinition;
};

function isComponentTool(
  value: unknown,
): value is ComponentExtension<"tool", ComponentToolConfig> {
  return value !== null
    && typeof value === "object"
    && (value as { kind?: unknown }).kind === "brain.component"
    && (value as { extension?: unknown }).extension === "tool";
}

function isEnvironmentComponent(value: unknown): value is ComponentExtension<"environment"> {
  return value !== null
    && typeof value === "object"
    && (value as { kind?: unknown }).kind === "brain.component"
    && (value as { extension?: unknown }).extension === "environment";
}

function componentToolDefinition(
  value: ComponentExtension<"tool", ComponentToolConfig>,
): ToolDefinition {
  const config = value.config;
  if (config === null || typeof config !== "object" || Array.isArray(config)) {
    throw new TypeError("Tool component config must be an object containing definition");
  }
  const definition = config.definition;
  if (definition === null || typeof definition !== "object" || Array.isArray(definition)) {
    throw new TypeError("Tool component config.definition is required");
  }
  return definition;
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

  get retainUntil(): string {
    return this.#data.retain_until;
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

  async suspend(options: Pick<RequestOptions, "signal"> = {}): Promise<this> {
    this.#data = await this.#transport.json<SessionData>(
      "POST",
      `/v1/sessions/${encodeURIComponent(this.id)}/suspend`,
      { signal: options.signal, retry: true },
    );
    return this;
  }

  async resume(options: Pick<RequestOptions, "signal"> = {}): Promise<this> {
    this.#data = await this.#transport.json<SessionData>(
      "POST",
      `/v1/sessions/${encodeURIComponent(this.id)}/resume`,
      { signal: options.signal, retry: true },
    );
    return this;
  }

  async setRetention(
    value: Date | string,
    options: Pick<RequestOptions, "signal"> & { allowShorten?: boolean } = {},
  ): Promise<this> {
    this.#data = await this.#transport.json<SessionData>(
      "POST",
      `/v1/sessions/${encodeURIComponent(this.id)}/retention`,
      {
        body: {
          retain_until: normalizeTimestamp(value, "retainUntil"),
          allow_shorten: options.allowShorten ?? false,
        },
        signal: options.signal,
        retry: true,
      },
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

function normalizeTimestamp(value: Date | string, field: string): string {
  const parsed = value instanceof Date ? value : new Date(value);
  if (!Number.isFinite(parsed.getTime())) throw new TypeError(`${field} must be a valid timestamp`);
  return parsed.toISOString();
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
