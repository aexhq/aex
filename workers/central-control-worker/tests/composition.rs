//! `central-control-worker`: the trigger, the drain and the order of a provision.
//!
//! The personal workspace and API-key routes commit authority rows and an
//! outbox message in one transaction. These tests prove both trigger shapes
//! reach the one launch-region projection, and that failures remain claimable.
//!
//! # The shared harness
//!
//! [`Harness`] records every projection effect in call order so publication-
//! before-central-activation remains an executable fact.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use aex_control_app::ports::{
    AcceptInvitationsTx, AccountProjection, BeginWorkspaceDeletionTx, BeginWorkspaceProvisionTx,
    ClaimDueOperations, ClaimOutbox, CompleteWorkspaceDeletionTx, ControlStore, ControlViewStore,
    CreateApiKeyTx, CreateInvitationTx, CreateOrganizationTx, FinishWorkspaceProvisionTx,
    GcExpired, GcReport, KeyMaterialReader, ListApiKeys, ListOperations, ListOrganizations,
    ListWorkspaces, MembershipView, OperationView, OrganizationView, Page, PageRequest,
    RevokeApiKeyTx, StoreError, TxOutcome, UserIdentity, WorkspaceKeyMaterial, WorkspaceView,
};
use aex_control_domain::{
    AccountProfile, AccountState, ApiKey, Fence, IntentHash, Invitation, Membership, Operation,
    OperationKind, OperationStatus, OperationVisibility, Organization, OutboxMessage, Revision,
    ScopeSet, Slug, Topic, Workspace, WorkspaceStatus,
};
use aex_session_dynamodb::projection_write::{KeyAuthorizationWrite, PlacementWrite};
use aex_wire::types::{Region, Timestamp};
use async_trait::async_trait;
use central_control_worker::runtime::{
    DrainSettings,
    OutboxWake, RegionalProjection, WakeDelay, WakeInvoker, Worker,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// The one region every fixture is placed in.
const REGION: Region = Region::EuWest1;

/// A stable `UUIDv7` for `n`, so a failure names the fixture it came from.
fn id(n: u8) -> Uuid {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[15] = n;
    Uuid::from_bytes(bytes)
}

fn workspace_id() -> Uuid {
    id(1)
}
fn organization_id() -> Uuid {
    id(2)
}
fn operation_id() -> Uuid {
    id(3)
}
fn api_key_id() -> Uuid {
    id(4)
}
fn idempotency_id() -> Uuid {
    id(5)
}

fn epoch() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + Duration::days(20_000)
}

fn intent() -> IntentHash {
    IntentHash::from_bytes([7_u8; 32])
}

/// The hexadecimal rendering the provision payload has to agree with.
fn intent_hex() -> String {
    use std::fmt::Write as _;

    intent()
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut rendered, byte| {
            let _ = write!(rendered, "{byte:02x}");
            rendered
        })
}

fn workspace(status: WorkspaceStatus) -> Workspace {
    Workspace {
        id: workspace_id(),
        organization_id: organization_id(),
        name: "Fixture".to_owned(),
        slug: Slug::parse("fixture").expect("a valid slug"),
        region: REGION,
        status,
        provision_operation_id: operation_id(),
        provision_fence: Fence::FIRST,
        deletion_operation_id: None,
        deletion_fence: None,
        revision: Revision::INITIAL,
        created_at: epoch(),
        updated_at: epoch(),
        activated_at: None,
        deleted_at: None,
        created_by_user_id: id(6),
    }
}

fn operation(kind: OperationKind, status: OperationStatus) -> Operation {
    Operation {
        id: operation_id(),
        kind,
        visibility: OperationVisibility::Public,
        organization_id: organization_id(),
        workspace_id: Some(workspace_id()),
        principal_id: id(6),
        scopes: ScopeSet::EMPTY,
        status,
        intent_hash: intent(),
        fence: Fence::FIRST,
        attempt: 0,
        lease: None,
        created_at: epoch(),
        started_at: None,
        updated_at: epoch(),
        terminal_at: None,
        due_at: None,
    }
}

