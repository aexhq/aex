//! Immutable balanced double-entry transactions.

use std::collections::BTreeSet;

use aex_wire::canonical::to_jcs_bytes;
use aex_wire::ids::OrganizationId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::account::AccountRef;
use crate::money::MicrousdDelta;

/// Stable journal transaction identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransactionId(Uuid);

impl TransactionId {
    /// Wraps a `UUIDv7` minted by the application layer.
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// Raw UUID.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl std::fmt::Display for TransactionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Closed set of journal transition kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
    /// Provider top-up completed.
    TopUpSettled,
    /// Provider processing fee recorded.
    ProcessorFee,
    /// Credit moved into reservation.
    UsageReserve,
    /// Reserved credit earned as revenue.
    UsageSettlement,
    /// Unused reservation returned.
    ReservationRelease,
    /// Customer refund initiated.
    Refund,
    /// Dispute opened.
    DisputeOpened,
    /// Dispute won and reversed.
    DisputeClosed,
    /// Operator goodwill credit.
    GoodwillCredit,
    /// Tax correction.
    TaxAdjustment,
    /// Exact reversal of another transaction.
    Reversal,
}

/// Canonical, lowercase idempotency key for a journal transaction.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BusinessKey(String);

impl BusinessKey {
    /// Parses the finance business-key grammar.
    ///
    /// # Errors
    /// Rejects length, uppercase, whitespace and non-ASCII input.
    pub fn parse(text: &str) -> Result<Self, BusinessKeyError> {
        if !(8..=512).contains(&text.len()) {
            return Err(BusinessKeyError::Length);
        }
        if !text.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b':' | b'_' | b'-' | b'.')
        }) || !text.contains(':')
        {
            return Err(BusinessKeyError::Grammar);
        }
        Ok(Self(text.to_owned()))
    }

    /// Canonical text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a business key was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BusinessKeyError {
    /// Not between 8 and 512 bytes.
    #[error("business key must be 8..=512 bytes")]
    Length,
    /// Not lowercase ASCII colon-separated text.
    #[error("business key has invalid grammar")]
    Grammar,
}

/// Digest of canonical intent JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IntentHash([u8; 32]);

impl IntentHash {
    /// Wraps a verified digest.
    #[must_use]
    pub const fn new(value: [u8; 32]) -> Self {
        Self(value)
    }

    /// Hashes the workspace's sole RFC 8785 representation.
    ///
    /// # Errors
    /// Returns canonicalization errors without a fallback encoding.
    pub fn from_canonical<T: Serialize>(
        value: &T,
    ) -> Result<Self, aex_wire::canonical::CanonicalError> {
        let bytes = to_jcs_bytes(value)?;
        Ok(Self(*blake3::hash(&bytes).as_bytes()))
    }

    /// Digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// One debit-positive signed posting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posting {
    /// Account receiving the posting.
    pub account: AccountRef,
    /// Debit-positive micro-USD amount.
    pub amount: MicrousdDelta,
}

/// A transaction proven balanced by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalancedTransaction {
    id: TransactionId,
    kind: TransactionKind,
    organization: Option<OrganizationId>,
    business_key: BusinessKey,
    intent_hash: IntentHash,
    occurred_at: OffsetDateTime,
    reverses: Option<TransactionId>,
    postings: Vec<Posting>,
}

impl BalancedTransaction {
    /// Constructs a transaction only when every conservation invariant holds.
    ///
    /// # Errors
    /// Returns the exact violated invariant.
    pub fn try_new(
        id: TransactionId,
        kind: TransactionKind,
        organization: Option<OrganizationId>,
        business_key: BusinessKey,
        intent_hash: IntentHash,
        occurred_at: OffsetDateTime,
        postings: Vec<Posting>,
    ) -> Result<Self, ConservationError> {
        if postings.len() < 2 {
            return Err(ConservationError::TooFewPostings(postings.len()));
        }
        let mut accounts = BTreeSet::new();
        let mut sum = 0_i64;
        for (index, posting) in postings.iter().enumerate() {
            if posting.amount.get() == 0 {
                return Err(ConservationError::ZeroAmount {
                    seq: u16::try_from(index).unwrap_or(u16::MAX),
                });
            }
            if !accounts.insert(posting.account) {
                return Err(ConservationError::DuplicateAccount(posting.account));
            }
            sum = sum
                .checked_add(posting.amount.get())
                .ok_or(ConservationError::Overflow)?;
        }
        if sum != 0 {
            return Err(ConservationError::Unbalanced {
                imbalance_microusd: sum,
            });
        }
        Ok(Self {
            id,
            kind,
            organization,
            business_key,
            intent_hash,
            occurred_at,
            reverses: None,
            postings,
        })
    }

    /// Builds the exact negating transaction.
    ///
    /// # Errors
    /// Returns an invariant error if the source was corrupted in memory.
    pub fn reverse(
        &self,
        id: TransactionId,
        occurred_at: OffsetDateTime,
    ) -> Result<Self, ConservationError> {
        let mut reversed = Self::try_new(
            id,
            TransactionKind::Reversal,
            self.organization,
            BusinessKey(format!("rev:{}", self.id)),
            self.intent_hash,
            occurred_at,
            self.postings
                .iter()
                .map(|posting| Posting {
                    account: posting.account,
                    amount: posting.amount.negate(),
                })
                .collect(),
        )?;
        reversed.reverses = Some(self.id);
        Ok(reversed)
    }

    /// Immutable postings.
    #[must_use]
    pub fn postings(&self) -> &[Posting] {
        &self.postings
    }

    /// Net signed posting for an account.
    #[must_use]
    pub fn net_for(&self, account: &AccountRef) -> MicrousdDelta {
        self.postings
            .iter()
            .find(|posting| &posting.account == account)
            .map_or(MicrousdDelta::ZERO, |posting| posting.amount)
    }

    /// Identity.
    #[must_use]
    pub const fn id(&self) -> TransactionId {
        self.id
    }

    /// Kind.
    #[must_use]
    pub const fn kind(&self) -> TransactionKind {
        self.kind
    }

    /// Business key.
    #[must_use]
    pub const fn business_key(&self) -> &BusinessKey {
        &self.business_key
    }

    /// Optional reversed transaction.
    #[must_use]
    pub const fn reverses(&self) -> Option<TransactionId> {
        self.reverses
    }
}

/// A violated conservation invariant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConservationError {
    /// Signed sum was not zero.
    #[error("transaction is unbalanced by {imbalance_microusd} micro-USD")]
    Unbalanced {
        /// Observed debit-positive sum.
        imbalance_microusd: i64,
    },
    /// Fewer than two postings.
    #[error("transaction needs at least two postings, observed {0}")]
    TooFewPostings(usize),
    /// Zero posting.
    #[error("posting {seq} is zero")]
    ZeroAmount {
        /// Zero-based posting sequence.
        seq: u16,
    },
    /// Currency disagreement (reserved for decoded store rows).
    #[error("transaction contains mixed currencies")]
    MixedCurrency,
    /// Same account appeared twice rather than being aggregated.
    #[error("account appears more than once")]
    DuplicateAccount(AccountRef),
    /// Signed sum overflowed.
    #[error("transaction sum overflowed")]
    Overflow,
}
