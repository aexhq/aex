//! `central-control-worker`: the trigger, the drain and the order of a provision.
//!
//! Three shipped routes — `api_key_create`, `workspace_delete` and
//! `invitation_create` — commit a row **and** an outbox message in one
//! transaction and then answer success. Nothing they do is finished until this
//! worker runs, and until this suite existed nothing proved that anything ever
//! makes it run. That is what these tests are: evidence that both trigger
//! shapes reach the drain, that the drain claims and dispatches, and that a
//! failure is left claimable rather than swallowed.
//!
//! # The shared harness
//!
//! [`Harness`] records every regional effect in call order, which is what makes
//! the capacity-bootstrap ordering constraint assertable at all:
//! [`the_regional_writes_of_one_provision_are_bootstrap_then_profile_then_placement`]
//! is the one test that says a placement is never published for a workspace
//! whose effective-limit set may not exist. There is deliberately one such test
//! and one such journal — a second harness recording a second order would let
//! the two disagree about which order production takes.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use aex_control_app::ports::{
    AcceptInvitationsTx, AccountProjection, BeginWorkspaceDeletionTx, BeginWorkspaceProvisionTx,
    ClaimDueOperations, ClaimOutbox, CompleteWorkspaceDeletionTx, ControlStore, ControlViewStore,
    CreateApiKeyTx, CreateInvitationTx, CreateOrganizationTx, DeleteWorkspaceRequest,
    DeleteWorkspaceResponse, EffectError, FinishWorkspaceProvisionTx, GcExpired, GcReport,
    KeyMaterialReader, ListApiKeys, ListOperations, ListOrganizations, ListWorkspaces,
    MembershipView, OperationView, OrganizationView, Page, PageRequest, ProvisionWorkspaceRequest,
    ProvisionWorkspaceResponse, RegionalControlPort, RevokeApiKeyTx, StoreError, TxOutcome,
    UserIdentity, WorkspaceKeyMaterial, WorkspaceView,
};
use aex_control_domain::{
    AccountProfile, AccountState, ApiKey, Fence, IntentHash, Invitation, Membership, Operation,
    OperationKind, OperationStatus, OperationVisibility, Organization, OutboxMessage, Revision,
    ScopeSet, Slug, Topic, Workspace, WorkspaceStatus,
};
use aex_session_dynamodb::projection_write::{KeyAuthorizationWrite, PlacementWrite, ProfileWrite};
use aex_wire::ids::PrefixedId as _;
use aex_wire::types::Region;
use async_trait::async_trait;
use central_control_worker::runtime::{
    Mail, OutboxWake, RegionalCapacity, RegionalProjection, SigningAdmin, WakeDelay, WakeInvoker,
    Worker,
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

fn view(status: WorkspaceStatus) -> WorkspaceView {
    WorkspaceView {
        workspace: workspace(status),
        account: AccountProjection {
            profile: AccountProfile {
                state: AccountState::Active,
                reason: None,
                revision: 1,
                changed_at: epoch(),
            },
            epoch: 1,
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
fn api_key_created() -> OutboxMessage {
    message(
        10,
        Topic::ApiKeyCreated,
        serde_json::json!({
            "apiKeyId": api_key_id(),
            "workspaceId": workspace_id(),
            "organizationId": organization_id(),
            "region": REGION.as_str(),
            "changedAt": epoch(),
            "epoch": 0,
        }),
    )
}

/// `workspace_delete`'s message: the operation stays open until this runs.
fn workspace_delete_requested() -> OutboxMessage {
    message(
        11,
        Topic::WorkspaceDeleteRequested,
        serde_json::json!({
            "workspaceId": workspace_id(),
            "organizationId": organization_id(),
            "region": REGION.as_str(),
            "operationId": operation_id(),
        }),
    )
}

/// `invitation_create`'s message: the row exists and the mail is unsent.
fn invitation_email_requested() -> OutboxMessage {
    message(
        12,
        Topic::InvitationEmailRequested,
        serde_json::json!({
            "invitationId": id(13),
            "organizationId": organization_id(),
            "email": "invitee@example.test",
            "role": "member",
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
    due: Mutex<Vec<Operation>>,
    /// Whether the authority answers at all. `false` makes the very first claim
    /// refuse, which is how an undrainable invocation is produced.
    reachable: bool,
    workspace_status: WorkspaceStatus,
    operation_status: OperationStatus,
    key_material: Option<WorkspaceKeyMaterial>,
    wake_transactions: Mutex<VecDeque<aex_control_aurora::OutboxWakeTransactionStatus>>,
}

impl FakeStore {
    fn new(journal: Arc<Journal>, outbox: Vec<OutboxMessage>, reachable: bool) -> Self {
        Self {
            journal,
            outbox: Mutex::new(outbox),
            due: Mutex::new(Vec::new()),
            reachable,
            workspace_status: WorkspaceStatus::Active,
            operation_status: OperationStatus::Running,
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
        if !self.reachable {
            return Err(StoreError::Unavailable);
        }
        Ok(std::mem::take(
            &mut *self.due.lock().expect("the due lock is never poisoned"),
        ))
    }

    async fn claim_outbox(&self, command: &ClaimOutbox) -> Result<Vec<OutboxMessage>, StoreError> {
        self.journal
            .record(format!("claim_outbox:{}", command.batch));
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
            OperationKind::WorkspaceDelete,
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
        unreachable!("the worker sends the invitation; the route writes it")
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
        Ok(Some(view(self.workspace_status)))
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
        unreachable!("the invitation payload carries the address")
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

/// The regional control authority, reached by direct invoke.
struct FakeRegional {
    journal: Arc<Journal>,
    available: bool,
}

#[async_trait]
impl RegionalControlPort for FakeRegional {
    async fn provision_workspace(
        &self,
        request: &ProvisionWorkspaceRequest,
    ) -> Result<ProvisionWorkspaceResponse, EffectError> {
        self.journal
            .record(format!("regional_provision:{}", request.workspace_id));
        if self.available {
            Ok(ProvisionWorkspaceResponse {
                workspace_id: request.workspace_id,
                created: true,
            })
        } else {
            Err(EffectError::Unavailable)
        }
    }

    async fn delete_workspace(
        &self,
        request: &DeleteWorkspaceRequest,
    ) -> Result<DeleteWorkspaceResponse, EffectError> {
        self.journal
            .record(format!("regional_delete:{}", request.workspace_id));
        if self.available {
            Ok(DeleteWorkspaceResponse { removed: true })
        } else {
            Err(EffectError::Unavailable)
        }
    }
}

/// The regional capacity authority.
///
/// Records the trigger in the same journal as the two projection writes, so the
/// relative order of all three is one sequence rather than three separate facts
/// a reader has to reconcile.
struct FakeCapacity {
    journal: Arc<Journal>,
    available: bool,
}

#[async_trait]
impl RegionalCapacity for FakeCapacity {
    async fn bootstrap(
        &self,
        _region: Region,
        workspace: aex_wire::ids::WorkspaceId,
    ) -> Result<(), String> {
        self.journal.record(format!("bootstrap:{workspace}"));
        if self.available {
            Ok(())
        } else {
            Err("regional_capacity_unavailable".to_owned())
        }
    }
}

/// The regional authorization projection.
struct FakeProjection {
    journal: Arc<Journal>,
}

#[async_trait]
impl RegionalProjection for FakeProjection {
    async fn put_profile(&self, write: &ProfileWrite) -> Result<(), String> {
        self.journal.record(format!("put_profile:{}", write.slug));
        Ok(())
    }

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

/// The invitation mailer.
struct FakeMail {
    journal: Arc<Journal>,
    available: bool,
}

#[async_trait]
impl Mail for FakeMail {
    async fn send(&self, to: &str, _subject: &str, _text: &str) -> Result<(), String> {
        self.journal.record(format!("mail:{to}"));
        if self.available {
            Ok(())
        } else {
            Err("ses_delivery_unavailable".to_owned())
        }
    }
}

/// The signing-key administrator.
struct FakeSigning {
    journal: Arc<Journal>,
}

#[async_trait]
impl SigningAdmin for FakeSigning {
    async fn confirm_published(&self, secret_ref: &str) -> Result<(), String> {
        self.journal.record(format!("signing:{secret_ref}"));
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
    Mail,
    Regional,
    Capacity,
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
        let projections: BTreeMap<Region, Arc<dyn RegionalProjection>> = BTreeMap::from([(
            REGION,
            Arc::new(FakeProjection {
                journal: Arc::clone(&journal),
            }) as Arc<dyn RegionalProjection>,
        )]);
        let worker = Worker::new(
            store as Arc<dyn central_control_worker::runtime::Store>,
            Arc::new(FakeWakeInvoker(Arc::clone(&journal))),
            Arc::new(FakeWakeDelay(Arc::clone(&journal))),
            Arc::new(FakeRegional {
                journal: Arc::clone(&journal),
                available: answers(Authority::Regional),
            }),
            Arc::new(FakeCapacity {
                journal: Arc::clone(&journal),
                available: answers(Authority::Capacity),
            }),
            projections,
            Arc::new(FakeMail {
                journal: Arc::clone(&journal),
                available: answers(Authority::Mail),
            }),
            Arc::new(FakeSigning {
                journal: Arc::clone(&journal),
            }),
            Arc::new(FixedClock),
            "central-control-worker:eu-west-1".to_owned(),
            10,
            Duration::seconds(60),
        );
        Self { worker, journal }
    }
}

/// The three rows the three shipped routes commit.
fn the_three_routes_outbox() -> Vec<OutboxMessage> {
    vec![
        api_key_created(),
        workspace_delete_requested(),
        invitation_email_requested(),
    ]
}

#[tokio::test]
async fn a_transaction_wake_drains_every_row_the_three_shipped_routes_commit() {
    let harness = Harness::with(the_three_routes_outbox());
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
    for (route, message) in [
        ("api_key_create", api_key_created()),
        ("workspace_delete", workspace_delete_requested()),
        ("invitation_create", invitation_email_requested()),
    ] {
        assert!(
            journal.contains(&format!("dispatched:{}", message.id)),
            "{route}'s outbox row was never dispatched: {journal:?}"
        );
    }
    // The three effects the routes promised and could not perform themselves.
    assert!(
        journal
            .iter()
            .any(|it| it.starts_with("put_key_authorization:"))
    );
    assert!(journal.iter().any(|it| it.starts_with("regional_delete:")));
    assert!(journal.contains(&"mail:invitee@example.test".to_owned()));
}

#[tokio::test]
async fn an_aurora_wake_carries_the_exact_anchor_and_transaction() {
    let harness = Harness::with(the_three_routes_outbox());
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
    assert_eq!(harness.journal.count("dispatched:"), 3);
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
            message(
                n,
                Topic::InvitationEmailRequested,
                serde_json::json!({
                    "invitationId": id(n + 50),
                    "organizationId": organization_id(),
                    "email": "invitee@example.test",
                    "role": "member",
                }),
            )
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
async fn a_queue_or_schedule_envelope_is_rejected() {
    let harness = Harness::with(the_three_routes_outbox());
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

    assert_eq!(error, "invalid_outbox_wake");
    assert_eq!(harness.journal.count("dispatched:"), 0);
}

#[tokio::test]
async fn an_aborted_transaction_wake_is_a_safe_no_op() {
    let harness = Harness::build_with_transaction(
        the_three_routes_outbox(),
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
        the_three_routes_outbox(),
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
    assert_eq!(harness.journal.count("dispatched:"), 3);
}

#[tokio::test]
async fn a_precommit_race_returns_an_error_for_lambda_retry() {
    let harness = Harness::build_with_transaction(
        the_three_routes_outbox(),
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

/// Everything this worker writes into a region, in the order it wrote it.
fn bootstrapped() -> String {
    let workspace = aex_wire::ids::WorkspaceId::from_uuid7(
        aex_wire::ids::Uuid7::from_bytes(*workspace_id().as_bytes()).expect("a UUIDv7 fixture"),
    );
    format!("bootstrap:{workspace}")
}

fn regional_effects(harness: &Harness) -> Vec<String> {
    harness
        .journal
        .entries()
        .into_iter()
        .filter(|entry| entry.starts_with("put_") || entry.starts_with("bootstrap:"))
        .collect()
}

#[tokio::test]
async fn the_regional_writes_of_one_provision_are_bootstrap_then_profile_then_placement() {
    // The ordering seam, and the one place this worker takes strong ordering
    // against the plane's eventual-consistency default.
    //
    // Placement is what makes a workspace visible to the regional edge, and the
    // edge's admission snapshot reads the effective-limit row on every request
    // and treats its absence as a self-contradiction. So a placement published
    // ahead of a bootstrap does not serve stale limits — it answers `401` to
    // every request the workspace ever makes, with its own API key, for as long
    // as the gap lasts.
    //
    // Profile stays before placement for the same shape of reason it always
    // did. The sequence below is therefore not a preference; each step is a
    // precondition of the one after it.
    let harness = Harness::with(provision_outbox());
    harness.worker.tick().await.expect("the drain runs");

    assert_eq!(
        regional_effects(&harness),
        vec![
            bootstrapped(),
            "put_profile:fixture".to_owned(),
            "put_placement:active".to_owned(),
        ],
        "the regional write order changed"
    );
}

#[tokio::test]
async fn a_placement_is_never_written_when_the_capacity_bootstrap_did_not_apply() {
    // The half the order alone does not prove. A bootstrap that ran first and
    // failed, followed by a placement written anyway, is byte-for-byte the same
    // outage as no bootstrap at all — so the refusal has to stop the sequence,
    // not merely precede it.
    let harness = Harness::build(provision_outbox(), &[Authority::Capacity]);
    harness
        .worker
        .tick()
        .await
        .expect_err("a failed effect must reach Lambda's async retry");

    assert_eq!(
        regional_effects(&harness),
        vec![bootstrapped()],
        "a projection row was published behind a bootstrap that did not apply"
    );
    let journal = harness.journal.entries();
    assert!(
        journal.iter().any(|entry| entry
            .starts_with(&format!("released:{}:", provision_outbox()[0].id))
            && entry.ends_with("regional_capacity_unavailable")),
        "the failure was not recorded on the row that owns the retry: {journal:?}"
    );
    assert_eq!(
        harness.journal.count("dispatched:"),
        0,
        "an incomplete provision was reported as delivered"
    );
}

#[tokio::test]
async fn a_duty_that_fails_is_released_with_a_backoff_and_never_marked_dispatched() {
    let harness = Harness::build(vec![invitation_email_requested()], &[Authority::Mail]);
    harness
        .worker
        .tick()
        .await
        .expect_err("a failed effect must reach Lambda's async retry");

    let journal = harness.journal.entries();
    assert!(
        journal.contains(&format!(
            "released:{}:ses_delivery_unavailable",
            invitation_email_requested().id
        )),
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
    let harness = Harness::build(the_three_routes_outbox(), &[Authority::Store]);
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