fn view(status: WorkspaceStatus, account_state: AccountState) -> WorkspaceView {
    let paused = account_state == AccountState::PausedTopUpRequired;
    WorkspaceView {
        workspace: workspace(status),
        account: AccountProjection {
            profile: AccountProfile {
                state: account_state,
                reason: paused.then(|| "top_up_required".to_owned()),
                revision: if paused { 2 } else { 1 },
                changed_at: epoch(),
            },
            epoch: if paused { 2 } else { 1 },
        },
        workspace_epoch: 1,
    }
}

/// One outbox row, exactly as the three routes commit it.
fn message(n: u8, topic: Topic, payload: serde_json::Value) -> OutboxMessage {
    OutboxMessage {
        id: id(n),
        topic,
        dedupe_key: format!("dedupe-{n}"),
        group_key: organization_id().to_string(),
        payload,
        attempts: 0,
        available_at: epoch(),
        claimed_by: None,
        claimed_until: None,
        dispatched_at: None,
        last_error: None,
        created_at: epoch(),
    }
}

/// `api_key_create`'s message: the key exists centrally and nowhere else.
///
/// `changedAt` carries the exact 24-character wire spelling the route commits
/// (`Timestamp::to_wire`), not `time`'s local serde representation, so the
/// fixture models the cross-service shape the worker parses in production.
fn api_key_created() -> OutboxMessage {
    message(
        10,
        Topic::ApiKeyCreated,
        serde_json::json!({
            "apiKeyId": api_key_id(),
            "workspaceId": workspace_id(),
            "organizationId": organization_id(),
            "region": REGION.as_str(),
            "changedAt": Timestamp::from_datetime_trunc_ms(epoch())
                .expect("the fixture instant is wire-representable")
                .to_wire(),
            "epoch": 0,
        }),
    )
}

fn account_state_changed() -> OutboxMessage {
    message(
        13,
        Topic::AccountStateChanged,
        serde_json::json!({
            "workspaceId": workspace_id(),
            "organizationId": organization_id(),
            "region": REGION.as_str(),
            "accountEpoch": 2,
            "accountRevision": 2,
            "changedAtMs": u64::try_from(epoch().unix_timestamp_nanos() / 1_000_000)
                .expect("the fixture timestamp is positive"),
        }),
    )
}

/// Every externally visible effect, in the order the worker produced it.
#[derive(Debug, Default)]
struct Journal(Mutex<Vec<String>>);

impl Journal {
    fn record(&self, effect: impl Into<String>) {
        self.0
            .lock()
            .expect("the journal lock is never poisoned")
            .push(effect.into());
    }

    fn entries(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("the journal lock is never poisoned")
            .clone()
    }

    fn count(&self, prefix: &str) -> usize {
        self.entries()
            .iter()
            .filter(|entry| entry.starts_with(prefix))
            .count()
    }
}

/// The control authority, holding exactly the rows the worker reads.
///
/// Every method the worker never calls is `unreachable!`: a fake that answered
/// a question the production path does not ask would let this suite pass while
/// the worker read something else entirely.
struct FakeStore {
    journal: Arc<Journal>,
    outbox: Mutex<Vec<OutboxMessage>>,
    /// Whether the authority answers at all. `false` makes the very first claim
    /// refuse, which is how an undrainable invocation is produced.
    reachable: bool,
    workspace_status: WorkspaceStatus,
    operation_status: OperationStatus,
    account_state: AccountState,
    key_material: Option<WorkspaceKeyMaterial>,
    wake_transactions: Mutex<VecDeque<aex_control_aurora::OutboxWakeTransactionStatus>>,
}

