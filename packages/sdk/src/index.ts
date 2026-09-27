export * from "@aexhq/brain";
export type * from "./generated.js";
export { AexSessionHandle } from "./session.js";
export { StructuredOutputError } from "./structured-output.js";
export type { StructuredSendOptions } from "./structured-output.js";
import { application } from "@aexhq/env-http";
import { composeApplication } from "./application.js";
import { Brain, type BrainOptions, type CreateSessionOptions, type OperationOptions, type ModelSelection } from "@aexhq/brain";
import { AexSessionHandle } from "./session.js";
import type { ClientAccess, EnvironmentCatalog, TokenUsage } from "./generated.js";
import type { Attachment, Account, Usage, ApiKey, IssuedKey, KeyInput, LoginGrantInput, LoginGrant, LoginExchange, AccountSession, Wallet, BillingSettings, LedgerPage, Topup, TopupInput, Refund, RefundInput, SyncPayment } from "./generated.js";

export type AexConnection = Omit<BrainOptions, "token" | "baseUrl"> & { baseUrl?: string };
export type AexOptions = AexConnection & { maxCostMicroUsd?: number } & (
  { apiKey: string; accountToken?: never; clientAccess?: never } |
  { accountToken: string; apiKey?: never; clientAccess?: never } |
  { clientAccess: ClientAccess; apiKey?: never; accountToken?: never });
export type AexSessionOptions = Omit<CreateSessionOptions, "model"> & { model: ModelSelection | Omit<ModelSelection, "apiKey"> };

function nonnegativeInteger(value: number, name: string): void {
  if (!Number.isSafeInteger(value) || value < 0) throw new TypeError(`${name} must be a nonnegative safe integer`);
}

class HostedBrain extends Brain {
  private readonly maxCostMicroUsd?: number;
  constructor({ maxCostMicroUsd, ...options }: BrainOptions & { maxCostMicroUsd?: number }) {
    super(options);
    if (maxCostMicroUsd !== undefined) nonnegativeInteger(maxCostMicroUsd, "maxCostMicroUsd");
    this.maxCostMicroUsd = maxCostMicroUsd;
  }

  /** Resource and legacy turn reservations use this ceiling; token hosting follows the account limit. */
  override request<T>(method: string, path: string, body?: unknown, idempotencyKey?: string, contentType = "application/json", signal?: AbortSignal, extraHeaders?: HeadersInit): Promise<T> {
    const headers = new Headers(extraHeaders);
    if (this.maxCostMicroUsd !== undefined && !headers.has("x-aex-max-cost-micro-usd")) headers.set("x-aex-max-cost-micro-usd", String(this.maxCostMicroUsd));
    return super.request(method, path, body, idempotencyKey, contentType, signal, headers);
  }
}

export class Aex {
  private readonly client: Brain;
  private readonly clientAccess?: ClientAccess;

  constructor({ apiKey, accountToken, clientAccess, maxCostMicroUsd, ...options }: AexOptions) {
    if ([apiKey, accountToken, clientAccess].filter(Boolean).length !== 1) throw new TypeError("one apiKey, accountToken or clientAccess is required");
    this.clientAccess = clientAccess;
    this.client = new HostedBrain({ baseUrl: "https://api.aex.dev", ...options, token: apiKey ?? accountToken ?? clientAccess?.token,
      ...(clientAccess && { credentials: clientAccess.credentials }), maxCostMicroUsd });
  }

  private composition(options: AexSessionOptions): CreateSessionOptions {
    if (this.clientAccess && "apiKey" in options.model) throw new TypeError("client access uses server-side model credentials; omit apiKey");
    const model = this.clientAccess ? { ...options.model, apiKey: this.clientAccess.token } : options.model;
    if (!("apiKey" in model) || typeof model.apiKey !== "string") throw new TypeError("server-side sessions require a model apiKey");
    return composeApplication({ ...options, model: { ...model, apiKey: model.apiKey } }, this.baseUrl);
  }

