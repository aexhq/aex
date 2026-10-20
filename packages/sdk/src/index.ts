export { Brain, BrainError, ApiKey, CredentialReference, Model, Tool, ManagedTool, connectTab } from "@aexhq/brain";
export type { SessionInput, SessionOptions, RuntimeTool, RuntimeContext, Session, Event, Input, CreateSession, ToolBinding, ToolScope, ModelCredential } from "@aexhq/brain";
export type * from "./generated.js";
export { Application } from "./application.js";
export { createApplicationHandler } from "./application-handler.js";
export type { ApplicationContext, ApplicationTool } from "./application-handler.js";
export { AexSessionHandle } from "./session.js";
export type { SendOptions, Turn, UserInput } from "./session.js";
export { StructuredOutputError } from "./structured-output.js";
export type { StructuredSendOptions } from "./structured-output.js";
import { Brain, BrainError, connectTab, type SessionInput, type CreateSession, type RuntimeTool } from "@aexhq/brain";
import { AexSessionHandle, type SendOptions } from "./session.js";
import type { ClientAccess, TokenUsage, Attachment, Account, Usage, ApiKey, IssuedKey, KeyInput, LoginGrantInput, LoginGrant, LoginExchange, AccountSession, Wallet, BillingSettings, LedgerPage, Topup, TopupInput, Refund, RefundInput, SyncPayment } from "./generated.js";

export type AexConnection = { baseUrl?: string; fetch?: typeof fetch; timeoutMs?: number };
export type AexOptions = AexConnection & { maxCostMicroUsd?: number } & (
  { apiKey: string; accountToken?: never; clientAccess?: never } |
  { accountToken: string; apiKey?: never; clientAccess?: never } |
  { clientAccess: ClientAccess; apiKey?: never; accountToken?: never });