impl FakeStore {
    fn new(journal: Arc<Journal>, outbox: Vec<OutboxMessage>, reachable: bool) -> Self {
        let workspace_status = if outbox
            .iter()
            .any(|message| message.topic == Topic::WorkspaceProvisionRequested)
        {
            WorkspaceStatus::Provisioning
        } else {
            WorkspaceStatus::Active
        };
        let account_state = if outbox
            .iter()
            .any(|message| message.topic == Topic::AccountStateChanged)
        {
            AccountState::PausedTopUpRequired
        } else {
            AccountState::Active
        };
        Self {
            journal,
            outbox: Mutex::new(outbox),
            reachable,
            workspace_status,
            operation_status: OperationStatus::Running,
            account_state,
            key_material: Some(WorkspaceKeyMaterial {
                key_id: api_key_id(),
                workspace_id: workspace_id(),
                verifier: [3_u8; 32],
                pepper_version: 1,
                scopes: ScopeSet::EMPTY,
            }),
            wake_transactions: Mutex::new(VecDeque::from([
                aex_control_aurora::OutboxWakeTransactionStatus::Committed,
            ])),
        }
    }
}

#[async_trait]
impl ControlStore for FakeStore {
    async fn claim_due_operations(
        &self,
        command: &ClaimDueOperations,
    ) -> Result<Vec<Operation>, StoreError> {
        self.journal.record(format!("claim_due:{}", command.batch));
        Ok(Vec::new())
    }

    async fn claim_outbox(&self, command: &ClaimOutbox) -> Result<Vec<OutboxMessage>, StoreError> {
        self.journal
            .record(format!("claim_outbox:{}", command.batch));
        if !self.reachable {
            return Err(StoreError::Unavailable);
        }
        let mut outbox = self
            .outbox
            .lock()
            .expect("the outbox lock is never poisoned");
        let count = outbox
            .len()
            .min(usize::try_from(command.batch).unwrap_or(usize::MAX));
        Ok(outbox.drain(..count).collect())
    }

    async fn mark_outbox_dispatched(
        &self,
        id: Uuid,
        _now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        self.journal.record(format!("dispatched:{id}"));
        Ok(())
    }

    async fn release_outbox(
        &self,
        id: Uuid,
        _available_at: OffsetDateTime,
        error: &str,
    ) -> Result<(), StoreError> {
        self.journal.record(format!("released:{id}:{error}"));
        Ok(())
    }

    async fn gc_expired(&self, _command: &GcExpired) -> Result<GcReport, StoreError> {
        self.journal.record("gc");
        Ok(GcReport::default())
    }

    async fn get_workspace(&self, id: Uuid) -> Result<Option<Workspace>, StoreError> {
        assert_eq!(id, workspace_id(), "the worker read a foreign workspace");
        Ok(Some(workspace(self.workspace_status)))
    }

    async fn get_operation(&self, id: Uuid) -> Result<Option<Operation>, StoreError> {
        assert_eq!(id, operation_id(), "the worker read a foreign operation");
        Ok(Some(operation(
            OperationKind::WorkspaceProvision,
            self.operation_status,
        )))
    }

    async fn finish_workspace_provision(
        &self,
        command: &FinishWorkspaceProvisionTx,
    ) -> Result<TxOutcome<Workspace>, StoreError> {
        self.journal
            .record(format!("finish_provision:{}", command.workspace_id));
        Ok(TxOutcome::Committed(workspace(WorkspaceStatus::Active)))
    }

    async fn complete_workspace_deletion(
        &self,
        command: &CompleteWorkspaceDeletionTx,
    ) -> Result<TxOutcome<Workspace>, StoreError> {
        self.journal
            .record(format!("complete_deletion:{}", command.workspace_id));
        Ok(TxOutcome::Committed(workspace(WorkspaceStatus::Deleted)))
    }

