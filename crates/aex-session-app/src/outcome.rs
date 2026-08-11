//! The eight-row unknown-outcome recovery matrix (D-10).
//!
//! It is named in three places in the rewrite corpus and enumerated in none.
//! This is the enumeration, and it is a **total function** over
//! {what the provider answered} × {what the durable facts say}, asserted on the
//! durable facts and never on a status code. "Eventually returned 200" is not a
//! recovery test.
//!
//! Cost is part of the contract. Rows 1-6 resolve from what the provider
//! already told us and issue **zero** extra reads; only the two
//! transport-ambiguous rows pay for one strongly consistent read, and only on
//! the ambiguous path. The happy path reads nothing.
//!
//! Three things this deliberately does not do. It never infers the outcome from
//! an HTTP status. It never retries under a fresh identity — the operation id is
//! caller-minted and stable, which is what makes rows 3 and 7 resolvable at all.
//! And it never produces a generic `unknown` customer error.

use aex_operation_domain::Operation;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{OperationId, WorkspaceId};

use crate::plan::{ConditionId, ItemKey};
use crate::ports::CommitError;

/// What the provider answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderAnswer {
    /// The transaction committed.
    Committed,
    /// One or more conditions did not hold, with their stable identities.
    ConditionFailed(Vec<ConditionId>),
    /// The answer did not say whether the transaction landed.
    Ambiguous(Vec<ItemKey>),
    /// The provider asked the caller to slow down.
    Throttled,
    /// The provider was unavailable and the write definitely did not land.
    Unavailable,
}

impl ProviderAnswer {
    /// Reads one commit failure as a provider answer.
    #[must_use]
    pub fn of(error: &CommitError) -> Self {
        match error {
            CommitError::ConditionFailed { failed } => Self::ConditionFailed(failed.clone()),
            CommitError::Ambiguous { targets } => Self::Ambiguous(targets.clone()),
            CommitError::Throttled => Self::Throttled,
            // A rejected plan never reached the provider, and an unavailable
            // provider refused before durable state moved. Neither is ambiguous.
            CommitError::Unavailable | CommitError::PlanRejected(_) => Self::Unavailable,
        }
    }
}

/// What the command asked for, as the resolver needs to recognise it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempted {
    /// The workspace the request was authorized for.
    pub workspace: WorkspaceId,
    /// The caller-minted operation identity.
    pub operation: OperationId,
    /// The digest of what was asked for.
    pub intent: IntentDigest,
}

/// The durable facts a resolution is asserted on.
///
/// Both fields are `Option` because "absent" is itself a fact here, and the two
/// absences mean different things: no operation **and** no session is row 2,
/// while no operation with a live session is row 7.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// The operation row, when one exists.
    pub operation: Option<Operation>,
    /// Whether the session the command named still exists.
    pub session_present: bool,
}

/// How an outcome resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Rows 1 and 3. The write landed; return the stored operation. No second
    /// write, ever.
    Replay(Box<Operation>),
    /// Row 2. Corruption or a completed purge. Refuse, and never re-admit.
    NotFound,
    /// Row 4. Someone else's command reused this operation id.
    IdempotencyConflict,
    /// Row 5. A guard row moved under a concurrent mutation. The exact failed
    /// identities are carried so the caller maps a typed domain error rather
    /// than guessing, and never retries blindly.
    GuardMoved(Vec<ConditionId>),
    /// Row 7. The write did not land and the latch is open: re-submit the
    /// **same** operation id, unchanged.
    Resubmit,
    /// Row 8. The latch is closed, so the operation may never fail. Re-drive
    /// the step under its fence; on the last attempt the work item becomes
    /// poison and the operation goes to operator quarantine with its status
    /// still `Running`.
    Quarantine,
    /// The provider refused before durable state could move. Not part of the
    /// matrix: nothing needs recovering.
    Retry,
}

