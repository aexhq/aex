export * from "@aexhq/brain";
export type * from "./generated.js";
import { Brain, type BrainOptions } from "@aexhq/brain";
import type { Account, Usage, ApiKey, IssuedKey, KeyInput } from "./generated.js";

export type AexOptions = Omit<BrainOptions, "token" | "baseUrl"> & { apiKey: string; baseUrl?: string };

export class Aex extends Brain {
  constructor({ apiKey, ...options }: AexOptions) {
    if (!apiKey) throw new TypeError("apiKey is required");
    super({ baseUrl: "https://api.aex.dev", ...options, token: apiKey });
  }

  readonly account = {
    get: (): Promise<Account> => this.request("GET", "/v1/account"),
    usage: (): Promise<Usage> => this.request("GET", "/v1/usage"),
  };

  /** Key management requires an account session; workload API keys cannot create credentials. */
  readonly keys = {
    list: (): Promise<ApiKey[]> => this.request("GET", "/v1/keys"),
    create: (input: KeyInput): Promise<IssuedKey> => this.request("POST", "/v1/keys", input),
    update: (id: string, input: KeyInput): Promise<ApiKey> => this.request("PATCH", `/v1/keys/${encodeURIComponent(id)}`, input),
    delete: (id: string): Promise<void> => this.request("DELETE", `/v1/keys/${encodeURIComponent(id)}`),
  };
}