    async fn create_organization(
        &self,
        _command: &CreateOrganizationTx,
    ) -> Result<TxOutcome<Organization>, StoreError> {
        unreachable!("the worker never creates an organization")
    }
    async fn list_organizations(
        &self,
        _query: &ListOrganizations,
    ) -> Result<Page<Organization>, StoreError> {
        unreachable!("the worker never lists organizations")
    }
    async fn get_organization(&self, _id: Uuid) -> Result<Option<Organization>, StoreError> {
        unreachable!("the worker never reads an organization")
    }
    async fn list_memberships(
        &self,
        _organization_id: Uuid,
        _page: &PageRequest,
    ) -> Result<Page<Membership>, StoreError> {
        unreachable!("the worker never lists memberships")
    }
    async fn create_invitation(
        &self,
        _command: &CreateInvitationTx,
    ) -> Result<TxOutcome<Invitation>, StoreError> {
        unreachable!("the launch projection worker has no invitation duty")
    }
    async fn accept_invitations_for_email(
        &self,
        _command: &AcceptInvitationsTx,
    ) -> Result<TxOutcome<Vec<Membership>>, StoreError> {
        unreachable!("no acceptance route exists")
    }
    async fn begin_workspace_provision(
        &self,
        _command: &BeginWorkspaceProvisionTx,
    ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError> {
        unreachable!("the worker finishes a provision; the route begins it")
    }
    async fn list_workspaces(
        &self,
        _query: &ListWorkspaces,
    ) -> Result<Page<Workspace>, StoreError> {
        unreachable!("the worker never lists workspaces")
    }
    async fn begin_workspace_deletion(
        &self,
        _command: &BeginWorkspaceDeletionTx,
    ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError> {
        unreachable!("the worker dispatches a deletion; the route accepts it")
    }
    async fn create_api_key(
        &self,
        _command: &CreateApiKeyTx,
    ) -> Result<TxOutcome<ApiKey>, StoreError> {
        unreachable!("the worker projects a key; the route mints it")
    }
    async fn get_api_key(&self, _id: Uuid) -> Result<Option<ApiKey>, StoreError> {
        unreachable!("the worker reads key material, never key metadata")
    }
    async fn list_api_keys(&self, _query: &ListApiKeys) -> Result<Page<ApiKey>, StoreError> {
        unreachable!("the worker never lists keys")
    }
    async fn revoke_api_key(&self, _command: &RevokeApiKeyTx) -> Result<TxOutcome<()>, StoreError> {
        unreachable!("the worker never revokes a key")
    }
    async fn list_operations(
        &self,
        _query: &ListOperations,
    ) -> Result<Page<Operation>, StoreError> {
        unreachable!("the worker claims due operations, never lists them")
    }
    async fn enqueue_outbox(&self, _message: &OutboxMessage) -> Result<(), StoreError> {
        unreachable!("the worker consumes the outbox; it never writes to it")
    }
    async fn account_state(&self, _organization_id: Uuid) -> Result<AccountState, StoreError> {
        unreachable!("the account gate is the HTTP edge's read, not the worker's")
    }
}

#[async_trait]
impl ControlViewStore for FakeStore {
    async fn get_workspace_view(&self, id: Uuid) -> Result<Option<WorkspaceView>, StoreError> {
        assert_eq!(id, workspace_id(), "the worker projected a foreign view");
        Ok(Some(view(self.workspace_status, self.account_state)))
    }

    async fn idempotency_id_for_operation(
        &self,
        _operation_id: Uuid,
    ) -> Result<Option<Uuid>, StoreError> {
        Ok(Some(idempotency_id()))
    }

    async fn list_organization_views(
        &self,
        _query: &ListOrganizations,
    ) -> Result<Page<OrganizationView>, StoreError> {
        unreachable!("view listings belong to the public boundary")
    }
    async fn get_organization_view(
        &self,
        _organization_id: Uuid,
        _user_id: Uuid,
    ) -> Result<Option<OrganizationView>, StoreError> {
        unreachable!("view reads belong to the public boundary")
    }
    async fn list_membership_views(
        &self,
        _organization_id: Uuid,
        _page: &PageRequest,
    ) -> Result<Page<MembershipView>, StoreError> {
        unreachable!("view listings belong to the public boundary")
    }
    async fn list_workspace_views(
        &self,
        _query: &ListWorkspaces,
    ) -> Result<Page<WorkspaceView>, StoreError> {
        unreachable!("view listings belong to the public boundary")
    }
    async fn get_operation_view(&self, _id: Uuid) -> Result<Option<OperationView>, StoreError> {
        unreachable!("view reads belong to the public boundary")
    }
    async fn list_operation_views(
        &self,
        _query: &ListOperations,
    ) -> Result<Page<OperationView>, StoreError> {
        unreachable!("view listings belong to the public boundary")
    }
    async fn user_identity(&self, _user_id: Uuid) -> Result<Option<UserIdentity>, StoreError> {
        unreachable!("the launch projection worker reads no user identity")
    }
    async fn account_profile(
        &self,
        _organization_id: Uuid,
    ) -> Result<Option<AccountProjection>, StoreError> {
        unreachable!("the worker reads the profile through the workspace view")
    }
}

#[async_trait]
impl KeyMaterialReader for FakeStore {
    async fn workspace_key_material(
        &self,
        key_id: Uuid,
    ) -> Result<Option<WorkspaceKeyMaterial>, StoreError> {
        assert_eq!(key_id, api_key_id(), "the worker read a foreign key");
        Ok(self.key_material.clone())
    }
}

#[async_trait]
impl central_control_worker::runtime::Store for FakeStore {
    async fn outbox_wake_state(
        &self,
        anchor_id: Uuid,
        _transaction_id: &str,
    ) -> Result<aex_control_aurora::OutboxWakeState, StoreError> {
        Ok(aex_control_aurora::OutboxWakeState {
            transaction: {
                let mut transactions = self
                    .wake_transactions
                    .lock()
                    .expect("the transaction-status lock is never poisoned");
                if transactions.len() > 1 {
                    transactions
                        .pop_front()
                        .expect("a non-empty status sequence")
                } else {
                    *transactions.front().expect("a non-empty status sequence")
                }
            },
            anchor_exists: true,
            anchor_dispatched: self
                .journal
                .entries()
                .contains(&format!("dispatched:{anchor_id}")),
        })
    }
}

struct FakeWakeInvoker(Arc<Journal>);

#[async_trait]
impl WakeInvoker for FakeWakeInvoker {
    async fn invoke(&self, wake: &OutboxWake) -> Result<(), String> {
        self.0.record(format!("continue:{}", wake.anchor_id));
        Ok(())
    }
}

struct FakeWakeDelay(Arc<Journal>);

#[async_trait]
impl WakeDelay for FakeWakeDelay {
    async fn pause(&self, delay: StdDuration) {
        self.0.record(format!("wake_wait:{}", delay.as_millis()));
    }
}

/// The regional authorization projection.
struct FakeProjection {
    journal: Arc<Journal>,
}

#[async_trait]
impl RegionalProjection for FakeProjection {
    async fn put_placement(&self, write: &PlacementWrite) -> Result<(), String> {
        self.journal
            .record(format!("put_placement:{}", write.status));
        Ok(())
    }

    async fn put_key_authorization(&self, write: &KeyAuthorizationWrite) -> Result<(), String> {
        self.journal
            .record(format!("put_key_authorization:{}", write.api_key));
        Ok(())
    }
}

/// The clock, fixed so a backoff is arithmetic rather than a race.
struct FixedClock;

impl aex_identity_app::ports::Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        epoch()
    }
}

