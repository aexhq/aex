//! Tool approvals.
//!
//! At most one unresolved approval exists per session; other tool work queues
//! behind it. `respond` **revalidates** the binding at decision time: drift in
//! any of the eleven bound fields commits the approval as
//! `Cancelled { BindingDrift }` and returns the drifted field list. That is fail
//! closed — the bound call is never dispatched against a binding the approver did
//! not see.
//!
//! Deny is not a cancel: it records exactly one canonical `approval_denied` tool
//! result, lets the model continue, and leaves the run live.

use aex_content_domain::ContentDigest;
use aex_internal_contracts::RunId;
use aex_wire::error::ErrorCode;
use aex_wire::ids::{AgentId, ApprovalId, GenerationId, SessionId, ToolCallId};
use aex_wire::types::Timestamp;

use aex_secret_domain::CustodyRevision;

/// Where an approval is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApprovalStatus {
    /// Waiting for a decision.
    Pending,
    /// Approved.
    Approved,
    /// Denied.
    Denied,
    /// Withdrawn without a decision.
    Cancelled,
    /// The explicit decision deadline elapsed.
    Expired,
}

impl ApprovalStatus {
    /// Every status, in canonical order.
    pub const ALL: [Self; 5] = [
        Self::Pending,
        Self::Approved,
        Self::Denied,
        Self::Cancelled,
        Self::Expired,
    ];

    /// Whether no transition leaves this status.
    #[must_use]
    pub const fn is_resolved(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

/// What an approver decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApprovalDecision {
    /// Dispatch the bound call.
    Approve,
    /// Do not dispatch; record a denial result and let the model continue.
    Deny,
}

/// Why a pending approval was withdrawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApprovalCancelCause {
    /// The public session cancellation command asked for it.
    SessionCancel,
    /// The run was cancelled.
    RunCancelled,
    /// The irreversible deletion fence was crossed.
    SessionDeleting,
    /// The account is paused.
    AccountPaused,
    /// The workspace generation is gone.
    ContinuityLost,
    /// The bound tool call itself was cancelled.
    ToolCallCancelled,
    /// The binding drifted between request and decision.
    BindingDrift,
}

impl ApprovalCancelCause {
    /// Every cause, in canonical order.
    pub const ALL: [Self; 7] = [
        Self::SessionCancel,
        Self::RunCancelled,
        Self::SessionDeleting,
        Self::AccountPaused,
        Self::ContinuityLost,
        Self::ToolCallCancelled,
        Self::BindingDrift,
    ];
}

/// The scope a cancellation applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelScope {
    /// The run being cancelled, when the cause names one.
    pub run: Option<RunId>,
    /// The generation being lost, when the cause names one.
    pub generation: Option<GenerationId>,
    /// The tool call being cancelled, when the cause names one.
    pub tool_call: Option<ToolCallId>,
}

impl CancelScope {
    /// A scope naming nothing, for the two unconditional causes.
    pub const UNSCOPED: Self = Self {
        run: None,
        generation: None,
        tool_call: None,
    };
}

/// One of the eleven fields an approval binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BindingField {
    /// The session.
    Session,
    /// The run.
    Run,
    /// The agent.
    Agent,
    /// The tool call.
    ToolCall,
    /// The tool name.
    Tool,
    /// The canonical argument digest.
    ArgumentDigest,
    /// The tool implementation digest.
    ImplementationDigest,
    /// The resolved configuration digest.
    ConfigDigest,
    /// The expected workspace generation.
    ExpectedGeneration,
    /// The expected custody revision.
    ExpectedCustody,
    /// The expected configuration revision.
    ExpectedConfigRevision,
}

impl BindingField {
    /// Every bound field. Exactly eleven.
    pub const ALL: [Self; 11] = [
        Self::Session,
        Self::Run,
        Self::Agent,
        Self::ToolCall,
        Self::Tool,
        Self::ArgumentDigest,
        Self::ImplementationDigest,
        Self::ConfigDigest,
        Self::ExpectedGeneration,
        Self::ExpectedCustody,
        Self::ExpectedConfigRevision,
    ];
}

