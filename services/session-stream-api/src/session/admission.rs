//! Pure message/run admission transaction compilation.

use aex_regional_http::idempotency::IdempotencyIdentity;
use aex_wire::ids::{MessageId, RunId, SessionId, WorkspaceId};
use sha2::Digest as _;

use crate::session::wire_pending::TransactionPlan;

/// One of the three authority tables in admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    /// Monotonic authorization projection.
    AuthzProjection,
    /// Session, reservation, receipt and event authority.
    SessionAuthority,
    /// Durable regional continuation authority.
    RegionalWork,
}

/// Closed condition vocabulary for the eight-action transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    /// Projected epochs are no newer and account state admits paid work.
    ProjectedEpochsAndAccountState,
    /// Reservation has this many whole cents available.
    SufficientRegionalAllocation {
        /// Requested reservation.
        cents: u64,
    },
    /// Item key does not already exist.
    AttributeNotExists,
    /// Session head has not raced with a mutation or deletion.
    SessionHead {
        /// Expected session revision.
        revision: u64,
        /// Expected deletion epoch.
        deletion_epoch: u64,
        /// Expected cancellation epoch.
        cancellation_epoch: u64,
    },
}

/// Exact action vocabulary; there is deliberately no queue action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionAction {
    /// T1.
    ConditionCheck(Condition),
    /// T2.
    Update(Condition),
    /// T3.
    PutMessage(Condition),
    /// T4.
    PutRun(Condition),
    /// T5.
    UpdateSessionHead(Condition),
    /// T6.
    PutRootContinuation(Condition),
    /// T7.
    PutIdempotencyReceipt(Condition),
    /// T8.
    PutRunAdmittedEvent(Condition),
}

impl TransactionAction {
    /// Authority table for this action.
    #[must_use]
    pub const fn table(&self) -> Table {
        match self {
            Self::ConditionCheck(_) => Table::AuthzProjection,
            Self::PutRootContinuation(_) => Table::RegionalWork,
            Self::Update(_)
            | Self::PutMessage(_)
            | Self::PutRun(_)
            | Self::UpdateSessionHead(_)
            | Self::PutIdempotencyReceipt(_)
            | Self::PutRunAdmittedEvent(_) => Table::SessionAuthority,
        }
    }
}

/// Stable authority identities and optimistic revisions for one admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionInput {
    /// Workspace authority.
    pub workspace_id: WorkspaceId,
    /// Session authority.
    pub session_id: SessionId,
    /// New message identity.
    pub message_id: MessageId,
    /// New run identity.
    pub run_id: RunId,
    /// Session head revision.
    pub expected_revision: u64,
    /// Deletion race fence.
    pub deletion_epoch: u64,
    /// Cancellation race fence.
    pub cancellation_epoch: u64,
    /// Finite spend reservation in whole cents.
    pub max_spend_cents: u64,
}

/// Why transaction compilation refused an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    /// A paid run cannot reserve zero cents.
    #[error("max spend must be positive")]
    ZeroReservation,
}

/// Compiles exactly one atomic eight-action plan and performs no I/O.
///
/// # Errors
///
/// Returns [`AdmissionError::ZeroReservation`] for a zero reservation.
pub fn compile_message_admission(
    identity: &IdempotencyIdentity,
    input: &AdmissionInput,
) -> Result<TransactionPlan<TransactionAction>, AdmissionError> {
    if input.max_spend_cents == 0 {
        return Err(AdmissionError::ZeroReservation);
    }
    let client_request_token = client_request_token(identity, input);
    let actions = vec![
        TransactionAction::ConditionCheck(Condition::ProjectedEpochsAndAccountState),
        TransactionAction::Update(Condition::SufficientRegionalAllocation {
            cents: input.max_spend_cents,
        }),
        TransactionAction::PutMessage(Condition::AttributeNotExists),
        TransactionAction::PutRun(Condition::AttributeNotExists),
        TransactionAction::UpdateSessionHead(Condition::SessionHead {
            revision: input.expected_revision,
            deletion_epoch: input.deletion_epoch,
            cancellation_epoch: input.cancellation_epoch,
        }),
        TransactionAction::PutRootContinuation(Condition::AttributeNotExists),
        TransactionAction::PutIdempotencyReceipt(Condition::AttributeNotExists),
        TransactionAction::PutRunAdmittedEvent(Condition::AttributeNotExists),
    ];
    Ok(TransactionPlan {
        client_request_token,
        actions,
    })
}

fn client_request_token(identity: &IdempotencyIdentity, input: &AdmissionInput) -> String {
    let mut digest = sha2::Sha256::new();
    digest.update(identity.scope);
    digest.update(identity.key.as_str().as_bytes());
    digest.update(identity.intent);
    digest.update(input.workspace_id.to_string());
    digest.update(input.session_id.to_string());
    digest.update(input.message_id.to_string());
    digest.update(input.run_id.to_string());
    let digest: [u8; 32] = digest.finalize().into();
    format!("aex-{}", hex::encode(&digest[..16]))
}