/// One of the authorities the worker depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Authority {
    Store,
}

/// One composed worker and the journal of everything it did.
struct Harness {
    worker: Worker,
    journal: Arc<Journal>,
}

impl Harness {
    /// A worker whose every dependency answers.
    fn with(outbox: Vec<OutboxMessage>) -> Self {
        Self::build(outbox, &[])
    }

    /// A worker in which exactly the listed authorities refuse.
    fn build(outbox: Vec<OutboxMessage>, down: &[Authority]) -> Self {
        Self::build_with_transaction(
            outbox,
            down,
            aex_control_aurora::OutboxWakeTransactionStatus::Committed,
        )
    }

    fn build_with_transaction(
        outbox: Vec<OutboxMessage>,
        down: &[Authority],
        wake_transaction: aex_control_aurora::OutboxWakeTransactionStatus,
    ) -> Self {
        Self::build_with_transactions(outbox, down, [wake_transaction])
    }

    fn build_with_transactions(
        outbox: Vec<OutboxMessage>,
        down: &[Authority],
        wake_transactions: impl IntoIterator<Item = aex_control_aurora::OutboxWakeTransactionStatus>,
    ) -> Self {
        let answers = |authority: Authority| !down.contains(&authority);
        let journal = Arc::new(Journal::default());
        let store = FakeStore::new(Arc::clone(&journal), outbox, answers(Authority::Store));
        *store
            .wake_transactions
            .lock()
            .expect("the transaction-status lock is never poisoned") =
            wake_transactions.into_iter().collect();
        assert!(
            !store
                .wake_transactions
                .lock()
                .expect("the transaction-status lock is never poisoned")
                .is_empty(),
            "a wake needs at least one transaction status"
        );
        let store = Arc::new(store);
        let projection = Arc::new(FakeProjection {
            journal: Arc::clone(&journal),
        }) as Arc<dyn RegionalProjection>;
        let worker = Worker::new(
            store as Arc<dyn central_control_worker::runtime::Store>,
            Arc::new(FakeWakeInvoker(Arc::clone(&journal))),
            Arc::new(FakeWakeDelay(Arc::clone(&journal))),
            projection,
            DrainSettings {
                region: REGION,
                owner: "central-control-worker:eu-west-1".to_owned(),
                batch: 10,
                lease: Duration::seconds(60),
            },
            Arc::new(FixedClock),
        );
        Self { worker, journal }
    }
}

