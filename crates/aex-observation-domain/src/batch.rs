//! The admission receipt state machine.
//!
//! A batch is admitted in two durable steps: transaction P creates a
//! `preparing` receipt bound to the batch's intent digest, and transaction C
//! flips it to `committed`. The only other reachable terminal state is
//! `aborted`, written by the reconciler's `batch.expire` duty. Nothing else is a
//! legal transition, and a terminal receipt never re-enters preparation — which
//! is what makes an ambiguous transport outcome resolvable by re-reading the
//! receipt and branching on `state`.

use aex_wire::idempotency::IntentDigest;

/// Why a receipt transition was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TransitionError {
    /// The receipt is already terminal; nothing moves it again.
    #[error("a `{state}` receipt is terminal")]
    Terminal {
        /// The terminal state, in its durable spelling.
        state: &'static str,
    },
    /// A preparation was resumed with a different intent digest, which is a
    /// batch-id reuse with different bytes and therefore `409`.
    #[error("the batch id was reused with a different intent digest")]
    IntentConflict,
}

/// The transitions a receipt admits.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReceiptTransition {
    /// Transaction C published the accepted range.
    Commit,
    /// The preparation expired or was abandoned.
    Abort,
}

impl ReceiptTransition {
    /// Every transition, in declared order.
    pub const ALL: &'static [ReceiptTransition] =
        &[ReceiptTransition::Commit, ReceiptTransition::Abort];

    /// The durable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Abort => "abort",
        }
    }
}

/// Where an admission receipt is in its life.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReceiptState {
    /// Transaction P landed; staged pages may still be arriving.
    Preparing {
        /// The digest the preparation is bound to.
        intent: IntentDigest,
    },
    /// Transaction C landed; the accepted range is public.
    Committed {
        /// The digest the commit is bound to.
        intent: IntentDigest,
    },
    /// The preparation was abandoned; the reservation was released.
    Aborted {
        /// The digest the abandoned preparation was bound to.
        intent: IntentDigest,
    },
}

impl ReceiptState {
    /// The state transaction P writes.
    #[must_use]
    pub const fn prepare(intent: IntentDigest) -> Self {
        Self::Preparing { intent }
    }

    /// The durable spelling of this state.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Preparing { .. } => "preparing",
            Self::Committed { .. } => "committed",
            Self::Aborted { .. } => "aborted",
        }
    }

    /// The digest this receipt is bound to.
    #[must_use]
    pub const fn intent(&self) -> IntentDigest {
        match self {
            Self::Preparing { intent } | Self::Committed { intent } | Self::Aborted { intent } => {
                *intent
            }
        }
    }

    /// Whether the receipt can still move.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Committed { .. } | Self::Aborted { .. })
    }

    /// Applies a transition.
    ///
    /// # Errors
    ///
    /// Returns [`TransitionError::Terminal`] from `committed` or `aborted`;
    /// there is no transition out of either.
    pub fn apply(&self, transition: ReceiptTransition) -> Result<Self, TransitionError> {
        let intent = self.intent();
        match self {
            Self::Preparing { .. } => Ok(match transition {
                ReceiptTransition::Commit => Self::Committed { intent },
                ReceiptTransition::Abort => Self::Aborted { intent },
            }),
            Self::Committed { .. } | Self::Aborted { .. } => Err(TransitionError::Terminal {
                state: self.as_str(),
            }),
        }
    }

    /// Resumes a preparation with the intent the caller believes it holds.
    ///
    /// This is the `attribute_not_exists(pk) OR (state = preparing AND
    /// intentDigest = :d)` condition of transaction P expressed in the domain.
    ///
    /// # Errors
    ///
    /// Returns [`TransitionError::IntentConflict`] when the digests differ and
    /// [`TransitionError::Terminal`] when the receipt already settled.
    pub fn resume(&self, intent: IntentDigest) -> Result<Self, TransitionError> {
        match self {
            Self::Preparing { intent: existing } if *existing == intent => Ok(*self),
            Self::Preparing { .. } => Err(TransitionError::IntentConflict),
            Self::Committed { .. } | Self::Aborted { .. } => Err(TransitionError::Terminal {
                state: self.as_str(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ReceiptState, ReceiptTransition, TransitionError};
    use aex_wire::idempotency::IntentDigest;

    #[test]
    fn the_durable_spellings_are_stable() {
        let intent = IntentDigest::from_bytes([7; 32]);
        assert_eq!(ReceiptState::prepare(intent).as_str(), "preparing");
        assert_eq!(ReceiptState::Committed { intent }.as_str(), "committed");
        assert_eq!(ReceiptState::Aborted { intent }.as_str(), "aborted");
        assert_eq!(ReceiptTransition::ALL.len(), 2);
        assert_eq!(ReceiptTransition::Commit.as_str(), "commit");
    }

    #[test]
    fn a_commit_preserves_the_bound_intent() {
        let intent = IntentDigest::from_bytes([9; 32]);
        let committed = ReceiptState::prepare(intent)
            .apply(ReceiptTransition::Commit)
            .expect("preparing commits");
        assert_eq!(committed.intent(), intent);
        assert!(committed.is_terminal());
        assert_eq!(
            committed.apply(ReceiptTransition::Abort),
            Err(TransitionError::Terminal { state: "committed" })
        );
    }
}
