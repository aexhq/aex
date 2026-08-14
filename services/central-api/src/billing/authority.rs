//! The ports `finance-api` composes, and the plain records they carry.
//!
//! Money crosses these ports as integer micro-USD and nothing else. There is no
//! `f64` in any signature here, and there is deliberately no conversion helper
//! that would let one appear: the only widening a caller may perform is
//! [`aex_finance_domain::Microusd::to_cents_exact`], which refuses a sub-cent
//! remainder rather than rounding it away.

use aex_control_domain::AccountProfile;
use aex_finance_domain::money::Microusd;
use aex_payment_contracts::{
    EffectId, PaymentCommandEnvelope, PaymentResult, ProviderCustomerRef, ProviderMethodRef,
    RedactedEmail,
};
use aex_wire::ids::OrganizationId;

/// Customer-safe metadata for one attached card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentMethodRecord {
    /// Aex-owned opaque identity.
    pub id: uuid::Uuid,
    /// Stripe's opaque method reference, used only for provider commands.
    pub provider_method: ProviderMethodRef,
    /// Stable lower-case network spelling.
    pub brand: String,
    /// Display-only final four digits.
    pub last4: String,
    /// Expiry month.
    pub expiry_month: u32,
    /// Four-digit expiry year.
    pub expiry_year: u32,
    /// Whether this is the current default.
    pub is_default: bool,
    /// Verified provider creation time, in epoch milliseconds.
    pub created_at_millis: i64,
}

/// One customer-visible immutable ledger transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRecord {
    /// Ledger identity.
    pub id: uuid::Uuid,
    /// Durable transaction kind.
    pub kind: String,
    /// Signed change to customer-available credit, in micro-USD.
    pub available_delta: i64,
    /// Available credit after this transaction, in micro-USD.
    pub balance_after: i64,
    /// Optional compensated transaction.
    pub reverses: Option<uuid::Uuid>,
    /// Session attribution when the business key carries one.
    pub session_id: Option<String>,
    /// Authoritative occurrence time, in epoch milliseconds.
    pub occurred_at_millis: i64,
}

/// A newest-first ledger page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionPageRecord {
    /// Page contents.
    pub items: Vec<TransactionRecord>,
    /// Last transaction identity when another page exists.
    pub next: Option<uuid::Uuid>,
}

/// Filters for a rated-usage read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageQuery<'a> {
    /// Continue after this accepted sequence.
    pub after_sequence: Option<i64>,
    /// Optional category spelling.
    pub category: Option<&'a str>,
    /// Optional session identity.
    pub session_id: Option<&'a str>,
    /// Inclusive service-time lower bound.
    pub from_millis: Option<i64>,
    /// Exclusive service-time upper bound.
    pub to_millis: Option<i64>,
    /// Bounded page size.
    pub limit: u32,
}

/// One rated usage row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    /// Monotonic regional acceptance position.
    pub accepted_sequence: i64,
    /// Launch category.
    pub category: String,
    /// Stable meter/unit.
    pub meter: String,
    /// Measured canonical integer quantity.
    pub quantity: u128,
    /// Quantity charged after policy.
    pub charged_quantity: u128,
    /// Rated amount in micro-USD.
    pub rated: Microusd,
    /// Whether an immutable transaction covers the row.
    pub settled: bool,
    /// Optional official provider spelling.
    pub provider: Option<String>,
    /// Optional provider model identifier.
    pub model: Option<String>,
    /// Optional session attribution.
    pub session_id: Option<String>,
    /// Service interval start.
    pub from_millis: i64,
    /// Service interval end.
    pub to_millis: i64,
}

/// Completeness evidence for a usage page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsagePageRecord {
    /// Page contents.
    pub items: Vec<UsageRecord>,
    /// Last sequence when another page exists.
    pub next_sequence: Option<i64>,
    /// All accepted facts at or before this instant are represented.
    pub complete_through_millis: i64,
    /// No accepted fact after this instant is represented.
    pub includes_through_millis: i64,
    /// Whether quarantine or a missing sequence prevents complete settlement.
    pub has_gap: bool,
}

/// The prepaid position of one organization, as the projection records it.
///
/// It carries money and nothing else. The account's operational state is a
/// separate read of a separate authority — see [`BillingAuthority::account_profile`]
/// — because a state derived here from a durable column and the live balance is a
/// second producer of a fact the control plane already publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceRecord {
    /// Spendable now.
    pub available: Microusd,
    /// Fenced by open reservations.
    pub reserved: Microusd,
    /// Rated but not yet settled.
    pub pending: Microusd,
    /// Monotonic projection revision.
    pub revision: u64,
    /// When the projection last moved, in epoch milliseconds.
    pub updated_at_millis: i64,
}

