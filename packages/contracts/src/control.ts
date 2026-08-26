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
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "AccountDeletionId".
 */
export type AccountDeletionId = string;
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
 * What the operator asserts about the prepaid balance the account still holds. `settled` refuses the deletion while the balance is a whole cent or more from zero in either direction, so an erasure request returns unused credit through a refund first; money never moves as a side effect of deleting an account. `written_off` deletes whatever the balance is and records it as a write-off on the ledger — the abuse-response answer, and the only one that can absorb unpaid usage.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "AccountBalanceDisposition".
 */
export type AccountBalanceDisposition = "settled" | "written_off";
/**
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "AccountDeletionStatus".
 */
export type AccountDeletionStatus = "pending" | "succeeded";
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
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "CreateAccountDeletionRequest".
 */
export interface CreateAccountDeletionRequest {
  email: string;
  /**
   * Operator audit reason for this deletion.
   */
  reason: string;
  balance_disposition: AccountBalanceDisposition;
}
/**
 * An operator-initiated, irreversible account deletion. Accepting it destroys the account token and every API key at once, and hands each of the account's sessions to the ordinary ensured session-deletion path; the account cannot authenticate or create anything from that moment. It stays `pending` until Brain confirms every one of those sessions physically deleted, and only then is the email erased, the outstanding invitation dropped, uploaded Tool artifacts purged and the remaining balance closed out. Top-ups, refunds, credit grants and rated session lines are retained under `account_id`, which survives as a pseudonym: they are the billing record, not personal data. Retrying the same Idempotency-Key returns the same deletion.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "AccountDeletion".
 */
export interface AccountDeletion {
  id: AccountDeletionId;
  object: "account_deletion";
  account_id: AccountId;
  status: AccountDeletionStatus;
  balance_disposition: AccountBalanceDisposition;
  reason: string;
  /**
   * Sessions of this account that are not yet confirmed physically deleted. It reaches zero exactly when the deletion succeeds.
   */
  sessions_pending: number;
  requested_at: Timestamp;
  updated_at: Timestamp;
  completed_at?: Timestamp;
  /**
   * Canonical signed decimal-string integer micro-USD; 1 USD = 1,000,000. Parse with arbitrary-precision integer arithmetic such as JavaScript BigInt; never Number or floating point.
   */
  closing_balance_microusd?: string;
}
/**
 * The public usage rate card. Hosted alpha compute is billed per second on its only physical shape: 0.5 vCPU plus 1 GiB, or $0.12/hour at these component rates. Transient provider burst or peak capacity is not separately metered and is not a promised entitlement. Idle and provider snapshot-storage costs are absorbed in alpha. Session storage covers explicit durable objects and bytes reserved by an outstanding direct upload. Storage GB is decimal (1e9 bytes); a month is `month_hours` hours.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "RateCard".
 */
export interface RateCard {
  object: "rate_card";
  vcpu_hour_microusd: MicroUsd;
  gb_hour_microusd: MicroUsd;
  session_storage_gb_month_microusd: MicroUsd;
  web_search_query_microusd: MicroUsd;
  month_hours: number;
}
/**
 * Current published bytes and outstanding upload-reserved capacity, as last reported by Brain (session/v1 StorageInfo). They remain separate so quota and billing are observable even before an upload is published.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "StorageMeters".
 */
export interface StorageMeters {
  session_storage_bytes: number;
  upload_reserved_bytes: number;
}
/**
 * One session's rated line. Compute time is the sum of turn intervals (turn.started to turn.completed/failed) folded from the session event log. Session-storage integrals are reconstructed from durable storage.usage gauge transitions with exact internal byte-millisecond carry; the public byte-millisecond projection adds the current derived open interval through metered_to so it reproduces the charge. Successful web_search tool results are counted from the same journal.
 *
 * This interface was referenced by `AexControlAPIV1Types`'s JSON-Schema
 * via the `definition` "SessionUsage".
 */
export interface SessionUsage {
  session_id: string;
  /**
   * The hosted alpha's only physical shape: 0.5 vCPU and 1 GiB.
   */
  shape: "1gb";
  /**
   * Lifecycle SessionState from session/v1 (open | ending | ended | deleting | deleted | failed); current-turn activity is a separate session projection.
   */
  state: string;
  /**
   * Cumulative running milliseconds as an exact canonical unsigned decimal string.
   */
  running_ms: string;
  /**
   * Published session-storage plus outstanding upload-reservation byte-milliseconds through metered_to as an exact canonical unsigned decimal string. It is the durable closed integral plus the derived open interval used for this response's charge; a delayed durable transition may replace that estimate upward or downward. Storage micro-USD is floor(value * session_storage_gb_month_microusd / (1000000000 * month_hours * 3600000)).
   */
  session_storage_byte_milliseconds: string;
  /**
   * Successful search results counted from the bounded per-session journal; the hosted 128 MiB journal ceiling makes this counter JavaScript-safe.
   */
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
