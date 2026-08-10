//! GENERATED — DO NOT EDIT.
//!
//! The public request, response and query models.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:1f778bdb0c16820c3ab2228484dcaff60fbc9870c1f636942bd6e589a8d46f0f`.
//! Regenerate with `cargo run -p aex-contract-gen -- build`.

#![allow(clippy::large_enum_variant, reason = "a wire union is never boxed")]
#![allow(clippy::match_same_arms, reason = "one arm per row")]
#![allow(clippy::too_many_lines, reason = "one arm per row")]

use crate::canonical::CanonicalJson;
use crate::cursor::Cursor;
use crate::error::ObservedErrorCode;
use crate::ids::AgentId;
use crate::ids::ApiKeyId;
use crate::ids::ApprovalId;
use crate::ids::ContentHash;
use crate::ids::ExportId;
use crate::ids::FilePath;
use crate::ids::GenerationId;
use crate::ids::InvitationId;
use crate::ids::MeasurementId;
use crate::ids::MembershipId;
use crate::ids::MessageId;
use crate::ids::ObservationId;
use crate::ids::OperationId;
use crate::ids::OrganizationId;
use crate::ids::ProviderCredentialId;
use crate::ids::ResourceName;
use crate::ids::RunId;
use crate::ids::SessionId;
use crate::ids::SpanId;
use crate::ids::StatementId;
use crate::ids::TelemetryBatchId;
use crate::ids::TelemetryGapId;
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

/// Why an account is paused, and therefore which remedy exists. One value per durable
/// `finance.billing_account.state` hold: collapsing all four onto `top_up_required` told a customer
/// under a dispute hold to make a payment that would not restore service.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountPauseReason {
    /// The prepaid balance is exhausted; a top-up restores service.
    TopUpRequired,
    /// A charge is held pending payment; a top-up does not clear it.
    PaymentHold,
    /// A chargeback is open; nothing the customer pays restores service.
    DisputeHold,
    /// The account is closed. There is no remedy.
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

/// A paused account and what it takes to restore it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AccountPausedState {
    /// When the state last changed.
    pub changed_at: Timestamp,
    /// When unfunded content is scheduled for deletion. Published and permanently absent: this is
    /// the regional content lifecycle's fact and that authority does not exist yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletion_scheduled_at: Option<Timestamp>,
    /// Smallest top-up that restores service. Present exactly when `reason` is `top_up_required`;
    /// the other three holds have no paying remedy, and naming an amount for them would be a false
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_restore_cents: Option<Cents>,
    /// Why the account is paused.
    pub reason: AccountPauseReason,
    /// How long retained content stays funded. Published and permanently absent: storage is billed
    /// monthly and unfunded storage is enforced through the account pause, so no authority owns
    /// this instant yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_funded_until: Option<Timestamp>,
    /// Monotonic state revision.
    pub revision: u64,
}

/// Workspace API key metadata. There is no public last-used timestamp.
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
    /// The workspace it authorizes.
    pub workspace_id: WorkspaceId,
}

/// Mint a workspace API key.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiKeyCreateRequest {
    /// Display name.
    pub name: String,
    /// The scopes to grant.
    pub scopes: Vec<ScopeId>,
    /// The workspace to authorize.
    pub workspace_id: WorkspaceId,
}

/// One page of API key metadata.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiKeyPage {
    /// The page.
    pub items: Vec<ApiKey>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// The complete automatic top-up policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AutoTopupPolicy {
    /// How much each automatic top-up adds.
    pub amount_cents: Cents,
    /// Whether automatic top-up runs.
    pub enabled: bool,
    /// The organization.
    pub organization_id: OrganizationId,
    /// Monotonic policy revision.
    pub revision: u64,
    /// Balance at or below which a top-up is attempted.
    pub threshold_cents: Cents,
    /// When the policy last changed.
    pub updated_at: Timestamp,
}

/// Replace the automatic top-up policy; every field is required.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AutoTopupPolicyRequest {
    /// How much each automatic top-up adds.
    pub amount_cents: Cents,
    /// Whether automatic top-up runs.
    pub enabled: bool,
    /// Balance at or below which a top-up is attempted.
    pub threshold_cents: Cents,
}

/// The prepaid balance of one organization.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingBalance {
    /// Spendable now.
    pub available_cents: Cents,
    /// Always USD.
    pub currency: Currency,
    /// What the balance implies.
    pub operational_state: AccountOperationalState,
    /// The organization.
    pub organization_id: OrganizationId,
    /// Recorded but not yet settled.
    pub pending_cents: Cents,
    /// Held against admitted work.
    pub reserved_cents: Cents,
    /// Monotonic balance revision.
    pub revision: u64,
    /// When the balance last changed.
    pub updated_at: Timestamp,
}

/// The only supported currency.
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

/// The one bounded read that fills the dashboard shell.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DashboardBootstrap {
    /// Current operational state for each organization in this snapshot.
    pub accounts: Vec<OrganizationAccount>,
    /// Their email address.
    pub email: String,
    /// When this snapshot was taken.
    pub generated_at: Timestamp,
    /// Organizations they belong to.
    pub organizations: Vec<Organization>,
    /// The signed-in person.
    pub user_id: UserId,
    /// Workspaces they can reach.
    pub workspaces: Vec<Workspace>,
}

/// A minted browser session, returned exactly once.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DashboardSessionCredential {
    /// When it stops verifying.
    pub expires_at: Timestamp,
    /// The bearer credential; hold it in a cookie the browser will not hand to script.
    pub session: String,
    /// The person it authenticates.
    pub user_id: UserId,
}

/// Complete a browser sign-in by handing this plane the authorization code a provider redirect
/// returned. `central-identity-api` performs the token exchange with the provider itself, so
/// nothing the caller asserts about who they are is believed: the person is whoever the provider
/// answers with.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DashboardSessionRequest {
    /// The single-use authorization code the redirect returned. It is redeemed exactly once and
    /// never retried: a code the provider may already have consumed cannot be re-presented.
    pub code: String,
    /// The RFC 7636 PKCE verifier the browser held in its own cookie for this plane's origin.
    /// Possessing it is what proves the redirect belongs to the browser that started this sign-in:
    /// a cross-site forgery carries somebody else's `state` and cannot present a verifier that
    /// hashes to it.
    pub code_verifier: String,
    /// Which provider issued the code.
    pub provider: IdentityProvider,
    /// The `state` the provider echoed back. It is the RFC 7636 S256 challenge of `codeVerifier`,
    /// and the exchange refuses any request where it is not.
    pub state: String,
}

/// A pending device authorization the user must approve in a browser.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceAuthorization {
    /// The polling secret.
    pub device_code: String,
    /// When the device code stops working.
    pub expires_at: Timestamp,
    /// Minimum polling interval.
    pub interval_seconds: u32,
    /// The short code the user types.
    pub user_code: String,
    /// Where the user approves.
    pub verification_uri: HttpsUrl,
    /// The same page with the code prefilled.
    pub verification_uri_complete: HttpsUrl,
}

/// Begin the CLI device-authorization flow.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceAuthorizationRequest {
    /// The public client identifier.
    pub client_id: String,
    /// The scopes requested.
    pub scopes: Vec<ScopeId>,
}

/// What a person chose about a device authorization.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceDecision {
    /// Let the device redeem an account token.
    Approve,
    /// Refuse the device; the code can never be redeemed.
    Deny,
}

impl DeviceDecision {
    /// Every value, in declared order.
    pub const ALL: &'static [DeviceDecision] = &[DeviceDecision::Approve, DeviceDecision::Deny];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
        }
    }
}

/// Approve or deny a pending device authorization by its user code.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceDecisionRequest {
    /// What the person chose.
    pub decision: DeviceDecision,
    /// The short code the device displayed.
    pub user_code: String,
}

/// The recorded outcome of a device decision.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceDecisionResult {
    /// When the decision was written.
    pub decided_at: Timestamp,
    /// What was recorded; never differs from the request.
    pub decision: DeviceDecision,
    /// The scopes the device asked for, so the page can name what it granted.
    pub scopes: Vec<ScopeId>,
}

/// An issued account token.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceToken {
    /// The bearer token; store it, never log it.
    pub account_token: String,
    /// When the token stops verifying.
    pub expires_at: Timestamp,
    /// The scopes it carries.
    pub scopes: Vec<ScopeId>,
}

/// Exchange an approved device code for an account token.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceTokenRequest {
    /// The public client identifier.
    pub client_id: String,
    /// The polling secret.
    pub device_code: String,
}

/// An external sign-in provider AEX links a person to.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityProvider {
    /// GitHub.
    Github,
    /// Google.
    Google,
}

impl IdentityProvider {
    /// Every value, in declared order.
    pub const ALL: &'static [IdentityProvider] =
        &[IdentityProvider::Github, IdentityProvider::Google];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Google => "google",
        }
    }
}

/// A pending invitation to join an organization.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Invitation {
    /// When it was sent.
    pub created_at: Timestamp,
    /// Who was invited.
    pub email: String,
    /// When it stops being redeemable.
    pub expires_at: Timestamp,
    /// Identity.
    pub id: InvitationId,
    /// The organization.
    pub organization_id: OrganizationId,
    /// When it was accepted, revoked or expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<Timestamp>,
    /// The role it grants.
    pub role: InvitationRole,
    /// Lifecycle position.
    pub status: InvitationStatus,
}