  get baseUrl(): string { return this.client.baseUrl; }

  readonly sessions = Object.freeze({
    create: async (options: AexSessionOptions, operation?: OperationOptions): Promise<AexSessionHandle> =>
      new AexSessionHandle(await this.client.sessions.create(this.composition(options), operation)),
    get: async (...args: Parameters<Brain["sessions"]["get"]>): Promise<AexSessionHandle> => new AexSessionHandle(await this.client.sessions.get(...args)),
    list: (): ReturnType<Brain["sessions"]["list"]> => this.client.sessions.list(),
  });

  request<T>(...args: Parameters<Brain["request"]>): Promise<T> { return this.client.request<T>(...args); }
  withToken(...args: Parameters<Brain["withToken"]>): ReturnType<Brain["withToken"]> { return this.client.withToken(...args); }
  close(): ReturnType<Brain["close"]> { return this.client.close(); }
  models(...args: Parameters<Brain["models"]>): ReturnType<Brain["models"]> { return this.client.models(...args); }
  stream(...args: Parameters<Brain["stream"]>): ReturnType<Brain["stream"]> { return this.client.stream(...args); }
  streamPath(...args: Parameters<Brain["streamPath"]>): ReturnType<Brain["streamPath"]> { return this.client.streamPath(...args); }
  admit(...args: Parameters<Brain["admit"]>): ReturnType<Brain["admit"]> { return this.client.admit(...args); }
  admitAgentloop(...args: Parameters<Brain["admitAgentloop"]>): ReturnType<Brain["admitAgentloop"]> { return this.client.admitAgentloop(...args); }
  admitTool(...args: Parameters<Brain["admitTool"]>): ReturnType<Brain["admitTool"]> { return this.client.admitTool(...args); }
  register(): ReturnType<Brain["register"]> { return this.client.register(); }
  credentials(): ReturnType<Brain["credentials"]> { return this.client.credentials(); }

  static exchangeLogin(input: LoginExchange, options: AexConnection = {}): Promise<AccountSession> {
    return new Brain({ baseUrl: "https://api.aex.dev", ...options }).request("POST", "/v1/auth/exchange", input);
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
    grant: async ({ session, origin, expiresAt = Math.floor(Date.now() / 1000) + 3600 }: {
      session: CreateSessionOptions; origin: string; expiresAt?: number;
    }): Promise<ClientAccess> => {
      if (this.clientAccess) throw new TypeError("client grants require a backend API key");
      const prepared = await this.client.sessions.prepare(this.composition(session), "authorized-client");
      return this.request("POST", "/v1/clients", { origin, expires_at: expiresAt, session: prepared });
    },
    revoke: (id: string): Promise<void> => this.request("DELETE", `/v1/clients/${encodeURIComponent(id)}`),
  };

  readonly environments = {
    application: (options: Omit<Parameters<typeof application>[0], "url" | "credential"> & { credential?: string }) => application({
      ...options, credential: options.credential ?? this.clientAccess?.token ?? "",
      url: new URL("/environments/application", this.baseUrl).href,
    }),
    http: (): Promise<import("./generated.js").HttpCatalog> => this.request("GET", "/v1/environments/http"),
    list: (): Promise<EnvironmentCatalog> => this.request("GET", "/v1/environments"),
  };

  /** Key management requires an account session; workload API keys cannot create credentials. */
  readonly keys = {
    list: (): Promise<ApiKey[]> => this.request("GET", "/v1/keys"),
    create: (input: KeyInput): Promise<IssuedKey> => this.request("POST", "/v1/keys", input),
    update: (id: string, input: KeyInput): Promise<ApiKey> => this.request("PATCH", `/v1/keys/${encodeURIComponent(id)}`, input),
    delete: (id: string): Promise<void> => this.request("DELETE", `/v1/keys/${encodeURIComponent(id)}`),
  };
}
