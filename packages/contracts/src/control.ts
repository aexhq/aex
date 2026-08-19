/* eslint-disable */
/**
 * GENERATED from contracts/control/v1/schemas.json by packages/contracts/scripts/gen.mjs (tools/gen.sh). DO NOT EDIT.
 */

/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "AccountId".
 */
export type AccountId = string;
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "KeyId".
 */
export type KeyId = string;
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "TopupId".
 */
export type TopupId = string;
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "RefundId".
 */
export type RefundId = string;
/**
 * RFC 3339, UTC.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "Timestamp".
 */
export type Timestamp = string;
/**
 * Integer micro-USD; 1 USD = 1,000,000. Never a float.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "MicroUsd".
 */
export type MicroUsd = number;
/**
 * Manages the account: keys, top-ups, the bill. Shown once at signup; only a hash is stored.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "AccountToken".
 */
export type AccountToken = string;
/**
 * Runs sessions. Shown once at creation; only a hash is stored.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "ApiKeySecret".
 */
export type ApiKeySecret = string;
/**
 * One-time Founding Beta invitation. Shown once to the operator; only a hash is stored.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "InvitationToken".
 */
export type InvitationToken = string;
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "WaitlistStatus".
 */
export type WaitlistStatus = "waiting" | "invited" | "joined";
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "TopupStatus".
 */
export type TopupStatus = "pending" | "paid" | "expired";
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "RefundStatus".
 */
export type RefundStatus = "pending" | "succeeded" | "failed";
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "ControlErrorCode".
 */
export type ControlErrorCode =
  | "invalid_request"
  | "unauthorized"
  | "forbidden"
  | "not_found"
  | "conflict"
  | "insufficient_balance"
  | "rate_limited"
  | "payment_error"
  | "upstream_error"
  | "internal";

/**
 * Component types of the control plane: identity (accounts, API keys), prepaid billing (top-ups, balance), and rated usage on the two-rate card. Paths are in openapi.yaml. Session operations are NOT redefined here: the control plane serves session/v1 paths verbatim, authorized by an API key, in front of a brain. All money on the wire is integer micro-USD (1 USD = 1,000,000 micro-USD) except top-up amounts, which are whole cents (the payment surface). Storage is metered in decimal GB (1 GB = 1e9 bytes); a month is 730 hours.
 */