/// What one acceptance redeemed. An invitation carries no secret, so acceptance names nothing:
/// every pending invitation addressed to the caller's verified email is redeemed in one
/// transaction. The list is empty when nothing was pending, which is what a repeated call answers.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InvitationAcceptResult {
    /// One membership per invitation redeemed by this call, created or raised to the invited role.
    /// Bounded by the same 100 the acceptance transaction reads.
    pub memberships: Vec<Membership>,
}

/// Invite a person to an organization.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InvitationCreateRequest {
    /// Who to invite.
    pub email: String,
    /// The role to grant.
    pub role: InvitationRole,
}

/// The role an invitation grants; ownership is never invited.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationRole {
    /// Manage members, workspaces and keys.
    Admin,
    /// Use the organization resources.
    Member,
}

impl InvitationRole {
    /// Every value, in declared order.
    pub const ALL: &'static [InvitationRole] = &[InvitationRole::Admin, InvitationRole::Member];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// Where an invitation is in its lifecycle.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationStatus {
    /// Sent, not yet resolved.
    Pending,
    /// Redeemed into a membership.
    Accepted,
    /// Withdrawn.
    Revoked,
    /// Timed out.
    Expired,
}

impl InvitationStatus {
    /// Every value, in declared order.
    pub const ALL: &'static [InvitationStatus] = &[
        InvitationStatus::Pending,
        InvitationStatus::Accepted,
        InvitationStatus::Revoked,
        InvitationStatus::Expired,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
        }
    }
}

/// A person's role inside one organization.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Membership {
    /// When it was created.
    pub created_at: Timestamp,
    /// The person's email address.
    pub email: String,
    /// Identity.
    pub id: MembershipId,
    /// The organization.
    pub organization_id: OrganizationId,
    /// The role.
    pub role: OrganizationRole,
    /// Whether the membership is usable.
    pub status: MembershipStatus,
    /// The person.
    pub user_id: UserId,
}

/// One page of memberships.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MembershipPage {
    /// The page.
    pub items: Vec<Membership>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Whether a membership is usable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipStatus {
    /// The only membership status v1 exposes.
    Active,
}

impl MembershipStatus {
    /// Every value, in declared order.
    pub const ALL: &'static [MembershipStatus] = &[MembershipStatus::Active];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
        }
    }
}

/// A freshly minted key. The value never appears in a later read.
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
    /// The workspace it authorizes.
    pub workspace_id: WorkspaceId,
}

/// Where a workspace operational state comes from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationalStateSource {
    /// Inherited from the owning account.
    Account,
}

impl OperationalStateSource {
    /// Every value, in declared order.
    pub const ALL: &'static [OperationalStateSource] = &[OperationalStateSource::Account];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
        }
    }
}

/// The billing and ownership boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Organization {
    /// What the calling principal may do here.
    pub caller_role: OrganizationRole,
    /// When it was created.
    pub created_at: Timestamp,
    /// Identity.
    pub id: OrganizationId,
    /// Display name.
    pub name: String,
    /// URL-safe name.
    pub slug: String,
}

/// One organization's operational state in a multi-organization dashboard snapshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OrganizationAccount {
    /// The organization this state governs.
    pub organization_id: OrganizationId,
    /// The organization's current operational state.
    pub state: AccountOperationalState,
}

/// Create an organization.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OrganizationCreateRequest {
    /// Display name.
    pub name: String,
}

/// One page of organizations.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OrganizationPage {
    /// The page.
    pub items: Vec<Organization>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// The caller role inside one organization.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationRole {
    /// Full control including deletion and billing.
    Owner,
    /// Manage members, workspaces and keys.
    Admin,
    /// Use the organization resources.
    Member,
}

impl OrganizationRole {
    /// Every value, in declared order.
    pub const ALL: &'static [OrganizationRole] = &[
        OrganizationRole::Owner,
        OrganizationRole::Admin,
        OrganizationRole::Member,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// Create a hosted billing portal session.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortalSessionRequest {
    /// Where to return when the user is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_url: Option<HttpsUrl>,
}

/// One immutable issued statement. The platform never rerenders history.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Statement {
    /// Hash of the issued artifact.
    pub artifact_hash: ContentHash,
    /// Always USD.
    pub currency: Currency,
    /// Identity.
    pub id: StatementId,
    /// When it was issued.
    pub issued_at: Timestamp,
    /// The priced lines.
    pub lines: Vec<StatementLine>,
    /// The organization.
    pub organization_id: OrganizationId,
    /// The billed period.
    pub period: TimeRange,
    /// The issued total.
    pub total_cents: Cents,
}

/// One priced category line. Lines sum exactly to the statement total.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StatementLine {
    /// The priced category.
    pub category: UsageCategory,
    /// The line total.
    pub total_cents: Cents,
}

/// The header of one immutable issued statement.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StatementSummary {
    /// Hash of the issued artifact.
    pub artifact_hash: ContentHash,
    /// Always USD.
    pub currency: Currency,
    /// Identity.
    pub id: StatementId,
    /// When it was issued.
    pub issued_at: Timestamp,
    /// The organization.
    pub organization_id: OrganizationId,
    /// The billed period.
    pub period: TimeRange,
    /// The issued total.
    pub total_cents: Cents,
}

/// One page of statement headers, newest first.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StatementSummaryPage {
    /// The page.
    pub items: Vec<StatementSummary>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Create a hosted top-up checkout.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TopUpCheckoutRequest {
    /// The amount to add, in whole cents.
    pub amount_cents: Cents,
    /// Where to return after cancellation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_url: Option<HttpsUrl>,
    /// Where to return after success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_url: Option<HttpsUrl>,
}

/// A region-pinned execution and content boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Workspace {
    /// The regional host this workspace answers on.
    pub api_url: HttpsUrl,
    /// When it was created.
    pub created_at: Timestamp,
    /// The running deletion operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletion_operation_id: Option<OperationId>,
    /// Identity.
    pub id: WorkspaceId,
    /// Display name.
    pub name: String,
    /// Inherited account state.
    pub operational_state: WorkspaceOperationalState,
    /// The owning organization.
    pub organization_id: OrganizationId,
    /// Immutable placement.
    pub region: Region,
    /// URL-safe name.
    pub slug: String,
    /// Whether it admits work.
    pub status: WorkspaceStatus,
}

/// Create a region-pinned workspace. Region is required and immutable.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceCreateRequest {
    /// Display name.
    pub name: String,
    /// The owning organization.
    pub organization_id: OrganizationId,
    /// Where the workspace lives, forever.
    pub region: Region,
}

/// Admit the global workspace-deletion operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceDeleteRequest {
    /// The workspace id, restated.
    pub confirmation: WorkspaceId,
}

/// The workspace view of the account operational state it inherits.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceOperationalState {
    /// Always the account.
    pub inherited_from: OperationalStateSource,
    /// The organization the state comes from.
    pub organization_id: OrganizationId,
    /// The inherited state.
    pub state: AccountOperationalState,
}

/// One page of workspaces.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspacePage {
    /// The page.
    pub items: Vec<Workspace>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Whether a workspace admits work.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    /// Admitting work.
    Active,
    /// A deletion operation is running.
    Deleting,
}

impl WorkspaceStatus {
    /// Every value, in declared order.
    pub const ALL: &'static [WorkspaceStatus] =
        &[WorkspaceStatus::Active, WorkspaceStatus::Deleting];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deleting => "deleting",
        }
    }
}

/// What remains of a deleted workspace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceTombstone {
    /// When deletion committed.
    pub deleted_at: Timestamp,
    /// The operation that deleted it.
    pub operation_id: OperationId,
    /// The owning organization.
    pub organization_id: OrganizationId,
    /// The deleted workspace.
    pub workspace_id: WorkspaceId,
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
    /// The telemetry gaps that blocked completeness.
    Gap(ErrorDetailsGap),
}

/// The telemetry gaps that blocked a completeness requirement.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ErrorDetailsGap {
    /// The blocking gaps.
    pub gap_ids: Vec<TelemetryGapId>,
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

/// A durable operation record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Operation {
    /// Whether cancellation is still possible.
    pub cancelable: bool,
    /// When the effect became durable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_at: Option<Timestamp>,
    /// When the operation was admitted.
    pub created_at: Timestamp,
    /// The terminal failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<OperationFailure>,
    /// The operation identity minted by the caller.
    pub id: OperationId,
    /// What the operation does.
    pub kind: OperationKind,
    /// Coarse progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<OperationProgress>,
    /// The typed successful result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<OperationResult>,
    /// The owning session, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// When work began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    /// Lifecycle position.
    pub status: OperationStatus,
    /// When the record reached a terminal status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<Timestamp>,
    /// Last change to this record.
    pub updated_at: Timestamp,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
}

