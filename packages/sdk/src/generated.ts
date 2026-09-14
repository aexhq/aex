/* Generated from aex-server schemas. */
import type {Media} from "@aexhq/brain";

/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Billing".
 */
export type Billing = "preview_customer_model_keys" | "prepaid_customer_model_keys";
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "BillingMode".
 */
export type BillingMode = "preview" | "prepaid";
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Mode".
 */
export type Mode = "test" | "live";
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "SyncPayment".
 */
export type SyncPayment =
  | {
      checkout_id?: string | null;
      id: string;
      kind: "topup";
    }
  | {
      id: string;
      kind: "refund";
      refund_id?: string | null;
    };

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
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Pricebook".
 */
export interface Pricebook {
  id: string;
  rates: {
    attachment_byte_secs?: Rate;
    egress_bytes?: Rate;
    sandbox_ms?: Rate;
    turn_ms?: Rate;
  };
}
/**
 * A rational price in micro-USD. Rating rounds once over cumulative resource usage.
 *
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Rate".
 */
export interface Rate {
  micro_usd: number;
  units: number;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Wallet".
 */
export interface Wallet {
  accepted_pricebook?: string | null;
  available_micro_usd: number;
  balance_micro_usd: number;
  currency: string;
  mode: BillingMode;
  offered_pricebook?: Pricebook | null;
  payment_mode?: Mode | null;
  reserved_micro_usd: number;
  spend_limit_micro_usd?: number | null;
  spent_this_month_micro_usd: number;
  suspended: boolean;
  topup_amounts_cents: number[];
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "BillingSettings".
 */
export interface BillingSettings {
  pricebook: string;
  spend_limit_micro_usd: number;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "LedgerEntry".
 */
export interface LedgerEntry {
  created: number;
  delta_micro_usd: number;
  description: string;
  id: number;
  kind: string;
  reference: string;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "LedgerPage".
 */
export interface LedgerPage {
  entries: LedgerEntry[];
  next_before?: number | null;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "TopupInput".
 */
export interface TopupInput {
  amount_cents: number;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Topup".
 */
export interface Topup {
  amount_cents: number;
  checkout_url?: string | null;
  created: number;
  id: string;
  receipt_url?: string | null;
  refunded_cents: number;
  state: string;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "RefundInput".
 */
export interface RefundInput {
  amount_cents: number;
  topup: string;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "Refund".
 */
export interface Refund {
  amount_cents: number;
  created: number;
  id: string;
  state: string;
  topup: string;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "EnvironmentProfile".
 */
export interface EnvironmentProfile {
  commands: {
    [k: string]: string[];
  };
  cpu: number;
  image: string;
  maxLifetimeMs: number;
  maxOutputBytes: number;
  memoryMiB: number;
  outboundDomains: string[];
  region: string;
  workdir: string;
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "EnvironmentCatalog".
 */
export interface EnvironmentCatalog {
  driver_url: string;
  profiles: {
    [k: string]: EnvironmentProfile;
  };
}
/**
 * This interface was referenced by `AexContracts`'s JSON-Schema
 * via the `definition` "EnvironmentSelection".
 */
export interface EnvironmentSelection {
  lifetimeMs: number;
  profile: string;
}