/// The exact call an approval authorizes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalBinding {
    /// The session.
    pub session: SessionId,
    /// The run.
    pub run: RunId,
    /// The agent.
    pub agent: AgentId,
    /// The tool call.
    pub tool_call: ToolCallId,
    /// The tool name.
    pub tool: String,
    /// The canonical argument digest.
    pub argument_digest: ContentDigest,
    /// The tool implementation digest.
    pub implementation_digest: ContentDigest,
    /// The resolved configuration digest.
    pub config_digest: ContentDigest,
    /// The workspace generation the call expects.
    pub expected_generation: Option<GenerationId>,
    /// The custody revision the call expects.
    pub expected_custody: CustodyRevision,
    /// The configuration revision the call expects.
    pub expected_config_revision: u64,
}

/// Which of the eleven bound fields differ.
#[must_use]
pub fn binding_drift(expected: &ApprovalBinding, current: &ApprovalBinding) -> Vec<BindingField> {
    let mut drifted = Vec::new();
    if expected.session != current.session {
        drifted.push(BindingField::Session);
    }
    if expected.run != current.run {
        drifted.push(BindingField::Run);
    }
    if expected.agent != current.agent {
        drifted.push(BindingField::Agent);
    }
    if expected.tool_call != current.tool_call {
        drifted.push(BindingField::ToolCall);
    }
    if expected.tool != current.tool {
        drifted.push(BindingField::Tool);
    }
    if expected.argument_digest != current.argument_digest {
        drifted.push(BindingField::ArgumentDigest);
    }
    if expected.implementation_digest != current.implementation_digest {
        drifted.push(BindingField::ImplementationDigest);
    }
    if expected.config_digest != current.config_digest {
        drifted.push(BindingField::ConfigDigest);
    }
    if expected.expected_generation != current.expected_generation {
        drifted.push(BindingField::ExpectedGeneration);
    }
    if expected.expected_custody != current.expected_custody {
        drifted.push(BindingField::ExpectedCustody);
    }
    if expected.expected_config_revision != current.expected_config_revision {
        drifted.push(BindingField::ExpectedConfigRevision);
    }
    drifted
}

/// One approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    /// Its identity.
    pub id: ApprovalId,
    /// The exact call it authorizes.
    pub binding: ApprovalBinding,
    /// Where it is.
    pub status: ApprovalStatus,
    /// When it was raised.
    pub requested_at: Timestamp,
    /// The explicit deadline after which it cannot be decided.
    pub expires_at: Timestamp,
    /// When it settled.
    pub resolved_at: Option<Timestamp>,
    /// What was decided.
    pub decision: Option<ApprovalDecision>,
    /// Why it was withdrawn.
    pub cancel_cause: Option<ApprovalCancelCause>,
}

/// What an approval transition produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalCommit {
    /// The approval after the transition.
    pub approval: Approval,
    /// Whether the bound call may now be dispatched.
    pub dispatch: bool,
    /// The canonical tool result a denial records, when one is recorded.
    pub denial_result: Option<ErrorCode>,
}

/// What one response to an approval resolved to.
///
/// The distinction is a return type rather than a convention because
/// [`ApprovalCommit::dispatch`] is not an imperative: a replayed `Approve` used
/// to hand back `dispatch: true` a second time, so an application that acted on
/// the flag would authorise and dispatch the bound call again on every retry.
/// A replay carries the stored approval and no commit at all, which makes that
/// double dispatch unrepresentable rather than merely discouraged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// This response decided the approval. The caller must persist the commit.
    Committed(ApprovalCommit),
    /// An earlier identical response already decided it. Nothing is written,
    /// nothing is dispatched, and the stored approval is the answer.
    Replayed(Approval),
}

