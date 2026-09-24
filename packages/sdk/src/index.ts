export * from "@aexhq/brain";
export type * from "./generated.js";
import { Brain, type BrainOptions } from "@aexhq/brain";
import type { EnvironmentCatalog, TokenUsage } from "./generated.js";
import type { Attachment, Account, Usage, ApiKey, IssuedKey, KeyInput, LoginGrantInput, LoginGrant, LoginExchange, AccountSession, Wallet, BillingSettings, LedgerPage, Topup, TopupInput, Refund, RefundInput, SyncPayment } from "./generated.js";

export type AexConnection = Omit<BrainOptions, "token" | "baseUrl"> & { baseUrl?: string };
export type AexOptions = AexConnection & { maxCostMicroUsd?: number } & ({ apiKey: string; accountToken?: never } | { accountToken: string; apiKey?: never });

function nonnegativeInteger(value: number, name: string): void {
  if (!Number.isSafeInteger(value) || value < 0) throw new TypeError(`${name} must be a nonnegative safe integer`);
}

export class Aex extends Brain {
  private readonly maxCostMicroUsd?: number;
  constructor({ apiKey, accountToken, maxCostMicroUsd, ...options }: AexOptions) {
    if (!(apiKey || accountToken) || (apiKey && accountToken)) throw new TypeError("one apiKey or accountToken is required");
    super({ baseUrl: "https://api.aex.dev", ...options, token: apiKey ?? accountToken });
    if (maxCostMicroUsd !== undefined) nonnegativeInteger(maxCostMicroUsd, "maxCostMicroUsd");
    this.maxCostMicroUsd = maxCostMicroUsd;
  }

  /** Resource and legacy turn reservations use this ceiling; token hosting follows the account limit. */
  override request<T>(method: string, path: string, body?: unknown, idempotencyKey?: string, contentType = "application/json", signal?: AbortSignal, extraHeaders?: HeadersInit): Promise<T> {
    const headers = new Headers(extraHeaders);
    if (this.maxCostMicroUsd !== undefined && !headers.has("x-aex-max-cost-micro-usd")) headers.set("x-aex-max-cost-micro-usd", String(this.maxCostMicroUsd));
    return super.request(method, path, body, idempotencyKey, contentType, signal, headers);
  }

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

  readonly environments = {
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
