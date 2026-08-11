//! Compilation of application-owned runnable work into `regional-work`.
//!
//! This module is the only bridge from `aex-operation-domain::WorkItem` to the
//! physical work row. The session transaction compiler delegates the whole
//! logical item here, preserving the one-action-per-item invariant.

use sha2::Digest as _;

use aex_operation_domain::{OperationKind, OperationVersion, WorkItem, WorkState};
use aex_session_app::plan::{AgentWake, Condition, Write};
use aex_session_dynamodb::application_plan::{
    AuthorityBinding, ExternalActionCompiler, LogicalAction,
};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{Participant, RegionalTables, TransactionPlan};

use crate::codec::{DeliveryEvidence, Payload, WorkRecord};

/// The production compiler for [`aex_session_app::plan::TableFamily::WorkAuthority`].
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkApplicationCompiler;

impl ExternalActionCompiler for WorkApplicationCompiler {
    fn compile_action(
        &self,
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        match action.write {
            Some(Write::PutWorkItem(work)) => {
                if action.conditions.len() != 1
                    || !matches!(action.conditions[0].1, Condition::ItemAbsent(target) if *target == action.target)
                {
                    return Err(StoreError::Invalid {
                        detail: "a new runnable-work row must carry its exact item-absent election"
                            .to_owned(),
                    });
                }
                let record = record_of(binding, work)?;
                output.put(
                    Participant::WORK_OPERATION_STEP,
                    crate::claim::enqueue(&tables.regional_work, &record)?,
                )?;
            }
            Some(Write::PutAgentWake(wake)) => {
                require_absent(action)?;
                let record = wake_record(binding, wake)?;
                output.put(
                    Participant::WORK_ROOT_WAKE,
                    crate::claim::enqueue(&tables.regional_work, &record)?,
                )?;
            }
            Some(Write::PutAgentWakeDedupe(wake)) => {
                require_absent(action)?;
                let record = wake_record(binding, wake)?;
                output.put(
                    Participant::WORK_DEDUPE,
                    crate::claim::enqueue_dedupe(&tables.regional_work, &record)?,
                )?;
            }
            Some(Write::CompleteWorkItem(completion)) => {
                if !action.conditions.is_empty() {
                    return Err(StoreError::Invalid {
                        detail: "a fenced work completion carries its condition in the claim"
                            .to_owned(),
                    });
                }
                if binding.session.is_none() {
                    return Err(StoreError::Invalid {
                        detail: "operation work completion requires a session binding".to_owned(),
                    });
                }
                if completion.work_id != action.target.partition
                    || action.target.sort != "STATE"
                    || completion.owner.is_empty()
                {
                    return Err(StoreError::Invalid {
                        detail: "the work completion does not match its canonical claim target"
                            .to_owned(),
                    });
                }
                output.update(
                    Participant::WORK_WAKE_DONE,
                    crate::claim::complete_fenced(
                        &tables.regional_work,
                        &completion.work_id,
                        completion.fence,
                        &completion.owner,
                        completion.at,
                    )?,
                )?;
            }
            _ => {
                return Err(StoreError::Invalid {
                    detail: "the work-authority family carries runnable-work writes only"
                        .to_owned(),
                });
            }
        }
        Ok(())
    }
}

fn require_absent(action: &LogicalAction<'_>) -> Result<(), StoreError> {
    if action.conditions.len() == 1
        && matches!(action.conditions[0].1, Condition::ItemAbsent(target) if *target == action.target)
    {
        Ok(())
    } else {
        Err(StoreError::Invalid {
            detail: "a new agent wake item must carry its exact item-absent election".to_owned(),
        })
    }
}

