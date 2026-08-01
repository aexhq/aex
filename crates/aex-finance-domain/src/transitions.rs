//! Canonical balanced journal transitions.

use aex_wire::ids::OrganizationId;
use time::OffsetDateTime;

use crate::account::AccountRef;
use crate::journal::{
    BalancedTransaction, BusinessKey, ConservationError, IntentHash, Posting, TransactionId,
    TransactionKind,
};
use crate::money::Microusd;

/// Common immutable identity for a transition.
pub struct TransitionIdentity {
    /// New journal id.
    pub id: TransactionId,
    /// Organization when customer accounts are involved.
    pub organization: Option<OrganizationId>,
    /// Durable idempotency key.
    pub business_key: BusinessKey,
    /// Canonical intent digest.
    pub intent_hash: IntentHash,
    /// Effective time supplied by the application clock.
    pub occurred_at: OffsetDateTime,
}

/// Posts a two-sided transition of one amount.
///
/// # Errors
/// Returns a conservation error if both accounts are identical or the amount is zero.
pub fn two_sided(
    identity: TransitionIdentity,
    kind: TransactionKind,
    debit: AccountRef,
    credit: AccountRef,
    amount: Microusd,
) -> Result<BalancedTransaction, ConservationError> {
    BalancedTransaction::try_new(
        identity.id,
        kind,
        identity.organization,
        identity.business_key,
        identity.intent_hash,
        identity.occurred_at,
        vec![
            Posting {
                account: debit,
                amount: amount.as_delta(),
            },
            Posting {
                account: credit,
                amount: amount.as_delta().negate(),
            },
        ],
    )
}
