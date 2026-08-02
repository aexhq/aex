//! The balanced transitions this deployable is allowed to post.
//!
//! Conservation is proved in Rust before a statement runs: every write is built
//! through [`aex_finance_domain::BalancedTransaction::try_new`], which cannot
//! construct an unbalanced value. The deferrable database trigger is the second
//! defence, not the first.

use aex_finance_domain::account::{AccountKind, AccountRef};
use aex_finance_domain::journal::{
    BalancedTransaction, BusinessKey, ConservationError, IntentHash, Posting, TransactionId,
    TransactionKind,
};
use aex_finance_domain::money::Microusd;
use aex_wire::ids::OrganizationId;
use time::OffsetDateTime;
use uuid::Uuid;

/// The seeded platform account identities, from `20260801000600`.
pub mod platform {
    use uuid::{Uuid, uuid};

    /// Funds accepted by the provider and not yet net-settled.
    pub const PROVIDER_CLEARING: Uuid = uuid!("00000000-0000-7000-8000-000000000001");
    /// Provider processing fees.
    pub const PROCESSOR_FEE_EXPENSE: Uuid = uuid!("00000000-0000-7000-8000-000000000002");
    /// Earned infrastructure revenue.
    pub const USAGE_REVENUE: Uuid = uuid!("00000000-0000-7000-8000-000000000003");
    /// Tax collected for remittance.
    pub const TAX_PAYABLE: Uuid = uuid!("00000000-0000-7000-8000-000000000004");
    /// Refunds in flight to the card network.
    pub const REFUND_CLEARING: Uuid = uuid!("00000000-0000-7000-8000-000000000005");
    /// Disputed funds whose outcome is open.
    pub const DISPUTE_HOLDING: Uuid = uuid!("00000000-0000-7000-8000-000000000006");
    /// A disputed amount with no customer credit behind it.
    pub const DISPUTE_LOSS_EXPENSE: Uuid = uuid!("00000000-0000-7000-8000-000000000007");
    /// Operator-granted credit.
    pub const GOODWILL_EXPENSE: Uuid = uuid!("00000000-0000-7000-8000-000000000008");
}

/// One transaction, proved balanced, with the identity the SQL needs.
///
/// `BalancedTransaction` keeps its identity private, so this record carries the
/// same parts alongside it rather than reaching into the domain type. The
/// domain value is still the thing that decides whether the write may exist.
#[derive(Debug, Clone)]
pub struct JournalWrite {
    /// The journal identity, a `UUIDv7`.
    pub id: TransactionId,
    /// Which canonical transition this is.
    pub kind: TransactionKind,
    /// The account the money belongs to.
    pub organization: Option<OrganizationId>,
    /// The durable idempotency key.
    pub business_key: BusinessKey,
    /// The canonical intent digest.
    pub intent_hash: IntentHash,
    /// The effective instant.
    pub occurred_at: OffsetDateTime,
    /// The debit-positive postings, in order.
    pub postings: Vec<Posting>,
}

impl JournalWrite {
    /// Builds a write only when every conservation invariant holds.
    ///
    /// # Errors
    ///
    /// Returns the exact invariant the postings violate.
    pub fn try_new(
        id: TransactionId,
        kind: TransactionKind,
        organization: Option<OrganizationId>,
        business_key: BusinessKey,
        intent_hash: IntentHash,
        occurred_at: OffsetDateTime,
        postings: Vec<Posting>,
    ) -> Result<Self, ConservationError> {
        // Constructed for its invariants: an unbalanced transaction cannot exist
        // as a value, so it cannot reach a statement either.
        let proved = BalancedTransaction::try_new(
            id,
            kind,
            organization,
            business_key.clone(),
            intent_hash,
            occurred_at,
            postings,
        )?;
        Ok(Self {
            id,
            kind,
            organization,
            business_key,
            intent_hash,
            occurred_at,
            postings: proved.postings().to_vec(),
        })
    }