/// The continuously projected launch facts.
fn launch_outbox() -> Vec<OutboxMessage> {
    vec![api_key_created(), account_state_changed()]
}

#[tokio::test]
async fn a_transaction_wake_drains_every_launch_projection_row() {
    let harness = Harness::with(launch_outbox());
    central_control_worker::handle_event(
        &harness.worker,
        serde_json::json!({
            "schema": OutboxWake::SCHEMA,
            "anchorId": api_key_created().id,
            "transactionId": "42"
        }),
    )
    .await
    .expect("the drain runs");

    let journal = harness.journal.entries();
    for (fact, message) in [
        ("api_key", api_key_created()),
        ("account_state", account_state_changed()),
    ] {
        assert!(
            journal.contains(&format!("dispatched:{}", message.id)),
            "{fact}'s outbox row was never dispatched: {journal:?}"
        );
    }
    assert!(
        journal
            .iter()
            .any(|it| it.starts_with("put_key_authorization:"))
    );
    assert!(journal.contains(&"put_placement:paused".to_owned()));
}

#[tokio::test]
async fn an_aurora_wake_carries_the_exact_anchor_and_transaction() {
    let harness = Harness::with(launch_outbox());
    let answer = central_control_worker::handle_event(
        &harness.worker,
        serde_json::json!({
            "schema": OutboxWake::SCHEMA,
            "anchorId": api_key_created().id,
            "transactionId": "42"
        }),
    )
    .await
    .expect("an Aurora wake succeeds");

    assert_eq!(answer, serde_json::json!({}));
    assert_eq!(harness.journal.count("dispatched:"), 2);
    assert_eq!(
        harness.journal.count("gc"),
        1,
        "a completed wake opportunistically sweeps"
    );
}