/// Resolves one outcome against the durable facts.
///
/// Total: every `(answer, observed)` pair maps to exactly one [`Resolution`],
/// so no caller can be left without an answer about whether its operation
/// exists.
#[must_use]
pub fn resolve(answer: &ProviderAnswer, attempted: Attempted, observed: &Observed) -> Resolution {
    match answer {
        // Row 1. Nothing to recover.
        ProviderAnswer::Committed => match &observed.operation {
            Some(operation) => Resolution::Replay(Box::new(operation.clone())),
            None => Resolution::Resubmit,
        },
        // Rows 2-5. Zero extra reads: the caller already held the operation it
        // read before planning, and a condition failure names its own guards.
        ProviderAnswer::ConditionFailed(failed) => match &observed.operation {
            // Row 2.
            None if !observed.session_present => Resolution::NotFound,
            // A guard moved and no operation row exists, so the failure is
            // about the session, not about the identity.
            None => Resolution::GuardMoved(failed.clone()),
            Some(operation) => landed(operation, attempted, failed.clone()),
        },
        // Rows 6-8. One strongly consistent read of every target has already
        // happened; `observed` is its result.
        ProviderAnswer::Ambiguous(_) => match &observed.operation {
            // Row 7, second half: nothing proves the write landed.
            None => Resolution::Resubmit,
            Some(operation) => {
                if operation.id != attempted.operation
                    || operation.workspace != attempted.workspace
                    || operation.intent != attempted.intent
                {
                    // Row 4 reached through the ambiguous path: the row that
                    // exists is not mine.
                    return Resolution::IdempotencyConflict;
                }
                // Row 8: a latched operation may never become `Failed`.
                // Row 3/7: an unlatched one landed and replays.
                if operation.committed_at.is_some() && !operation.status.is_terminal() {
                    Resolution::Quarantine
                } else {
                    Resolution::Replay(Box::new(operation.clone()))
                }
            }
        },
        ProviderAnswer::Throttled | ProviderAnswer::Unavailable => Resolution::Retry,
    }
}

/// Rows 3, 4 and 5 over an operation row that exists.
fn landed(operation: &Operation, attempted: Attempted, failed: Vec<ConditionId>) -> Resolution {
    if operation.id != attempted.operation || operation.workspace != attempted.workspace {
        // A foreign workspace is never told the record exists.
        return Resolution::NotFound;
    }
    if operation.intent != attempted.intent {
        // Row 4.
        return Resolution::IdempotencyConflict;
    }
    if failed.is_empty() {
        // Row 3: my write landed and the acknowledgement was lost.
        return Resolution::Replay(Box::new(operation.clone()));
    }
    // Row 5: my row is there and a guard also moved. The guard is the more
    // specific fact and the caller maps it onto the typed domain error.
    Resolution::GuardMoved(failed)
}

