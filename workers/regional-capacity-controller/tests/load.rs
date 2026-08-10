//! Bounded local performance contract for capacity reconciliation.

use std::sync::Mutex;

use aex_capacity_dynamodb::{
    AppliedCapacity, CapacityCommand, CapacityDefaults, CapacityState, CapacityStoreError,
    canonical_defaults, plan_capacity_change,
};
use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use regional_capacity_controller::{
    CapacityController, Clock, ControllerRequest, ControllerResponse, Store,
};

struct MemoryStore(Mutex<Option<CapacityState>>);

#[async_trait]
impl Store for MemoryStore {
    async fn apply(
        &self,
        defaults: &CapacityDefaults,
        command: &CapacityCommand,
        now: Timestamp,
    ) -> Result<AppliedCapacity, CapacityStoreError> {
        let mut state = self.0.lock().expect("lock");
        let planned = plan_capacity_change(state.as_ref(), defaults, command, now)?;
        if planned.changed {
            *state = Some(planned.state.clone());
        }
        Ok(AppliedCapacity {
            state: planned.state,
            changed: planned.changed,
        })
    }

    async fn load(
        &self,
        _workspace: WorkspaceId,
    ) -> Result<Option<CapacityState>, aex_session_dynamodb::error::StoreError> {
        Ok(self.0.lock().expect("lock").clone())
    }

    async fn page_workspaces(
        &self,
        _budget: u32,
        _after: Option<WorkspaceId>,
    ) -> Result<Vec<WorkspaceId>, aex_session_dynamodb::error::StoreError> {
        unreachable!("this contract measures one workspace's reconcile, never a walk")
    }
}

#[derive(Clone, Copy)]
struct FixedClock(Timestamp);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

#[tokio::test]
async fn no_op_reconciliation_remains_stable_under_load() {
    let workspace_id = WorkspaceId::from_uuid7(Uuid7::compose(1, [9; 10]));
    let controller = CapacityController::new(
        MemoryStore(Mutex::new(None)),
        canonical_defaults().expect("defaults"),
        FixedClock(Timestamp::from_unix_millis(1).expect("timestamp")),
    );
    controller
        .handle(ControllerRequest::Command(CapacityCommand::Bootstrap {
            workspace_id,
        }))
        .await
        .expect("bootstrap");

    for _ in 0..10_000 {
        let response = controller
            .handle(ControllerRequest::Command(CapacityCommand::Reconcile {
                workspace_id,
                expected_revision: 1,
            }))
            .await
            .expect("reconcile");
        assert!(matches!(
            response,
            ControllerResponse::Applied {
                state,
                changed: false,
            } if state.revision == 1
        ));
    }
}