fn wake_record(binding: AuthorityBinding, wake: &AgentWake) -> Result<WorkRecord, StoreError> {
    if binding.session != Some(wake.session)
        || !wake.work_id.starts_with("wrk_")
        || wake.dedupe_key.len() != 64
        || !wake
            .dedupe_key
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(StoreError::Invalid {
            detail: "the root wake disagrees with its tenant or canonical identities".to_owned(),
        });
    }
    Ok(WorkRecord {
        work_id: wake.work_id.clone(),
        workspace: binding.workspace,
        organization: binding.organization,
        session: Some(wake.session),
        agent: Some(wake.agent),
        kind: "agent.wake".to_owned(),
        priority: 0,
        due_at: wake.at,
        state: "pending".to_owned(),
        attempt: 0,
        max_attempts: 8,
        fence: 0,
        claim_owner: None,
        lease_expires_at: None,
        dedupe_key: wake.dedupe_key.clone(),
        payload: Payload::new()
            .set("sessionId", wake.session.to_string())
            .set("agentId", wake.agent.to_string())
            .set("fromSeq", wake.from.0.to_string())
            .set("cancelEpoch", wake.cancellation.0.to_string()),
        delivery: DeliveryEvidence::default(),
        created_at: wake.at,
        updated_at: wake.at,
    })
}

fn record_of(binding: AuthorityBinding, work: &WorkItem) -> Result<WorkRecord, StoreError> {
    let session = binding.session.ok_or_else(|| StoreError::Invalid {
        detail: "operation work requires a session-scoped authenticated binding".to_owned(),
    })?;
    if work.state != WorkState::Runnable
        || work.attempt != 0
        || work.lease.is_some()
        || work.cancel_requested
        || work.dedup.operation != work.operation
        || work.dedup.step != 0
    {
        return Err(StoreError::Invalid {
            detail: "the application compiler admits only a fresh deterministic operation step"
                .to_owned(),
        });
    }
    if !matches!(
        work.kind,
        OperationKind::SessionCancel
            | OperationKind::SessionSuspend
            | OperationKind::SessionResume
            | OperationKind::SessionTerminate
            | OperationKind::SessionDelete
    ) {
        return Err(StoreError::Invalid {
            detail: "the session API may enqueue only a public session lifecycle operation"
                .to_owned(),
        });
    }
    let priority = u8::try_from(work.priority).map_err(|_| StoreError::Invalid {
        detail: "the work priority does not fit the regional-work priority band".to_owned(),
    })?;
    if usize::from(priority) >= crate::keys::PRIORITY_LEAD_SECONDS.len() {
        return Err(StoreError::Invalid {
            detail: "the work priority is outside the closed regional-work priority bands"
                .to_owned(),
        });
    }
    let suffix = work.id.0.encode_suffix();
    let suffix = std::str::from_utf8(&suffix).map_err(|_| StoreError::Invalid {
        detail: "the internal work UUID did not render as Crockford ASCII".to_owned(),
    })?;
    let work_id = format!("wrk_{suffix}");
    let dedupe_key = dedupe_key(work);
    Ok(WorkRecord {
        work_id,
        workspace: binding.workspace,
        organization: binding.organization,
        session: Some(session),
        agent: None,
        kind: "operation.step".to_owned(),
        priority,
        due_at: work.due_at,
        state: "pending".to_owned(),
        attempt: u64::from(work.attempt),
        max_attempts: u64::from(work.max_attempts),
        fence: 0,
        claim_owner: None,
        lease_expires_at: None,
        dedupe_key,
        payload: Payload::new()
            .set("operationId", work.operation.to_string())
            .set("sessionId", session.to_string())
            .set("version", OperationVersion::FIRST.0.to_string()),
        delivery: DeliveryEvidence::default(),
        created_at: work.due_at,
        updated_at: work.due_at,
    })
}

fn dedupe_key(work: &WorkItem) -> String {
    let mut hash = sha2::Sha256::new();
    hash.update(b"aex.operation.step.v1\0");
    hash.update(work.operation.to_string().as_bytes());
    hash.update(b"\0");
    hash.update(work.dedup.step.to_be_bytes());
    hex::encode(hash.finalize())
}