export interface AexControlAPIV1Types {
  [k: string]: unknown | undefined;
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "JoinWaitlistRequest".
 */
export interface JoinWaitlistRequest {
  email: string;
}
/**
 * A privacy-preserving acknowledgement. It does not reveal whether the email was already waiting, invited, or joined.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "WaitlistSubmission".
 */
export interface WaitlistSubmission {
  object: "waitlist_submission";
  email: string;
  status: "received";
  received_at: Timestamp;
}
/**
 * Operator view of one canonical Founding Beta waitlist record.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "WaitlistEntry".
 */
export interface WaitlistEntry {
  object: "waitlist_entry";
  email: string;
  status: WaitlistStatus;
  created_at: Timestamp;
  invited_at?: Timestamp;
  joined_at?: Timestamp;
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "WaitlistEntryList".
 */
export interface WaitlistEntryList {
  object: "list";
  data: WaitlistEntry[];
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "CreateInvitationRequest".
 */
export interface CreateInvitationRequest {
  email: string;
}
/**
 * The invitation token appears here and never again. Creating another invitation rotates it.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "InvitationCreated".
 */
export interface InvitationCreated {
  object: "invitation";
  email: string;
  invite_token: InvitationToken;
  invited_at: Timestamp;
}
/**
 * Abuse controls (ARCHITECTURE-v1 §2.9): card + minimum top-up, concurrency and create-rate caps.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "AccountLimits".
 */
export interface AccountLimits {
  max_concurrent_sessions: number;
  session_creates_per_hour: number;
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "Account".
 */
export interface Account {
  id: AccountId;
  object: "account";
  email: string;
  created_at: Timestamp;
  limits: AccountLimits;
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "CreateAccountRequest".
 */
export interface CreateAccountRequest {
  email: string;
  invite_token: InvitationToken;
}
/**
 * The account token appears here and never again.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "AccountCreated".
 */
export interface AccountCreated {
  account: Account;
  account_token: AccountToken;
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "ApiKey".
 */
export interface ApiKey {
  id: KeyId;
  object: "api_key";
  name: string;
  /**
   * First characters of the secret, for recognising a key in a list. Never enough to authenticate.
   */
  prefix: string;
  created_at: Timestamp;
  last_used_at?: Timestamp;
  revoked_at?: Timestamp;
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "CreateApiKeyRequest".
 */
export interface CreateApiKeyRequest {
  name: string;
}
/**
 * The secret appears here and never again.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "ApiKeyCreated".
 */
export interface ApiKeyCreated {
  key: ApiKey;
  secret: ApiKeySecret;
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "ApiKeyList".
 */
export interface ApiKeyList {
  object: "list";
  data: ApiKey[];
}
/**
 * Prepaid balance = credits minus rated usage, metered up to `metered_to`. May be negative: usage is rated after the fact; new sessions and messages are refused while it is not positive.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "Balance".
 */
export interface Balance {
  object: "balance";
  microusd: MicroUsd;
  /**
   * Display form, whole cents, rounded toward zero.
   */
  usd: string;
  metered_to: Timestamp;
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "CreateTopupRequest".
 */
export interface CreateTopupRequest {
  /**
   * Whole cents. Founding Beta top-ups are $10.00 to $1,000.00.
   */
  amount_cents: number;
}
/**
 * A prepaid credit purchase. `checkout_url` is where the customer pays (Stripe Checkout); present while pending. The balance is credited when the payment provider reports it paid — on webhook or on poll, idempotently.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "Topup".
 */
export interface Topup {
  id: TopupId;
  object: "topup";
  amount_cents: number;
  status: TopupStatus;
  checkout_url?: string;
  created_at: Timestamp;
  paid_at?: Timestamp;
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "TopupList".
 */
export interface TopupList {
  object: "list";
  data: Topup[];
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "CreateRefundRequest".
 */
export interface CreateRefundRequest {
  topup_id: TopupId;
  /**
   * Whole cents of unused prepaid credit to return from this top-up.
   */
  amount_cents: number;
}
/**
 * An operator-initiated return of unused prepaid credit. Credit is reserved before the payment provider is called. Retrying the same Idempotency-Key returns the same refund.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "Refund".
 */
export interface Refund {
  id: RefundId;
  object: "refund";
  topup_id: TopupId;
  amount_cents: number;
  status: RefundStatus;
  created_at: Timestamp;
  updated_at: Timestamp;
  /**
   * Operator-facing payment-provider failure detail; present only when status is failed.
   */
  failure_reason?: string;
}
/**
 * The two-rate card (ARCHITECTURE-v1 D4). Compute is billed per second while running on the shape's BASELINE (vCPU = memory/2; bursts are free); the pre-suspend idle window is absorbed. Suspended storage covers the bytes the substrate holds for a suspended hand; workspace storage covers synced workspace objects AND persisted artifacts. GB is decimal (1e9 bytes); a month is `month_hours` hours.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "RateCard".
 */
export interface RateCard {
  object: "rate_card";
  vcpu_hour_microusd: MicroUsd;
  gb_hour_microusd: MicroUsd;
  suspended_gb_month_microusd: MicroUsd;
  workspace_gb_month_microusd: MicroUsd;
  web_search_query_microusd: MicroUsd;
  month_hours: number;
}
/**
 * Current stored bytes, as last reported by the brain (session/v1 StorageInfo).
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "StorageMeters".
 */
export interface StorageMeters {
  workspace_bytes: number;
  suspended_bytes: number;
  artifact_bytes: number;
}
/**
 * One session's rated line. Compute time is the sum of turn intervals (turn.started to turn.completed/failed) folded from the session's event log — the journal is the billing record. Storage integrals are exact byte-seconds of the brain-reported meters, piecewise-constant between meter readings. Successful web_search tool results are counted from the same event log.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "SessionUsage".
 */
export interface SessionUsage {
  session_id: string;
  /**
   * HandShape from session/v1 (1gb | 2gb | 4gb | 8gb).
   */
  shape: string;
  /**
   * SessionState from session/v1 (active | idle | deleted | failed).
   */
  state: string;
  running_ms: number;
  suspended_byte_seconds: number;
  workspace_byte_seconds: number;
  artifact_byte_seconds: number;
  web_search_queries: number;
  compute_microusd: MicroUsd;
  storage_microusd: MicroUsd;
  web_search_microusd: MicroUsd;
  total_microusd: MicroUsd;
  storage: StorageMeters;
  metered_to: Timestamp;
}
/**
 * The bill: every session's rated line, the account balance after them, and the rate card they were rated on.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "Usage".
 */
export interface Usage {
  object: "usage";
  account_id: AccountId;
  balance_microusd: MicroUsd;
  total_microusd: MicroUsd;
  sessions: SessionUsage[];
  rates: RateCard;
  metered_to: Timestamp;
}
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "ControlError".
 */
export interface ControlError {
  code: ControlErrorCode;
  message: string;
  request_id?: string;
}
/**
 * Error envelope of the control plane's own endpoints. Proxied session/v1 endpoints keep the session ApiError envelope; the control plane injects only codes that ApiErrorCode already has (insufficient_balance, rate_limited, unauthorized, forbidden, not_found).
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "ControlErrorResponse".
 */
export interface ControlErrorResponse {
  error: ControlError;
}
