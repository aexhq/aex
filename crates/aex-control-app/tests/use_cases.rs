//! Control-command behaviour against scripted ports, including the whole
//! unknown-outcome matrix for workspace provisioning.
//!
//! "Eventually returned 200" is not a recovery test. Every row below asserts a
//! durable fact: which authority holds what, and whether the workspace is
//! visible.

use std::sync::Mutex;

use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use aex_control_app::ports::{
    AcceptInvitationsTx, BeginWorkspaceDeletionTx, BeginWorkspaceProvisionTx, ClaimDueOperations,
    ClaimOutbox, CompleteWorkspaceDeletionTx, ControlStore, CreateApiKeyTx, CreateInvitationTx,
    CreateOrganizationTx, DeleteWorkspaceRequest, DeleteWorkspaceResponse, EffectError,
    FinishWorkspaceProvisionTx, GcExpired, GcReport, IdempotencyRecordKey, ListApiKeys,
    ListOperations, ListOrganizations, ListWorkspaces, Page, PageRequest,
    ProvisionWorkspaceRequest, ProvisionWorkspaceResponse, RegionalControlPort, RevokeApiKeyTx,
    StoreError, TxOutcome, UnknownCommit,
};
use aex_control_app::use_cases::{
    CancelOperation, ControlError, CreateApiKey, CreateWorkspace, ceremony,
};
use aex_control_domain::{
    ActorKind, ApiKey, AuditEvent, AuditOutcome, Fence, IdempotencyKeyKind, IntentHash, Invitation,
    Membership, Operation, OperationKind, OperationStatus, Organization, OrganizationStatus,
    OutboxMessage, PrincipalKindTag, ResourceKind, Revision, Scope, ScopeKind, ScopeSet, Slug,
    Topic, Workspace, WorkspaceStatus,
};
use aex_identity_app::ports::ReconcileIdentity;
use aex_wire::types::{HttpMethod, Region};

const WORKSPACE: u128 = 0x0192_3f2a_1c00_7000_8000_0000_0000_0011;
const OTHER_WORKSPACE: u128 = 0x0192_3f2a_1c00_7000_8000_0000_0000_0099;
const OPERATION: u128 = 0x0192_3f2a_1c00_7000_8000_0000_0000_0021;
const ORGANIZATION: u128 = 0x0192_3f2a_1c00_7000_8000_0000_0000_0031;

fn at() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}

fn audit() -> AuditEvent {
    AuditEvent {
        id: Uuid::from_u128(0x41),
        organization_id: Some(Uuid::from_u128(ORGANIZATION)),
        workspace_id: Some(Uuid::from_u128(WORKSPACE)),
        actor_kind: ActorKind::User,
        actor_id: Some(Uuid::from_u128(0x51)),
        action: "workspace.create".to_owned(),
        resource_kind: ResourceKind::Workspace,
        resource_id: Some(Uuid::from_u128(WORKSPACE)),
        outcome: AuditOutcome::Allowed,
        request_id: "req-1".to_owned(),
        operation_id: Some(Uuid::from_u128(OPERATION)),
        detail: serde_json::json!({ "region": "eu-west-1" }),
        occurred_at: at(),
    }
}

fn idempotency() -> IdempotencyRecordKey {
    IdempotencyRecordKey {
        id: Uuid::from_u128(0x61),
        key_kind: IdempotencyKeyKind::IdempotencyKey,
        key_value: "k-1".to_owned(),
        principal_kind: PrincipalKindTag::AccountActor,
        principal_id: Uuid::from_u128(0x51),
        scope_kind: ScopeKind::Organization,
        scope_id: Uuid::from_u128(ORGANIZATION),
        method: HttpMethod::Post,
        route: "/api/workspaces".to_owned(),
        intent_hash: IntentHash::from_bytes([7_u8; 32]),
        expires_at: at() + Duration::hours(24),
    }
}