impl ApprovalOutcome {
    /// The approval as it now stands, however the outcome was reached.
    #[must_use]
    pub const fn approval(&self) -> &Approval {
        match self {
            Self::Committed(commit) => &commit.approval,
            Self::Replayed(approval) => approval,
        }
    }

    /// The commit to persist, and `None` for a replay.
    #[must_use]
    pub const fn commit(&self) -> Option<&ApprovalCommit> {
        match self {
            Self::Committed(commit) => Some(commit),
            Self::Replayed(_) => None,
        }
    }
}

/// Why an approval transition was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalRejection {
    /// No such approval.
    #[error("approval not found")]
    NotFound,
    /// The session already has an unresolved approval.
    #[error("approval {0} is already pending for this session")]
    Pending(ApprovalId),
    /// The approval already settled.
    #[error("approval is already {0:?}")]
    AlreadyResolved(ApprovalStatus),
    /// The caller supplied no future decision window.
    #[error("approval expiry {expires_at:?} is not after request time {requested_at:?}")]
    InvalidExpiry {
        /// When the approval was raised.
        requested_at: Timestamp,
        /// The rejected deadline.
        expires_at: Timestamp,
    },
    /// The deadline elapsed while a response was being admitted.
    #[error("approval expired before the decision committed")]
    ExpiryReached {
        /// The terminal expiry commit the caller must persist.
        commit: Box<ApprovalCommit>,
    },
    /// The binding drifted between request and decision.
    #[error("approval binding changed in {} field(s)", fields.len())]
    BindingChanged {
        /// Which fields drifted.
        fields: Vec<BindingField>,
        /// The commit that cancels the approval, which the caller must still
        /// write: failing closed is a decision, not a no-op.
        commit: Box<ApprovalCommit>,
    },
}

/// Raises an approval.
///
/// # Errors
///
/// Returns [`ApprovalRejection::InvalidExpiry`] when the supplied deadline is
/// not in the future, [`ApprovalRejection::Pending`] when the session already
/// has an unresolved approval, and [`ApprovalRejection::BindingChanged`] when
/// the requested binding already disagrees with the current one.
pub fn request_approval(
    id: ApprovalId,
    binding: ApprovalBinding,
    current: &ApprovalBinding,
    unresolved: Option<&Approval>,
    now: Timestamp,
    expires_at: Timestamp,
) -> Result<ApprovalCommit, ApprovalRejection> {
    if expires_at <= now {
        return Err(ApprovalRejection::InvalidExpiry {
            requested_at: now,
            expires_at,
        });
    }
    if let Some(open) = unresolved
        && !open.status.is_resolved()
    {
        return Err(ApprovalRejection::Pending(open.id));
    }
    let drifted = binding_drift(&binding, current);
    if !drifted.is_empty() {
        return Err(ApprovalRejection::BindingChanged {
            fields: drifted,
            commit: Box::new(cancelled(
                id,
                binding,
                ApprovalCancelCause::BindingDrift,
                now,
                expires_at,
                now,
            )),
        });
    }
    Ok(ApprovalCommit {
        approval: Approval {
            id,
            binding,
            status: ApprovalStatus::Pending,
            requested_at: now,
            expires_at,
            resolved_at: None,
            decision: None,
            cancel_cause: None,
        },
        dispatch: false,
        denial_result: None,
    })
}

fn cancelled(
    id: ApprovalId,
    binding: ApprovalBinding,
    cause: ApprovalCancelCause,
    requested_at: Timestamp,
    expires_at: Timestamp,
    now: Timestamp,
) -> ApprovalCommit {
    ApprovalCommit {
        approval: Approval {
            id,
            binding,
            status: ApprovalStatus::Cancelled,
            requested_at,
            expires_at,
            resolved_at: Some(now),
            decision: None,
            cancel_cause: Some(cause),
        },
        dispatch: false,
        denial_result: None,
    }
}

