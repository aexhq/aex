//! GENERATED — DO NOT EDIT.
//!
//! The public request, response and query models.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:e7f95cd7fc830a0871b1276cde9ae11e4245d0b843cd3dba567e133e551e051f`.
//! Regenerate with `cargo run -p aex-contract-gen -- build`.

#![allow(clippy::large_enum_variant, reason = "a wire union is never boxed")]
#![allow(clippy::match_same_arms, reason = "one arm per row")]
#![allow(clippy::too_many_lines, reason = "one arm per row")]

use crate::canonical::CanonicalJson;
use crate::cursor::Cursor;
use crate::error::ObservedErrorCode;
use crate::ids::AccountId;
use crate::ids::ApiKeyId;
use crate::ids::BillingTransactionId;
use crate::ids::ContentHash;
use crate::ids::FilePath;
use crate::ids::GenerationId;
use crate::ids::MeasurementId;
use crate::ids::MessageId;
use crate::ids::OperationId;
use crate::ids::PaymentMethodId;
use crate::ids::ResourceName;
use crate::ids::SessionId;
use crate::ids::SpanId;
use crate::ids::ToolCallId;
use crate::ids::TraceId;
use crate::ids::UploadId;
use crate::ids::UserId;
use crate::ids::WorkspaceId;
use crate::limits::LimitId;
use crate::scopes::ScopeId;
use crate::types::ByteRange;
use crate::types::Cents;
use crate::types::ComputeSize;
use crate::types::DecimalU128;
use crate::types::ETag;
use crate::types::HttpsUrl;
use crate::types::JsonPointer;
use crate::types::MetadataValue;
use crate::types::Region;
use crate::types::Timestamp;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

// --- central -------------------------------------------------------

/// An account in good standing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AccountActiveState {
    /// When the state last changed.
    pub changed_at: Timestamp,
    /// Monotonic state revision.
    pub revision: u64,
}

/// Whether the account may consume paid capacity.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AccountOperationalState {
    /// Active.
    Active(AccountActiveState),
    /// Paused.
    Paused(AccountPausedState),
}

/// Why the personal prepaid account cannot admit paid work.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountPauseReason {
    /// The prepaid balance is exhausted; a top-up restores service.
    TopUpRequired,
    /// A payment is unresolved; another payment does not automatically clear it.
    PaymentHold,
    /// A chargeback is open and requires reconciliation.
    DisputeHold,
    /// The account is closed.
    AccountClosed,
}

impl AccountPauseReason {
    /// Every value, in declared order.
    pub const ALL: &'static [AccountPauseReason] = &[
        AccountPauseReason::TopUpRequired,
        AccountPauseReason::PaymentHold,
        AccountPauseReason::DisputeHold,
        AccountPauseReason::AccountClosed,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TopUpRequired => "top_up_required",
            Self::PaymentHold => "payment_hold",
            Self::DisputeHold => "dispute_hold",
            Self::AccountClosed => "account_closed",
        }
    }
}

/// A paused account and its customer-visible remedy.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AccountPausedState {
    /// When the state last changed.
    pub changed_at: Timestamp,
    /// Smallest restoring top-up, only for `top_up_required`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_restore_cents: Option<Cents>,
    /// Why it is paused.
    pub reason: AccountPauseReason,
    /// Monotonic state revision.
    pub revision: u64,
}

/// Workspace API key metadata. Secret material is never returned by reads.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiKey {
    /// When it was minted.
    pub created_at: Timestamp,
    /// Identity.
    pub id: ApiKeyId,
    /// Display name.
    pub name: String,
    /// When it was revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,
    /// The scopes it carries.
    pub scopes: Vec<ScopeId>,
    /// The fixed workspace it authorizes.
    pub workspace_id: WorkspaceId,
}

/// Mint a key for the account's fixed workspace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiKeyCreateRequest {
    /// Display name.
    pub name: String,
    /// Launch scopes to grant.
    pub scopes: Vec<ScopeId>,
    /// Must name the fixed workspace.
    pub workspace_id: WorkspaceId,
}

/// One bounded page of API key metadata.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiKeyPage {
    /// The page.
    pub items: Vec<ApiKey>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// The current prepaid position of the caller's personal account.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingBalance {
    /// The personal account.
    pub account_id: AccountId,
    /// Credit available for new reservations.
    pub available_cents: Cents,
    /// Always USD at launch.
    pub currency: Currency,
    /// Rated usage not yet settled to the ledger.
    pub pending_cents: Cents,
    /// Credit held for admitted sessions.
    pub reserved_cents: Cents,
    /// When this projection last changed.
    pub updated_at: Timestamp,
}

/// One immutable customer-visible ledger transaction.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingTransaction {
    /// Absolute amount in whole cents.
    pub amount_cents: Cents,
    /// Available balance after this transaction.
    pub balance_after_cents: Cents,
    /// Always USD at launch.
    pub currency: Currency,
    /// Its effect on prepaid value.
    pub direction: BillingTransactionDirection,
    /// Transaction identity.
    pub id: BillingTransactionId,
    /// Why it was posted.
    pub kind: BillingTransactionKind,
    /// Authoritative ledger time.
    pub occurred_at: Timestamp,
    /// The compensated transaction, for reversals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverses_transaction_id: Option<BillingTransactionId>,
    /// Session attribution when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
}

/// Whether the transaction increases or decreases available prepaid value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingTransactionDirection {
    /// Increases prepaid value.
    Credit,
    /// Decreases prepaid value.
    Debit,
}

impl BillingTransactionDirection {
    /// Every value, in declared order.
    pub const ALL: &'static [BillingTransactionDirection] = &[
        BillingTransactionDirection::Credit,
        BillingTransactionDirection::Debit,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Credit => "credit",
            Self::Debit => "debit",
        }
    }
}

/// The customer-visible reason for an immutable balanced ledger transaction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingTransactionKind {
    /// Verified prepaid credit purchase.
    TopUp,
    /// Settled trusted usage.
    Usage,
    /// Credit returned by a verified refund.
    Refund,
    /// A compensating entry for a prior transaction.
    Reversal,
    /// Credit reserved for admitted work.
    Reservation,
    /// Unused reservation returned.
    ReservationRelease,
}

impl BillingTransactionKind {
    /// Every value, in declared order.
    pub const ALL: &'static [BillingTransactionKind] = &[
        BillingTransactionKind::TopUp,
        BillingTransactionKind::Usage,
        BillingTransactionKind::Refund,
        BillingTransactionKind::Reversal,
        BillingTransactionKind::Reservation,
        BillingTransactionKind::ReservationRelease,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TopUp => "top_up",
            Self::Usage => "usage",
            Self::Refund => "refund",
            Self::Reversal => "reversal",
            Self::Reservation => "reservation",
            Self::ReservationRelease => "reservation_release",
        }
    }
}