    /// How many postings the header declares.
    #[must_use]
    pub fn posting_count(&self) -> i64 {
        i64::try_from(self.postings.len()).unwrap_or(i64::MAX)
    }
}

/// The durable spelling of a transition kind.
#[must_use]
pub const fn kind_sql(kind: TransactionKind) -> &'static str {
    match kind {
        TransactionKind::TopUpSettled => "top_up_settled",
        TransactionKind::ProcessorFee => "processor_fee",
        TransactionKind::UsageReserve => "usage_reserve",
        TransactionKind::UsageSettlement => "usage_settlement",
        TransactionKind::ReservationRelease => "reservation_release",
        TransactionKind::Refund => "refund",
        TransactionKind::DisputeOpened => "dispute_opened",
        TransactionKind::DisputeClosed => "dispute_closed",
        TransactionKind::GoodwillCredit => "goodwill_credit",
        TransactionKind::TaxAdjustment => "tax_adjustment",
        TransactionKind::Reversal => "reversal",
    }
}

/// The deterministic per-organization account identity for `kind`.
///
/// Derived from the organization and the account kind rather than minted, so a
/// concurrent creation of the same account converges on one row instead of
/// racing to two.
#[must_use]
pub fn customer_account(organization: OrganizationId, kind: AccountKind) -> AccountRef {
    use aex_wire::ids::PrefixedId as _;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"aex:finance:account:");
    hasher.update(organization.encode().as_str().as_bytes());
    hasher.update(b":");
    hasher.update(account_kind_sql(kind).as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Stamp the UUID version and variant so the value is a well-formed v7-shaped
    // identifier rather than raw digest bytes.
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    AccountRef::new(Uuid::from_bytes(bytes), kind)
}

/// The durable spelling of an account kind.
#[must_use]
pub const fn account_kind_sql(kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::ProviderClearing => "provider_clearing",
        AccountKind::ProcessorFeeExpense => "processor_fee_expense",
        AccountKind::CustomerAvailable => "customer_available",
        AccountKind::CustomerReserved => "customer_reserved",
        AccountKind::UsageRevenue => "usage_revenue",
        AccountKind::TaxPayable => "tax_payable",
        AccountKind::RefundClearing => "refund_clearing",
        AccountKind::DisputeHolding => "dispute_holding",
        AccountKind::DisputeLossExpense => "dispute_loss_expense",
        AccountKind::GoodwillExpense => "goodwill_expense",
    }
}

/// `top_up_settled`: DR provider clearing gross, CR customer available net,
/// CR tax payable tax.
///
/// # Errors
///
/// Returns the violated conservation invariant, which for this transition means
/// the net and tax parts do not add up to the gross.
pub fn top_up_settled(
    id: TransactionId,
    organization: OrganizationId,
    business_key: BusinessKey,
    intent_hash: IntentHash,
    occurred_at: OffsetDateTime,
    net: Microusd,
    tax: Microusd,
) -> Result<JournalWrite, ConservationError> {
    let gross = net
        .checked_add(tax)
        .map_err(|_| ConservationError::Overflow)?;
    let mut postings = vec![
        Posting {
            account: AccountRef::new(platform::PROVIDER_CLEARING, AccountKind::ProviderClearing),
            amount: gross.as_delta(),
        },
        Posting {
            account: customer_account(organization, AccountKind::CustomerAvailable),
            amount: net.as_delta().negate(),
        },
    ];
    if tax.get() > 0 {
        postings.push(Posting {
            account: AccountRef::new(platform::TAX_PAYABLE, AccountKind::TaxPayable),
            amount: tax.as_delta().negate(),
        });
    }
    JournalWrite::try_new(
        id,
        TransactionKind::TopUpSettled,
        Some(organization),
        business_key,
        intent_hash,
        occurred_at,
        postings,
    )
}