/// Decides an approval.
///
/// The binding is revalidated here, not at request time only: an approver who
/// saw one set of arguments must never authorize a different one.
///
/// # Errors
///
/// Returns [`ApprovalRejection::AlreadyResolved`] for the opposite decision on a
/// settled approval, [`ApprovalRejection::ExpiryReached`] when the deadline won
/// the race, and [`ApprovalRejection::BindingChanged`] on drift. An exact replay
/// of the winning decision is [`ApprovalOutcome::Replayed`]: the stored approval,
/// no commit and therefore no second dispatch.
///
/// The idempotency key is `(approvalId, decision)` — the wire body carries
/// nothing else — and whether the **stored** status was `Pending` is the whole
/// difference between a first decision and a replay. That difference is in the
/// return type so a caller cannot lose it (I's D-8).
pub fn respond(
    approval: &Approval,
    current: &ApprovalBinding,
    decision: ApprovalDecision,
    now: Timestamp,
) -> Result<ApprovalOutcome, ApprovalRejection> {
    if let Some(commit) = expire_pending(approval, now) {
        return Err(ApprovalRejection::ExpiryReached {
            commit: Box::new(commit),
        });
    }
    if approval.status.is_resolved() {
        if approval.decision == Some(decision) {
            return Ok(ApprovalOutcome::Replayed(approval.clone()));
        }
        return Err(ApprovalRejection::AlreadyResolved(approval.status));
    }

    let drifted = binding_drift(&approval.binding, current);
    if !drifted.is_empty() {
        return Err(ApprovalRejection::BindingChanged {
            fields: drifted,
            commit: Box::new(cancelled(
                approval.id,
                approval.binding.clone(),
                ApprovalCancelCause::BindingDrift,
                approval.requested_at,
                approval.expires_at,
                now,
            )),
        });
    }

    let mut settled = approval.clone();
    settled.status = match decision {
        ApprovalDecision::Approve => ApprovalStatus::Approved,
        ApprovalDecision::Deny => ApprovalStatus::Denied,
    };
    settled.decision = Some(decision);
    settled.resolved_at = Some(now);
    Ok(ApprovalOutcome::Committed(ApprovalCommit {
        approval: settled,
        dispatch: decision == ApprovalDecision::Approve,
        // A denial records exactly one canonical tool result and does not
        // cancel the run: the model sees the denial and continues.
        denial_result: (decision == ApprovalDecision::Deny)
            .then_some(ErrorCode::PreconditionFailed),
    }))
}

/// Terminalizes a pending approval once its explicit deadline is reached.
///
/// Expiry is deliberately not a cancellation cause: a stop, binding drift and
/// elapsed time are different customer-visible facts even though all three
/// prevent dispatch.
#[must_use]
pub fn expire_pending(approval: &Approval, now: Timestamp) -> Option<ApprovalCommit> {
    if approval.status != ApprovalStatus::Pending || now < approval.expires_at {
        return None;
    }
    let mut expired = approval.clone();
    expired.status = ApprovalStatus::Expired;
    expired.resolved_at = Some(now);
    expired.decision = None;
    expired.cancel_cause = None;
    Some(ApprovalCommit {
        approval: expired,
        dispatch: false,
        denial_result: None,
    })
}

/// Whether a cause applies to this approval, given the scope it names.
#[must_use]
pub fn cause_in_scope(
    approval: &Approval,
    cause: ApprovalCancelCause,
    scope: &CancelScope,
) -> bool {
    match cause {
        ApprovalCancelCause::SessionDeleting | ApprovalCancelCause::AccountPaused => true,
        ApprovalCancelCause::SessionCancel | ApprovalCancelCause::RunCancelled => {
            scope.run == Some(approval.binding.run)
        }
        ApprovalCancelCause::ContinuityLost => {
            scope.generation.is_some() && scope.generation == approval.binding.expected_generation
        }
        ApprovalCancelCause::ToolCallCancelled => {
            scope.run == Some(approval.binding.run)
                && scope.tool_call == Some(approval.binding.tool_call)
        }
        // Drift is decided by `respond`, never by a scoped sweep.
        ApprovalCancelCause::BindingDrift => false,
    }
}