/// One newest-first page of immutable ledger transactions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingTransactionPage {
    /// The page.
    pub items: Vec<BillingTransaction>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// The independently measured and rated launch categories.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingUsageCategory {
    /// Provider model input/output/cache tokens.
    Model,
    /// Trusted sandbox compute and memory.
    Runtime,
    /// Retained workspace/session bytes over time.
    Storage,
    /// Measured outbound bytes.
    Transfer,
}

impl BillingUsageCategory {
    /// Every value, in declared order.
    pub const ALL: &'static [BillingUsageCategory] = &[
        BillingUsageCategory::Model,
        BillingUsageCategory::Runtime,
        BillingUsageCategory::Storage,
        BillingUsageCategory::Transfer,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Runtime => "runtime",
            Self::Storage => "storage",
            Self::Transfer => "transfer",
        }
    }
}

/// The trustworthy completeness boundary for one usage answer.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingUsageCoverage {
    /// All accepted facts at or before this instant are represented.
    pub complete_through: Timestamp,
    /// Whether a poisoned or missing fact prevents complete settlement.
    pub has_gap: bool,
    /// No fact after this instant is represented.
    pub includes_through: Timestamp,
}

/// One bounded rated-usage aggregate with exact charge evidence.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingUsageItem {
    /// Exact rated amount in whole cents.
    pub amount_cents: Cents,
    /// What was measured.
    pub category: BillingUsageCategory,
    /// Quantity actually charged after policy.
    pub charged_quantity: DecimalU128,
    /// Always USD at launch.
    pub currency: Currency,
    /// Provider model identifier for model usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Official provider for model usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderId>,
    /// Measured quantity before rating.
    pub quantity: DecimalU128,
    /// Half-open service-time range.
    pub service_time: TimeRange,
    /// Session attribution when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Ledger coverage.
    pub settlement: BillingUsageSettlement,
    /// Stable unit such as `input_tokens` or `byte_minutes`.
    pub unit: String,
}

/// One bounded page of rated usage and its explicit completeness boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingUsagePage {
    /// How complete this answer is.
    pub coverage: BillingUsageCoverage,
    /// The page.
    pub items: Vec<BillingUsageItem>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Whether this rated row has been posted to the money ledger.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingUsageSettlement {
    /// Trusted and rated but not yet posted.
    Pending,
    /// Covered by an immutable ledger transaction.
    Settled,
}

impl BillingUsageSettlement {
    /// Every value, in declared order.
    pub const ALL: &'static [BillingUsageSettlement] = &[
        BillingUsageSettlement::Pending,
        BillingUsageSettlement::Settled,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Settled => "settled",
        }
    }
}

/// Customer-safe card-network display metadata reported by Stripe.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardBrand {
    /// Visa.
    Visa,
    /// Mastercard.
    Mastercard,
    /// American Express.
    Amex,
    /// Discover.
    Discover,
    /// Diners Club.
    Diners,
    /// JCB.
    Jcb,
    /// `UnionPay`.
    Unionpay,
    /// A network Stripe did not map to this display vocabulary.
    Unknown,
}

impl CardBrand {
    /// Every value, in declared order.
    pub const ALL: &'static [CardBrand] = &[
        CardBrand::Visa,
        CardBrand::Mastercard,
        CardBrand::Amex,
        CardBrand::Discover,
        CardBrand::Diners,
        CardBrand::Jcb,
        CardBrand::Unionpay,
        CardBrand::Unknown,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Visa => "visa",
            Self::Mastercard => "mastercard",
            Self::Amex => "amex",
            Self::Discover => "discover",
            Self::Diners => "diners",
            Self::Jcb => "jcb",
            Self::Unionpay => "unionpay",
            Self::Unknown => "unknown",
        }
    }
}

/// The only launch currency.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    /// United States dollar.
    #[serde(rename = "USD")]
    USD,
}

impl Currency {
    /// Every value, in declared order.
    pub const ALL: &'static [Currency] = &[Currency::USD];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::USD => "USD",
        }
    }
}

/// The single bounded read that fills the launch dashboard shell.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DashboardBootstrap {
    /// Their personal prepaid account.
    pub account_id: AccountId,
    /// Current paid-capacity state.
    pub account_state: AccountOperationalState,
    /// Their verified GitHub email.
    pub email: String,
    /// When this snapshot was assembled.
    pub generated_at: Timestamp,
    /// The signed-in person.
    pub user_id: UserId,
    /// The fixed eu-west-1 workspace.
    pub workspace: Workspace,
}

/// A rotated browser session returned exactly once.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DashboardSessionCredential {
    /// When it stops verifying.
    pub expires_at: Timestamp,
    /// Bearer credential for an `HttpOnly` cookie.
    pub session: String,
    /// The GitHub-linked person.
    pub user_id: UserId,
}

/// Complete a GitHub OAuth browser sign-in using PKCE-bound state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DashboardSessionRequest {
    /// Single-use GitHub authorization code.
    pub code: String,
    /// The browser-held PKCE verifier.
    pub code_verifier: String,
    /// The echoed S256 challenge.
    pub state: String,
}

/// A freshly minted key; its value is returned once and never stored.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NewApiKey {
    /// When it was minted.
    pub created_at: Timestamp,
    /// Identity.
    pub id: ApiKeyId,
    /// Display name.
    pub name: String,
    /// The scopes it carries.
    pub scopes: Vec<ScopeId>,
    /// The one-time secret value.
    pub value: String,
    /// The fixed workspace.
    pub workspace_id: WorkspaceId,
}

/// Display-only metadata for one Stripe card; never PAN, CVC, or a client secret.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PaymentMethod {
    /// Card network.
    pub brand: CardBrand,
    /// When verified webhook state registered it.
    pub created_at: Timestamp,
    /// Expiry month.
    pub expiry_month: u32,
    /// Four-digit expiry year.
    pub expiry_year: u32,
    /// Aex's opaque card reference.
    pub id: PaymentMethodId,
    /// Whether Stripe reports this as the default card.
    pub is_default: bool,
    /// Last four digits for display.
    pub last4: String,
}

/// The bounded set of cards attached to the caller's account.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PaymentMethodPage {
    /// Cards, newest first.
    pub items: Vec<PaymentMethod>,
}

/// Request a Stripe-hosted card setup session.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PaymentMethodSessionRequest {
    /// Browser return after cancellation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_url: Option<HttpsUrl>,
    /// Must be true: the caller explicitly consents to saving this card for future prepaid
    /// payments.
    pub consent: bool,
    /// Browser return after successful setup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_url: Option<HttpsUrl>,
}

/// Request a one-time Stripe-hosted prepaid top-up.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TopUpCheckoutRequest {
    /// Credit to buy in whole cents, within configured bounds.
    pub amount_cents: Cents,
    /// Browser return after cancellation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_url: Option<HttpsUrl>,
    /// Browser return after provider success; not money authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_url: Option<HttpsUrl>,
}