/// A durable operation failure. It carries no synthetic request identity.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperationFailure {
    /// The stable public failure code.
    pub code: ObservedErrorCode,
    /// Customer-safe detail, when the domain produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<CanonicalJson>,
    /// Whether the same operation step may be retried.
    pub retryable: bool,
}

/// Every durable operation the platform admits.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// Stop the running turn.
    SessionStop,
    /// Persist the live workspace.
    SessionPersist,
    /// Discard the live workspace.
    WorkspaceDiscard,
    /// Move the session into the recovery window.
    SessionTrash,
    /// Bring the session back out of the recovery window.
    SessionRestore,
    /// Destroy the session irreversibly.
    SessionPurge,
    /// Rebind session credentials.
    CredentialRebind,
    /// Produce a telemetry export artifact.
    TelemetryExport,
    /// Delete the workspace across both planes.
    WorkspaceDelete,
}

impl OperationKind {
    /// Every value, in declared order.
    pub const ALL: &'static [OperationKind] = &[
        OperationKind::SessionStop,
        OperationKind::SessionPersist,
        OperationKind::WorkspaceDiscard,
        OperationKind::SessionTrash,
        OperationKind::SessionRestore,
        OperationKind::SessionPurge,
        OperationKind::CredentialRebind,
        OperationKind::TelemetryExport,
        OperationKind::WorkspaceDelete,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionStop => "session_stop",
            Self::SessionPersist => "session_persist",
            Self::WorkspaceDiscard => "workspace_discard",
            Self::SessionTrash => "session_trash",
            Self::SessionRestore => "session_restore",
            Self::SessionPurge => "session_purge",
            Self::CredentialRebind => "credential_rebind",
            Self::TelemetryExport => "telemetry_export",
            Self::WorkspaceDelete => "workspace_delete",
        }
    }
}

/// One page of durable operations.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperationPage {
    /// The page.
    pub items: Vec<Operation>,
    /// Continuation token; absent on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Coarse progress; never a completion promise.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperationProgress {
    /// Units done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<DecimalU128>,
    /// The named phase.
    pub phase: String,
    /// Units expected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<DecimalU128>,
}

/// The typed successful result of a durable operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationResult {
    /// Stop result.
    SessionStop(SessionStopResult),
    /// Persist result.
    SessionPersist(SessionPersistResult),
    /// Discard result.
    WorkspaceDiscard(WorkspaceDiscardResult),
    /// Trash result.
    SessionTrash(SessionTrashResult),
    /// Restore result.
    SessionRestore(SessionRestoreResult),
    /// Session tombstone.
    SessionPurge(SessionTombstone),
    /// Rebind result.
    CredentialRebind(CredentialRebindResult),
    /// Export result.
    TelemetryExport(TelemetryExportResult),
    /// Workspace tombstone.
    WorkspaceDelete(WorkspaceTombstone),
}

/// Where a durable operation is in its lifecycle.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    /// Admitted, not started.
    Queued,
    /// In progress.
    Running,
    /// Committed.
    Succeeded,
    /// Terminal failure.
    Failed,
    /// Cancelled before its commit point.
    Cancelled,
}

impl OperationStatus {
    /// Every value, in declared order.
    pub const ALL: &'static [OperationStatus] = &[
        OperationStatus::Queued,
        OperationStatus::Running,
        OperationStatus::Succeeded,
        OperationStatus::Failed,
        OperationStatus::Cancelled,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
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

/// What a live read actually did to the session workspace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceAccess {
    /// The consistency the read achieved.
    pub consistency: FileConsistency,
    /// The generation the read observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<GenerationId>,
    /// Whether the read woke a retained session.
    pub woke: bool,
}

// --- observation -------------------------------------------------------

/// What an export does when the window has recorded gaps.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportCompleteness {
    /// Refuse to produce an artifact over a gap.
    Require,
    /// Produce the artifact and record the gaps in the manifest.
    AllowGaps,
}

impl ExportCompleteness {
    /// Every value, in declared order.
    pub const ALL: &'static [ExportCompleteness] =
        &[ExportCompleteness::Require, ExportCompleteness::AllowGaps];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Require => "require",
            Self::AllowGaps => "allow_gaps",
        }
    }
}

/// The two export formats. Parquet is deliberately absent: its encoder is a typed refusal pending a
/// writer that can be pinned to byte-identical output, and admitting an operation guaranteed to
/// fail is worse than refusing the request. Re-adding it is a wire change.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// Newline-delimited JSON.
    Ndjson,
    /// OTLP JSON. Refuses `events` and trace summaries rather than coercing them.
    OtlpJson,
}

impl ExportFormat {
    /// Every value, in declared order.
    pub const ALL: &'static [ExportFormat] = &[ExportFormat::Ndjson, ExportFormat::OtlpJson];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ndjson => "ndjson",
            Self::OtlpJson => "otlp_json",
        }
    }
}

/// What one export walks. Deliberately not an `ObservationQuery`: an export has no caller-visible
/// pagination, so a cursor, a limit and a walk direction are meaningless states rather than states
/// to refuse at runtime. The partition list and the snapshot position are pinned at admission and
/// live on the export row, which is the only channel the task has.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExportQuery {
    /// How much must be observed before the window is admissible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consistency: Option<ObservationConsistency>,
    /// The bounded filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<ObservationFilter>,
    /// Which signal.
    pub signal: ObservationSignal,
    /// The observation-time window.
    pub time_range: TimeRange,
}

/// Where an export is in its lifecycle.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportStatus {
    /// Being produced.
    Preparing,
    /// Downloadable.
    Ready,
    /// Past its retention.
    Expired,
    /// Explicitly revoked.
    Revoked,
}

impl ExportStatus {
    /// Every value, in declared order.
    pub const ALL: &'static [ExportStatus] = &[
        ExportStatus::Preparing,
        ExportStatus::Ready,
        ExportStatus::Expired,
        ExportStatus::Revoked,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        }
    }
}

/// The calculations a metric aggregation may request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricAggregationCalculation {
    /// Sum.
    Sum,
    /// Count.
    Count,
    /// Minimum.
    Min,
    /// Maximum.
    Max,
    /// Arithmetic mean.
    Avg,
}

impl MetricAggregationCalculation {
    /// Every value, in declared order.
    pub const ALL: &'static [MetricAggregationCalculation] = &[
        MetricAggregationCalculation::Sum,
        MetricAggregationCalculation::Count,
        MetricAggregationCalculation::Min,
        MetricAggregationCalculation::Max,
        MetricAggregationCalculation::Avg,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Count => "count",
            Self::Min => "min",
            Self::Max => "max",
            Self::Avg => "avg",
        }
    }
}

/// One aggregated group.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MetricAggregationGroup {
    /// The grouping attributes.
    pub key: BTreeMap<String, String>,
    /// How many points contributed.
    pub sample_count: DecimalU128,
    /// The computed value.
    pub value: f64,
}

/// One page of aggregated groups plus the coverage they are complete over.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MetricAggregationPage {
    /// What the answer covers.
    pub coverage: ObservationCoverage,
    /// The page.
    pub items: Vec<MetricAggregationGroup>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// A bounded metric aggregation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MetricAggregationRequest {
    /// What to compute.
    pub calculation: MetricAggregationCalculation,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// The bounded filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<ObservationFilter>,
    /// Grouping attribute paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<Vec<String>>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The metric name.
    pub metric: String,
    /// The observation-time window.
    pub time_range: TimeRange,
}

/// A known hole in the ordered series.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MissingInterval {
    /// The recorded gap.
    pub gap_id: TelemetryGapId,
    /// The affected window.
    pub range: TimeRange,
}

/// One admitted observation, in the canonical envelope.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Observation {
    /// When AEX admitted it.
    pub accepted_at: Timestamp,
    /// The signal-specific payload.
    pub body: CanonicalJson,
    /// Identity.
    pub id: ObservationId,
    /// When the producer says it happened.
    pub observed_at: Timestamp,
    /// The owning run, when attributable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Position in the ordered series.
    pub sequence: DecimalU128,
    /// The owning session, when attributable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Which signal it belongs to.
    pub signal: ObservationSignal,
    /// The W3C span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<SpanId>,
    /// The W3C trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<TraceId>,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
}

/// How much of the accepted series a read must observe.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationConsistency {
    /// Whatever is indexed now.
    Indexed,
    /// Everything accepted, waiting for indexing if needed.
    Accepted,
}

impl ObservationConsistency {
    /// Every value, in declared order.
    pub const ALL: &'static [ObservationConsistency] = &[
        ObservationConsistency::Indexed,
        ObservationConsistency::Accepted,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Indexed => "indexed",
            Self::Accepted => "accepted",
        }
    }
}

/// What the answer is actually complete over. Never omitted. All four watermarks are accepted-time
/// positions in epoch milliseconds (O-04).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationCoverage {
    /// Admission watermark, in epoch milliseconds.
    pub accepted: DecimalU128,
    /// Whether indexing has reached admission.
    pub caught_up: bool,
    /// Whether the window has no known holes.
    pub complete: bool,
    /// Earliest replayable position, in epoch milliseconds.
    pub earliest_replay: DecimalU128,
    /// Indexing watermark, in epoch milliseconds.
    pub indexed: DecimalU128,
    /// Known holes.
    pub missing_intervals: Vec<MissingInterval>,
    /// The pinned snapshot position, in epoch milliseconds.
    pub snapshot: DecimalU128,
    /// Gaps with no known bound.
    pub unbounded_gaps: Vec<TelemetryGapId>,
}