export type AexSessionOptions = SessionInput;
function nonnegativeInteger(value: number, name: string): void {
  if (!Number.isSafeInteger(value) || value < 0) throw new TypeError(`${name} must be a nonnegative safe integer`);
}
export class Aex {
  readonly baseUrl: string;
  private readonly token: string;
  private readonly fetcher: typeof fetch;
  private readonly abort = new AbortController();
  private readonly brains = new Set<Brain>();
  private readonly timeoutMs: number;
  private readonly maxCostMicroUsd?: number;
  private readonly clientAccess?: ClientAccess;
  private readonly client: Brain;
  private closing?: Promise<void>;
  constructor({ apiKey, accountToken, clientAccess, baseUrl = "https://api.aex.dev", fetch: fetcher = (...args) => globalThis.fetch(...args), timeoutMs = 30000, maxCostMicroUsd }: AexOptions) {
    if ([apiKey, accountToken, clientAccess].filter(Boolean).length !== 1) throw new TypeError("one apiKey, accountToken or clientAccess is required");
    const url = new URL(baseUrl);
    if (url.origin !== baseUrl || !["http:", "https:"].includes(url.protocol)) throw new TypeError("baseUrl must be an HTTP origin");
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) throw new TypeError("timeoutMs must be positive");
    if (maxCostMicroUsd !== undefined) nonnegativeInteger(maxCostMicroUsd, "maxCostMicroUsd");
    this.baseUrl = baseUrl; this.token = apiKey ?? accountToken ?? clientAccess!.token;
    this.fetcher = fetcher; this.timeoutMs = timeoutMs; this.maxCostMicroUsd = maxCostMicroUsd; this.clientAccess = clientAccess;
    this.client = this.brain();
  }
  private brain(operationKey?: string): Brain {
    const transport: typeof fetch = (input, init = {}) => {
      const url = new URL(input instanceof Request ? input.url : String(input));
      const cleanup = init.method === "POST" && /^\/executors\/[a-f0-9]{32}\/close$/.test(url.pathname);
      if (!cleanup) this.abort.signal.throwIfAborted();
      if (url.origin !== this.baseUrl) throw new TypeError("hosted requests must use the configured origin");
      const headers = new Headers(init.headers);
      if (init.method === "POST" && url.pathname === "/sessions") headers.set("idempotency-key", operationKey ?? crypto.randomUUID());
      if (this.maxCostMicroUsd !== undefined) headers.set("x-aex-max-cost-micro-usd", String(this.maxCostMicroUsd));
      const signal = AbortSignal.any([...(cleanup ? [] : [this.abort.signal]), ...(init.signal ? [init.signal] : [])]);
      return this.fetcher(input, { ...init, headers, signal, redirect: "error" });
    };
    const client = new Brain(this.baseUrl, this.token, transport);
    this.brains.add(client);
    return client;
  }
  readonly sessions = Object.freeze({
    create: async (input: AexSessionOptions, options: SendOptions = {}): Promise<AexSessionHandle> => {
      if ("runtime_transport" in input && input.runtime_transport !== undefined && input.runtime_transport !== "sse") throw new TypeError("hosted runtime callbacks use SSE/HTTP");
      const client = this.brain(options.idempotencyKey);
      try { return new AexSessionHandle(client, await client.create(input, options.signal)); }
      catch (error) { this.brains.delete(client); await client.dispose(); throw error; }
    },
    get: async (id: string, options: { signal?: AbortSignal } = {}): Promise<AexSessionHandle> => new AexSessionHandle(this.client, await this.client.get(id, options.signal)),
    list: (after = "", limit = 100): ReturnType<Brain["list"]> => this.client.list(after, limit),
  });
  attachTab(input: { create: SessionInput } | { session: string; tools: RuntimeTool[] }): Promise<{ session: AexSessionHandle; detach(): Promise<void> }> {
    return connectTab(this.client, input).then(attached => ({ ...attached, session: new AexSessionHandle(this.client, attached.session) }));
  }
  models(): Promise<unknown> { return this.request("GET", "/v1/models"); }
  tools(): Promise<import("./generated.js").Catalog> { return this.request("GET", "/tools"); }
  close(): Promise<void> {
    if (this.closing) return this.closing;
    this.abort.abort(new DOMException("Aex client closed", "AbortError"));
    const clients = [...this.brains]; this.brains.clear();
    return this.closing = Promise.allSettled(clients.map(client => client.dispose())).then(outcomes => {
      const errors = outcomes.filter((value): value is PromiseRejectedResult => value.status === "rejected");
      if (errors.length) throw new AggregateError(errors.map(value => value.reason), "executor cleanup failed");
    });
  }
  async request<T>(method: string, path: string, body?: unknown, idempotencyKey?: string, contentType = "application/json", signal?: AbortSignal, extraHeaders?: HeadersInit): Promise<T> {
    this.abort.signal.throwIfAborted();
    const url = new URL(path, this.baseUrl);
    if (url.origin !== this.baseUrl) throw new TypeError("hosted requests must use the configured origin");
    const headers = new Headers(extraHeaders);
    if (path !== "/v1/auth/exchange") headers.set("authorization", `Bearer ${this.token}`);
    headers.set("content-type", contentType);
    if (idempotencyKey) headers.set("idempotency-key", idempotencyKey);
    if (this.maxCostMicroUsd !== undefined && !headers.has("x-aex-max-cost-micro-usd")) headers.set("x-aex-max-cost-micro-usd", String(this.maxCostMicroUsd));
    const response = await this.fetcher(url, { method, headers, ...(body === undefined ? {} : { body: body instanceof Uint8Array ? new Uint8Array(body) : JSON.stringify(body) }), signal: AbortSignal.any([this.abort.signal, signal ?? AbortSignal.timeout(this.timeoutMs)]), redirect: "error" });
    if (!response.ok) {
      const envelope = await response.json() as { error?: { code: string; message: string }; request_id?: string };
      throw new BrainError(response.status, envelope.error?.code ?? "http_error", envelope.error?.message ?? response.statusText, envelope.request_id);
    }
    return response.status === 204 ? undefined as T : response.json() as Promise<T>;
  }
  static async exchangeLogin(input: LoginExchange, options: AexConnection = {}): Promise<AccountSession> {
    const client = new Aex({ ...options, accountToken: "login-exchange" });
    try { return await client.request("POST", "/v1/auth/exchange", input); } finally { await client.close(); }
  }
  readonly attachments = {
    limits: (): Promise<import("./generated.js").AttachmentLimits> => this.request("GET", "/v1/attachments/limits"),
    grant: (sessionId: string, input: import("./generated.js").UploadGrantInput, options: { idempotencyKey: string; downloadBudgetBytes?: number; maxCostMicroUsd?: number }): Promise<import("./generated.js").UploadGrant> => {
      const headers = new Headers();
      if (options.downloadBudgetBytes !== undefined) { nonnegativeInteger(options.downloadBudgetBytes, "downloadBudgetBytes"); headers.set("x-aex-download-budget-bytes", String(options.downloadBudgetBytes)); }
      if (options.maxCostMicroUsd !== undefined) { nonnegativeInteger(options.maxCostMicroUsd, "maxCostMicroUsd"); headers.set("x-aex-max-cost-micro-usd", String(options.maxCostMicroUsd)); }
      return this.request("POST", `/v1/sessions/${encodeURIComponent(sessionId)}/attachment-grants`, input, options.idempotencyKey, "application/json", undefined, headers);
    },
    get: (sessionId: string, id: string): Promise<Attachment> => this.request("GET", `/v1/sessions/${encodeURIComponent(sessionId)}/attachments/${encodeURIComponent(id)}`),
    upload: (sessionId: string, bytes: Uint8Array, options: { contentType: string; expiresAt?: number; idempotencyKey: string; downloadBudgetBytes?: number; maxCostMicroUsd?: number; signal?: AbortSignal }): Promise<Attachment> => {
      if (options.expiresAt !== undefined && (!Number.isSafeInteger(options.expiresAt) || options.expiresAt <= 0)) throw new TypeError("expiresAt must be positive Unix seconds");
      const headers: Record<string, string> = {};
      if (options.expiresAt !== undefined) headers["x-aex-expires-at"] = String(options.expiresAt);
      if (options.maxCostMicroUsd !== undefined) { nonnegativeInteger(options.maxCostMicroUsd,"maxCostMicroUsd"); headers["x-aex-max-cost-micro-usd"] = String(options.maxCostMicroUsd); }
      if (options.downloadBudgetBytes !== undefined) { nonnegativeInteger(options.downloadBudgetBytes,"downloadBudgetBytes"); headers["x-aex-download-budget-bytes"] = String(options.downloadBudgetBytes); }
      return this.request("POST", `/v1/sessions/${encodeURIComponent(sessionId)}/attachments`, bytes,
        options.idempotencyKey, options.contentType, options.signal, headers);
    },
    delete: (sessionId: string, id: string): Promise<void> => this.request("DELETE", `/v1/sessions/${encodeURIComponent(sessionId)}/attachments/${encodeURIComponent(id)}`),
  };

  readonly account = {
    get: (): Promise<Account> => this.request("GET", "/v1/account"),
    usage: (): Promise<Usage> => this.request("GET", "/v1/usage"),
    modelUsage: (sessionId: string): Promise<TokenUsage> => this.request("GET", `/v1/usage/${encodeURIComponent(sessionId)}`),
    logout: (): Promise<void> => this.request("DELETE", "/v1/account/session"),
    authorizeLogin: (input: LoginGrantInput): Promise<LoginGrant> => this.request("POST", "/v1/auth/grants", input),
  };

  readonly billing = {
    get: (): Promise<Wallet> => this.request("GET", "/v1/billing"),
    update: (input: BillingSettings): Promise<Wallet> => this.request("PUT", "/v1/billing", input),
    ledger: (before?: number): Promise<LedgerPage> => {
      if (before !== undefined) nonnegativeInteger(before,"before");
      return this.request("GET", `/v1/billing/ledger${before === undefined ? "" : `?before=${before}`}`);
    },
    topups: (): Promise<Topup[]> => this.request("GET", "/v1/billing/topups"),
    topup: (input: TopupInput, idempotencyKey: string): Promise<Topup> => this.request("POST", "/v1/billing/topups", input, idempotencyKey),
    refunds: (): Promise<Refund[]> => this.request("GET", "/v1/billing/refunds"),
    refund: (input: RefundInput, idempotencyKey: string): Promise<Refund> => this.request("POST", "/v1/billing/refunds", input, idempotencyKey),
    sync: (input: SyncPayment): Promise<Wallet> => this.request("POST", "/v1/billing/sync", input),
  };

  readonly clients = {
    composition: (): Promise<import("./generated.js").Composition> => this.request("GET", "/composition"),
    grant: ({ session, tools = [], origin, expiresAt = Math.floor(Date.now()/1000)+3600 }: { session: CreateSession | string; tools?: import("@aexhq/brain").Tool[]; origin: string; expiresAt?: number }): Promise<ClientAccess> => {
      if (this.clientAccess) throw new TypeError("client grants require a backend API key");
      return this.request("POST", "/v1/clients", { origin, expires_at: expiresAt, session, tools });
    },
    revoke: (id: string): Promise<void> => this.request("DELETE", `/v1/clients/${encodeURIComponent(id)}`),
  };
  /** Key management requires an account session; workload API keys cannot create credentials. */
  readonly keys = {
    list: (): Promise<ApiKey[]> => this.request("GET", "/v1/keys"),
    create: (input: KeyInput): Promise<IssuedKey> => this.request("POST", "/v1/keys", input),
    update: (id: string, input: KeyInput): Promise<ApiKey> => this.request("PATCH", `/v1/keys/${encodeURIComponent(id)}`, input),
    delete: (id: string): Promise<void> => this.request("DELETE", `/v1/keys/${encodeURIComponent(id)}`),
  };
}