/// `refund`: DR customer available net, DR tax payable tax, CR refund clearing.
///
/// # Errors
///
/// Returns the violated conservation invariant.
pub fn refund(
    id: TransactionId,
    organization: OrganizationId,
    business_key: BusinessKey,
    intent_hash: IntentHash,
    occurred_at: OffsetDateTime,
    net: Microusd,
    tax: Microusd,
) -> Result<JournalWrite, ConservationError> {
    let gross = net
        .checked_add(tax)
        .map_err(|_| ConservationError::Overflow)?;
    let mut postings = vec![
        Posting {
            account: customer_account(organization, AccountKind::CustomerAvailable),
            amount: net.as_delta(),
        },
        Posting {
            account: AccountRef::new(platform::REFUND_CLEARING, AccountKind::RefundClearing),
            amount: gross.as_delta().negate(),
        },
    ];
    if tax.get() > 0 {
        postings.push(Posting {
            account: AccountRef::new(platform::TAX_PAYABLE, AccountKind::TaxPayable),
            amount: tax.as_delta(),
        });
    }
    JournalWrite::try_new(
        id,
        TransactionKind::Refund,
        Some(organization),
        business_key,
        intent_hash,
        occurred_at,
        postings,
    )
}

/// `dispute_opened`: DR customer available for what credit remains, DR dispute
/// loss expense for the remainder, CR provider clearing for the disputed total.
///
/// # Errors
///
/// Returns the violated conservation invariant.
pub fn dispute_opened(
    id: TransactionId,
    organization: OrganizationId,
    business_key: BusinessKey,
    intent_hash: IntentHash,
    occurred_at: OffsetDateTime,
    disputed: Microusd,
    available: Microusd,
) -> Result<JournalWrite, ConservationError> {
    let clawed_back = Microusd::new(disputed.get().min(available.get()))
        .map_err(|_| ConservationError::Overflow)?;
    let remainder = disputed
        .checked_sub(clawed_back)
        .map_err(|_| ConservationError::Overflow)?;
    let mut postings = vec![Posting {
        account: AccountRef::new(platform::PROVIDER_CLEARING, AccountKind::ProviderClearing),
        amount: disputed.as_delta().negate(),
    }];
    if clawed_back.get() > 0 {
        postings.push(Posting {
            account: customer_account(organization, AccountKind::CustomerAvailable),
            amount: clawed_back.as_delta(),
        });
    }
    if remainder.get() > 0 {
        postings.push(Posting {
            account: AccountRef::new(
                platform::DISPUTE_LOSS_EXPENSE,
                AccountKind::DisputeLossExpense,
            ),
            amount: remainder.as_delta(),
        });
    }
    JournalWrite::try_new(
        id,
        TransactionKind::DisputeOpened,
        Some(organization),
        business_key,
        intent_hash,
        occurred_at,
        postings,
    )
}

#[cfg(test)]
mod tests {
    use aex_finance_domain::account::AccountKind;
    use aex_finance_domain::journal::{BusinessKey, IntentHash, TransactionId, TransactionKind};
    use aex_finance_domain::money::Microusd;
    use aex_wire::PrefixedId as _;
    use aex_wire::ids::OrganizationId;

    use super::{customer_account, dispute_opened, kind_sql, refund, top_up_settled};

    fn organization() -> OrganizationId {
        OrganizationId::parse("org_01kyw2qa4pew48j2gb1g6gw3rg").expect("a fixture organization")
    }

    fn identity() -> (TransactionId, BusinessKey, IntentHash) {
        (
            TransactionId::from_uuid(uuid::Uuid::now_v7()),
            BusinessKey::parse("topup:pi_abcdefghijklmn").expect("a business key"),
            IntentHash::new([7u8; 32]),
        )
    }