fn outbox() -> OutboxMessage {
    OutboxMessage {
        id: Uuid::from_u128(0x71),
        topic: Topic::WorkspaceProvisionRequested,
        dedupe_key: Uuid::from_u128(WORKSPACE).to_string(),
        group_key: Uuid::from_u128(ORGANIZATION).to_string(),
        payload: serde_json::json!({}),
        attempts: 0,
        available_at: at(),
        claimed_by: None,
        claimed_until: None,
        dispatched_at: None,
        last_error: None,
        created_at: at(),
    }
}

fn begin() -> BeginWorkspaceProvisionTx {
    BeginWorkspaceProvisionTx {
        preassigned_workspace_id: Uuid::from_u128(WORKSPACE),
        preassigned_operation_id: Uuid::from_u128(OPERATION),
        organization_id: Uuid::from_u128(ORGANIZATION),
        name: "Prod".to_owned(),
        slug: Slug::parse("prod").expect("a valid slug"),
        region: Region::EuWest1,
        created_by_user_id: Uuid::from_u128(0x51),
        idempotency: idempotency(),
        outbox: outbox(),
        audit: audit(),
        completion_audit_id: Uuid::from_u128(0x42),
        now: at(),
    }
}

fn api_key() -> ApiKey {
    ApiKey {
        id: Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0081),
        workspace_id: Uuid::from_u128(WORKSPACE),
        organization_id: Uuid::from_u128(ORGANIZATION),
        name: "ci".to_owned(),
        scopes: ScopeSet::of(&[Scope::SessionsRead]),
        region: Region::EuWest1,
        pepper_version: 1,
        created_at: at(),
        revoked_at: None,
        revision: Revision::INITIAL,
        created_by_user_id: Uuid::from_u128(0x51),
    }
}

fn create_api_key() -> CreateApiKeyTx {
    let key = api_key();
    CreateApiKeyTx {
        preassigned_id: key.id,
        workspace_id: key.workspace_id,
        organization_id: key.organization_id,
        name: key.name,
        scopes: key.scopes,
        region: key.region,
        verifier: [9; 32],
        pepper_version: 1,
        created_by_user_id: key.created_by_user_id,
        outbox: outbox(),
        idempotency: idempotency(),
        audit: audit(),
        now: at(),
    }
}

fn provisioning_workspace() -> Workspace {
    Workspace {
        id: Uuid::from_u128(WORKSPACE),
        organization_id: Uuid::from_u128(ORGANIZATION),
        name: "Prod".to_owned(),
        slug: Slug::parse("prod").expect("a valid slug"),
        region: Region::EuWest1,
        status: WorkspaceStatus::Provisioning,
        provision_operation_id: Uuid::from_u128(OPERATION),
        provision_fence: Fence::FIRST,
        deletion_operation_id: None,
        deletion_fence: None,
        revision: Revision::INITIAL,
        created_at: at(),
        updated_at: at(),
        activated_at: None,
        deleted_at: None,
        created_by_user_id: Uuid::from_u128(0x51),
    }
}

fn operation(status: OperationStatus) -> Operation {
    Operation {
        id: Uuid::from_u128(OPERATION),
        kind: OperationKind::WorkspaceProvision,
        visibility: OperationKind::WorkspaceProvision.visibility(),
        organization_id: Uuid::from_u128(ORGANIZATION),
        workspace_id: Some(Uuid::from_u128(WORKSPACE)),
        principal_id: Uuid::from_u128(0x51),
        scopes: ScopeSet::ADMIN,
        status,
        intent_hash: IntentHash::from_bytes([7_u8; 32]),
        fence: Fence::FIRST,
        attempt: 0,
        lease: None,
        created_at: at(),
        started_at: None,
        updated_at: at(),
        terminal_at: None,
        due_at: None,
    }
}

/// What the scripted store was told to answer.
#[derive(Debug, Clone)]
struct Script {
    begin: Option<TxOutcome<(Workspace, Operation)>>,
    finish: Option<Result<TxOutcome<Workspace>, StoreError>>,
    finish_audit_ids: Vec<Uuid>,
    operation: Option<Operation>,
    key: Option<TxOutcome<ApiKey>>,
    calls: Vec<&'static str>,
}