/// A bounded filter AST. Structural bounds are enforced before construction.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ObservationFilter {
    /// Conjunction.
    All(ObservationFilterAll),
    /// Disjunction.
    Any(ObservationFilterAny),
    /// Negation.
    Not(ObservationFilterNot),
    /// Comparison.
    Compare(ObservationFilterCompare),
    /// Set membership.
    In(ObservationFilterIn),
    /// Presence.
    Exists(ObservationFilterExists),
}

/// Every child must match.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationFilterAll {
    /// The children.
    pub filters: Vec<ObservationFilter>,
}

/// At least one child must match.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationFilterAny {
    /// The children.
    pub filters: Vec<ObservationFilter>,
}

/// A comparison against one field.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationFilterCompare {
    /// The field path.
    pub field: String,
    /// The comparison.
    pub operator: ObservationOperator,
    /// The operand.
    pub value: String,
}

/// Presence of a field.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationFilterExists {
    /// The field path.
    pub field: String,
}

/// Membership of a bounded value set.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationFilterIn {
    /// The field path.
    pub field: String,
    /// The operands.
    pub values: Vec<String>,
}

/// The child must not match.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationFilterNot {
    /// The single child.
    pub filters: Vec<ObservationFilter>,
}

/// One NDJSON frame. `stream` and `listen` emit nothing else.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ObservationFrame {
    /// Observations.
    Records(ObservationFrameRecords),
    /// A surfaced gap.
    Gap(ObservationFrameGap),
    /// A resumable position.
    Cursor(ObservationFrameCursor),
    /// The terminal condition.
    Rotate(ObservationFrameRotate),
}

/// A resumable position.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationFrameCursor {
    /// What has been delivered so far.
    pub coverage: ObservationCoverage,
    /// The position.
    pub cursor: Cursor,
}

/// A gap surfaced inline; it is never silently skipped.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationFrameGap {
    /// The recorded gap.
    pub gap: TelemetryGap,
}

/// A batch of observations. The server splits before the effective frame bound.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationFrameRecords {
    /// The batch.
    pub items: Vec<Observation>,
}

/// The terminal condition after a 200. A stream always ends with a reason.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationFrameRotate {
    /// The last resumable position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// The failure, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiErrorBody>,
    /// Why the stream ended.
    pub reason: RotateReason,
    /// Whether reconnecting from the last cursor can succeed.
    pub retryable: bool,
}

/// Capture the current accepted position and follow it. Origins are rejected.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationListenRequest {
    /// The bounded filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<ObservationFilter>,
    /// Which signal.
    pub signal: ObservationSignal,
}

/// The comparison operators a filter leaf may use.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationOperator {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Lte,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Gte,
    /// String prefix.
    Prefix,
}

impl ObservationOperator {
    /// Every value, in declared order.
    pub const ALL: &'static [ObservationOperator] = &[
        ObservationOperator::Eq,
        ObservationOperator::Ne,
        ObservationOperator::Lt,
        ObservationOperator::Lte,
        ObservationOperator::Gt,
        ObservationOperator::Gte,
        ObservationOperator::Prefix,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::Lt => "lt",
            Self::Lte => "lte",
            Self::Gt => "gt",
            Self::Gte => "gte",
            Self::Prefix => "prefix",
        }
    }
}

/// Which direction a read walks the ordered series.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationOrder {
    /// Oldest first.
    Ascending,
    /// Newest first.
    Descending,
}

impl ObservationOrder {
    /// Every value, in declared order.
    pub const ALL: &'static [ObservationOrder] =
        &[ObservationOrder::Ascending, ObservationOrder::Descending];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }
}

/// Exactly one origin. A stream that declares none or two is rejected.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum ObservationOrigin {
    /// An exact prior position.
    Cursor(ObservationOriginCursor),
    /// An instant.
    Time(ObservationOriginTime),
    /// The earliest replayable position.
    Earliest(ObservationOriginEarliest),
}

/// Resume from an exact prior position.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationOriginCursor {
    /// The prior position.
    pub cursor: Cursor,
}

/// Start from the earliest replayable position.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationOriginEarliest {}

/// Start from an instant.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationOriginTime {
    /// The instant.
    pub at: Timestamp,
}

/// One page of observations plus the coverage it is complete over.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationPage {
    /// What the answer covers.
    pub coverage: ObservationCoverage,
    /// The page.
    pub items: Vec<Observation>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// A bounded, paged observation read.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationQuery {
    /// How much must be observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consistency: Option<ObservationConsistency>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// The bounded filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<ObservationFilter>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Walk direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<ObservationOrder>,
    /// Which signal.
    pub signal: ObservationSignal,
    /// The observation-time window.
    pub time_range: TimeRange,
}

/// The six observable signals.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSignal {
    /// Structured platform events.
    Events,
    /// Log records.
    Logs,
    /// Individual spans.
    Spans,
    /// Metric points.
    Metrics,
    /// Assembled traces.
    Traces,
    /// Every signal, interleaved.
    Telemetry,
}

impl ObservationSignal {
    /// Every value, in declared order.
    pub const ALL: &'static [ObservationSignal] = &[
        ObservationSignal::Events,
        ObservationSignal::Logs,
        ObservationSignal::Spans,
        ObservationSignal::Metrics,
        ObservationSignal::Traces,
        ObservationSignal::Telemetry,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Events => "events",
            Self::Logs => "logs",
            Self::Spans => "spans",
            Self::Metrics => "metrics",
            Self::Traces => "traces",
            Self::Telemetry => "telemetry",
        }
    }
}

/// Replay from exactly one declared origin as NDJSON.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservationStreamRequest {
    /// The bounded filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<ObservationFilter>,
    /// Exactly one origin.
    pub origin: ObservationOrigin,
    /// Which signal.
    pub signal: ObservationSignal,
}

/// Why a stream ended after its 200.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RotateReason {
    /// The connection reached its byte or time budget.
    BudgetExhausted,
    /// The serving process is going away.
    ServerRotating,
    /// The client stopped reading.
    ClientIdle,
    /// The observation authority became unreachable.
    UpstreamUnavailable,
    /// An error ended the stream; `error` names it.
    Failed,
}

impl RotateReason {
    /// Every value, in declared order.
    pub const ALL: &'static [RotateReason] = &[
        RotateReason::BudgetExhausted,
        RotateReason::ServerRotating,
        RotateReason::ClientIdle,
        RotateReason::UpstreamUnavailable,
        RotateReason::Failed,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BudgetExhausted => "budget_exhausted",
            Self::ServerRotating => "server_rotating",
            Self::ClientIdle => "client_idle",
            Self::UpstreamUnavailable => "upstream_unavailable",
            Self::Failed => "failed",
        }
    }
}

/// The AEX receipt returned alongside the standard OTLP response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryAdmissionReceipt {
    /// Records admitted.
    pub accepted: DecimalU128,
    /// When admission committed.
    pub accepted_at: Timestamp,
    /// The admitted batch.
    pub batch_id: TelemetryBatchId,
    /// Decoded payload bytes.
    pub bytes: DecimalU128,
    /// Records rejected.
    pub rejected: DecimalU128,
}

/// One telemetry export record. Session exports remain workspace-owned.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryExport {
    /// The completeness rule it was produced under.
    pub completeness: ExportCompleteness,
    /// When it was admitted.
    pub created_at: Timestamp,
    /// When it stops being downloadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// The artifact format.
    pub format: ExportFormat,
    /// Gaps inside the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_ids: Option<Vec<TelemetryGapId>>,
    /// Identity.
    pub id: ExportId,
    /// Hash of the export manifest, once ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<ContentHash>,
    /// When it became downloadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<Timestamp>,
    /// When it was revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,
    /// The session it was scoped to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Artifact size, once ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<DecimalU128>,
    /// Lifecycle position.
    pub status: ExportStatus,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
}

/// Admit a durable telemetry-export operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryExportRequest {
    /// What to do about recorded gaps.
    pub completeness: ExportCompleteness,
    /// The artifact format.
    pub format: ExportFormat,
    /// What to export. Normalized and pinned onto the export row at admission, so two runs of one
    /// export walk the same plan.
    pub query: ExportQuery,
}

/// The result of a telemetry-export operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryExportResult {
    /// When the artifact stops being downloadable.
    pub expires_at: Timestamp,
    /// The produced export.
    pub export_id: ExportId,
    /// The artifact format.
    pub format: ExportFormat,
    /// Hash of the export manifest.
    pub manifest_hash: ContentHash,
}