    #[test]
    fn a_top_up_conserves_gross_against_net_plus_tax() {
        let (id, key, hash) = identity();
        let write = top_up_settled(
            id,
            organization(),
            key,
            hash,
            time::OffsetDateTime::UNIX_EPOCH,
            Microusd::new(9_000_000).expect("net"),
            Microusd::new(1_000_000).expect("tax"),
        )
        .expect("the transition balances");
        assert_eq!(write.posting_count(), 3);
        let sum: i64 = write
            .postings
            .iter()
            .map(|posting| posting.amount.get())
            .sum();
        assert_eq!(sum, 0, "every transaction sums to zero");
    }

    #[test]
    fn a_zero_tax_top_up_posts_two_sides_rather_than_a_zero_posting() {
        let (id, key, hash) = identity();
        let write = top_up_settled(
            id,
            organization(),
            key,
            hash,
            time::OffsetDateTime::UNIX_EPOCH,
            Microusd::new(10_000_000).expect("net"),
            Microusd::ZERO,
        )
        .expect("the transition balances");
        assert_eq!(write.posting_count(), 2);
        assert!(
            write
                .postings
                .iter()
                .all(|posting| posting.amount.get() != 0)
        );
    }

    #[test]
    fn a_refund_and_a_top_up_of_the_same_amount_are_exact_opposites_on_the_customer() {
        let (id, key, hash) = identity();
        let credit = top_up_settled(
            id,
            organization(),
            key.clone(),
            hash,
            time::OffsetDateTime::UNIX_EPOCH,
            Microusd::new(5_000_000).expect("net"),
            Microusd::ZERO,
        )
        .expect("balanced");
        let debit = refund(
            id,
            organization(),
            key,
            hash,
            time::OffsetDateTime::UNIX_EPOCH,
            Microusd::new(5_000_000).expect("net"),
            Microusd::ZERO,
        )
        .expect("balanced");
        let account = customer_account(organization(), AccountKind::CustomerAvailable);
        let net = |write: &super::JournalWrite| {
            write
                .postings
                .iter()
                .filter(|posting| posting.account == account)
                .map(|posting| posting.amount.get())
                .sum::<i64>()
        };
        assert_eq!(net(&credit) + net(&debit), 0);
    }

    #[test]
    fn a_dispute_larger_than_the_balance_splits_into_a_claw_back_and_a_loss() {
        let (id, key, hash) = identity();
        let write = dispute_opened(
            id,
            organization(),
            key,
            hash,
            time::OffsetDateTime::UNIX_EPOCH,
            Microusd::new(10_000_000).expect("disputed"),
            Microusd::new(4_000_000).expect("available"),
        )
        .expect("the transition balances");
        assert_eq!(write.posting_count(), 3);
        let sum: i64 = write
            .postings
            .iter()
            .map(|posting| posting.amount.get())
            .sum();
        assert_eq!(sum, 0);
    }

    #[test]
    fn a_customer_account_identity_is_derived_and_therefore_stable() {
        let first = customer_account(organization(), AccountKind::CustomerAvailable);
        let second = customer_account(organization(), AccountKind::CustomerAvailable);
        let reserved = customer_account(organization(), AccountKind::CustomerReserved);
        assert_eq!(first, second, "the same account resolves to the same row");
        assert_ne!(
            first.id(),
            reserved.id(),
            "available and reserved are different accounts"
        );
        assert_eq!(first.id().get_version_num(), 7);
    }

    #[test]
    fn every_transition_kind_has_exactly_one_durable_spelling() {
        for kind in [
            TransactionKind::TopUpSettled,
            TransactionKind::ProcessorFee,
            TransactionKind::UsageReserve,
            TransactionKind::UsageSettlement,
            TransactionKind::ReservationRelease,
            TransactionKind::Refund,
            TransactionKind::DisputeOpened,
            TransactionKind::DisputeClosed,
            TransactionKind::GoodwillCredit,
            TransactionKind::TaxAdjustment,
            TransactionKind::Reversal,
        ] {
            let spelling = kind_sql(kind);
            assert!(
                spelling
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            );
        }
    }

    #[test]
    fn the_organization_fixture_encodes_back_to_itself() {
        assert!(organization().encode().as_str().starts_with("org_"));
    }
}