#[derive(Debug)]
struct Store(Mutex<Script>);

impl Store {
    fn new(begin: TxOutcome<(Workspace, Operation)>) -> Self {
        Self(Mutex::new(Script {
            begin: Some(begin),
            finish: Some(Ok(TxOutcome::Committed(Workspace {
                status: WorkspaceStatus::Active,
                activated_at: Some(at()),
                ..provisioning_workspace()
            }))),
            finish_audit_ids: Vec::new(),
            operation: None,
            key: None,
            calls: Vec::new(),
        }))
    }

    fn finish_answer(&self, answer: Result<TxOutcome<Workspace>, StoreError>) {
        self.0.lock().expect("script").finish = Some(answer);
    }

    fn with_operation(&self, operation: Operation) {
        self.0.lock().expect("script").operation = Some(operation);
    }

    fn key_answer(&self, answer: TxOutcome<ApiKey>) {
        self.0.lock().expect("script").key = Some(answer);
    }

    fn calls(&self) -> Vec<&'static str> {
        self.0.lock().expect("script").calls.clone()
    }

    fn finish_audit_ids(&self) -> Vec<Uuid> {
        self.0.lock().expect("script").finish_audit_ids.clone()
    }

    fn record(&self, name: &'static str) {
        self.0.lock().expect("script").calls.push(name);
    }
}

/// Every method this suite does not drive. Reaching one is a test bug, not a
/// silent default: a scripted port that invents an answer proves nothing.
const UNDRIVEN: &str = "this suite drives the provisioning and cancellation paths only";

#[async_trait]
impl ControlStore for Store {
    async fn create_organization(
        &self,
        _command: &CreateOrganizationTx,
    ) -> Result<TxOutcome<Organization>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn list_organizations(
        &self,
        _query: &ListOrganizations,
    ) -> Result<Page<Organization>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn get_organization(&self, _id: Uuid) -> Result<Option<Organization>, StoreError> {
        Ok(Some(Organization {
            id: Uuid::from_u128(ORGANIZATION),
            name: "Acme".to_owned(),
            slug: Slug::parse("acme").expect("a valid slug"),
            status: OrganizationStatus::Active,
            revision: Revision::INITIAL,
            created_at: at(),
            updated_at: at(),
            created_by_user_id: Uuid::from_u128(0x51),
        }))
    }