/// The account's fixed eu-west-1 execution and content boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Workspace {
    /// Regional session endpoint.
    pub api_url: HttpsUrl,
    /// When first login provisioned it.
    pub created_at: Timestamp,
    /// Identity.
    pub id: WorkspaceId,
    /// Display name.
    pub name: String,
    /// Inherited account state.
    pub operational_state: WorkspaceOperationalState,
    /// Immutable placement; eu-west-1 at launch.
    pub region: Region,
}

/// The fixed workspace's inherited personal-account state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceOperationalState {
    /// The personal account.
    pub account_id: AccountId,
    /// The inherited state.
    pub state: AccountOperationalState,
}

// --- common -------------------------------------------------------

/// The one error envelope every failing request returns.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiError {
    /// The failure.
    pub error: ApiErrorBody,
}

/// The failure itself.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiErrorBody {
    /// The closed v1 error code.
    pub code: ObservedErrorCode,
    /// Typed remediation detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ErrorDetails>,
    /// A human message; never a provider body.
    pub message: String,
    /// The durable operation this failure belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    /// The diagnostic request identifier.
    pub request_id: String,
    /// Whether an identical retry can succeed.
    pub retryable: bool,
}

/// A short-lived signed grant for exactly one object and range.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DownloadGrant {
    /// Bytes this grant authorizes.
    pub authorized_bytes: DecimalU128,
    /// When the signature stops verifying.
    pub expires_at: Timestamp,
    /// The metered download measurement.
    pub measurement_id: MeasurementId,
    /// The signed range, when the grant is partial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<ByteRange>,
    /// The immutable whole-object hash.
    pub sha256: ContentHash,
    /// The whole object size.
    pub size_bytes: DecimalU128,
    /// The signed URL; never printed by the CLI.
    pub url: HttpsUrl,
}

/// A body that carries no fields; still strict, so an extra member is rejected.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EmptyRequest {}

/// Closed, typed remediation detail; never an untyped bag.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ErrorDetails {
    /// The scope the credential lacks.
    RequiredScope(ErrorDetailsRequiredScope),
    /// The effective limit that was exceeded.
    Limit(ErrorDetailsLimit),
    /// The quota dimension that was exhausted.
    Quota(ErrorDetailsQuota),
    /// The authoritative region for the workspace.
    Region(ErrorDetailsRegion),
    /// How long to wait before retrying.
    Retry(ErrorDetailsRetry),
    /// The original operation that already exists.
    Operation(ErrorDetailsOperation),
    /// Exactly where the body failed validation.
    Validation(ErrorDetailsValidation),
}

/// The effective limit that was exceeded.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ErrorDetailsLimit {
    /// The effective value at the time of the request.
    pub effective: DecimalU128,
    /// Which limit.
    pub limit: LimitId,
    /// What the request would have required.
    pub measured: DecimalU128,
}

/// The original operation the replay resolved to.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ErrorDetailsOperation {
    /// The original operation.
    pub operation_id: OperationId,
}

/// The quota dimension that was exhausted.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ErrorDetailsQuota {
    /// What this request would have added.
    pub attempted: DecimalU128,
    /// Which quota dimension.
    pub dimension: LimitId,
    /// Already consumed.
    pub used: DecimalU128,
}

/// The authoritative placement of the workspace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ErrorDetailsRegion {
    /// The regional host to reissue against.
    pub api_url: HttpsUrl,
    /// Where the workspace actually lives.
    pub expected: Region,
}

/// The scope the credential lacks.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ErrorDetailsRequiredScope {
    /// The missing scope.
    pub scope: ScopeId,
}

/// How long to wait before retrying.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ErrorDetailsRetry {
    /// Milliseconds to wait.
    pub retry_after_ms: u32,
}

/// Exactly where the body failed validation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ErrorDetailsValidation {
    /// RFC 6901 pointer to the offending position.
    pub pointer: JsonPointer,
    /// Why that position was rejected.
    pub reason: String,
}

/// A short-lived hosted provider page.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HostedSession {
    /// When the page stops working.
    pub expires_at: Timestamp,
    /// The hosted page.
    pub url: HttpsUrl,
}

/// One request header a signed grant requires the caller to send verbatim.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HttpHeader {
    /// The header name.
    pub name: String,
    /// The exact value to send.
    pub value: String,
}

/// A half-open instant range, `gte` inclusive and `lt` exclusive.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TimeRange {
    /// Inclusive lower bound.
    pub gte: Timestamp,
    /// Exclusive upper bound.
    pub lt: Timestamp,
}

/// Which retained session generation answered a live workspace call.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceAccess {
    /// The exact retained generation that answered.
    pub generation_id: GenerationId,
    /// Whether this call first resumed the suspended generation.
    pub resumed: bool,
}

// --- provider -------------------------------------------------------

/// An explicit provider and model pair; a model-only override is rejected.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ModelSelection {
    /// The exact provider-native model id.
    pub model: String,
    /// The provider.
    pub provider: ProviderId,
}

/// The candidate official BYOK authorities. Only models admitted by the current Rig-backed catalog
/// are selectable; arbitrary base URLs are not accepted.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// The `OpenAI` API.
    Openai,
    /// Anthropic.
    Anthropic,
    /// The `DeepSeek` API.
    Deepseek,
    /// The official xAI API.
    Xai,
    /// The official Meta API.
    Meta,
    /// The Moonshot AI API.
    Moonshotai,
    /// The international Alibaba Model Studio API.
    Alibaba,
}

impl ProviderId {
    /// Every value, in declared order.
    pub const ALL: &'static [ProviderId] = &[
        ProviderId::Openai,
        ProviderId::Anthropic,
        ProviderId::Deepseek,
        ProviderId::Xai,
        ProviderId::Meta,
        ProviderId::Moonshotai,
        ProviderId::Alibaba,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Deepseek => "deepseek",
            Self::Xai => "xai",
            Self::Meta => "meta",
            Self::Moonshotai => "moonshotai",
            Self::Alibaba => "alibaba",
        }
    }
}

// --- regional -------------------------------------------------------

/// How inline blob bytes are encoded.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobEncoding {
    /// The data member is the literal text.
    Utf8,
    /// The data member is standard base64.
    Base64,
}

impl BlobEncoding {
    /// Every value, in declared order.
    pub const ALL: &'static [BlobEncoding] = &[BlobEncoding::Utf8, BlobEncoding::Base64];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Utf8 => "utf8",
            Self::Base64 => "base64",
        }
    }
}

/// A small value carried in the request.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BlobInline {
    /// The encoded bytes. A larger payload goes through the upload surface.
    pub data: String,
    /// How the data is encoded.
    pub encoding: BlobEncoding,
    /// Hash of the decoded bytes.
    pub sha256: ContentHash,
}

/// Where a registered value's bytes come from.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlobInput {
    /// Carried in the request.
    Inline(BlobInline),
    /// Staged through the upload surface.
    Upload(BlobUpload),
    /// Fetched asynchronously; admission immediately replaces the current record with pending.
    Url(BlobUrl),
}