/// The durable automatic top-up policy of one organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRecord {
    /// Whether automatic recharge runs.
    pub enabled: bool,
    /// Balance at or below which a recharge is attempted.
    pub threshold: Microusd,
    /// How much each recharge adds.
    pub amount: Microusd,
    /// Whether a reusable payment method exists.
    pub has_payment_method: bool,
    /// Monotonic policy revision, which is also the entity tag.
    pub revision: u64,
    /// When the policy last changed, in epoch milliseconds.
    pub updated_at_millis: i64,
}

/// A requested policy replacement, already converted to micro-USD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyChange {
    /// Whether automatic recharge runs.
    pub enabled: bool,
    /// Balance at or below which a recharge is attempted.
    pub threshold: Microusd,
    /// How much each recharge adds.
    pub amount: Microusd,
}

/// One issued statement header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementHeader {
    /// Identity.
    pub statement_id: uuid::Uuid,
    /// The billed calendar month, `YYYY-MM`.
    pub period: String,
    /// The issued total.
    pub closing: Microusd,
    /// The artifact digest, present once the statement is issued.
    pub content_sha256: Option<[u8; 32]>,
    /// The artifact object key, present once the statement is issued.
    pub object_key: Option<String>,
    /// When it was issued, in epoch milliseconds.
    pub issued_at_millis: i64,
}

/// One priced category line of an issued statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementLineRecord {
    /// The priced category, exactly as the usage inbox spells it.
    pub category: String,
    /// The line total.
    pub total: Microusd,
}

/// A page of statement headers with its continuation position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementPage {
    /// The page, newest first.
    pub items: Vec<StatementHeader>,
    /// The period to continue after, when more remain.
    pub next_period: Option<String>,
}

/// Everything the caller needs to prepare one provider effect.
///
/// The provider customer is deliberately not here. `EnsureCustomer` is the one
/// command whose whole purpose is that no customer exists yet, so a preparation
/// that demanded one could never prepare it — which is exactly why nothing ever
/// constructed the command and why `provider_customer_id` had no writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectPreparation {
    /// The effect identity, which is also the provider idempotency key input.
    pub effect: EffectId,
    /// The account the effect belongs to.
    pub organization: OrganizationId,
    /// The canonical intent digest committed with the effect.
    pub intent_hash: [u8; 32],
}

/// Why an authority call did not answer.
///
/// `Unavailable` and `OutcomeUnknown` are deliberately distinct. The first says
/// nothing happened; the second says something may have. A money path that
/// collapses them charges twice.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthorityError {
    /// The organization has no finance record.
    #[error("organization has no billing account")]
    UnknownOrganization,
    /// The named resource does not exist.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// A revision precondition did not hold.
    #[error("revision precondition failed")]
    RevisionConflict,
    /// The request contradicts durable state.
    #[error("{0}")]
    Refused(String),
    /// An unresolved effect of the same kind already exists.
    #[error("an unresolved provider effect already exists for this organization")]
    EffectAlreadyOpen,
    /// The authority is unreachable. Nothing was applied.
    #[error("the finance authority is unavailable: {0}")]
    Unavailable(String),
    /// The commit response was lost. The transition may or may not be durable.
    #[error("the finance commit outcome is unknown: {0}")]
    OutcomeUnknown(String),
    /// The stored row does not match what this deployable projects.
    #[error("the finance authority returned an undecodable row: {0}")]
    Decode(String),
}

