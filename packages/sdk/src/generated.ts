/* Generated from aex-server schemas. */
import type {Media} from "@aexhq/brain";

/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Billing".
 */
export type Billing = "preview_customer_model_keys";

export interface AexContracts {}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Attachment".
 */
export interface Attachment {
  expires_at: number;
  id: string;
  media: Media;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Limits".
 */
export interface Limits {
  accounts: number;
  active_turns_per_account: number;
  artifact_bytes_per_account: number;
  artifacts_per_account: number;
  claims_per_account: number;
  hosts_per_account: number;
  keys_per_account: number;
  minimum_free_disk_bytes: number;
  request_bytes: number;
  requests: number;
  requests_per_account: number;
  response_bytes: number;
  retained_bytes_per_account: number;
  retention_secs: number;
  sessions_per_account: number;
  streams_per_account: number;
  turn_reserve_bytes: number;
  upstream_timeout_secs: number;
  usage_max_age_secs: number;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Usage".
 */
export interface Usage {
  active_turns: number;
  attachment_bytes: number;
  attachments: number;
  measured_at?: number | null;
  retained_bytes: number;
  sessions: number;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Account".
 */
export interface Account {
  billing: Billing;
  created: number;
  email?: string | null;
  id: string;
  limits: Limits;
  usage: Usage;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "ApiKey".
 */
export interface ApiKey {
  active: boolean;
  created: number;
  id: string;
  name: string;
  prefix: string;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "IssuedKey".
 */
export interface IssuedKey {
  key: ApiKey;
  token: string;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "KeyInput".
 */
export interface KeyInput {
  name: string;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "LoginGrantInput".
 */
export interface LoginGrantInput {
  code_challenge: string;
  redirect_uri: string;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "LoginGrant".
 */
export interface LoginGrant {
  code: string;
  expires: number;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "LoginExchange".
 */
export interface LoginExchange {
  code: string;
  code_verifier: string;
  redirect_uri: string;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "AccountSession".
 */
export interface AccountSession {
  expires: number;
  token: string;
}