#[cfg(test)]
mod tests {
    use aex_operation_domain::{DedupIdentity, OperationKind, WorkId, WorkItem, WorkState};
    use aex_session_app::plan::{Condition, WorkCompletion, Write};
    use aex_session_dynamodb::application_plan::{
        AuthorityBinding, ExternalActionCompiler, LogicalAction,
    };
    use aex_session_dynamodb::plan::{RegionalTables, TransactionPlan};
    use aex_wire::ids::{
        OperationId, OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId,
    };
    use aex_wire::types::Timestamp;

    use super::*;

    fn id<T: aex_wire::ids::PrefixedId>(tag: u8) -> T {
        T::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn stamp() -> Timestamp {
        Timestamp::from_unix_millis(10).expect("in range")
    }

    fn work() -> WorkItem {
        let operation = id::<OperationId>(1);
        let id = WorkId(operation.uuid7());
        WorkItem {
            id,
            operation,
            kind: OperationKind::SessionSuspend,
            due_at: stamp(),
            priority: 0,
            attempt: 0,
            max_attempts: 5,
            lease: None,
            state: WorkState::Runnable,
            dedup: DedupIdentity { operation, step: 0 },
            cancel_requested: false,
        }
    }

    #[test]
    fn a_fresh_operation_step_becomes_one_typed_immutable_work_row() {
        let work = work();
        let write = Write::PutWorkItem(Box::new(work));
        let target = write.target();
        let condition = Condition::ItemAbsent(target.clone());
        let action = LogicalAction {
            target,
            conditions: vec![(aex_session_app::plan::ConditionId(0), &condition)],
            write: Some(&write),
        };
        let binding = AuthorityBinding {
            workspace: id::<WorkspaceId>(2),
            organization: id::<OrganizationId>(3),
            session: Some(id::<SessionId>(4)),
        };
        let tables = RegionalTables::composed("dev", "eu-west-1");
        let mut output = TransactionPlan::new("work-compiler-test");
        WorkApplicationCompiler
            .compile_action(&tables, binding, &action, &mut output)
            .expect("compiles");
        assert_eq!(output.len(), 1);
        assert_eq!(
            output.participants(),
            &[aex_session_dynamodb::plan::Participant::WORK_OPERATION_STEP]
        );
    }

    #[test]
    fn a_non_elected_work_write_is_refused() {
        let write = Write::PutWorkItem(Box::new(work()));
        let target = write.target();
        let action = LogicalAction {
            target,
            conditions: Vec::new(),
            write: Some(&write),
        };
        let binding = AuthorityBinding {
            workspace: id::<WorkspaceId>(2),
            organization: id::<OrganizationId>(3),
            session: Some(id::<SessionId>(4)),
        };
        let tables = RegionalTables::composed("dev", "eu-west-1");
        let mut output = TransactionPlan::new("work-compiler-test");
        assert!(
            WorkApplicationCompiler
                .compile_action(&tables, binding, &action, &mut output)
                .is_err()
        );
    }

    #[test]
    fn an_exact_claim_completion_becomes_one_fenced_retirement() {
        let write = Write::CompleteWorkItem(Box::new(WorkCompletion {
            work_id: "wrk_00000000000000000000000001".to_owned(),
            fence: 7,
            owner: "session-operation-worker:test".to_owned(),
            at: stamp(),
        }));
        let action = LogicalAction {
            target: write.target(),
            conditions: Vec::new(),
            write: Some(&write),
        };
        let binding = AuthorityBinding {
            workspace: id::<WorkspaceId>(2),
            organization: id::<OrganizationId>(3),
            session: Some(id::<SessionId>(4)),
        };
        let tables = RegionalTables::composed("dev", "eu-west-1");
        let mut output = TransactionPlan::new("work-completion-test");
        WorkApplicationCompiler
            .compile_action(&tables, binding, &action, &mut output)
            .expect("compiles");
        assert_eq!(output.len(), 1);
        assert_eq!(
            output.participants(),
            &[aex_session_dynamodb::plan::Participant::WORK_WAKE_DONE]
        );
    }
}