/// A recorded hole in the observation series. There is no public repair route.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryGap {
    /// Bytes known lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_count: Option<DecimalU128>,
    /// When it was recorded.
    pub detected_at: Timestamp,
    /// First affected position, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_sequence: Option<DecimalU128>,
    /// Identity.
    pub id: TelemetryGapId,
    /// Observations known lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_count: Option<DecimalU128>,
    /// Why it exists.
    pub reason: TelemetryGapReason,
    /// Whether the records can still be recovered.
    pub recoverable: bool,
    /// What repaired it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair_source: Option<String>,
    /// When it was repaired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repaired_at: Option<Timestamp>,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// The owning session, when attributable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Affected signals.
    pub signals: Vec<ObservationSignal>,
    /// The affected observation-time window, when its extent is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_range: Option<TimeRange>,
    /// Last affected position, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_sequence: Option<DecimalU128>,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
}

/// One page of recorded gaps.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryGapPage {
    /// The page.
    pub items: Vec<TelemetryGap>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// A bounded query over recorded gaps.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryGapQuery {
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Restrict by recoverability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recoverable: Option<bool>,
    /// Restrict to these signals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signals: Option<Vec<ObservationSignal>>,
    /// The observation-time window.
    pub time_range: TimeRange,
}

/// Why a gap exists.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryGapReason {
    /// A batch failed admission.
    AdmissionRejected,
    /// A spooled chunk could not be recovered.
    SpoolLost,
    /// The producer reported dropping records.
    ProducerDropped,
    /// Retained for the vocabulary; unreachable at launch.
    ReplayExpired,
    /// The authority was unreachable during the window.
    AuthorityUnavailable,
}

impl TelemetryGapReason {
    /// Every value, in declared order.
    pub const ALL: &'static [TelemetryGapReason] = &[
        TelemetryGapReason::AdmissionRejected,
        TelemetryGapReason::SpoolLost,
        TelemetryGapReason::ProducerDropped,
        TelemetryGapReason::ReplayExpired,
        TelemetryGapReason::AuthorityUnavailable,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdmissionRejected => "admission_rejected",
            Self::SpoolLost => "spool_lost",
            Self::ProducerDropped => "producer_dropped",
            Self::ReplayExpired => "replay_expired",
            Self::AuthorityUnavailable => "authority_unavailable",
        }
    }
}

/// One assembled trace and the observations it was assembled from.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TraceDetail {
    /// What the assembly covers.
    pub coverage: ObservationCoverage,
    /// The spans.
    pub spans: Vec<Observation>,
    /// The header.
    pub summary: TraceSummary,
}

/// The header of one assembled trace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TraceSummary {
    /// Latest span end.
    pub ended_at: Timestamp,
    /// The root span name.
    pub root_name: String,
    /// How many spans were assembled.
    pub span_count: DecimalU128,
    /// Earliest span start.
    pub started_at: Timestamp,
    /// The W3C trace identifier.
    pub trace_id: TraceId,
    /// The owning workspace, when attributable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
}

// --- provider -------------------------------------------------------

/// An explicit provider and model pair; a model-only override is rejected.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ModelSelection {
    /// Which BYOK binding to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<ProviderCredentialId>,
    /// The exact provider-native model id.
    pub model: String,
    /// The provider.
    pub provider: ProviderId,
}

/// The eight customer-owned BYOK authorities. Gateway authorities use fixed endpoints; arbitrary
/// base URLs are not accepted.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// The `OpenAI` API.
    Openai,
    /// Anthropic.
    Anthropic,
    /// The `DeepSeek` API.
    Deepseek,
    /// The Z.ai API.
    Zai,
    /// The Moonshot AI API.
    Moonshotai,
    /// Google.
    Google,
    /// `OpenRouter`, authenticated with the customer's `OpenRouter` key.
    Openrouter,
    /// `Vercel AI Gateway`, authenticated with the customer's `AI Gateway` key.
    VercelAiGateway,
}

impl ProviderId {
    /// Every value, in declared order.
    pub const ALL: &'static [ProviderId] = &[
        ProviderId::Openai,
        ProviderId::Anthropic,
        ProviderId::Deepseek,
        ProviderId::Zai,
        ProviderId::Moonshotai,
        ProviderId::Google,
        ProviderId::Openrouter,
        ProviderId::VercelAiGateway,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Deepseek => "deepseek",
            Self::Zai => "zai",
            Self::Moonshotai => "moonshotai",
            Self::Google => "google",
            Self::Openrouter => "openrouter",
            Self::VercelAiGateway => "vercel_ai_gateway",
        }
    }
}

// --- regional -------------------------------------------------------

/// A pending or resolved tool approval.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Approval {
    /// Exactly what may run.
    pub bound_call: ApprovalBoundCall,
    /// When it was raised.
    pub created_at: Timestamp,
    /// The recorded decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ApprovalDecision>,
    /// When it stops being decidable.
    pub expires_at: Timestamp,
    /// Identity.
    pub id: ApprovalId,
    /// When it was decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<Timestamp>,
    /// The owning session.
    pub session_id: SessionId,
    /// Lifecycle position.
    pub status: ApprovalStatus,
}

/// The exact call an approval is bound to. Nothing else may run under it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalBoundCall {
    /// The agent inside the run.
    pub agent_id: AgentId,
    /// Canonical digest of the arguments.
    pub arguments_digest: ContentHash,
    /// Digest of the resolved tool configuration.
    pub config_digest: ContentHash,
    /// The configuration revision the call expects.
    pub expected_config_revision: u64,
    /// The credential-custody revision the call expects.
    pub expected_custody_revision: u64,
    /// The generation the call must run in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_generation_id: Option<GenerationId>,
    /// Digest of the tool implementation.
    pub implementation_digest: ContentHash,
    /// The run.
    pub run_id: RunId,
    /// The tool call.
    pub tool_call_id: ToolCallId,
    /// The canonical tool name.
    pub tool_name: ResourceName,
}

/// The two decisions a caller may make.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Let the bound call run.
    Approve,
    /// Refuse the bound call.
    Deny,
}

impl ApprovalDecision {
    /// Every value, in declared order.
    pub const ALL: &'static [ApprovalDecision] =
        &[ApprovalDecision::Approve, ApprovalDecision::Deny];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
        }
    }
}

/// One page of approvals.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalPage {
    /// The page.
    pub items: Vec<Approval>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// When a bound tool call needs a human decision.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ApprovalPolicy {
    /// Never ask.
    AllowAll(ApprovalPolicyAllowAll),
    /// Ask for the named tools.
    RequireForTools(ApprovalPolicyRequireForTools),
}

/// Every tool call runs without an approval.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalPolicyAllowAll {}

/// Named tools require an explicit decision before they run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalPolicyRequireForTools {
    /// Tools that need approval.
    pub tools: Vec<ResourceName>,
}

/// Decide a pending approval. Exact replay returns the winner.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApprovalRespondRequest {
    /// The decision.
    pub decision: ApprovalDecision,
}

/// Where an approval is in its lifecycle.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Waiting on a decision.
    Pending,
    /// Approved; the bound call may run.
    Approved,
    /// Denied; the bound call will not run.
    Denied,
    /// Withdrawn by a stop, cancellation, deletion, pause, continuity loss, tool-call cancellation,
    /// or binding drift.
    Cancelled,
    /// Its explicit decision deadline elapsed.
    Expired,
}

impl ApprovalStatus {
    /// Every value, in declared order.
    pub const ALL: &'static [ApprovalStatus] = &[
        ApprovalStatus::Pending,
        ApprovalStatus::Approved,
        ApprovalStatus::Denied,
        ApprovalStatus::Cancelled,
        ApprovalStatus::Expired,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}

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

/// The only accepted bundle format.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleFormat {
    /// A gzipped tar of a normalized POSIX tree.
    #[serde(rename = "tar.gz")]
    TarGz,
}

impl BundleFormat {
    /// Every value, in declared order.
    pub const ALL: &'static [BundleFormat] = &[BundleFormat::TarGz];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
        }
    }
}

/// One resolved compute dimension.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ComputeShape {
    /// Memory in mebibytes.
    pub memory_mi_b: u32,
    /// Virtual CPUs; fractional at the smaller shapes.
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

/// The result of a credential-rebind operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CredentialRebindResult {
    /// The custody revision after the rebind.
    pub custody_revision: u64,
    /// The bound secrets.
    pub secrets: Vec<SecretRef>,
    /// The session.
    pub session_id: SessionId,
}

/// A session whose deletion is running; rendered at 410.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeletingSession {
    /// Identity.
    pub id: SessionId,
    /// The deletion operation.
    pub operation_id: OperationId,
    /// When deletion began.
    pub started_at: Timestamp,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
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

/// How coherent a live read has to be.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileConsistency {
    /// Observe a single consistent generation.
    Coherent,
    /// Accept whatever the current generation reports.
    BestEffort,
}

impl FileConsistency {
    /// Every value, in declared order.
    pub const ALL: &'static [FileConsistency] =
        &[FileConsistency::Coherent, FileConsistency::BestEffort];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Coherent => "coherent",
            Self::BestEffort => "best_effort",
        }
    }
}

/// Mint a download grant for one persisted file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileDownloadRequest {
    /// The entry to download.
    pub path: FilePath,
    /// An explicit bounded range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<ByteRange>,
}

