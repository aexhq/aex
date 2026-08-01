//! Lost-commit resolution by the original durable identity.
//!
//! The Data `API` answers a failed commit with [`CommitFailure`], and the two
//! arms mean different things for money that is already staged:
//! [`CommitFailure::RolledBack`] is the service stating nothing was applied,
//! while [`CommitFailure::Unknown`] is a lost response that may or may not have
//! posted.
//!
//! [`CommitDisposition::classify`] is the single place that distinction is
//! made, and [`UnknownCommit`] — the token [`resolve_unknown_commit`] requires —
//! has no other mint site. A rolled-back commit therefore cannot be routed
//! through the ambiguity path, and an ambiguous commit cannot be replayed until
//! the original business key has been re-queried. Neither rule depends on a
//! caller remembering it.

use aex_rds_data::{CommitFailure, DataApiError, TransactionId};

/// Durable receipt returned by a posted transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostedReceipt {
    /// Finance transaction identity.
    pub transaction_id: String,
    /// Original unique business key.
    pub business_key: String,
    /// Original canonical intent digest.
    pub intent_hash: [u8; 32],
}

/// A commit whose outcome the transport could not establish.
///
/// The fields are private and there is no constructor: the only value of this
/// type in existence came out of the [`CommitFailure::Unknown`] arm of
/// [`CommitDisposition::classify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCommit {
    transaction: TransactionId,
    cause: DataApiError,
}

impl UnknownCommit {
    /// The transport transaction whose commit response was lost.
    #[must_use]
    pub const fn transaction(&self) -> &TransactionId {
        &self.transaction
    }

    /// The classified transport failure that lost the response.
    #[must_use]
    pub const fn cause(&self) -> &DataApiError {
        &self.cause
    }
}

/// What a failed commit means for the postings it staged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitDisposition {
    /// The service stated the transaction aborted. No posting was applied, so
    /// the exact transaction may be replayed under the same business key
    /// without a probe.
    NotApplied(DataApiError),
    /// The commit response was lost. Nothing may be replayed until the original
    /// business key has been re-queried; see [`resolve_unknown_commit`].
    Unresolved(UnknownCommit),
}

impl CommitDisposition {
    /// Classifies a failed commit on the money path.
    ///
    /// `transaction` is the identity the transport minted, captured before
    /// [`aex_rds_data::Transaction::commit`] consumed the transaction.
    ///
    /// # Errors
    /// Returns [`UnknownCommitError::MissingTransportIdentity`] when the
    /// transport did not name the transaction that failed, because an
    /// unnameable transaction cannot be reconciled by anything.
    pub fn classify(
        failure: CommitFailure,
        transaction: TransactionId,
    ) -> Result<Self, UnknownCommitError> {
        if transaction.as_str().is_empty() {
            return Err(UnknownCommitError::MissingTransportIdentity);
        }
        Ok(match failure {
            CommitFailure::RolledBack(cause) => Self::NotApplied(cause),
            CommitFailure::Unknown(cause) => Self::Unresolved(UnknownCommit { transaction, cause }),
        })
    }
}

/// Result of re-querying the original business key outside the lost transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitProbe {
    /// Matching durable transaction exists.
    Committed(PostedReceipt),
    /// No durable row exists; the exact transaction may retry.
    Absent,
    /// Identity exists but transaction remains unresolved.
    Pending,
}

/// Safe action after the probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitResolution {
    /// Return the originally committed receipt.
    Original(PostedReceipt),
    /// Retry the exact transaction under the same business key.
    CleanRetry,
    /// Return a typed retryable result; never mint a key.
    Retryable,
}

/// Resolves an outcome-unknown commit without a second transaction identity.
///
/// `unknown` is the whole precondition. It cannot be constructed except from
/// [`CommitFailure::Unknown`], so holding one is the proof that this is answering
/// an ambiguous commit rather than a rolled-back one, and `probe` is the answer
/// the caller already got by re-querying the original business key.
///
/// # Errors
/// Returns [`UnknownCommitError::IntentConflict`] if durable state has the same
/// business key but a different intent hash.
pub fn resolve_unknown_commit(
    unknown: &UnknownCommit,
    probe: CommitProbe,
    expected_intent: [u8; 32],
) -> Result<CommitResolution, UnknownCommitError> {
    // Re-checks the construction invariant, so a second mint site added later
    // fails a test rather than silently reconciling an unnameable transaction.
    debug_assert!(!unknown.transaction().as_str().is_empty());
    match probe {
        CommitProbe::Committed(receipt) if receipt.intent_hash == expected_intent => {
            Ok(CommitResolution::Original(receipt))
        }
        CommitProbe::Committed(_) => Err(UnknownCommitError::IntentConflict),
        CommitProbe::Absent => Ok(CommitResolution::CleanRetry),
        CommitProbe::Pending => Ok(CommitResolution::Retryable),
    }
}

/// Lost-commit resolution failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UnknownCommitError {
    /// Re-query found a different intent under the same business key.
    #[error("business key is committed with a different intent")]
    IntentConflict,
    /// Transport failed to report which transaction became ambiguous.
    #[error("outcome-unknown commit has no transport transaction id")]
    MissingTransportIdentity,
}
