export * from "@aexhq/brain";
export type * from "./generated.js";
import { Brain, type BrainOptions } from "@aexhq/brain";
import type { Attachment, Account, Usage, ApiKey, IssuedKey, KeyInput, LoginGrantInput, LoginGrant, LoginExchange, AccountSession } from "./generated.js";

export type AexConnection = Omit<BrainOptions, "token" | "baseUrl"> & { baseUrl?: string };
export type AexOptions = AexConnection & ({ apiKey: string; accountToken?: never } | { accountToken: string; apiKey?: never });

export class Aex extends Brain {
  constructor({ apiKey, accountToken, ...options }: AexOptions) {
    if (!(apiKey || accountToken) || (apiKey && accountToken)) throw new TypeError("one apiKey or accountToken is required");
    super({ baseUrl: "https://api.aex.dev", ...options, token: apiKey ?? accountToken });
  }

  static exchangeLogin(input: LoginExchange, options: AexConnection = {}): Promise<AccountSession> {
    return new Brain({ baseUrl: "https://api.aex.dev", ...options }).request("POST", "/v1/auth/exchange", input);
  }

  readonly attachments = {
    upload: (sessionId: string, bytes: Uint8Array, options: { contentType: string; expiresAt?: number; idempotencyKey: string; signal?: AbortSignal }): Promise<Attachment> => {
      if (options.expiresAt !== undefined && (!Number.isSafeInteger(options.expiresAt) || options.expiresAt <= 0)) throw new TypeError("expiresAt must be positive Unix seconds");
      return this.request("POST", `/v1/sessions/${encodeURIComponent(sessionId)}/attachments`, bytes,
        options.idempotencyKey, options.contentType, options.signal,
        options.expiresAt === undefined ? undefined : { "x-aex-expires-at": String(options.expiresAt) });
    },
    delete: (sessionId: string, id: string): Promise<void> => this.request("DELETE", `/v1/sessions/${encodeURIComponent(sessionId)}/attachments/${encodeURIComponent(id)}`),
  };

  readonly account = {
    get: (): Promise<Account> => this.request("GET", "/v1/account"),
    usage: (): Promise<Usage> => this.request("GET", "/v1/usage"),
    logout: (): Promise<void> => this.request("DELETE", "/v1/account/session"),
    authorizeLogin: (input: LoginGrantInput): Promise<LoginGrant> => this.request("POST", "/v1/auth/grants", input),
  };

  /** Key management requires an account session; workload API keys cannot create credentials. */
  readonly keys = {
    list: (): Promise<ApiKey[]> => this.request("GET", "/v1/keys"),
    create: (input: KeyInput): Promise<IssuedKey> => this.request("POST", "/v1/keys", input),
    update: (id: string, input: KeyInput): Promise<ApiKey> => this.request("PATCH", `/v1/keys/${encodeURIComponent(id)}`, input),
    delete: (id: string): Promise<void> => this.request("DELETE", `/v1/keys/${encodeURIComponent(id)}`),
  };
}