/// One file entry, identified by its normalized POSIX path.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileEntry {
    /// POSIX mode, four octal digits.
    pub mode: String,
    /// Modification time.
    pub mtime: Timestamp,
    /// The normalized POSIX path.
    pub path: FilePath,
    /// Content hash, for regular files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<ContentHash>,
    /// Size in bytes.
    pub size_bytes: DecimalU128,
    /// Link target, for symlinks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<FilePath>,
    /// What the entry is.
    pub type_: FileEntryType,
}

/// One page of file entries.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileEntryPage {
    /// The page.
    pub items: Vec<FileEntry>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// What a file entry is.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileEntryType {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
}

impl FileEntryType {
    /// Every value, in declared order.
    pub const ALL: &'static [FileEntryType] = &[
        FileEntryType::File,
        FileEntryType::Directory,
        FileEntryType::Symlink,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Symlink => "symlink",
        }
    }
}

/// List persisted files; an observational read that never mutates.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileListRequest {
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Subtree to list; defaults to the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<FilePath>,
    /// Whether to descend; defaults to false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
}

/// Where a referenced file lives. Only persisted files may be attached.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSource {
    /// The persisted workspace tree.
    Persisted,
}

impl FileSource {
    /// Every value, in declared order.
    pub const ALL: &'static [FileSource] = &[FileSource::Persisted];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Persisted => "persisted",
        }
    }
}

/// Stat one persisted file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileStatRequest {
    /// The entry to stat.
    pub path: FilePath,
}

/// Whether a live read may wake a retained session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileWakePolicy {
    /// Wake a retained session if needed.
    Retained,
    /// Never wake; fail instead.
    Never,
}

impl FileWakePolicy {
    /// Every value, in declared order.
    pub const ALL: &'static [FileWakePolicy] = &[FileWakePolicy::Retained, FileWakePolicy::Never];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retained => "retained",
            Self::Never => "never",
        }
    }
}

/// The guest network policy.
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

/// A download grant plus what minting it did to the workspace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LiveDownloadGrant {
    /// The grant.
    pub grant: DownloadGrant,
    /// What minting it did.
    pub workspace_access: WorkspaceAccess,
}

/// Mint a download grant for one live workspace file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LiveFileDownloadRequest {
    /// How coherent the read must be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consistency: Option<FileConsistency>,
    /// Only read this exact generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_generation_id: Option<GenerationId>,
    /// The entry to download.
    pub path: FilePath,
    /// An explicit bounded range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<ByteRange>,
    /// Whether a retained session may be woken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<FileWakePolicy>,
}

/// One live file entry plus what the read did to the workspace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LiveFileEntry {
    /// The entry.
    pub entry: FileEntry,
    /// What the read did.
    pub workspace_access: WorkspaceAccess,
}

/// One page of live file entries plus what the read did.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LiveFileEntryPage {
    /// The page.
    pub items: Vec<FileEntry>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
    /// What the read did.
    pub workspace_access: WorkspaceAccess,
}

/// List live workspace files.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LiveFileListRequest {
    /// How coherent the read must be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consistency: Option<FileConsistency>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Only read this exact generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_generation_id: Option<GenerationId>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Subtree to list; defaults to the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<FilePath>,
    /// Whether to descend; defaults to false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
    /// Whether a retained session may be woken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<FileWakePolicy>,
}

/// Stat one live workspace file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LiveFileStatRequest {
    /// How coherent the read must be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consistency: Option<FileConsistency>,
    /// Only read this exact generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_generation_id: Option<GenerationId>,
    /// The entry to stat.
    pub path: FilePath,
    /// Whether a retained session may be woken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<FileWakePolicy>,
}

/// One MCP request header whose value comes from a workspace secret.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct McpHeader {
    /// The header name.
    pub name: String,
    /// The secret whose value is sent.
    pub secret_name: ResourceName,
}

/// The only accepted MCP transport.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// Streamable HTTP.
    StreamableHttp,
}

impl McpTransport {
    /// Every value, in declared order.
    pub const ALL: &'static [McpTransport] = &[McpTransport::StreamableHttp];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StreamableHttp => "streamable_http",
        }
    }
}

/// One message in a session.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Message {
    /// The parts.
    pub content: Vec<MessagePart>,
    /// When it was recorded.
    pub created_at: Timestamp,
    /// Identity.
    pub id: MessageId,
    /// Who produced it.
    pub role: MessageRole,
    /// The run that produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// The owning session.
    pub session_id: SessionId,
}

/// One page of messages.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessagePage {
    /// The page.
    pub items: Vec<Message>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// One part of a message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePart {
    /// Text.
    Text(MessagePartText),
    /// A persisted file reference.
    File(MessagePartFile),
    /// A canonical tool call.
    ToolCall(MessagePartToolCall),
    /// A canonical tool result.
    ToolResult(MessagePartToolResult),
}

/// A reference to a persisted file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessagePartFile {
    /// Declared media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The normalized POSIX path.
    pub path: FilePath,
    /// Always the persisted tree.
    pub source: FileSource,
}

/// A text part.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessagePartText {
    /// The text.
    pub text: String,
}

/// A tool call emitted by the model.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessagePartToolCall {
    /// The canonical tool-argument digest.
    pub arguments_digest: ContentHash,
    /// The call identity.
    pub id: ToolCallId,
}

/// The canonical result of a tool call.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessagePartToolResult {
    /// The call identity.
    pub id: ToolCallId,
    /// The canonical tool-result digest.
    pub result_digest: ContentHash,
}

/// Who produced a message.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// The customer.
    User,
    /// The model.
    Assistant,
    /// A tool result.
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

/// Admit a message and start or queue its run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageSendRequest {
    /// The parts.
    pub content: Vec<MessagePart>,
    /// Spend ceiling for the resulting run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_spend_cents: Option<Cents>,
}

/// The admitted message and the run it started.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageSendResult {
    /// The admitted message.
    pub message: Message,
    /// The queued or started run.
    pub run: Run,
}

/// What the guest may reach.
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

/// The package ecosystems a session may pre-install from.
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

/// One exactly-pinned package to pre-install.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PackageRequest {
    /// Which ecosystem.
    pub ecosystem: PackageEcosystem,
    /// The package name.
    pub name: String,
    /// The exact version; ranges are rejected.
    pub version: String,
}

/// A BYOK provider-credential binding. The key material never appears here.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderCredential {
    /// When it was registered.
    pub created_at: Timestamp,
    /// A one-way fingerprint of the key material.
    pub fingerprint: ContentHash,
    /// The stable binding identity.
    pub id: ProviderCredentialId,
    /// A human label.
    pub name: ResourceName,
    /// Which provider the credential is for.
    pub provider: ProviderId,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// When it was revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,
    /// Whether it is selectable.
    pub state: ProviderCredentialState,
    /// When it last changed.
    pub updated_at: Timestamp,
}

/// One page of provider-credential bindings.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderCredentialPage {
    /// The page.
    pub items: Vec<ProviderCredential>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Register a BYOK provider credential. This request carries plaintext.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderCredentialRegisterRequest {
    /// The provider key; write-only, never returned.
    pub api_key: String,
    /// A human label.
    pub name: ResourceName,
    /// Which provider the credential is for.
    pub provider: ProviderId,
}

/// Whether a BYOK binding may still be selected.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialState {
    /// Selectable.
    Ready,
    /// Revoked; sessions holding it fail closed.
    Revoked,
}

impl ProviderCredentialState {
    /// Every value, in declared order.
    pub const ALL: &'static [ProviderCredentialState] = &[
        ProviderCredentialState::Ready,
        ProviderCredentialState::Revoked,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Revoked => "revoked",
        }
    }
}

/// How a purge treats the session's lineage descendants.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PurgeCascade {
    /// Clear each descendant's origin link and leave it alive.
    DetachDescendants,
    /// Purge the whole lineage-descendant closure.
    PurgeClosure,
}

impl PurgeCascade {
    /// Every value, in declared order.
    pub const ALL: &'static [PurgeCascade] =
        &[PurgeCascade::DetachDescendants, PurgeCascade::PurgeClosure];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DetachDescendants => "detach_descendants",
            Self::PurgeClosure => "purge_closure",
        }
    }
}

/// A registered file entry, complete.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredFile {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Hash of the canonical value document, not of the payload. The payload's own hash is
    /// `value.content.sha256`.
    pub sha256: ContentHash,
    /// Size of the canonical value document in bytes, not of the payload.
    pub size_bytes: DecimalU128,
    /// Always current.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
    /// The complete value.
    pub value: RegisteredFileRead,
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
    /// Where it is mounted in the guest.
    pub mount_path: FilePath,
}

/// One registered file as a collection row. A listing carries no value document; read the entry
/// itself to get one.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredFileRow {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Hash of the canonical value document.
    pub sha256: ContentHash,
    /// Size of the canonical value document in bytes.
    pub size_bytes: DecimalU128,
    /// Always current.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
}

/// A registered file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredFileValue {
    /// The bytes.
    pub content: BlobInput,
    /// Declared media type.
    pub media_type: String,
    /// POSIX mode.
    pub mode: RegisteredFileMode,
    /// Where it is mounted in the guest.
    pub mount_path: FilePath,
}