/// The reads and money transitions `finance-api` performs.
///
/// One trait rather than eight so the composition root wires exactly one
/// adapter, and so a test double is one type rather than a matrix of them.
#[async_trait::async_trait]
pub trait BillingAuthority: Send + Sync + 'static {
    /// Proves this deployable can reach the database **as its own role**.
    ///
    /// Readiness is not "the process booted"; it is "my grants are real". A
    /// deployable that cannot prove its own grants must not take traffic.
    async fn probe_role(&self) -> Result<(), AuthorityError>;

    /// Reads the prepaid position of one organization.
    async fn balance(&self, organization: OrganizationId) -> Result<BalanceRecord, AuthorityError>;

    /// Lists verified card display metadata, newest first.
    async fn payment_methods(
        &self,
        organization: OrganizationId,
    ) -> Result<Vec<PaymentMethodRecord>, AuthorityError>;

    /// Resolves one account-owned card for a detach command.
    async fn payment_method(
        &self,
        organization: OrganizationId,
        payment_method: uuid::Uuid,
    ) -> Result<PaymentMethodRecord, AuthorityError>;

    /// Lists immutable customer-visible ledger transactions.
    async fn transactions(
        &self,
        organization: OrganizationId,
        after: Option<uuid::Uuid>,
        limit: u32,
    ) -> Result<TransactionPageRecord, AuthorityError>;

    /// Reads bounded rated usage together with an explicit coverage boundary.
    async fn usage(
        &self,
        organization: OrganizationId,
        query: UsageQuery<'_>,
    ) -> Result<UsagePageRecord, AuthorityError>;

    /// Reads the published account state of one organization.
    ///
    /// This is `finance.account_state_v1` — the projection finance's own pause
    /// and resume trigger writes — and it is the sole input to the operational
    /// state this service publishes. The state can therefore lag the balance
    /// printed beside it by one trigger, which is the correct semantics: the
    /// pause is the trigger's fact.
    async fn account_profile(
        &self,
        organization: OrganizationId,
    ) -> Result<AccountProfile, AuthorityError>;

    /// Reads the automatic top-up policy of one organization.
    async fn policy(&self, organization: OrganizationId) -> Result<PolicyRecord, AuthorityError>;

    /// Replaces the automatic top-up policy under a revision precondition.
    async fn replace_policy(
        &self,
        organization: OrganizationId,
        change: PolicyChange,
        expect_revision: u64,
    ) -> Result<PolicyRecord, AuthorityError>;

    /// Reads one page of issued statement headers, newest first.
    async fn statements(
        &self,
        organization: OrganizationId,
        before_period: Option<&str>,
        limit: u32,
    ) -> Result<StatementPage, AuthorityError>;

    /// Reads one issued statement header.
    async fn statement(
        &self,
        organization: OrganizationId,
        statement_id: uuid::Uuid,
    ) -> Result<StatementHeader, AuthorityError>;

    /// Reads the priced category lines of one issued statement.
    async fn statement_lines(
        &self,
        organization: OrganizationId,
        period: &str,
    ) -> Result<Vec<StatementLineRecord>, AuthorityError>;

    /// Commits a `prepared` provider effect **before** any provider call.
    ///
    /// The effect identity is minted here and never by the caller, so a retry
    /// of the same intent resolves the original effect instead of opening a
    /// second one.
    async fn prepare_effect(
        &self,
        organization: OrganizationId,
        kind: aex_payment_contracts::CommandKind,
        intent: &[u8],
        amount: Option<Microusd>,
        deadline_millis: i64,
    ) -> Result<EffectPreparation, AuthorityError>;

    /// The organization's provider customer, when one has been created.
    async fn provider_customer(
        &self,
        organization: OrganizationId,
    ) -> Result<Option<ProviderCustomerRef>, AuthorityError>;

    /// The address a provider customer record is created against.
    ///
    /// Finance stores no address of its own; this reaches identity through the
    /// one `SECURITY DEFINER` function it holds `EXECUTE` on.
    async fn billing_contact(
        &self,
        organization: OrganizationId,
    ) -> Result<RedactedEmail, AuthorityError>;

    /// Closes an `EnsureCustomer` effect and records what it created, atomically.
    ///
    /// One commit rather than two, so no window exists in which the effect reads
    /// `succeeded` and the organization still has no customer. Answers `None`
    /// when the provider did not succeed; the effect is finalized either way.
    async fn settle_customer(
        &self,
        effect: EffectId,
        organization: OrganizationId,
        result: &PaymentResult,
    ) -> Result<Option<ProviderCustomerRef>, AuthorityError>;

    /// Binds the complete admitted command to its already-durable effect.
    ///
    /// The provider is never contacted until the exact envelope and
    /// idempotency key it received can be replayed after a lost response.
    async fn bind_effect_command(
        &self,
        effect: EffectId,
        intent_hash: [u8; 32],
        envelope: &PaymentCommandEnvelope,
    ) -> Result<(), AuthorityError>;

    /// Records the provider's answer to a dispatched effect.
    ///
    /// An indeterminate answer is recorded as `outcome_unknown`; this call has
    /// no arm that turns one into a determinate failure.
    async fn finalize_effect(
        &self,
        effect: EffectId,
        result: &PaymentResult,
    ) -> Result<(), AuthorityError>;
}

/// The one path from Rust to Stripe: a synchronous invoke of the command edge.
#[async_trait::async_trait]
pub trait PaymentGateway: Send + Sync + 'static {
    /// Executes one admitted command and reports what the provider did.
    ///
    /// # Errors
    ///
    /// Returns [`GatewayError`] only when the command could not be handed to
    /// the edge at all. Everything the provider did — including a 5xx — comes
    /// back as a [`PaymentResult`], because only the edge can tell a decline
    /// from an indeterminate failure.
    async fn execute(
        &self,
        envelope: &PaymentCommandEnvelope,
    ) -> Result<PaymentResult, GatewayError>;
}

/// Why a payment command could not be executed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GatewayError {
    /// The edge could not be invoked. The command was not started.
    #[error("the payment command edge is unavailable: {0}")]
    Unavailable(String),
    /// The invoke may or may not have reached the edge.
    #[error("the payment command outcome is unknown: {0}")]
    OutcomeUnknown(String),
    /// The edge answered with something this contract does not admit.
    #[error("the payment command edge answered off-contract: {0}")]
    OffContract(String),
}