#[tokio::test]
async fn a_statement_larger_than_one_batch_always_accepts_a_continuation() {
    let rows = (30_u8..41)
        .map(|n| {
            let mut row = api_key_created();
            row.id = id(n);
            row.dedupe_key = format!("dedupe-{n}");
            row
        })
        .collect::<Vec<_>>();
    let anchor = rows[0].id;
    let harness = Harness::with(rows);
    let wake = serde_json::json!({
        "schema": OutboxWake::SCHEMA,
        "anchorId": anchor,
        "transactionId": "43"
    });

    central_control_worker::handle_event(&harness.worker, wake.clone())
        .await
        .expect("the full first page accepts continuation");
    assert_eq!(harness.journal.count("dispatched:"), 10);
    assert_eq!(harness.journal.count("continue:"), 1);
    assert_eq!(harness.journal.count("gc"), 0);

    central_control_worker::handle_event(&harness.worker, wake)
        .await
        .expect("the continuation drains the short final page");
    assert_eq!(harness.journal.count("dispatched:"), 11);
    assert_eq!(harness.journal.count("gc"), 1);
}

#[tokio::test]
async fn an_unknown_queue_envelope_is_rejected() {
    let harness = Harness::with(launch_outbox());
    let error = central_control_worker::handle_event(
        &harness.worker,
        serde_json::json!({
            "Records": [{
                "messageId": "wakeup-1",
                "receiptHandle": "handle-1",
                "body": "this body is never parsed",
                "attributes": {},
                "messageAttributes": {},
                "eventSource": "aws:sqs",
                "awsRegion": "eu-west-1",
            }],
        }),
    )
    .await
    .expect_err("there is no polled queue recovery path");

    assert_eq!(error, "invalid_control_worker_event");
    assert_eq!(harness.journal.count("dispatched:"), 0);
}

#[tokio::test]
async fn the_recovery_schedule_drains_rows_whose_direct_wake_was_lost() {
    let harness = Harness::with(launch_outbox());

    central_control_worker::handle_event(
        &harness.worker,
        serde_json::json!({
            "source": "aex.scheduler",
            "detail-type": "aex.control_recovery",
            "detail": {}
        }),
    )
    .await
    .expect("the recovery sweep runs");

    assert_eq!(harness.journal.count("dispatched:"), 2);
}

#[tokio::test]
async fn a_pause_becomes_the_launch_region_admission_fence() {
    let harness = Harness::with(vec![account_state_changed()]);

    harness.worker.tick().await.expect("the pause is projected");

    let journal = harness.journal.entries();
    assert!(journal.contains(&"put_placement:paused".to_owned()));
    assert!(!journal.iter().any(|entry| entry.starts_with("regional_")));
    assert_eq!(harness.journal.count("dispatched:"), 1);
}

#[tokio::test]
async fn an_aborted_transaction_wake_is_a_safe_no_op() {
    let harness = Harness::build_with_transaction(
        launch_outbox(),
        &[],
        aex_control_aurora::OutboxWakeTransactionStatus::Aborted,
    );
    central_control_worker::handle_event(
        &harness.worker,
        serde_json::json!({
            "schema": OutboxWake::SCHEMA,
            "anchorId": api_key_created().id,
            "transactionId": "44"
        }),
    )
    .await
    .expect("rollback has no durable work");
    assert_eq!(harness.journal.count("dispatched:"), 0);
    assert_eq!(harness.journal.count("gc"), 0);
}