/// A registered instruction entry, complete.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredInstruction {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Hash of the canonical value document.
    pub sha256: ContentHash,
    /// Size of the canonical value document in bytes.
    pub size_bytes: DecimalU128,
    /// Always current.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
    /// The complete value.
    pub value: RegisteredInstructionRead,
}

/// One page of registered instructions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredInstructionPage {
    /// The page.
    pub items: Vec<RegisteredInstructionRow>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// What a registered instruction is, as published. It has no payload.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredInstructionRead {
    /// The instruction text.
    pub text: String,
}

/// One registered instruction as a collection row.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredInstructionRow {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Hash of the canonical value document.
    pub sha256: ContentHash,
    /// Size of the canonical value document in bytes.
    pub size_bytes: DecimalU128,
    /// Always current.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
}

/// A registered instruction.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredInstructionValue {
    /// The instruction text.
    pub text: String,
}

/// A registered MCP server entry, complete.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredMcpServer {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Hash of the canonical value document.
    pub sha256: ContentHash,
    /// Size of the canonical value document in bytes.
    pub size_bytes: DecimalU128,
    /// Always current.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
    /// The complete value.
    pub value: RegisteredMcpServerRead,
}

/// One page of registered MCP servers.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredMcpServerPage {
    /// The page.
    pub items: Vec<RegisteredMcpServerRow>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// What a registered MCP server is, as published. It has no payload, and configuration never
/// contains a secret value.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredMcpServerRead {
    /// Secret-backed headers.
    pub headers: Vec<McpHeader>,
    /// The transport.
    pub transport: McpTransport,
    /// The server endpoint.
    pub url: HttpsUrl,
}

/// One registered MCP server as a collection row.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredMcpServerRow {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Hash of the canonical value document.
    pub sha256: ContentHash,
    /// Size of the canonical value document in bytes.
    pub size_bytes: DecimalU128,
    /// Always current.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
}

/// A registered MCP server. Configuration never contains a secret value.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredMcpServerValue {
    /// Secret-backed headers.
    pub headers: Vec<McpHeader>,
    /// The transport.
    pub transport: McpTransport,
    /// The server endpoint.
    pub url: HttpsUrl,
}

/// A registered skill entry, complete.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredSkill {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Hash of the canonical value document, not of the bundle. The bundle's own hash is
    /// `value.bundle.sha256`.
    pub sha256: ContentHash,
    /// Size of the canonical value document in bytes, not of the bundle.
    pub size_bytes: DecimalU128,
    /// Always current.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
    /// The complete value.
    pub value: RegisteredSkillRead,
}

/// One page of registered skills.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredSkillPage {
    /// The page.
    pub items: Vec<RegisteredSkillRow>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// What a registered skill is, as published.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredSkillRead {
    /// The stored bundle.
    pub bundle: ContentRef,
    /// The bundle format.
    pub bundle_format: BundleFormat,
    /// What the skill is for.
    pub description: String,
}

/// One registered skill as a collection row.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredSkillRow {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Hash of the canonical value document.
    pub sha256: ContentHash,
    /// Size of the canonical value document in bytes.
    pub size_bytes: DecimalU128,
    /// Always current.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
}

/// A registered skill bundle containing `SKILL.md`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredSkillValue {
    /// The bundle bytes.
    pub bundle: BlobInput,
    /// The bundle format.
    pub bundle_format: BundleFormat,
    /// What the skill is for.
    pub description: String,
}

/// The only state a registered entry has; there is no version history.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisteredState {
    /// The current replacement.
    Current,
}

impl RegisteredState {
    /// Every value, in declared order.
    pub const ALL: &'static [RegisteredState] = &[RegisteredState::Current];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
        }
    }
}

/// A registered tool entry, complete.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredTool {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Hash of the canonical value document, not of the bundle. The bundle's own hash is
    /// `value.bundle.sha256`.
    pub sha256: ContentHash,
    /// Size of the canonical value document in bytes, not of the bundle.
    pub size_bytes: DecimalU128,
    /// Always current.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
    /// The complete value.
    pub value: RegisteredToolRead,
}

/// One page of registered tools.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredToolPage {
    /// The page.
    pub items: Vec<RegisteredToolRow>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// What a registered tool is, as published.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredToolRead {
    /// The stored bundle.
    pub bundle: ContentRef,
    /// The bundle format.
    pub bundle_format: BundleFormat,
    /// What the tool does.
    pub description: String,
    /// Relative entry path inside the bundle.
    pub entry: String,
    /// JSON Schema 2020-12 for the arguments.
    pub input_schema: CanonicalJson,
}

/// One registered tool as a collection row.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredToolRow {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// Hash of the canonical value document.
    pub sha256: ContentHash,
    /// Size of the canonical value document in bytes.
    pub size_bytes: DecimalU128,
    /// Always current.
    pub state: RegisteredState,
    /// When last replaced.
    pub updated_at: Timestamp,
}

/// A registered custom tool.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredToolValue {
    /// The bundle bytes.
    pub bundle: BlobInput,
    /// The bundle format.
    pub bundle_format: BundleFormat,
    /// What the tool does.
    pub description: String,
    /// Relative entry path inside the bundle.
    pub entry: String,
    /// JSON Schema 2020-12 for the arguments.
    pub input_schema: CanonicalJson,
}

/// Mint a download grant for a registered file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegistryDownloadRequest {
    /// An explicit bounded range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<ByteRange>,
}

/// The complete provider-derived capacity of the selected shape.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolvedCompute {
    /// Guaranteed capacity.
    pub baseline: ComputeShape,
    /// Endpoint bandwidth.
    pub endpoint_bandwidth_m_bps: u32,
    /// Concurrent connection ceiling.
    pub max_concurrent_connections: u32,
    /// Maximum guest disk.
    pub max_disk_gi_b: u32,
    /// Burst ceiling.
    pub peak: ComputeShape,
    /// The selected baseline token.
    pub size: ComputeSize,
}

/// Everything the platform actually resolved for this session.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolvedConfig {
    /// Resolved approval policy.
    pub approval_policy: ApprovalPolicy,
    /// Resolved capacity.
    pub compute: ResolvedCompute,
    /// The exact provider-native model id.
    pub model: String,
    /// Resolved network policy.
    pub network: ResolvedNetwork,
    /// Resolved packages.
    pub packages: Vec<PackageRequest>,
    /// The provider.
    pub provider: ProviderId,
    /// The BYOK binding in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_credential_id: Option<ProviderCredentialId>,
    /// Resolved registered selection.
    pub registered: SessionRegisteredSelection,
}

/// The resolved guest network policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolvedNetwork {
    /// What the guest may reach.
    pub hands: HandsNetworkRequest,
}

/// One admitted turn of execution.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Run {
    /// The terminal failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RunFailure>,
    /// Identity.
    pub id: RunId,
    /// The effective spend ceiling.
    pub max_spend_cents: Cents,
    /// The message that admitted it.
    pub message_id: MessageId,
    /// Messages it produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_message_ids: Option<Vec<MessageId>>,
    /// When it was admitted.
    pub queued_at: Timestamp,
    /// The owning session.
    pub session_id: SessionId,
    /// When execution began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    /// Where it ended up.
    pub status: RunStatus,
    /// Whether the run has no recorded gaps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_complete: Option<bool>,
    /// Gaps recorded during the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_gap_ids: Option<Vec<TelemetryGapId>>,
    /// When it reached a terminal status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<Timestamp>,
}

/// A durable run failure with no synthetic request identity.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunFailure {
    /// The stable public failure code.
    pub code: ObservedErrorCode,
    /// Typed customer-safe detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<CanonicalJson>,
    /// Customer-safe failure detail.
    pub message: String,
    /// Whether the same run step may be retried.
    pub retryable: bool,
}

/// One page of runs.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunPage {
    /// The page.
    pub items: Vec<Run>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Where a run ended up.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Admitted, not started.
    Queued,
    /// Executing.
    Running,
    /// Completed normally.
    Succeeded,
    /// Terminal failure.
    Failed,
    /// Exceeded its wall bound.
    TimedOut,
    /// Stopped by an explicit operation.
    Cancelled,
    /// Lost its execution substrate.
    Interrupted,
}

impl RunStatus {
    /// Every value, in declared order.
    pub const ALL: &'static [RunStatus] = &[
        RunStatus::Queued,
        RunStatus::Running,
        RunStatus::Succeeded,
        RunStatus::Failed,
        RunStatus::TimedOut,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

/// Secret metadata. Values are write-only and never readable.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecretMetadata {
    /// When first written.
    pub created_at: Timestamp,
    /// The public identity.
    pub name: ResourceName,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// When it was revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,
    /// Whether it is admissible.
    pub state: SecretState,
    /// When last replaced.
    pub updated_at: Timestamp,
}

/// One page of secret metadata.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecretMetadataPage {
    /// The page.
    pub items: Vec<SecretMetadata>,
    /// Continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Set a secret value. The value is never returned by any read.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecretPutRequest {
    /// The secret value.
    pub value: String,
}

/// A reference to a workspace secret by name. Values never appear here.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecretRef {
    /// The secret name.
    pub name: ResourceName,
}