/// A large value staged through the upload surface.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BlobUpload {
    /// Hash of the staged bytes.
    pub sha256: ContentHash,
    /// Size of the staged bytes.
    pub size_bytes: DecimalU128,
    /// The ready staged upload.
    pub upload_id: UploadId,
}

/// An HTTPS source fetched asynchronously under the platform SSRF, redirect, time and size policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BlobUrl {
    /// The exact initial HTTPS URL.
    pub url: HttpsUrl,
}

/// One resolved sandbox compute dimension.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ComputeShape {
    /// Memory in mebibytes.
    pub memory_mi_b: u32,
    /// Virtual CPUs.
    pub vcpus: f64,
}

/// A reference to a stored payload. The bytes are never published in a registry response; they are
/// reached through a download grant.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ContentRef {
    /// Hash of the stored payload bytes.
    pub sha256: ContentHash,
    /// Size of the stored payload in bytes.
    pub size_bytes: DecimalU128,
}

/// Minimal deletion marker returned while content cleanup runs.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeletingSession {
    /// Session.
    pub id: SessionId,
    /// Diagnostic deletion identity.
    pub operation_id: OperationId,
    /// When deletion began.
    pub started_at: Timestamp,
}

/// One effective workspace safety limit. There is no public mutation route.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectiveWorkspaceLimit {
    /// When the value last changed.
    pub changed_at: Timestamp,
    /// The value in force.
    pub effective_value: LimitValue,
    /// Which limit.
    pub id: LimitId,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Where the value came from.
    pub source: LimitSource,
}

/// One page of effective workspace limits.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectiveWorkspaceLimitPage {
    /// The page.
    pub items: Vec<EffectiveWorkspaceLimit>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// The sandbox network policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HandsNetworkRequest {
    /// What the guest may reach.
    pub mode: NetworkMode,
}

/// A limit whose effective value is a map of named dimensions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LimitMapValue {
    /// The effective value per dimension.
    pub values: BTreeMap<String, DecimalU128>,
}

/// A single non-negative integer limit.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LimitScalarValue {
    /// The effective value.
    pub value: DecimalU128,
}

/// Where an effective limit value came from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitSource {
    /// The platform default.
    Default,
    /// An audited workspace override.
    WorkspaceOverride,
}

impl LimitSource {
    /// Every value, in declared order.
    pub const ALL: &'static [LimitSource] = &[LimitSource::Default, LimitSource::WorkspaceOverride];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::WorkspaceOverride => "workspace_override",
        }
    }
}

/// The effective value of a limit, discriminated by shape.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum LimitValue {
    /// One integer.
    Scalar(LimitScalarValue),
    /// A map of named dimensions.
    Map(LimitMapValue),
}

/// One frozen MCP definition and its bounded tool namespace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct McpServer {
    /// Unique server/tool namespace.
    pub name: ResourceName,
    /// How the built-in sandbox MCP tool reaches it.
    pub transport: McpTransport,
}

/// The two admitted MCP transports.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransport {
    /// Remote Streamable HTTP.
    RemoteHttp(RemoteMcpServer),
    /// Process inside the one sandbox.
    SandboxProcess(SandboxMcpServer),
}

/// One complete sealed message in immutable visibility order.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Message {
    /// Parts.
    pub content: Vec<MessagePart>,
    /// Display time; not collection ordering authority.
    pub created_at: Timestamp,
    /// Identity.
    pub id: MessageId,
    /// Producer.
    pub role: MessageRole,
    /// Session.
    pub session_id: SessionId,
}

/// One page of complete messages.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessagePage {
    /// The page.
    pub items: Vec<Message>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// One part of a complete committed message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePart {
    /// Text.
    Text(MessagePartText),
    /// Tool call.
    ToolCall(MessagePartToolCall),
    /// Tool result.
    ToolResult(MessagePartToolResult),
}

/// Text content.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessagePartText {
    /// Text.
    pub text: String,
}

/// Canonical provider-independent tool call.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessagePartToolCall {
    /// Validated arguments.
    pub arguments: CanonicalJson,
    /// Call identity.
    pub id: ToolCallId,
    /// Official or namespaced MCP tool name.
    pub name: String,
}

/// Bounded tool result; large/full bytes remain at its sandbox path unless storage.persist
/// explicitly registers them as a workspace file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessagePartToolResult {
    /// Call identity.
    pub id: ToolCallId,
    /// Bounded live/model preview.
    pub preview: String,
    /// Stable call-scoped full-result path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_path: Option<FilePath>,
    /// Full-result digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<ContentHash>,
    /// Full-result length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<DecimalU128>,
    /// Whether full output exceeded the preview.
    pub truncated: bool,
}

/// Who produced a committed message.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// Customer.
    User,
    /// Model.
    Assistant,
    /// Canonical tool result.
    Tool,
}

impl MessageRole {
    /// Every value, in declared order.
    pub const ALL: &'static [MessageRole] =
        &[MessageRole::User, MessageRole::Assistant, MessageRole::Tool];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// Text-only user message; arbitrary files are referenced by mounted paths in this text.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageSendRequest {
    /// Earlier absolute deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<Timestamp>,
    /// Earlier per-message spend fence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_spend_cents: Option<Cents>,
    /// Native structured output request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// Complete user text; there is no attachment field.
    pub text: String,
}

/// The admitted user message and session now processing it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageSendResult {
    /// Admitted message.
    pub message: Message,
    /// Updated session.
    pub session: Session,
}

/// One bounded NDJSON assistant-stream frame.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageStreamFrame {
    /// Frame semantics.
    pub kind: MessageStreamKind,
    /// Complete committed/reconcile message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// Message correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<MessageId>,
    /// Monotonic connection-independent sequence.
    pub sequence: DecimalU128,
    /// Bounded preview text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Preview/commit reconciliation protocol.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStreamKind {
    /// Uncommitted assistant delta; may be lost.
    Preview,
    /// A complete message became durable.
    Committed,
    /// Replace preview state from committed authority.
    Reconcile,
    /// One or more live frames were dropped; replay committed messages.
    Gap,
    /// Keeps the bounded connection alive.
    Heartbeat,
}

impl MessageStreamKind {
    /// Every value, in declared order.
    pub const ALL: &'static [MessageStreamKind] = &[
        MessageStreamKind::Preview,
        MessageStreamKind::Committed,
        MessageStreamKind::Reconcile,
        MessageStreamKind::Gap,
        MessageStreamKind::Heartbeat,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Committed => "committed",
            Self::Reconcile => "reconcile",
            Self::Gap => "gap",
            Self::Heartbeat => "heartbeat",
        }
    }
}

/// What the sandbox may reach.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// No outbound network.
    None,
    /// Managed outbound Internet egress.
    PublicInternet,
}