#[tokio::test]
async fn a_normal_precommit_race_drains_during_the_bounded_wait() {
    let harness = Harness::build_with_transactions(
        launch_outbox(),
        &[],
        [
            aex_control_aurora::OutboxWakeTransactionStatus::InProgress,
            aex_control_aurora::OutboxWakeTransactionStatus::InProgress,
            aex_control_aurora::OutboxWakeTransactionStatus::Committed,
        ],
    );
    central_control_worker::handle_event(
        &harness.worker,
        serde_json::json!({
            "schema": OutboxWake::SCHEMA,
            "anchorId": api_key_created().id,
            "transactionId": "45"
        }),
    )
    .await
    .expect("the producer committed inside the bounded wait");
    assert_eq!(harness.journal.count("wake_wait:"), 2);
    assert_eq!(harness.journal.count("dispatched:"), 2);
}

#[tokio::test]
async fn a_precommit_race_returns_an_error_for_lambda_retry() {
    let harness = Harness::build_with_transaction(
        launch_outbox(),
        &[],
        aex_control_aurora::OutboxWakeTransactionStatus::InProgress,
    );
    let error = central_control_worker::handle_event(
        &harness.worker,
        serde_json::json!({
            "schema": OutboxWake::SCHEMA,
            "anchorId": api_key_created().id,
            "transactionId": "45"
        }),
    )
    .await
    .expect_err("the accepted event must be retried after commit");
    assert_eq!(error, "outbox_wake_transaction_in_progress");
    assert_eq!(
        harness
            .journal
            .entries()
            .into_iter()
            .filter(|entry| entry.starts_with("wake_wait:"))
            .collect::<Vec<_>>(),
        [
            "wake_wait:25",
            "wake_wait:50",
            "wake_wait:100",
            "wake_wait:200"
        ]
    );
    assert_eq!(harness.journal.count("dispatched:"), 0);
}

fn provision_outbox() -> Vec<OutboxMessage> {
    vec![message(
        20,
        Topic::WorkspaceProvisionRequested,
        serde_json::json!({
            "workspaceId": workspace_id(),
            "organizationId": organization_id(),
            "region": REGION.as_str(),
            "operationId": operation_id(),
            "idempotencyId": idempotency_id(),
            "intentHash": intent_hex(),
        }),
    )]
}

fn projection_effects(harness: &Harness) -> Vec<String> {
    harness
        .journal
        .entries()
        .into_iter()
        .filter(|entry| entry.starts_with("put_") || entry.starts_with("finish_provision:"))
        .collect()
}

#[tokio::test]
async fn a_personal_workspace_is_projected_before_central_activation() {
    let harness = Harness::with(provision_outbox());
    harness.worker.tick().await.expect("the drain runs");

    assert_eq!(
        projection_effects(&harness),
        vec![
            "put_placement:active".to_owned(),
            format!("finish_provision:{}", workspace_id()),
        ],
        "central activation must not outrun the admission projection"
    );
}

#[tokio::test]
async fn an_outbox_row_for_another_region_is_released_and_never_dispatched() {
    let mut row = api_key_created();
    row.payload["region"] = serde_json::json!("us-east-1");
    let harness = Harness::with(vec![row.clone()]);
    harness
        .worker
        .tick()
        .await
        .expect_err("a failed effect must reach Lambda's async retry");

    let journal = harness.journal.entries();
    assert!(
        journal.contains(&format!("released:{}:outbox_region_mismatch", row.id)),
        "the failure is recorded on the row it belongs to: {journal:?}"
    );
    assert_eq!(
        harness.journal.count("dispatched:"),
        0,
        "a row whose effect failed was reported as delivered"
    );
}

#[tokio::test]
async fn an_aurora_wake_that_cannot_drain_fails_loudly_rather_than_answering_success() {
    let harness = Harness::build(launch_outbox(), &[Authority::Store]);
    let error = central_control_worker::handle_event(
        &harness.worker,
        serde_json::json!({
            "schema": OutboxWake::SCHEMA,
            "anchorId": api_key_created().id,
            "transactionId": "42"
        }),
    )
    .await
    .expect_err("an undrainable wake is a failed invocation");

    assert_eq!(error, "store_unavailable");
    assert_eq!(harness.journal.count("dispatched:"), 0);
}