    async fn list_memberships(
        &self,
        _organization_id: Uuid,
        _page: &PageRequest,
    ) -> Result<Page<Membership>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn create_invitation(
        &self,
        _command: &CreateInvitationTx,
    ) -> Result<TxOutcome<Invitation>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn accept_invitations_for_email(
        &self,
        _command: &AcceptInvitationsTx,
    ) -> Result<TxOutcome<Vec<Membership>>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn begin_workspace_provision(
        &self,
        _command: &BeginWorkspaceProvisionTx,
    ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError> {
        self.record("begin");
        Ok(self
            .0
            .lock()
            .expect("script")
            .begin
            .clone()
            .expect("a begin answer was programmed"))
    }

    async fn finish_workspace_provision(
        &self,
        command: &FinishWorkspaceProvisionTx,
    ) -> Result<TxOutcome<Workspace>, StoreError> {
        self.record("finish");
        let mut script = self.0.lock().expect("script");
        script.finish_audit_ids.push(command.audit.id);
        script
            .finish
            .clone()
            .expect("a finish answer was programmed")
    }

    async fn list_workspaces(
        &self,
        _query: &ListWorkspaces,
    ) -> Result<Page<Workspace>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn get_workspace(&self, _id: Uuid) -> Result<Option<Workspace>, StoreError> {
        Ok(Some(provisioning_workspace()))
    }

    async fn begin_workspace_deletion(
        &self,
        _command: &BeginWorkspaceDeletionTx,
    ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn complete_workspace_deletion(
        &self,
        _command: &CompleteWorkspaceDeletionTx,
    ) -> Result<TxOutcome<Workspace>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn create_api_key(
        &self,
        _command: &CreateApiKeyTx,
    ) -> Result<TxOutcome<ApiKey>, StoreError> {
        self.record("create_api_key");
        Ok(self
            .0
            .lock()
            .expect("script")
            .key
            .clone()
            .expect("a key answer was programmed"))
    }

    async fn get_api_key(&self, _id: Uuid) -> Result<Option<ApiKey>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn list_api_keys(&self, _query: &ListApiKeys) -> Result<Page<ApiKey>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn revoke_api_key(&self, _command: &RevokeApiKeyTx) -> Result<TxOutcome<()>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn get_operation(&self, _id: Uuid) -> Result<Option<Operation>, StoreError> {
        Ok(self.0.lock().expect("script").operation.clone())
    }

    async fn list_operations(
        &self,
        _query: &ListOperations,
    ) -> Result<Page<Operation>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn claim_due_operations(
        &self,
        _command: &ClaimDueOperations,
    ) -> Result<Vec<Operation>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn claim_outbox(&self, _command: &ClaimOutbox) -> Result<Vec<OutboxMessage>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn enqueue_outbox(&self, _message: &OutboxMessage) -> Result<(), StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn mark_outbox_dispatched(
        &self,
        _id: Uuid,
        _now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn release_outbox(
        &self,
        _id: Uuid,
        _available_at: OffsetDateTime,
        _error: &str,
    ) -> Result<(), StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn gc_expired(&self, _command: &GcExpired) -> Result<GcReport, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn account_state(
        &self,
        _organization_id: Uuid,
    ) -> Result<aex_control_domain::AccountState, StoreError> {
        unreachable!("{UNDRIVEN}")
    }
}

/// A region programmed with one answer.
#[derive(Debug)]
struct Regional(
    Mutex<(
        Option<Result<ProvisionWorkspaceResponse, EffectError>>,
        usize,
    )>,
);

impl Regional {
    fn new(answer: Result<ProvisionWorkspaceResponse, EffectError>) -> Self {
        Self(Mutex::new((Some(answer), 0)))
    }

    fn calls(&self) -> usize {
        self.0.lock().expect("regional").1
    }
}

#[async_trait]
impl RegionalControlPort for Regional {
    async fn provision_workspace(
        &self,
        _request: &ProvisionWorkspaceRequest,
    ) -> Result<ProvisionWorkspaceResponse, EffectError> {
        let mut state = self.0.lock().expect("regional");
        state.1 += 1;
        state.0.clone().expect("an answer was programmed")
    }

    async fn delete_workspace(
        &self,
        _request: &DeleteWorkspaceRequest,
    ) -> Result<DeleteWorkspaceResponse, EffectError> {
        unreachable!("{UNDRIVEN}")
    }
}

/// The region's answer when it created the preassigned workspace.
fn created() -> ProvisionWorkspaceResponse {
    ProvisionWorkspaceResponse {
        workspace_id: Uuid::from_u128(WORKSPACE),
        created: true,
    }
}

fn body() -> serde_json::Value {
    serde_json::json!({ "id": "wsp_x" })
}

fn committed_store() -> Store {
    Store::new(TxOutcome::Committed((
        provisioning_workspace(),
        operation(OperationStatus::Queued),
    )))
}

// --- the unknown-outcome matrix ---------------------------------------------

#[tokio::test]
async fn row_one_the_happy_path_activates_the_workspace() {
    let store = committed_store();
    let regional = Regional::new(Ok(created()));
    let command = begin();
    let provisioned = CreateWorkspace::run(&store, &regional, &command, body(), at())
        .await
        .expect("both halves are durable");
    assert_eq!(provisioned.workspace.status, WorkspaceStatus::Active);
    assert!(provisioned.workspace.is_publicly_visible());
    assert_eq!(store.calls(), vec!["begin", "finish"]);
    assert_ne!(
        store.finish_audit_ids(),
        vec![command.audit.id],
        "the two provisioning transactions must not insert the same audit primary key"
    );
    assert_eq!(store.finish_audit_ids(), vec![command.completion_audit_id]);
}

#[tokio::test]
async fn row_two_a_failure_before_the_effect_leaves_nothing_and_calls_no_region() {
    let store = Store::new(TxOutcome::Unknown(UnknownCommit {
        identity: ReconcileIdentity {
            ceremony: ceremony::BEGIN_WORKSPACE_PROVISION,
            id: Uuid::from_u128(WORKSPACE),
        },
    }));
    let regional = Regional::new(Ok(created()));
    let error = CreateWorkspace::run(&store, &regional, &begin(), body(), at())
        .await
        .expect_err("an unknown T1 does not proceed");
    assert_eq!(
        error,
        ControlError::CommitOutcomeUnknown {
            ceremony: ceremony::BEGIN_WORKSPACE_PROVISION,
            id: Uuid::from_u128(WORKSPACE)
        }
    );
    assert!(error.retryable());
    assert_eq!(regional.calls(), 0, "no regional effect was attempted");
}

#[tokio::test]
async fn row_three_a_lost_regional_response_leaves_the_workspace_hidden_and_retryable() {
    let store = committed_store();
    let regional = Regional::new(Err(EffectError::Unknown));
    let error = CreateWorkspace::run(&store, &regional, &begin(), body(), at())
        .await
        .expect_err("an unknown effect is never a success");
    assert_eq!(error, ControlError::WorkspaceProvisionPending);
    assert!(error.retryable());
    assert_eq!(store.calls(), vec!["begin"], "T2 never ran");
}

#[tokio::test]
async fn row_four_an_unavailable_region_is_also_pending_rather_than_a_failure() {
    let store = committed_store();
    let regional = Regional::new(Err(EffectError::Unavailable));
    assert_eq!(
        CreateWorkspace::run(&store, &regional, &begin(), body(), at()).await,
        Err(ControlError::WorkspaceProvisionPending)
    );
}

#[tokio::test]
async fn row_five_a_retryable_rejection_is_pending_and_a_permanent_one_is_fatal() {
    let store = committed_store();
    let regional = Regional::new(Err(EffectError::Rejected {
        code: "regional_capacity",
        retryable: true,
    }));
    assert_eq!(
        CreateWorkspace::run(&store, &regional, &begin(), body(), at()).await,
        Err(ControlError::WorkspaceProvisionPending)
    );

    let store = committed_store();
    let regional = Regional::new(Err(EffectError::Rejected {
        code: "region_not_enabled",
        retryable: false,
    }));
    assert_eq!(
        CreateWorkspace::run(&store, &regional, &begin(), body(), at()).await,
        Err(ControlError::Fatal("region_not_enabled".to_owned()))
    );
}

#[tokio::test]
async fn row_six_a_lost_t2_commit_is_pending_rather_than_a_false_two_hundred_and_one() {
    let store = committed_store();
    store.finish_answer(Err(StoreError::Unknown));
    let regional = Regional::new(Ok(created()));
    assert_eq!(
        CreateWorkspace::run(&store, &regional, &begin(), body(), at()).await,
        Err(ControlError::WorkspaceProvisionPending)
    );
    assert_eq!(store.calls(), vec!["begin", "finish"]);
}

#[tokio::test]
async fn row_seven_a_replayed_identity_returns_the_recorded_workspace() {
    let store = Store::new(TxOutcome::Replayed((
        provisioning_workspace(),
        operation(OperationStatus::Queued),
    )));
    let regional = Regional::new(Ok(created()));
    let provisioned = CreateWorkspace::run(&store, &regional, &begin(), body(), at())
        .await
        .expect("a replay resolves to the same workspace");
    assert_eq!(provisioned.workspace.id, Uuid::from_u128(WORKSPACE));
}

#[tokio::test]
async fn row_eight_a_changed_intent_under_the_same_key_conflicts() {
    let store = Store::new(TxOutcome::IntentConflict);
    let regional = Regional::new(Ok(created()));
    let error = CreateWorkspace::run(&store, &regional, &begin(), body(), at())
        .await
        .expect_err("a changed intent is a conflict");
    assert_eq!(error, ControlError::IntentConflict);
    assert!(!error.retryable());
    assert_eq!(regional.calls(), 0);
}

#[tokio::test]
async fn a_region_answering_about_another_workspace_is_never_accepted() {
    let store = committed_store();
    let regional = Regional::new(Ok(ProvisionWorkspaceResponse {
        workspace_id: Uuid::from_u128(OTHER_WORKSPACE),
        created: true,
    }));
    let error = CreateWorkspace::run(&store, &regional, &begin(), body(), at())
        .await
        .expect_err("the region must answer about the preassigned workspace");
    assert!(matches!(error, ControlError::Fatal(_)), "{error:?}");
    assert_eq!(store.calls(), vec!["begin"], "T2 never ran");
}

#[test]
fn the_workspace_id_is_preassigned_so_a_retry_reconciles_rather_than_recreates() {
    let first = begin();
    let second = begin();
    assert_eq!(
        first.preassigned_workspace_id, second.preassigned_workspace_id,
        "the same intent names the same workspace"
    );
    assert_eq!(
        first.idempotency.intent_hash,
        second.idempotency.intent_hash
    );
}

#[tokio::test]
async fn an_api_key_replay_is_metadata_only_and_is_never_mistaken_for_a_second_secret() {
    let store = Store::new(TxOutcome::Unknown(UnknownCommit {
        identity: ReconcileIdentity {
            ceremony: ceremony::BEGIN_WORKSPACE_PROVISION,
            id: Uuid::from_u128(WORKSPACE),
        },
    }));
    store.key_answer(TxOutcome::Committed(api_key()));
    let first = CreateApiKey::run(&store, &create_api_key())
        .await
        .expect("first mint");
    assert!(first.first);

    store.key_answer(TxOutcome::Replayed(api_key()));
    let replay = CreateApiKey::run(&store, &create_api_key())
        .await
        .expect("metadata replay");
    assert!(
        !replay.first,
        "the edge must return api_key_secret_unavailable"
    );
    assert_eq!(replay.key, first.key);
}

// --- cancellation ------------------------------------------------------------

#[tokio::test]
async fn cancelling_a_central_operation_always_reports_not_cancelable() {
    for kind in OperationKind::ALL {
        let store = committed_store();
        let mut running = operation(OperationStatus::Running);
        running.kind = kind;
        running.visibility = kind.visibility();
        store.with_operation(running);
        assert_eq!(
            CancelOperation::run(&store, Uuid::from_u128(OPERATION), at()).await,
            Err(ControlError::NotCancelable),
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn cancelling_a_finished_operation_refuses_rather_than_reporting_success() {
    // The route has no success path at all, so a `200` carrying the unchanged
    // row is indistinguishable from a cancellation that worked. Every terminal
    // status refuses with the one code the route declares.
    for status in [
        OperationStatus::Succeeded,
        OperationStatus::Failed,
        OperationStatus::Cancelled,
    ] {
        let store = committed_store();
        let mut done = operation(status);
        done.terminal_at = Some(at());
        store.with_operation(done);
        assert_eq!(
            CancelOperation::run(&store, Uuid::from_u128(OPERATION), at()).await,
            Err(ControlError::NotCancelable),
            "{status:?}"
        );
    }
}

#[tokio::test]
async fn cancelling_an_unknown_operation_is_not_found() {
    let store = committed_store();
    assert_eq!(
        CancelOperation::run(&store, Uuid::from_u128(OPERATION), at()).await,
        Err(ControlError::NotFound)
    );
}