impl NetworkMode {
    /// Every value, in declared order.
    pub const ALL: &'static [NetworkMode] = &[NetworkMode::None, NetworkMode::PublicInternet];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::PublicInternet => "public_internet",
        }
    }
}

/// Supported setup package ecosystems.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageEcosystem {
    /// Debian packages.
    Apt,
    /// Python packages.
    Pip,
    /// Node packages.
    Npm,
}

impl PackageEcosystem {
    /// Every value, in declared order.
    pub const ALL: &'static [PackageEcosystem] = &[
        PackageEcosystem::Apt,
        PackageEcosystem::Pip,
        PackageEcosystem::Npm,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Apt => "apt",
            Self::Pip => "pip",
            Self::Npm => "npm",
        }
    }
}

/// One exactly pinned setup package.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PackageRequest {
    /// Which package manager.
    pub ecosystem: PackageEcosystem,
    /// Package name.
    pub name: String,
    /// Exact version; ranges are rejected.
    pub version: String,
}

/// A registered file entry, complete.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredFile {
    /// When first written.
    pub created_at: Timestamp,
    /// Stable bounded failure code; present only when failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    /// The public identity.
    pub name: ResourceName,
    /// Lifecycle of this one current overwrite.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
    /// Verified current bytes; present only when ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<RegisteredFileRead>,
}

/// The two permitted registered-file modes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisteredFileMode {
    /// Read-write for the owner, read for others.
    #[serde(rename = "0644")]
    V0644,
    /// Executable.
    #[serde(rename = "0755")]
    V0755,
}

impl RegisteredFileMode {
    /// Every value, in declared order.
    pub const ALL: &'static [RegisteredFileMode] =
        &[RegisteredFileMode::V0644, RegisteredFileMode::V0755];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V0644 => "0644",
            Self::V0755 => "0755",
        }
    }
}

/// One page of registered files.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredFilePage {
    /// The page.
    pub items: Vec<RegisteredFileRow>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// What a registered file is, as published. The payload bytes are referenced, never inlined; fetch
/// them with a download grant.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredFileRead {
    /// The stored bytes.
    pub content: ContentRef,
    /// Declared media type.
    pub media_type: String,
    /// POSIX mode.
    pub mode: RegisteredFileMode,
}

/// One registered file as a collection row. A listing carries no value document; read the entry
/// itself to get one.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredFileRow {
    /// When first written.
    pub created_at: Timestamp,
    /// Stable bounded failure code; present only when failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    /// The public identity.
    pub name: ResourceName,
    /// Lifecycle of this one current overwrite.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
}

/// A latest-only registered file replacement.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredFileValue {
    /// The bytes.
    pub content: BlobInput,
    /// Declared media type.
    pub media_type: String,
    /// POSIX mode.
    pub mode: RegisteredFileMode,
}

/// State of the single current value; prior values are never addressable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisteredState {
    /// The latest URL/import or upload overwrite was admitted but has not published bytes.
    Pending,
    /// The latest overwrite has verified downloadable bytes.
    Ready,
    /// The latest asynchronous overwrite failed and did not fall back to prior bytes.
    Failed,
}

impl RegisteredState {
    /// Every value, in declared order.
    pub const ALL: &'static [RegisteredState] = &[
        RegisteredState::Pending,
        RegisteredState::Ready,
        RegisteredState::Failed,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

/// Mint a download grant for a registered file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegistryDownloadRequest {
    /// An explicit bounded range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<ByteRange>,
}

/// A remote Streamable HTTP MCP server frozen at session setup and invoked on demand inside the
/// sandbox.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RemoteMcpServer {
    /// Write-only session-encrypted request headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    /// Official server endpoint.
    pub url: HttpsUrl,
}

/// Complete provider-derived capacity of the frozen shape.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolvedCompute {
    /// Guaranteed capacity.
    pub baseline: ComputeShape,
    /// Endpoint bandwidth.
    pub endpoint_bandwidth_m_bps: u32,
    /// Connection ceiling.
    pub max_concurrent_connections: u32,
    /// Maximum guest disk.
    pub max_disk_gi_b: u32,
    /// Burst ceiling.
    pub peak: ComputeShape,
    /// Selected token.
    pub size: ComputeSize,
}

/// Customer-safe frozen session configuration; all plaintext secrets are omitted.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolvedConfig {
    /// Present only when sandbox is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute: Option<ResolvedCompute>,
    /// Immutable session bounds.
    pub lifecycle: SessionLifecyclePolicy,
    /// Frozen server names; secrets and transport credentials are omitted.
    pub mcp_server_names: Vec<ResourceName>,
    /// Current model selected at admission.
    pub model: String,
    /// Present only when sandbox is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<ResolvedNetwork>,
    /// Resolved setup packages.
    pub packages: Vec<PackageRequest>,
    /// Provider.
    pub provider: ProviderId,
    /// Frozen mount selection; hashes remain private.
    pub registered: SessionRegisteredSelection,
    /// Whether the one sandbox exists.
    pub sandbox_enabled: bool,
}

/// Resolved sandbox network policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolvedNetwork {
    /// Guest policy.
    pub hands: HandsNetworkRequest,
}

/// Optional native structured-output request; prompt emulation is never presented as strict.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResponseFormat {
    /// Format.
    pub kind: ResponseFormatKind,
    /// Required only for `json_schema`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<CanonicalJson>,
}

/// Requested final response format.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormatKind {
    /// Ordinary text.
    Text,
    /// Native provider JSON Schema support is required.
    JsonSchema,
}

impl ResponseFormatKind {
    /// Every value, in declared order.
    pub const ALL: &'static [ResponseFormatKind] =
        &[ResponseFormatKind::Text, ResponseFormatKind::JsonSchema];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::JsonSchema => "json_schema",
        }
    }
}

/// An MCP process launched inside the sandbox after workspace materialization.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SandboxMcpServer {
    /// Bounded arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Executable inside the sandbox.
    pub command: String,
    /// Write-only session-encrypted environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<BTreeMap<String, String>>,
    /// Normalized sandbox working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<FilePath>,
}

/// Observable preparation state of the session's single default-on sandbox.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxStatus {
    /// The caller explicitly opted out; sandbox tools return `sandbox_disabled`.
    Disabled,
    /// Preparation was durably requested.
    Requested,
    /// Ready to execute a waiting tool call.
    Ready,
    /// No waiter exists and initial preparation is suspending.
    Suspending,
    /// Prepared and suspended until the first sandbox tool call.
    Suspended,
    /// A tool waiter is resuming the exact generation.
    Resuming,
    /// The generation was lost; tool calls receive a normal structured error.
    Lost,
}

