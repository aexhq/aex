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
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "CreditGrantId".
 */
export type CreditGrantId = string;
/**
 * RFC 3339, UTC.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "Timestamp".
 */
export type Timestamp = string;
/**
 * Canonical signed decimal-string integer micro-USD; 1 USD = 1,000,000. Parse with arbitrary-precision integer arithmetic such as JavaScript BigInt; never Number or floating point.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "MicroUsd".
 */
export type MicroUsd = string;
/**
 * Canonical unsigned decimal-string integer. Parse with arbitrary-precision integer arithmetic such as JavaScript BigInt; never Number or floating point.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "UnsignedDecimalInteger".
 */
export type UnsignedDecimalInteger = string;
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
 * One-time alpha invitation. Shown once to the operator; only a hash is stored.
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
 * Component types of the control plane: identity (accounts, API keys), prepaid billing (top-ups, balance), and rated usage on the public rate card. Paths are in openapi.yaml. Session operations are NOT redefined here: the control plane serves session/v1 paths verbatim, authorized by an API key, in front of a brain. All money on the wire is a canonical decimal-string integer in micro-USD (1 USD = 1,000,000 micro-USD) except top-up amounts, which are whole cents (the payment surface). Storage is metered in decimal GB (1 GB = 1e9 bytes); a month is 730 hours.
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
 * Operator view of one canonical alpha waitlist record.
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
 * Account-level limits for resource-bearing root sessions and root-session creation rate. Open, asynchronously ending, failed, and deleting roots consume the concurrent limit until a strong ended projection or physical deletion proves resource release; durable child sessions are bounded by the root's sealed child policy.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "AccountLimits".
 */
export interface AccountLimits {
  /**
   * Maximum resource-bearing root sessions in open, ending, failed, or deleting lifecycle. Child sessions do not consume or bypass this account limit.
   */
  max_concurrent_sessions: number;
  /**
   * Maximum root-session creates per rolling hour.
   */
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
   * Whole cents. Alpha top-ups are $10.00 to $1,000.00.
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
 * via the `definition` "CreateCreditGrantRequest".
 */
export interface CreateCreditGrantRequest {
  email: string;
  /**
   * Whole cents of operator-issued service credit.
   */
  amount_cents: number;
  /**
   * Operator audit reason for this grant.
   */
  reason: string;
}
/**
 * An operator-issued service-credit grant. It is not backed by a payment and is not refundable as a top-up. Retrying the same Idempotency-Key returns the same grant.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "CreditGrant".
 */
export interface CreditGrant {
  id: CreditGrantId;
  object: "credit_grant";
  account_id: AccountId;
  email: string;
  amount_cents: number;
  reason: string;
  created_at: Timestamp;
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
 * The hosted model gateway is billed at the exact cost reported by the upstream AI Gateway receipt, with no Aex markup.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "RateCard".
 */
export interface RateCard {
  object: "rate_card";
  model_gateway: "pass_through";
}
/**
 * One session's model usage, folded exactly once from Brain's ordered model-result events and their AI Gateway receipts.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "SessionUsage".
 */
export interface SessionUsage {
  session_id: string;
  /**
   * The latest session state observed by the Aex control plane.
   */
  state: string;
  model_calls: number;
  input_tokens: UnsignedDecimalInteger;
  output_tokens: UnsignedDecimalInteger;
  model_microusd: MicroUsd;
  total_microusd: MicroUsd;
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
