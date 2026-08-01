//! Lost-commit resolution by the original durable identity.

use crate::wire_pending::CommitOutcomeUnknown;

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
/// # Errors
/// Returns [`UnknownCommitError::IntentConflict`] if durable state has the same
/// business key but a different intent hash.
pub fn resolve_unknown_commit(
    unknown: &CommitOutcomeUnknown,
    probe: CommitProbe,
    expected_intent: [u8; 32],
) -> Result<CommitResolution, UnknownCommitError> {
    if unknown.transaction_id.is_empty() {
        return Err(UnknownCommitError::MissingTransportIdentity);
    }
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