impl SandboxStatus {
    /// Every value, in declared order.
    pub const ALL: &'static [SandboxStatus] = &[
        SandboxStatus::Disabled,
        SandboxStatus::Requested,
        SandboxStatus::Ready,
        SandboxStatus::Suspending,
        SandboxStatus::Suspended,
        SandboxStatus::Resuming,
        SandboxStatus::Lost,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Requested => "requested",
            Self::Ready => "ready",
            Self::Suspending => "suspending",
            Self::Suspended => "suspended",
            Self::Resuming => "resuming",
            Self::Lost => "lost",
        }
    }
}

/// The only public execution resource; sandbox loss never terminates agent state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Session {
    /// The one active root user message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_message_id: Option<MessageId>,
    /// Admission time.
    pub created_at: Timestamp,
    /// Session expiry.
    pub expires_at: Timestamp,
    /// Identity.
    pub id: SessionId,
    /// Caller labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, MetadataValue>>,
    /// Frozen customer-safe configuration.
    pub resolved_config: ResolvedConfig,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// One-Hand preparation/suspension state.
    pub sandbox_status: SandboxStatus,
    /// Conversation lifecycle.
    pub status: SessionStatus,
    /// When explicit compute termination committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminated_at: Option<Timestamp>,
    /// Last durable change.
    pub updated_at: Timestamp,
    /// Owning fixed workspace.
    pub workspace_id: WorkspaceId,
}

/// Admission receipt for an asynchronous session command; there is no generic operations resource.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionCommandReceipt {
    /// When the command committed.
    pub accepted_at: Timestamp,
    /// Diagnostic and idempotency correlation identity.
    pub operation_id: OperationId,
    /// Target session.
    pub session_id: SessionId,
}

/// The single sandbox's bounded compute selector.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionComputeRequest {
    /// Defaults to the launch baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ComputeSize>,
}

/// Create a durable session with a write-only BYOK key, frozen file/MCP config and a default-on
/// eager sandbox.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionCreateRequest {
    /// Optional earlier expiry within the launch maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// Session prepaid reservation ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_spend_cents: Option<Cents>,
    /// Remote and sandbox MCP definitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<McpServer>>,
    /// Caller labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, MetadataValue>>,
    /// Current admitted provider-native model id.
    pub model: String,
    /// Qualified official provider family.
    pub provider: ProviderId,
    /// Write-only session-scoped key; encrypted and never returned.
    pub provider_api_key: String,
    /// Files to resolve and freeze.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered: Option<SessionRegisteredSelection>,
    /// Defaults enabled and prepares asynchronously.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SessionSandboxRequest>,
}

/// Immutable public session bounds; sandbox suspension is an internal lifecycle.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionLifecyclePolicy {
    /// Session lifetime, never more than eight hours at launch.
    pub maximum_lifetime_seconds: u32,
    /// Root depth is zero; children may reach depth three.
    pub maximum_subagent_depth: u32,
    /// Exactly 12 non-root identities over the session lifetime.
    pub maximum_subagents: u32,
}

/// Bounded collection projection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionListItem {
    /// Active root message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_message_id: Option<MessageId>,
    /// Admission time.
    pub created_at: Timestamp,
    /// Expiry.
    pub expires_at: Timestamp,
    /// Identity.
    pub id: SessionId,
    /// Model.
    pub model: String,
    /// Provider.
    pub provider: ProviderId,
    /// Sandbox lifecycle.
    pub sandbox_status: SandboxStatus,
    /// Conversation lifecycle.
    pub status: SessionStatus,
    /// Last change.
    pub updated_at: Timestamp,
    /// Workspace.
    pub workspace_id: WorkspaceId,
}

/// One page of sessions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionListPage {
    /// The page.
    pub items: Vec<SessionListItem>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Network policy for the one sandbox.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionNetworkRequest {
    /// Guest policy.
    pub hands: HandsNetworkRequest,
}

/// Files resolved once at session admission; later workspace overwrites cannot mutate this
/// manifest.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionRegisteredSelection {
    /// Explicit name-to-path mounts.
    pub mounts: Vec<WorkspaceFileMount>,
}

/// Default-on sandbox configuration; enabled=false creates no Hand.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionSandboxRequest {
    /// Bounded compute shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute: Option<SessionComputeRequest>,
    /// Defaults true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Guest egress policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<SessionNetworkRequest>,
    /// Pinned packages applied before initial suspension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packages: Option<Vec<PackageRequest>>,
}

/// Customer-visible session lifecycle, independent of sandbox suspension.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Ready for another user message.
    Idle,
    /// Processing the one active root message.
    Running,
    /// Destroying sandbox compute.
    Terminating,
    /// Sandbox compute is gone; metadata and messages remain.
    Terminated,
    /// Irreversible session-content deletion is running.
    Deleting,
}

impl SessionStatus {
    /// Every value, in declared order.
    pub const ALL: &'static [SessionStatus] = &[
        SessionStatus::Idle,
        SessionStatus::Running,
        SessionStatus::Terminating,
        SessionStatus::Terminated,
        SessionStatus::Deleting,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Terminating => "terminating",
            Self::Terminated => "terminated",
            Self::Deleting => "deleting",
        }
    }
}

/// Minimal marker after irreversible deletion.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionTombstone {
    /// Commit time.
    pub deleted_at: Timestamp,
    /// Deleted session.
    pub id: SessionId,
}

/// Short-lived signed download for one immutable compressed telemetry export.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryDownloadGrant {
    /// Grant expiry.
    pub expires_at: Timestamp,
    /// Compressed NDJSON or OTLP bundle media type.
    pub media_type: String,
    /// Exact compressed object digest.
    pub sha256: ContentHash,
    /// Compressed size.
    pub size_bytes: DecimalU128,
    /// Signed S3 URL; never printed by the CLI.
    pub url: HttpsUrl,
}

/// Select a bounded half-open telemetry export.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryDownloadRequest {
    /// First included sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_sequence: Option<DecimalU128>,
    /// Exclusive sequence ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_sequence: Option<DecimalU128>,
}

/// One bounded normalized telemetry frame transmitted live and persisted in compressed S3 segments.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryFrame {
    /// Bounded customer-safe structured event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<CanonicalJson>,
    /// Signal family.
    pub kind: TelemetryKind,
    /// Trusted producer time.
    pub occurred_at: Timestamp,
    /// Bounded tool/assistant output preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Monotonic per-session sequence.
    pub sequence: DecimalU128,
    /// Span correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<SpanId>,
    /// Trace correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<TraceId>,
    /// Whether retained data is larger than this frame.
    pub truncated: bool,
}

/// Trusted live and retained session signals.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryKind {
    /// Assistant preview/commit lifecycle.
    Assistant,
    /// Tool waiting, start, preview and completion.
    Tool,
    /// Sandbox prepare/materialize/suspend/resume/loss lifecycle.
    Runtime,
    /// OpenTelemetry Log record.
    Log,
    /// OpenTelemetry Span record.
    Span,
    /// Trusted model/runtime/storage/transfer usage fact.
    Usage,
    /// Bounded live delivery dropped frames; replay from S3.
    Gap,
    /// Stream heartbeat.
    Heartbeat,
}