/// The receipt of a secret revocation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecretRevocation {
    /// The revoked secret.
    pub name: ResourceName,
    /// The revision after revocation.
    pub revision: u64,
    /// When revocation committed.
    pub revoked_at: Timestamp,
}

/// Whether a secret may still be admitted into custody.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretState {
    /// Admissible.
    Ready,
    /// Revoked; current custody was cancelled too.
    Revoked,
}

impl SecretState {
    /// Every value, in declared order.
    pub const ALL: &'static [SecretState] = &[SecretState::Ready, SecretState::Revoked];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Revoked => "revoked",
        }
    }
}

/// A durable conversation and the workspace it owns.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Session {
    /// What survives between runs.
    pub continuity: WorkspaceContinuity,
    /// When it was created.
    pub created_at: Timestamp,
    /// Identity.
    pub id: SessionId,
    /// Immutable provenance.
    pub lineage: SessionLineage,
    /// Caller-defined labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, MetadataValue>>,
    /// Everything the platform resolved.
    pub resolved_config: ResolvedConfig,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// What the session is doing.
    pub status: SessionStatus,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
}

/// The only customer compute selector.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionComputeRequest {
    /// The baseline shape; defaults to `1gb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ComputeSize>,
}

/// Create a session. The provider and model pair is explicit and required.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionCreateRequest {
    /// When to ask for a decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    /// The baseline compute shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute: Option<SessionComputeRequest>,
    /// Secrets to take custody of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<SessionCredentials>,
    /// Caller-defined labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, MetadataValue>>,
    /// The exact provider-native model id.
    pub model: String,
    /// The guest network policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<SessionNetworkRequest>,
    /// Packages to pre-install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packages: Option<Vec<PackageRequest>>,
    /// The direct BYOK provider.
    pub provider: ProviderId,
    /// Which BYOK binding to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_credential_id: Option<ProviderCredentialId>,
    /// Registered resources to mount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered: Option<SessionRegisteredSelection>,
}

/// Admit the durable credential-rebind operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionCredentialRebindRequest {
    /// The new custody set.
    pub secrets: Vec<SecretRef>,
}

/// Which workspace secrets the session may hold custody of.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionCredentials {
    /// The named secrets.
    pub secrets: Vec<SecretRef>,
}

/// Immutable provenance. A clone records its origin exactly once.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionLineage {
    /// The operation that cloned it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_operation_id: Option<OperationId>,
    /// The source revision cloned from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloned_at_persist_revision: Option<u64>,
    /// The session this was cloned from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_session_id: Option<SessionId>,
}

/// The collection projection of a session; `resolvedConfig` is omitted.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionListItem {
    /// When it was created.
    pub created_at: Timestamp,
    /// Identity.
    pub id: SessionId,
    /// The model.
    pub model: String,
    /// The provider.
    pub provider: ProviderId,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// What the session is doing.
    pub status: SessionStatus,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// The owning workspace.
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

/// Network policy for the session.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionNetworkRequest {
    /// The guest policy.
    pub hands: HandsNetworkRequest,
}

/// Admit the durable persist operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionPersistRequest {
    /// Selection globs to exclude.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
    /// Selection globs to include.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
}

/// The result of a persist operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionPersistResult {
    /// Entries added.
    pub added: DecimalU128,
    /// Bytes written to durable content.
    pub bytes_moved: DecimalU128,
    /// Whether the tree actually changed.
    pub changed: bool,
    /// Entries removed.
    pub deleted: DecimalU128,
    /// When the persist committed.
    pub last_persisted_at: Timestamp,
    /// The revision after the persist.
    pub persist_revision: u64,
    /// The persisted root.
    pub root_hash: ContentHash,
    /// The session.
    pub session_id: SessionId,
    /// Entries replaced.
    pub updated: DecimalU128,
}

/// Admit the durable purge operation. Purge is irreversible.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionPurgeRequest {
    /// How lineage descendants are treated.
    pub cascade: PurgeCascade,
}

/// Which registered resources the session mounts.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionRegisteredSelection {
    /// Registered files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<ResourceName>>,
    /// Registered instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Vec<ResourceName>>,
    /// Registered MCP servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<ResourceName>>,
    /// Registered skills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<ResourceName>>,
    /// Registered tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ResourceName>>,
}

/// The result of a restore operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionRestoreResult {
    /// When the restore committed.
    pub restored_at: Timestamp,
    /// The session.
    pub session_id: SessionId,
    /// The revision after the restore.
    pub session_revision: u64,
    /// What the session is doing now.
    pub status: SessionStatus,
}

/// What a session is doing right now.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// No run is in flight.
    Idle,
    /// A run is executing.
    Running,
    /// A bound tool call is waiting on a decision.
    AwaitingApproval,
    /// A deletion operation is running.
    Deleting,
}

impl SessionStatus {
    /// Every value, in declared order.
    pub const ALL: &'static [SessionStatus] = &[
        SessionStatus::Idle,
        SessionStatus::Running,
        SessionStatus::AwaitingApproval,
        SessionStatus::Deleting,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Deleting => "deleting",
        }
    }
}

/// The result of a stop operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionStopResult {
    /// Whether anything was actually stopped.
    pub changed: bool,
    /// The session.
    pub session_id: SessionId,
    /// The revision after the stop.
    pub session_revision: u64,
}

/// What remains of a deleted session; rendered at 410.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionTombstone {
    /// Whether descendants were deleted too.
    pub cascaded: bool,
    /// When deletion committed.
    pub deleted_at: Timestamp,
    /// The deleted session.
    pub id: SessionId,
    /// The operation that deleted it.
    pub operation_id: OperationId,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
}

/// The result of a trash operation. Trash starts the recovery window.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionTrashResult {
    /// After this instant restore is refused.
    pub recovery_deadline: Timestamp,
    /// The session.
    pub session_id: SessionId,
    /// The revision after the trash.
    pub session_revision: u64,
    /// When the trash committed.
    pub trashed_at: Timestamp,
}

/// Admit the durable workspace-discard operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionWorkspaceDiscardRequest {
    /// Only discard this exact generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_generation_id: Option<GenerationId>,
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

/// Complete a staged upload.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadCompleteRequest {
    /// Every written part.
    pub parts: Vec<UploadPart>,
}

/// Stage a large registered-resource value.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadCreateRequest {
    /// The media type.
    pub content_type: String,
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

/// The presigned grants for the requested parts.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadPartGrants {
    /// The grants.
    pub grants: Vec<UploadPartGrant>,
    /// The staged upload.
    pub upload_id: UploadId,
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

/// Mint presigned PUT grants for the named parts. At most 1000 parts per call against the
/// 10000-part ceiling, so a maximal upload needs at least ten calls.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadPartsRequest {
    /// The parts to grant.
    pub parts: Vec<UploadPartRequest>,
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

/// What survives between runs, and from when.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceContinuity {
    /// When the workspace was last persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_persisted_at: Option<Timestamp>,
    /// The live generation, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_generation_id: Option<GenerationId>,
    /// Monotonic persist revision.
    pub persist_revision: u64,
    /// The persisted workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_hash: Option<ContentHash>,
}

/// The result of a workspace-discard operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceDiscardResult {
    /// Whether a live workspace was actually discarded.
    pub changed: bool,
    /// Continuity after the discard.
    pub continuity: WorkspaceContinuity,
    /// The session.
    pub session_id: SessionId,
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
    /// The run, when attributable. Never populated by `usage_query`, as `sessionId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
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

/// Query parameters of `account_get`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AccountGetQuery {
    /// Which organization's account to read. Required: a user can belong to many organizations, so
    /// a rule that derived it from the credential would silently change meaning the day a user
    /// joins a second one.
    pub organization_id: OrganizationId,
}

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

/// Query parameters of `billing_balance_get`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingBalanceGetQuery {
    /// Required unless the credential derives one organization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<OrganizationId>,
}

/// Query parameters of `billing_statements_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BillingStatementsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `central_operations_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CentralOperationsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Restrict to one operation kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<OperationKind>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The organization whose operations are listed.
    pub organization_id: OrganizationId,
    /// Restrict to one status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<OperationStatus>,
}

/// Query parameters of `memberships_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MembershipsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `organizations_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OrganizationsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `provider_credentials_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderCredentialsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Restrict to one provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderId>,
}

/// Query parameters of `regional_operations_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegionalOperationsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Restrict to one operation kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<OperationKind>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Restrict to one session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Restrict to one status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<OperationStatus>,
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

/// Query parameters of `registry_instructions_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegistryInstructionsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `registry_mcp_servers_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegistryMcpServersListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `registry_skills_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegistrySkillsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `registry_tools_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegistryToolsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `secrets_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecretsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Query parameters of `session_approvals_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionApprovalsListQuery {
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

/// Query parameters of `session_runs_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionRunsListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
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

/// Query parameters of `usage_query`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageQueryQuery {
    /// Required when an account token selects the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
}

/// Query parameters of `workspaces_list`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspacesListQuery {
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Restrict to one organization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<OrganizationId>,
}