/// Withdraws a pending approval when the cause matches its scope.
///
/// Returns `None` when the approval is already resolved or the cause does not
/// apply, so a sweep writes nothing for approvals it does not own.
#[must_use]
pub fn cancel_pending(
    approval: &Approval,
    cause: ApprovalCancelCause,
    scope: &CancelScope,
    now: Timestamp,
) -> Option<ApprovalCommit> {
    if approval.status.is_resolved() || !cause_in_scope(approval, cause, scope) {
        return None;
    }
    Some(cancelled(
        approval.id,
        approval.binding.clone(),
        cause,
        approval.requested_at,
        approval.expires_at,
        now,
    ))
}

#[cfg(test)]
mod tests {
    use aex_internal_contracts::RunId;
    use aex_wire::ids::{ApprovalId, PrefixedId as _, Uuid7};
    use aex_wire::types::Timestamp;

    use super::{
        ApprovalCancelCause, ApprovalDecision, ApprovalOutcome, ApprovalRejection, ApprovalStatus,
        BindingField, CancelScope, binding_drift, cancel_pending, expire_pending, request_approval,
        respond,
    };
    use crate::testing::{approval_binding, drift_field};

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn approval_id(tag: u8) -> ApprovalId {
        ApprovalId::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    #[test]
    fn the_binding_has_exactly_eleven_fields_and_each_is_detected() {
        assert_eq!(BindingField::ALL.len(), 11);
        let binding = approval_binding();
        assert!(binding_drift(&binding, &binding).is_empty());
        for field in BindingField::ALL {
            let drifted = drift_field(&binding, field);
            assert_eq!(
                binding_drift(&binding, &drifted),
                vec![field],
                "{field:?} must be detected"
            );
        }
    }

    #[test]
    fn a_second_pending_approval_is_refused() {
        let binding = approval_binding();
        let open = request_approval(
            approval_id(1),
            binding.clone(),
            &binding,
            None,
            moment(0),
            moment(100),
        )
        .expect("raises")
        .approval;
        assert_eq!(
            request_approval(
                approval_id(2),
                binding.clone(),
                &binding,
                Some(&open),
                moment(1),
                moment(100)
            ),
            Err(ApprovalRejection::Pending(approval_id(1)))
        );
    }

    #[test]
    fn drift_fails_closed_and_dispatches_nothing() {
        let binding = approval_binding();
        let pending = request_approval(
            approval_id(1),
            binding.clone(),
            &binding,
            None,
            moment(0),
            moment(100),
        )
        .expect("raises")
        .approval;
        let drifted = drift_field(&binding, BindingField::ArgumentDigest);
        let Err(ApprovalRejection::BindingChanged { fields, commit }) =
            respond(&pending, &drifted, ApprovalDecision::Approve, moment(1))
        else {
            panic!("drift must fail closed");
        };
        assert_eq!(fields, vec![BindingField::ArgumentDigest]);
        assert!(!commit.dispatch);
        assert_eq!(commit.approval.status, ApprovalStatus::Cancelled);
        assert_eq!(
            commit.approval.cancel_cause,
            Some(ApprovalCancelCause::BindingDrift)
        );
    }

    #[test]
    fn deny_records_one_result_and_leaves_the_run_live() {
        let binding = approval_binding();
        let pending = request_approval(
            approval_id(1),
            binding.clone(),
            &binding,
            None,
            moment(0),
            moment(100),
        )
        .expect("raises")
        .approval;
        let ApprovalOutcome::Committed(denied) =
            respond(&pending, &binding, ApprovalDecision::Deny, moment(1)).expect("decides")
        else {
            panic!("the first decision commits");
        };
        assert_eq!(denied.approval.status, ApprovalStatus::Denied);
        assert!(!denied.dispatch);
        assert!(denied.denial_result.is_some());

        // Exact replay returns the stored decision; the opposite one is refused.
        assert_eq!(
            respond(
                &denied.approval,
                &binding,
                ApprovalDecision::Deny,
                moment(2)
            )
            .expect("replays"),
            ApprovalOutcome::Replayed(denied.approval.clone())
        );
        assert_eq!(
            respond(
                &denied.approval,
                &binding,
                ApprovalDecision::Approve,
                moment(2)
            ),
            Err(ApprovalRejection::AlreadyResolved(ApprovalStatus::Denied))
        );
    }

    #[test]
    fn cancellation_fires_exactly_when_the_cause_matches_its_scope() {
        let binding = approval_binding();
        let pending = request_approval(
            approval_id(1),
            binding.clone(),
            &binding,
            None,
            moment(0),
            moment(100),
        )
        .expect("raises")
        .approval;

        // Unconditional causes always apply.
        for cause in [
            ApprovalCancelCause::SessionDeleting,
            ApprovalCancelCause::AccountPaused,
        ] {
            assert!(cancel_pending(&pending, cause, &CancelScope::UNSCOPED, moment(1)).is_some());
        }

        // Run-scoped causes need the matching run.
        let wrong_run = CancelScope {
            run: Some(RunId::from_uuid7(Uuid7::compose(9, [9; 10]))),
            ..CancelScope::UNSCOPED
        };
        assert!(
            cancel_pending(
                &pending,
                ApprovalCancelCause::SessionCancel,
                &wrong_run,
                moment(1)
            )
            .is_none()
        );
        let right_run = CancelScope {
            run: Some(binding.run),
            ..CancelScope::UNSCOPED
        };
        assert!(
            cancel_pending(
                &pending,
                ApprovalCancelCause::RunCancelled,
                &right_run,
                moment(1)
            )
            .is_some()
        );

        // A tool-call cause needs both the run and the call.
        let call_scope = CancelScope {
            run: Some(binding.run),
            tool_call: Some(binding.tool_call),
            generation: None,
        };
        assert!(
            cancel_pending(
                &pending,
                ApprovalCancelCause::ToolCallCancelled,
                &call_scope,
                moment(1)
            )
            .is_some()
        );

        // Drift is never decided by a sweep.
        assert!(
            cancel_pending(
                &pending,
                ApprovalCancelCause::BindingDrift,
                &CancelScope::UNSCOPED,
                moment(1)
            )
            .is_none()
        );
    }

    #[test]
    fn a_replayed_approve_can_never_re_assert_dispatch() {
        let binding = approval_binding();
        let pending = request_approval(
            approval_id(1),
            binding.clone(),
            &binding,
            None,
            moment(0),
            moment(100),
        )
        .expect("raises")
        .approval;
        let first = respond(&pending, &binding, ApprovalDecision::Approve, moment(1))
            .expect("the first decision");
        let commit = first.commit().expect("a first decision commits");
        assert!(commit.dispatch, "the first approve authorises the call");

        let replay = respond(
            first.approval(),
            &binding,
            ApprovalDecision::Approve,
            moment(2),
        )
        .expect("an exact replay is not an error");
        assert_eq!(
            replay,
            ApprovalOutcome::Replayed(commit.approval.clone()),
            "a replay carries the stored approval and no commit"
        );
        assert!(
            replay.commit().is_none(),
            "there is no `dispatch` flag on a replay to act on, which is the point"
        );
    }

    #[test]
    fn expiry_is_a_distinct_terminal_fact_and_never_a_cancel_synonym() {
        let binding = approval_binding();
        let pending = request_approval(
            approval_id(1),
            binding.clone(),
            &binding,
            None,
            moment(1),
            moment(10),
        )
        .expect("requests")
        .approval;

        assert_eq!(pending.status, ApprovalStatus::Pending);
        assert!(expire_pending(&pending, moment(9)).is_none());
        let expired = expire_pending(&pending, moment(10)).expect("deadline reached");
        assert_eq!(expired.approval.status, ApprovalStatus::Expired);
        assert_eq!(expired.approval.resolved_at, Some(moment(10)));
        assert_eq!(expired.approval.cancel_cause, None);
    }
}