impl TelemetryKind {
    /// Every value, in declared order.
    pub const ALL: &'static [TelemetryKind] = &[
        TelemetryKind::Assistant,
        TelemetryKind::Tool,
        TelemetryKind::Runtime,
        TelemetryKind::Log,
        TelemetryKind::Span,
        TelemetryKind::Usage,
        TelemetryKind::Gap,
        TelemetryKind::Heartbeat,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::Runtime => "runtime",
            Self::Log => "log",
            Self::Span => "span",
            Self::Usage => "usage",
            Self::Gap => "gap",
            Self::Heartbeat => "heartbeat",
        }
    }
}

/// One staged multipart upload. Uploads are transport plumbing, not assets.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Upload {
    /// The declared media type.
    pub content_type: String,
    /// When staging began.
    pub created_at: Timestamp,
    /// When the orphan grace expires.
    pub expires_at: Timestamp,
    /// Identity.
    pub id: UploadId,
    /// How many parts the object needs.
    pub part_count: u32,
    /// The required part size.
    pub part_size_bytes: DecimalU128,
    /// The declared whole-object hash.
    pub sha256: ContentHash,
    /// The declared object size.
    pub size_bytes: DecimalU128,
    /// Lifecycle position.
    pub state: UploadState,
}

/// One pending current-file overwrite and every bounded part grant needed to upload it; no extra
/// public grant route exists.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadAdmission {
    /// Every part grant, in ascending order.
    pub grants: Vec<UploadPartGrant>,
    /// The admitted upload.
    pub upload: Upload,
}

/// Complete a staged upload.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadCompleteRequest {
    /// Every written part.
    pub parts: Vec<UploadPart>,
}

/// Stage a large current workspace-file replacement.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadCreateRequest {
    /// The media type.
    pub content_type: String,
    /// The latest-only logical file name to replace immediately with pending.
    pub name: ResourceName,
    /// Every contiguous part declaration used to mint the checksum-bound admission grants.
    pub parts: Vec<UploadPartRequest>,
    /// The exact whole-object hash.
    pub sha256: ContentHash,
    /// The exact object size.
    pub size_bytes: DecimalU128,
}

/// One written part and the entity tag the store returned.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadPart {
    /// The entity tag the store returned.
    pub etag: ETag,
    /// Which part.
    pub part_number: u32,
}

/// One presigned PUT grant. The headers must be sent verbatim: they carry the part checksum and
/// length the signature covers, and a PUT without them is rejected by the object store.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadPartGrant {
    /// When the signature stops verifying.
    pub expires_at: Timestamp,
    /// Headers the caller must replay verbatim.
    pub headers: Vec<HttpHeader>,
    /// Which part.
    pub part_number: u32,
    /// The signed URL.
    pub url: HttpsUrl,
}

/// One part to presign. The part's own SHA-256 is required because S3 verifies it on the way up and
/// the completion manifest checks it again; the server never sees the bytes, so it cannot compute
/// it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadPartRequest {
    /// Which part.
    pub part_number: u32,
    /// The part's own SHA-256.
    pub sha256: ContentHash,
    /// The exact Content-Length to be signed.
    pub size_bytes: DecimalU128,
}

/// Where a staged upload is in its lifecycle.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadState {
    /// Parts may still be written.
    Staging,
    /// Complete and pinnable by a registry PUT.
    Ready,
}

impl UploadState {
    /// Every value, in declared order.
    pub const ALL: &'static [UploadState] = &[UploadState::Staging, UploadState::Ready];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::Ready => "ready",
        }
    }
}

/// Resolve one current workspace-file name to a private immutable hash and mount path at admission.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceFileMount {
    /// Current logical workspace-file name.
    pub name: ResourceName,
    /// Normalized absolute sandbox destination under /workspace.
    pub path: FilePath,
}

// --- usage -------------------------------------------------------

/// One resource aggregate, discriminated by category.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "category", rename_all = "snake_case")]
pub enum UsageAggregate {
    /// Storage.
    Storage(UsageStorageAggregate),
    /// Compute.
    Compute(UsageComputeAggregate),
    /// Memory.
    Memory(UsageMemoryAggregate),
    /// Data transfer.
    DataTransfer(UsageDataTransferAggregate),
}

/// What a regional aggregate is attributed to.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageAttribution {
    /// The operation, when attributable. Never populated by `usage_query`, as `sessionId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    /// Where the usage happened.
    pub region: Region,
    /// The service-time interval.
    pub service_time: TimeRange,
    /// The session, when attributable. Never populated by `usage_query`: that route answers from a
    /// coarse face that carries no session identity, and its absence is honest rather than an
    /// omission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Which authority produced the facts. One of the `UsageAuthority` identifiers, constant per
    /// partition.
    pub source: String,
    /// The workspace.
    pub workspace_id: WorkspaceId,
}

/// The three authorities that admit usage facts. There are three rather than four because memory
/// and compute are one contiguous fact sequence behind one authority, so they share one frontier;
/// publishing four frontiers with two of them always identical would claim an independence that
/// does not exist.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageAuthority {
    /// The storage authority.
    Storage,
    /// The compute authority, which admits both compute and memory facts.
    Compute,
    /// The data-transfer authority.
    Transfer,
}

impl UsageAuthority {
    /// Every value, in declared order.
    pub const ALL: &'static [UsageAuthority] = &[
        UsageAuthority::Storage,
        UsageAuthority::Compute,
        UsageAuthority::Transfer,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Compute => "compute",
            Self::Transfer => "transfer",
        }
    }
}

/// The time grain one usage query answers at. All bucketing is UTC and there is no timezone
/// parameter; a range must be aligned to whole buckets of the requested grain and a misaligned
/// range is refused rather than widened to the enclosing buckets.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageBucket {
    /// One item per whole UTC hour.
    Hour,
    /// One item per whole UTC day.
    Day,
    /// One item for the whole range. Never paged: a partial total is a wrong number.
    Total,
}

impl UsageBucket {
    /// Every value, in declared order.
    pub const ALL: &'static [UsageBucket] =
        &[UsageBucket::Hour, UsageBucket::Day, UsageBucket::Total];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Total => "total",
        }
    }
}

/// The four priced categories.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageCategory {
    /// Retained bytes over time.
    Storage,
    /// Millicpu-milliseconds.
    Compute,
    /// Byte-milliseconds.
    Memory,
    /// Measured outbound bytes.
    DataTransfer,
}

impl UsageCategory {
    /// Every value, in declared order.
    pub const ALL: &'static [UsageCategory] = &[
        UsageCategory::Storage,
        UsageCategory::Compute,
        UsageCategory::Memory,
        UsageCategory::DataTransfer,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Compute => "compute",
            Self::Memory => "memory",
            Self::DataTransfer => "data_transfer",
        }
    }
}