#[cfg(test)]
mod tests {
    use aex_operation_domain::operation::{OperationScope, OperationStatus};
    use aex_operation_domain::{Operation, OperationKind};
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{OperationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{Attempted, Observed, ProviderAnswer, Resolution, resolve};
    use crate::plan::ConditionId;

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn attempted() -> Attempted {
        Attempted {
            workspace: workspace(),
            operation: OperationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            intent: IntentDigest::from_bytes([9; 32]),
        }
    }

    fn stored(status: OperationStatus, latched: bool) -> Operation {
        let session = SessionId::from_uuid7(Uuid7::compose(1, [3; 10]));
        Operation {
            id: attempted().operation,
            workspace: workspace(),
            session: Some(session),
            kind: OperationKind::SessionCancel,
            status,
            intent: attempted().intent,
            scope: OperationScope::Session(session),
            progress: None,
            cursor: None,
            cancel_requested: false,
            result: None,
            error: None,
            created_at: moment(1),
            started_at: Some(moment(1)),
            updated_at: moment(1),
            committed_at: latched.then(|| moment(1)),
            terminal_at: None,
        }
    }

    fn observed(operation: Option<Operation>, session_present: bool) -> Observed {
        Observed {
            operation,
            session_present,
        }
    }

    #[test]
    fn row_1_nothing_to_recover() {
        let record = stored(OperationStatus::Succeeded, true);
        assert_eq!(
            resolve(
                &ProviderAnswer::Committed,
                attempted(),
                &observed(Some(record.clone()), true)
            ),
            Resolution::Replay(Box::new(record))
        );
    }

    #[test]
    fn row_2_no_operation_and_no_session_is_refused_and_never_readmitted() {
        assert_eq!(
            resolve(
                &ProviderAnswer::ConditionFailed(vec![ConditionId(0)]),
                attempted(),
                &observed(None, false)
            ),
            Resolution::NotFound
        );
    }

    #[test]
    fn row_3_a_lost_acknowledgement_replays_and_writes_nothing() {
        let record = stored(OperationStatus::Succeeded, true);
        assert_eq!(
            resolve(
                &ProviderAnswer::ConditionFailed(Vec::new()),
                attempted(),
                &observed(Some(record.clone()), true)
            ),
            Resolution::Replay(Box::new(record))
        );
    }

    #[test]
    fn row_4_a_reused_identity_conflicts_rather_than_replaying() {
        let mut foreign = stored(OperationStatus::Succeeded, true);
        foreign.intent = IntentDigest::from_bytes([7; 32]);
        assert_eq!(
            resolve(
                &ProviderAnswer::ConditionFailed(vec![ConditionId(1)]),
                attempted(),
                &observed(Some(foreign), true)
            ),
            Resolution::IdempotencyConflict
        );
    }

    #[test]
    fn row_5_a_moved_guard_keeps_its_exact_identities() {
        let record = stored(OperationStatus::Succeeded, true);
        assert_eq!(
            resolve(
                &ProviderAnswer::ConditionFailed(vec![ConditionId(0), ConditionId(2)]),
                attempted(),
                &observed(Some(record), true)
            ),
            Resolution::GuardMoved(vec![ConditionId(0), ConditionId(2)])
        );
    }

    #[test]
    fn row_6_a_foreign_workspace_is_never_told_the_record_exists() {
        let mut foreign = stored(OperationStatus::Succeeded, true);
        foreign.workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [8; 10]));
        assert_eq!(
            resolve(
                &ProviderAnswer::ConditionFailed(vec![ConditionId(0)]),
                attempted(),
                &observed(Some(foreign), true)
            ),
            Resolution::NotFound
        );
    }

    #[test]
    fn row_7_an_open_latch_with_no_row_resubmits_the_same_identity() {
        assert_eq!(
            resolve(
                &ProviderAnswer::Ambiguous(Vec::new()),
                attempted(),
                &observed(None, true)
            ),
            Resolution::Resubmit
        );
    }

    #[test]
    fn row_8_a_closed_latch_quarantines_rather_than_failing() {
        assert_eq!(
            resolve(
                &ProviderAnswer::Ambiguous(Vec::new()),
                attempted(),
                &observed(Some(stored(OperationStatus::Running, true)), true)
            ),
            Resolution::Quarantine,
            "a latched operation may never become Failed"
        );
        assert!(matches!(
            resolve(
                &ProviderAnswer::Ambiguous(Vec::new()),
                attempted(),
                &observed(Some(stored(OperationStatus::Running, false)), true)
            ),
            Resolution::Replay(_)
        ));
    }

    #[test]
    fn the_matrix_is_total_over_every_answer_and_every_observation() {
        let answers = [
            ProviderAnswer::Committed,
            ProviderAnswer::ConditionFailed(Vec::new()),
            ProviderAnswer::ConditionFailed(vec![ConditionId(0)]),
            ProviderAnswer::Ambiguous(Vec::new()),
            ProviderAnswer::Throttled,
            ProviderAnswer::Unavailable,
        ];
        let observations = [
            observed(None, false),
            observed(None, true),
            observed(Some(stored(OperationStatus::Running, false)), true),
            observed(Some(stored(OperationStatus::Running, true)), true),
            observed(Some(stored(OperationStatus::Succeeded, true)), true),
        ];
        for answer in &answers {
            for observation in &observations {
                // Every pair resolves; the assertion is that none panics and
                // none is left undecided.
                let _ = resolve(answer, attempted(), observation);
            }
        }
    }
}