/// Millicpu-milliseconds of guest compute.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageComputeAggregate {
    /// What it is attributed to.
    pub attribution: UsageAttribution,
    /// Millicpu-milliseconds.
    pub millicpu_milliseconds: DecimalU128,
    /// Whether this item is settled as of `completeThrough`.
    pub settlement: UsageSettlement,
}

/// Measured outbound bytes.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageDataTransferAggregate {
    /// What it is attributed to.
    pub attribution: UsageAttribution,
    /// Measured egress bytes.
    pub egress_bytes: DecimalU128,
    /// Whether this item is settled as of `completeThrough`.
    pub settlement: UsageSettlement,
}

/// How far the pipeline has advanced for one workspace and authority. The answer is exact at or
/// below `completeThrough` and contains nothing above `includesThrough`; between the two it is
/// partial, and saying so is the point. Both are monotone across repeated queries.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageFrontier {
    /// Facts accepted by the authority.
    pub accepted_sequence: DecimalU128,
    /// The authority whose contiguous fact sequence this frontier describes. Not a priced category:
    /// memory and compute share one.
    pub category: UsageAuthority,
    /// The answer contains every matching fact at or below this sequence. Pinned on the first page
    /// and reported unchanged on every later page.
    pub complete_through: DecimalU128,
    /// No fact above this sequence is present in the answer. Per page.
    pub includes_through: DecimalU128,
    /// Facts folded into the query projection.
    pub projected_sequence: DecimalU128,
    /// Facts delivered to central settlement.
    pub published_sequence: DecimalU128,
    /// The region.
    pub region: Region,
    /// Service time the authority has observed through, when any fact has been admitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_through: Option<Timestamp>,
    /// Facts settled centrally.
    pub settled_sequence: DecimalU128,
    /// Why the fold stopped, present exactly when `state` is `stalled`. A stalled frontier is the
    /// honest signal; a lag is not a stall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_reason: Option<String>,
    /// The sequence the fold stopped at, present exactly when `state` is `stalled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled_at: Option<DecimalU128>,
    /// Whether the fold is advancing or parked.
    pub state: UsageFrontierState,
    /// The workspace.
    pub workspace_id: WorkspaceId,
}

/// Whether a fold is advancing or parked.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageFrontierState {
    /// The fold is applying facts in sequence.
    Advancing,
    /// The fold is parked behind a record it refused, and nothing beyond it is in the answer.
    Stalled,
}

impl UsageFrontierState {
    /// Every value, in declared order.
    pub const ALL: &'static [UsageFrontierState] =
        &[UsageFrontierState::Advancing, UsageFrontierState::Stalled];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Advancing => "advancing",
            Self::Stalled => "stalled",
        }
    }
}

/// The axes a usage query may group by. Session, run and operation are deliberately absent: every
/// stored aggregate row is keyed by a hash of a seven-member dimension tuple that includes the
/// session, so grouping by one of them would read hundreds of thousands of rows for a single
/// monthly total. "How much did session X cost?" is not answerable by any route in v1, and
/// publishing an axis that is always refused would be a false capability.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageGrouping {
    /// Priced category.
    Category,
    /// Region.
    Region,
    /// Workspace.
    Workspace,
}

impl UsageGrouping {
    /// Every value, in declared order.
    pub const ALL: &'static [UsageGrouping] = &[
        UsageGrouping::Category,
        UsageGrouping::Region,
        UsageGrouping::Workspace,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Category => "category",
            Self::Region => "region",
            Self::Workspace => "workspace",
        }
    }
}

/// Byte-milliseconds of guest memory.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageMemoryAggregate {
    /// What it is attributed to.
    pub attribution: UsageAttribution,
    /// Byte-milliseconds.
    pub byte_milliseconds: DecimalU128,
    /// Whether this item is settled as of `completeThrough`.
    pub settlement: UsageSettlement,
}

/// One page of usage quantities plus the frontiers that bound its completeness. Monetary statements
/// remain a central finance read.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsagePage {
    /// Pipeline frontiers, one per authority the query touched.
    pub frontiers: Vec<UsageFrontier>,
    /// The page.
    pub items: Vec<UsageAggregate>,
    /// Continuation token. Never present for `bucket: total`: a partial total is a wrong number, so
    /// an over-budget total is refused instead of paged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// A bounded query over authoritative regional usage quantities.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageQuery {
    /// The time grain to answer at.
    pub bucket: UsageBucket,
    /// Restrict to these categories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<UsageCategory>>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Grouping axes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<Vec<UsageGrouping>>,
    /// Page size. Absent means 25. Above the maximum the request is refused, never clamped: a
    /// caller silently given fewer items than it asked for cannot tell a short page from the end of
    /// a collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The service-time window, aligned to whole `bucket` grains in UTC. A misaligned range is
    /// refused, never widened. A range longer than 400 days is refused.
    pub time_range: TimeRange,
}

/// Whether an item is covered by a committed settlement receipt as of `completeThrough`. Not
/// permanently terminal: a correction or a void arrives as a new fact at a higher sequence and
/// re-opens its bucket to `provisional`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSettlement {
    /// Settled as of `completeThrough`.
    Settled,
    /// Not yet covered by a settlement receipt.
    Provisional,
}

impl UsageSettlement {
    /// Every value, in declared order.
    pub const ALL: &'static [UsageSettlement] =
        &[UsageSettlement::Settled, UsageSettlement::Provisional];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::Provisional => "provisional",
        }
    }
}

/// Retained bytes over time.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageStorageAggregate {
    /// What it is attributed to.
    pub attribution: UsageAttribution,
    /// Byte-minutes of retained storage.
    pub byte_minutes: DecimalU128,
    /// Whether this item is settled as of `completeThrough`.
    pub settlement: UsageSettlement,
}

// --- query parameters ------------------------------------------------------

/// Query parameters of `api_keys_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiKeysListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The workspace whose keys are listed.
    pub workspace_id: WorkspaceId,
}

/// Query parameters of `billing_transactions_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingTransactionsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size; defaults to 25.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `billing_usage_get`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingUsageGetQuery {
    /// Restrict to one rated category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<BillingUsageCategory>,
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Inclusive service-time lower bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Timestamp>,
    /// Page size; defaults to 25.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Restrict to one session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Exclusive service-time upper bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Timestamp>,
}

/// Query parameters of `registry_files_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegistryFilesListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `session_messages_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionMessagesListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `session_messages_stream`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionMessagesStreamQuery {
    /// Resume after this stream sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<DecimalU128>,
}

/// Query parameters of `session_telemetry_replay`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionTelemetryReplayQuery {
    /// Replay after this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<DecimalU128>,
    /// Maximum frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `session_telemetry_stream`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionTelemetryStreamQuery {
    /// Resume after this live sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<DecimalU128>,
}

/// Query parameters of `sessions_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Restrict to one status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SessionStatus>,
}
