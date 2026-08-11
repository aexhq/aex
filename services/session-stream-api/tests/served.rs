//! The routes `regional-session-api` genuinely serves, driven through the real
//! router.
//!
//! These are not handler unit tests. Each case builds the router with
//! `mount_unary` over the real `UnaryDispatch`, sends an HTTP request, and reads
//! the status and the published body back. What is proved is the whole path the
//! deployable owns: the generated route match, the generated dispatcher's path
//! and query decoding, the handler, the projection, the entity tag and the
//! response rendering.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_content_aws::object_store::ContentObjectStore;
use aex_content_domain::identity::{RegistryKind, Revision};
use aex_content_dynamodb::store::ContentMetadataStore;
use aex_internal_contracts::RunId;
use aex_operation_domain::operation::{
    Operation, OperationKind, OperationResult, OperationScope, OperationStatus,
};
use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::cursor::{CursorKey, CursorKeyRing};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission, mount_unary};
use aex_regional_http::projection::entity_tag;
use aex_regional_http::router::{RouteOwner, route_owner};
use aex_registry_dynamodb::store::{
    DeleteCommitted, PointerPage, RegistryStore, SetCommit, SetCommitted,
};
use aex_secret_custody_dynamodb::codec::{
    CredentialState, ProviderCredential as StoredCredential, SecretMetadata as StoredSecret,
};
use aex_secret_custody_dynamodb::store::{Page, SecretCustodyStore};
use aex_secret_domain::custody::CustodyRevision;
use aex_secret_domain::plaintext::SecretPlaintext;
use aex_secret_domain::secret::{SecretName, SourceGeneration};
use aex_session_domain::{Message, Run, Session};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::{PageBudget, PagePosition};
use aex_session_dynamodb::plan::{Participant, TransactionPlan};
use aex_session_dynamodb::projection::{
    AuthorizationProjection, ProjectionPage, WorkspaceProjection,
};
use aex_session_dynamodb::replay::Receipt;
use aex_session_dynamodb::store::{OperationApiStore, OperationCancelOutcome, OperationFilter};
use aex_session_dynamodb::store::{
    PositionPage, SessionListFilter, SessionListStatus, SessionPage, SessionQueries, SessionScoped,
};
use aex_session_dynamodb::transactions::operation_cancel_owned;
use aex_session_dynamodb::wire_pending::{
    Approval, ProjectedLimitBundle as StoredBundle, ProjectedLimitBundleHead as StoredBundleHead,
    ProjectedWorkspaceLimit as StoredLimit, StoredOperation, WorkspacePlacement as StoredPlacement,
    WorkspaceProfile as StoredProfile,
};
use aex_usage_query_dynamodb::expressions::{
    AggregateRequest, CoarseRequest, CoverageRow, Generation, QueryError,
};
use aex_usage_query_dynamodb::store::{AggregatePageRows, CoarsePageRows, UsageProjectionReads};
use aex_wire::CanonicalJson;
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::idempotency::IntentDigest;
use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::OperationId;
use aex_wire::ids::{
    ApiKeyId, ApprovalId, GenerationId, PrefixedId, ProviderCredentialId, ResourceName, SessionId,
    Uuid7, WorkspaceId,
};
use aex_wire::ids::{ContentHash, UploadId};
use aex_wire::models;
use aex_wire::routes::{RouteId, route};
use aex_wire::scopes::ScopeSet;
use aex_wire::server::RouteGroup;
use aex_wire::types::ETag;
use aex_wire::types::{Region, RequestId, Timestamp};
use aex_workspace_domain::registry::{
    RegistryPointer, RegistryRow as StoredRegistryRow, ValueDocument,
};
use aex_workspace_domain::upload::{Upload as StoredUpload, UploadState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use session_stream_api::session::handlers::{Dispatcher, Routes, Shared};
use tower::ServiceExt as _;

// --- fixtures -------------------------------------------------------------------

struct NoLiveFiles;

impl aex_brain_hands::LiveFileBackend for NoLiveFiles {
    fn ensure_ready<'a>(
        &'a self,
        _session: SessionId,
        _generation: GenerationId,
    ) -> aex_brain_app::ports::BoxFuture<
        'a,
        Result<aex_brain_hands::LiveGenerationReady, aex_brain_app::ports::HandsError>,
    > {
        Box::pin(async { panic!("a non-live fixture launched a generation") })
    }

    fn call<'a>(
        &'a self,
        _session: SessionId,
        _generation: GenerationId,
        _activity: aex_hands_protocol::rpc::HandsOperationId,
        _requests: &'a [aex_hands_protocol::files::FileRequest],
    ) -> aex_brain_app::ports::BoxFuture<
        'a,
        Result<aex_brain_hands::LiveFileReply, aex_brain_app::ports::HandsError>,
    > {
        Box::pin(async { panic!("a non-live fixture reached guest transport") })
    }

    fn abort_unpublished<'a>(
        &'a self,
        _session: SessionId,
        _generation: GenerationId,
    ) -> aex_brain_app::ports::BoxFuture<'a, Result<(), aex_brain_app::ports::HandsError>> {
        Box::pin(async { panic!("a non-live fixture compensated generation create") })
    }
}

fn sample<I: PrefixedId>(seed: u8) -> I {
    I::from_uuid7(Uuid7::compose(1_754_051_696_789, [seed; 10]))
}

fn workspace() -> WorkspaceId {
    sample::<WorkspaceId>(2)
}

fn moment(spelling: &str) -> Timestamp {
    Timestamp::parse(spelling).expect("a pinned spelling")
}

fn stored_credential() -> StoredCredential {
    let credential: ProviderCredentialId = sample(6);
    StoredCredential {
        credential,
        workspace: workspace(),
        name: ResourceName::parse("primary-openai").expect("a resource name"),
        provider: models::ProviderId::Openai,
        secret_name: ResourceName::parse("openai-key").expect("a resource name"),
        source_generation: SourceGeneration::FIRST,
        fingerprint: SecretPlaintext::new(b"sk-live-fixture".to_vec())
            .expect("a bounded plaintext")
            .credential_fingerprint(workspace(), credential),
        revision: 1,
        state: CredentialState::Ready,
        created_at: moment("2026-08-01T12:34:56.789Z"),
        updated_at: moment("2026-08-01T12:34:56.789Z"),
        revoked_at: None,
    }
}

fn stored_operation(
    seed: u8,
    kind: OperationKind,
    status: OperationStatus,
    session: Option<SessionId>,
) -> StoredOperation {
    let created_at = moment(&format!("2026-08-01T12:34:{seed:02}.000Z"));
    let result =
        (kind == OperationKind::SessionSuspend && status == OperationStatus::Succeeded).then(|| {
            OperationResult {
                measurement: None,
                content: Some(
                    CanonicalJson::parse(&format!(
                        r#"{{"changed":true,"sessionId":"{}","sessionRevision":9,"suspendedAt":"2026-08-01T12:34:56.789Z"}}"#,
                        session.expect("a suspend operation is session scoped")
                    ))
                    .expect("a typed suspend result"),
                ),
            }
        });
    StoredOperation {
        record: Operation {
            id: sample::<OperationId>(seed),
            workspace: workspace(),
            session,
            kind,
            status,
            intent: IntentDigest::from_bytes([seed; 32]),
            scope: session.map_or(
                OperationScope::Workspace(workspace()),
                OperationScope::Session,
            ),
            progress: None,
            cursor: None,
            cancel_requested: false,
            result,
            error: None,
            created_at,
            started_at: (status != OperationStatus::Queued).then_some(created_at),
            updated_at: created_at,
            committed_at: (status == OperationStatus::Succeeded).then_some(created_at),
            terminal_at: status.is_terminal().then_some(created_at),
        },
        version: 4,
    }
}

#[derive(Debug, Default)]
struct FakeOperations {
    rows: std::sync::Mutex<Vec<StoredOperation>>,
    page_size: usize,
    filters: std::sync::Mutex<Vec<OperationFilter>>,
    writes: std::sync::Mutex<usize>,
}

#[async_trait::async_trait]
impl OperationApiStore for FakeOperations {
    async fn load(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<StoredOperation>, StoreError> {
        Ok(self
            .rows
            .lock()
            .expect("an uncontended fixture")
            .iter()
            .find(|row| row.record.workspace == workspace && row.record.id == operation)
            .cloned())
    }

    async fn page(
        &self,
        workspace: WorkspaceId,
        filter: &OperationFilter,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<PositionPage<StoredOperation>, StoreError> {
        self.filters
            .lock()
            .expect("an uncontended fixture")
            .push(*filter);
        let mut rows: Vec<_> = self
            .rows
            .lock()
            .expect("an uncontended fixture")
            .iter()
            .filter(|row| row.record.workspace == workspace && row.record.kind.is_public())
            .cloned()
            .collect();
        rows.sort_unstable_by_key(|row| (row.record.created_at, row.record.id));
        let start = after
            .and_then(|position| position.index_sk.as_ref())
            .and_then(|sort| {
                rows.iter().position(|row| {
                    aex_session_dynamodb::keys::workspace_index::operation_sort(
                        row.record.created_at,
                        row.record.id,
                    ) == *sort
                })
            })
            .map_or(0, |index| index + 1);
        let page_size = self
            .page_size
            .max(1)
            .min(usize::try_from(budget.items()).expect("the page budget fits usize"));
        let end = rows.len().min(start + page_size);
        let items = rows[start..end]
            .iter()
            .filter(|row| {
                filter.kind.is_none_or(|kind| row.record.kind == kind)
                    && filter
                        .status
                        .is_none_or(|status| row.record.status == status)
                    && filter
                        .session
                        .is_none_or(|session| row.record.session == Some(session))
            })
            .cloned()
            .collect();
        let next = (end < rows.len()).then(|| {
            let last = &rows[end - 1].record;
            PagePosition {
                pk: format!("OP#{}", last.id),
                sk: "STATE".to_owned(),
                index_pk: Some(
                    aex_session_dynamodb::keys::workspace_index::operation_partition(workspace),
                ),
                index_sk: Some(aex_session_dynamodb::keys::workspace_index::operation_sort(
                    last.created_at,
                    last.id,
                )),
            }
        });
        Ok(PositionPage {
            items,
            next,
            isolated: 0,
        })
    }

    async fn request_cancel(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
        now: Timestamp,
    ) -> Result<OperationCancelOutcome, StoreError> {
        let mut rows = self.rows.lock().expect("an uncontended fixture");
        let Some(stored) = rows.iter_mut().find(|row| {
            row.record.workspace == workspace
                && row.record.id == operation
                && row.record.kind.is_public()
        }) else {
            return Ok(OperationCancelOutcome::NotFound);
        };
        if stored.record.status == OperationStatus::Cancelled || stored.record.cancel_requested {
            return Ok(OperationCancelOutcome::Accepted(Box::new(stored.clone())));
        }
        if !operation_cancel_owned(stored.record.kind) {
            return Ok(OperationCancelOutcome::NotCancelable);
        }
        let Ok(commit) = aex_operation_domain::operation::cancel(&stored.record, now) else {
            return Ok(OperationCancelOutcome::NotCancelable);
        };
        stored.record = commit.operation;
        stored.version = stored.version.checked_add(1).expect("a fixture version");
        *self.writes.lock().expect("an uncontended fixture") += 1;
        Ok(OperationCancelOutcome::Accepted(Box::new(stored.clone())))
    }
}

#[derive(Debug, Default)]
struct FakeSessions {
    sessions: Vec<Session>,
    session_page_calls: std::sync::Mutex<Vec<(SessionListFilter, Timestamp, Option<PagePosition>)>>,
    messages: Vec<Message>,
    runs: Vec<Run>,
    next: Option<PagePosition>,
    deleted: bool,
    deletion_epoch: u64,
}

#[async_trait::async_trait]
impl SessionQueries for FakeSessions {
    async fn page_sessions(
        &self,
        workspace: WorkspaceId,
        filter: &SessionListFilter,
        snapshot: Timestamp,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<PositionPage<Session>, StoreError> {
        self.session_page_calls
            .lock()
            .expect("an uncontended fixture")
            .push((*filter, snapshot, after.cloned()));
        let filtered = self
            .sessions
            .iter()
            .filter(|session| {
                filter.status.is_none_or(|status| match status {
                    SessionListStatus::Idle => {
                        session.status == aex_session_domain::SessionStatus::Idle
                    }
                    SessionListStatus::Running => {
                        session.status == aex_session_domain::SessionStatus::Running
                    }
                    SessionListStatus::Suspending => {
                        session.status == aex_session_domain::SessionStatus::Suspending
                    }
                    SessionListStatus::Suspended => {
                        session.status == aex_session_domain::SessionStatus::Suspended
                    }
                    SessionListStatus::Resuming => {
                        session.status == aex_session_domain::SessionStatus::Resuming
                    }
                    SessionListStatus::Terminating => {
                        session.status == aex_session_domain::SessionStatus::Terminating
                    }
                    SessionListStatus::Terminated => {
                        session.status == aex_session_domain::SessionStatus::Terminated
                    }
                    SessionListStatus::Deleting => {
                        session.status == aex_session_domain::SessionStatus::Deleting
                    }
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let start = after
            .and_then(|position| {
                filtered.iter().position(|session| {
                    position.pk == aex_session_dynamodb::keys::head(session.id).pk
                })
            })
            .map_or(0, |index| index + 1);
        let end = filtered
            .len()
            .min(start.saturating_add(usize::try_from(budget.items()).unwrap_or(usize::MAX)));
        let items = filtered[start..end].to_vec();
        let next = (end < filtered.len()).then(|| {
            let session = &filtered[end - 1];
            let head = aex_session_dynamodb::keys::head(session.id);
            PagePosition {
                pk: head.pk,
                sk: head.sk,
                index_pk: Some(
                    aex_session_dynamodb::keys::workspace_index::session_partition(workspace),
                ),
                index_sk: Some(aex_session_dynamodb::keys::workspace_index::session_sort(
                    session.created_at,
                    session.id,
                )),
            }
        });
        Ok(PositionPage {
            items,
            next,
            isolated: 0,
        })
    }

    async fn load_session(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<SessionScoped<Session>, StoreError> {
        if self.deleted {
            return Ok(SessionScoped::Deleted);
        }
        let mut parent = aex_session_domain::testing::session_fixture();
        parent.id = session;
        parent.workspace = workspace;
        parent.deletion.session = session;
        parent.deletion.epoch = aex_session_domain::DeletionEpoch(self.deletion_epoch);
        Ok(SessionScoped::Active(parent))
    }

    async fn page_messages(
        &self,
        _workspace: WorkspaceId,
        session: SessionId,
        _expected_deletion_epoch: Option<aex_session_domain::DeletionEpoch>,
        _budget: PageBudget,
        _after: Option<&PagePosition>,
    ) -> Result<SessionScoped<SessionPage<Message>>, StoreError> {
        if self.deleted {
            return Ok(SessionScoped::Deleted);
        }
        Ok(SessionScoped::Active(SessionPage {
            items: self
                .messages
                .iter()
                .filter(|stored| stored.session == session)
                .cloned()
                .collect(),
            next: self.next.clone(),
            deletion_epoch: aex_session_domain::DeletionEpoch(self.deletion_epoch),
        }))
    }

    async fn load_run(
        &self,
        _workspace: WorkspaceId,
        session: SessionId,
        run: RunId,
    ) -> Result<SessionScoped<Option<Run>>, StoreError> {
        if self.deleted {
            return Ok(SessionScoped::Deleted);
        }
        Ok(SessionScoped::Active(
            self.runs
                .iter()
                .find(|stored| stored.session == session && stored.id == run)
                .cloned(),
        ))
    }

    async fn page_runs(
        &self,
        _workspace: WorkspaceId,
        session: SessionId,
        _expected_deletion_epoch: Option<aex_session_domain::DeletionEpoch>,
        _budget: PageBudget,
        _after: Option<&PagePosition>,
    ) -> Result<SessionScoped<SessionPage<Run>>, StoreError> {
        if self.deleted {
            return Ok(SessionScoped::Deleted);
        }
        Ok(SessionScoped::Active(SessionPage {
            items: self
                .runs
                .iter()
                .filter(|stored| stored.session == session)
                .cloned()
                .collect(),
            next: self.next.clone(),
            deletion_epoch: aex_session_domain::DeletionEpoch(self.deletion_epoch),
        }))
    }

    async fn load_approval(
        &self,
        _workspace: WorkspaceId,
        _session: SessionId,
        _approval: ApprovalId,
    ) -> Result<SessionScoped<Option<Approval>>, StoreError> {
        if self.deleted {
            return Ok(SessionScoped::Deleted);
        }
        Ok(SessionScoped::Active(None))
    }

    async fn page_approvals(
        &self,
        _workspace: WorkspaceId,
        _session: SessionId,
        _expected_deletion_epoch: Option<aex_session_domain::DeletionEpoch>,
        _budget: PageBudget,
        _after: Option<&PagePosition>,
    ) -> Result<SessionScoped<SessionPage<Approval>>, StoreError> {
        if self.deleted {
            return Ok(SessionScoped::Deleted);
        }
        Ok(SessionScoped::Active(SessionPage {
            items: Vec::new(),
            next: self.next.clone(),
            deletion_epoch: aex_session_domain::DeletionEpoch(self.deletion_epoch),
        }))
    }
}

/// An in-memory custody authority.
///
/// Only the read surface the served routes use is answered. Every other method
/// returns a typed refusal rather than panicking, so a handler that reached for
/// an authority it must not touch fails the assertion with a name rather than
/// unwinding.
#[derive(Debug, Default)]
struct FakeCustody {
    secrets: BTreeMap<String, StoredSecret>,
    credentials: Vec<StoredCredential>,
    page_size: usize,
    /// Every plan the handler committed, so a case can assert the expression and
    /// not only the published body.
    committed: std::sync::Mutex<Vec<CommittedPlan>>,
}

/// What one committed transaction said, flattened for assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CommittedPlan {
    client_request_token: String,
    participants: Vec<String>,
    conditions: Vec<String>,
    updates: Vec<String>,
}

impl FakeCustody {
    fn out_of_scope(name: &'static str) -> StoreError {
        StoreError::Misconfigured {
            table: name.to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl SecretCustodyStore for FakeCustody {
    async fn load_secret(
        &self,
        _workspace: WorkspaceId,
        name: &SecretName,
    ) -> Result<Option<StoredSecret>, StoreError> {
        Ok(self.secrets.get(name.as_str()).cloned())
    }

    async fn list_secrets(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
    ) -> Result<Vec<StoredSecret>, StoreError> {
        Ok(self.page_secrets(workspace, budget, None).await?.items)
    }

    async fn load_generation(
        &self,
        _workspace: WorkspaceId,
        _name: &SecretName,
        _generation: SourceGeneration,
    ) -> Result<Option<aex_secret_custody_dynamodb::codec::StoredGeneration>, StoreError> {
        Err(Self::out_of_scope("secret.generation"))
    }

    async fn load_custody(
        &self,
        _workspace: WorkspaceId,
        _session: SessionId,
    ) -> Result<Option<aex_secret_custody_dynamodb::codec::CustodyHead>, StoreError> {
        Err(Self::out_of_scope("custody.head"))
    }

    async fn list_provider_credentials(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
    ) -> Result<Vec<StoredCredential>, StoreError> {
        Ok(self
            .page_provider_credentials(workspace, budget, None)
            .await?
            .items)
    }

    async fn load_provider_credential(
        &self,
        _workspace: WorkspaceId,
        credential: ProviderCredentialId,
    ) -> Result<Option<StoredCredential>, StoreError> {
        Ok(self
            .credentials
            .iter()
            .find(|row| row.credential == credential)
            .cloned())
    }

    async fn page_secrets(
        &self,
        _workspace: WorkspaceId,
        _budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<Page<StoredSecret>, StoreError> {
        let names: Vec<&String> = self.secrets.keys().collect();
        let start = match after {
            None => 0,
            Some(position) => names
                .iter()
                .position(|name| position.sk == format!("NAME#{name}"))
                .map_or(0, |at| at + 1),
        };
        let end = names.len().min(start + self.page_size.max(1));
        let items: Vec<StoredSecret> = names[start..end]
            .iter()
            .map(|name| self.secrets[*name].clone())
            .collect();
        let next = (end < names.len()).then(|| PagePosition {
            pk: format!("SEC#{}", workspace()),
            sk: format!("NAME#{}", names[end - 1]),
            index_pk: None,
            index_sk: None,
        });
        Ok(Page { items, next })
    }

    async fn page_provider_credentials(
        &self,
        _workspace: WorkspaceId,
        _budget: PageBudget,
        _after: Option<&PagePosition>,
    ) -> Result<Page<StoredCredential>, StoreError> {
        Ok(Page {
            items: self.credentials.clone(),
            next: None,
        })
    }

    async fn load_receipt(
        &self,
        _workspace: WorkspaceId,
        _scope: &str,
        _key_sha256_hex: &str,
        _now: Timestamp,
    ) -> Result<Option<Receipt>, StoreError> {
        Err(Self::out_of_scope("idempotency.receipt"))
    }

    async fn commit(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        self.committed
            .lock()
            .expect("an uncontended fixture")
            .push(CommittedPlan {
                client_request_token: plan.client_request_token().to_owned(),
                participants: plan
                    .participants()
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                conditions: plan
                    .actions()
                    .iter()
                    .filter_map(|action| {
                        action
                            .update()
                            .and_then(|update| update.condition_expression())
                            .map(str::to_owned)
                    })
                    .collect(),
                updates: plan
                    .actions()
                    .iter()
                    .filter_map(|action| {
                        action
                            .update()
                            .map(|update| update.update_expression().to_owned())
                    })
                    .collect(),
            });
        Ok(())
    }

    async fn commit_update(
        &self,
        _builder: aws_sdk_dynamodb::types::builders::UpdateBuilder,
        _participant: Participant,
    ) -> Result<(), StoreError> {
        Err(Self::out_of_scope("custody.update"))
    }
}

// --- the workspace projection doubles ---------------------------------------------

/// The cold workspace surface, holding exactly the rows a case gives it.
///
/// Absence is `Misconfigured`, which is the projection's single constructor for
/// "this region holds no usable record of that" — the same value the real reader
/// produces, so a handler that mapped it differently from production would fail
/// here rather than in a region.
#[derive(Debug, Default)]
struct FakeWorkspaceProjection {
    profile: Option<StoredProfile>,
    limits: BTreeMap<aex_wire::limits::LimitId, StoredLimit>,
    bundle: Option<StoredBundle>,
}

fn absent() -> StoreError {
    StoreError::Misconfigured {
        table: "dev-eu-west-1-regional-authz-projection".to_owned(),
    }
}

#[async_trait::async_trait]
impl WorkspaceProjection for FakeWorkspaceProjection {
    async fn read_profile(
        &self,
        _workspace: WorkspaceId,
    ) -> Result<Option<StoredProfile>, StoreError> {
        Ok(self.profile.clone())
    }

    async fn read_limit(
        &self,
        _workspace: WorkspaceId,
        limit: aex_wire::limits::LimitId,
    ) -> Result<Option<StoredLimit>, StoreError> {
        Ok(self.limits.get(&limit).cloned())
    }

    async fn page_limits(
        &self,
        _workspace: WorkspaceId,
        _budget: PageBudget,
        _after: Option<&PagePosition>,
    ) -> Result<ProjectionPage<StoredLimit>, StoreError> {
        unreachable!("the public list is served from the bundle item, never a paged member query")
    }

    async fn read_limit_bundle_head(
        &self,
        _workspace: WorkspaceId,
    ) -> Result<StoredBundleHead, StoreError> {
        unreachable!("the strong head is an admission fence, not a customer read")
    }

    async fn read_limit_bundle(&self, _workspace: WorkspaceId) -> Result<StoredBundle, StoreError> {
        self.bundle.clone().ok_or_else(absent)
    }
}

/// The placement reader, for the one workspace fact the context does not carry.
#[derive(Debug, Default)]
struct FakePlacements {
    placement: Option<StoredPlacement>,
}

#[async_trait::async_trait]
impl AuthorizationProjection for FakePlacements {
    async fn read_placement(&self, _workspace: WorkspaceId) -> Result<StoredPlacement, StoreError> {
        self.placement.clone().ok_or_else(absent)
    }

    async fn read_key_authorization(
        &self,
        _api_key: ApiKeyId,
    ) -> Result<aex_session_dynamodb::wire_pending::KeyAuthorization, StoreError> {
        unreachable!("the edge admitted this request; a handler never re-reads the key")
    }

    async fn read_admission_snapshot(
        &self,
        _api_key: ApiKeyId,
        _workspace: WorkspaceId,
    ) -> Result<aex_session_dynamodb::wire_pending::AdmissionSnapshot, StoreError> {
        unreachable!("admission ran before dispatch")
    }

    async fn read_frontier(
        &self,
    ) -> Result<aex_session_dynamodb::wire_pending::FeedFrontier, StoreError> {
        unreachable!("the frontier is the readiness probe's, not a request's")
    }
}

fn stored_profile(pause_reason: Option<&str>) -> StoredProfile {
    StoredProfile {
        workspace: workspace(),
        name: "Fixture".to_owned(),
        slug: "fixture".to_owned(),
        created_at: moment("2026-08-01T12:34:56.789Z"),
        account_revision: 7,
        account_changed_at: moment("2026-08-02T00:00:00.000Z"),
        account_pause_reason: pause_reason.map(str::to_owned),
    }
}

fn stored_placement(status: &str) -> StoredPlacement {
    StoredPlacement {
        workspace: workspace(),
        organization: sample(3),
        plane: "regional".to_owned(),
        region: Region::EuWest1.as_str().to_owned(),
        status: status.to_owned(),
        key_epoch: 0,
        account_epoch: 0,
        revocation_epoch: 0,
        feed_sequence: 1,
        updated_at: moment("2026-08-02T00:00:00.000Z"),
    }
}

/// One effective limit, as the capacity authority projects it.
fn effective_limit(id: aex_wire::limits::LimitId) -> models::EffectiveWorkspaceLimit {
    models::EffectiveWorkspaceLimit {
        changed_at: moment("2026-08-02T00:00:00.000Z"),
        effective_value: models::LimitValue::Scalar(models::LimitScalarValue {
            value: aex_wire::types::DecimalU128::new(65_536),
        }),
        id,
        revision: 3,
        source: models::LimitSource::Default,
    }
}

fn stored_limit(id: aex_wire::limits::LimitId) -> StoredLimit {
    let wire = effective_limit(id);
    StoredLimit {
        workspace: workspace(),
        id: wire.id,
        effective_value: wire.effective_value,
        source: wire.source,
        revision: wire.revision,
        changed_at: wire.changed_at,
    }
}

/// A bundle carrying exactly `count` of the registry's limits, in order.
fn stored_bundle(count: usize) -> StoredBundle {
    StoredBundle {
        workspace: workspace(),
        revision: 3,
        limits: aex_wire::limits::LimitId::ALL
            .iter()
            .copied()
            .take(count)
            .map(effective_limit)
            .collect(),
    }
}

fn context(request_id: RequestId, route_id: RouteId) -> RequestContext {
    RequestContext {
        request_id,
        route: route_id,
        auth: RegionalAuthorization {
            principal: PrincipalScope::WorkspaceKey {
                key: sample::<ApiKeyId>(1),
                workspace: workspace(),
                organization: sample(3),
            },
            credential_binding: [7; 32],
            organization_id: sample(3),
            workspace_id: workspace(),
            placement: Region::EuWest1,
            scopes: ScopeSet::default(),
            account_state: AccountState::Active,
            epochs: AuthorizationEpochs::default(),
        },
        limits: EffectiveLimits {
            json_body_bytes: 65_536,
            otlp_body_bytes: 4 * 1_024 * 1_024,
            query_page_items: 100,
            query_page_bytes: 1_048_576,
        },
        operation_id: None,
        idempotency: None,
        if_match: None,
        received_at: time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1_754_051_696),
    }
}

struct Admit;

#[async_trait::async_trait]
impl EdgeAdmission for Admit {
    async fn admit(&self, request: &AdmissionRequest<'_>) -> Result<RequestContext, WireError> {
        Ok(context(request.request_id.clone(), request.route))
    }
}

// --- the registry authority double ------------------------------------------------

/// A registry holding one pointer per kind, plus a recorded continuation.
///
/// It answers `list_pointers` from an in-memory map keyed by the kind it was
/// asked for, so a handler that named the wrong kind reads an empty page and the
/// case fails on the item count rather than passing by accident.
#[derive(Debug, Default)]
struct FakeRegistry {
    pointers: BTreeMap<&'static str, Vec<RegistryPointer>>,
    next: Option<PagePosition>,
    asked: std::sync::Mutex<Vec<RegistryKind>>,
    fails: bool,
}

fn kind_key(kind: RegistryKind) -> &'static str {
    match kind {
        RegistryKind::File => "file",
        RegistryKind::Skill => "skill",
        RegistryKind::Tool => "tool",
        RegistryKind::Instruction => "instruction",
        RegistryKind::McpServer => "mcp_server",
    }
}

/// The canonical value document each kind stores.
///
/// Written out per kind rather than shared, because the point projection decodes
/// it as that kind's read model and one shared shape would prove nothing.
fn value_document(kind: RegistryKind) -> ValueDocument {
    let payload = ContentHash::of(b"body").to_wire();
    let text = match kind {
        RegistryKind::File => format!(
            r#"{{"mountPath":"/etc/notes.md","mediaType":"text/markdown","mode":"0644",
                "content":{{"sha256":"{payload}","sizeBytes":"4"}}}}"#
        ),
        RegistryKind::Skill => format!(
            r#"{{"description":"review","bundleFormat":"tar.gz",
                "bundle":{{"sha256":"{payload}","sizeBytes":"4"}}}}"#
        ),
        RegistryKind::Tool => format!(
            r#"{{"description":"search","inputSchema":{{"type":"object"}},"entry":"main.js",
                "bundleFormat":"tar.gz",
                "bundle":{{"sha256":"{payload}","sizeBytes":"4"}}}}"#
        ),
        RegistryKind::Instruction => r#"{"text":"be concise"}"#.to_owned(),
        RegistryKind::McpServer => {
            r#"{"url":"https://example.test/mcp","transport":"streamable_http","headers":[]}"#
                .to_owned()
        }
    };
    ValueDocument::new(aex_wire::CanonicalJson::parse(&text).expect("valid JSON"))
}

fn registry_row(kind: RegistryKind, name: &str) -> StoredRegistryRow {
    StoredRegistryRow {
        workspace: workspace(),
        kind,
        name: ResourceName::parse(name).expect("a resource name"),
        revision: Revision(4),
        etag: ETag::parse("\"registry-4\"").expect("a strong validator"),
        sha256: ContentHash::of(b"body"),
        size_bytes: 128,
        created_at: moment("2026-08-01T12:34:56.789Z"),
        updated_at: moment("2026-08-01T12:34:56.789Z"),
    }
}

fn pointer(kind: RegistryKind, name: &str) -> RegistryPointer {
    RegistryPointer {
        row: registry_row(kind, name),
        value_doc: value_document(kind),
    }
}

#[async_trait::async_trait]
impl RegistryStore for FakeRegistry {
    async fn load_pointer(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
        name: &str,
    ) -> Result<Option<RegistryPointer>, StoreError> {
        assert_eq!(workspace, crate::workspace(), "a point read is scoped");
        if self.fails {
            return Err(StoreError::Contended);
        }
        Ok(self
            .pointers
            .get(kind_key(kind))
            .and_then(|rows| {
                rows.iter()
                    .find(|pointer| pointer.row.name.as_str() == name)
            })
            .cloned())
    }

    async fn list_pointers(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
        _budget: PageBudget,
        _from: Option<&PagePosition>,
    ) -> Result<PointerPage, StoreError> {
        assert_eq!(
            workspace,
            crate::workspace(),
            "the listing is workspace-scoped"
        );
        self.asked
            .lock()
            .expect("an uncontended fixture")
            .push(kind);
        if self.fails {
            return Err(StoreError::Contended);
        }
        Ok(PointerPage {
            rows: self
                .pointers
                .get(kind_key(kind))
                .map(|rows| rows.iter().map(|pointer| pointer.row.clone()).collect())
                .unwrap_or_default(),
            next: self.next.clone(),
        })
    }

    async fn commit_set(
        &self,
        _workspace: WorkspaceId,
        _commit: SetCommit<'_>,
    ) -> Result<SetCommitted, StoreError> {
        Ok(SetCommitted::Committed)
    }

    async fn commit_delete(
        &self,
        _workspace: WorkspaceId,
        _kind: RegistryKind,
        _name: &str,
        _if_match: Option<&ETag>,
    ) -> Result<DeleteCommitted, StoreError> {
        Ok(DeleteCommitted::Removed)
    }

    async fn load_count(
        &self,
        _workspace: WorkspaceId,
        _kind: RegistryKind,
    ) -> Result<u64, StoreError> {
        Ok(0)
    }

    async fn load_upload(
        &self,
        _workspace: WorkspaceId,
        _upload: UploadId,
    ) -> Result<Option<StoredUpload>, StoreError> {
        Err(StoreError::Contended)
    }

    async fn create_upload(&self, _upload: &StoredUpload) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn transition_upload(
        &self,
        _upload: UploadId,
        _from: UploadState,
        _to: UploadState,
    ) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn transition_upload_fenced(
        &self,
        _upload: UploadId,
        _from: UploadState,
        _to: UploadState,
        _provider_upload_id: &str,
    ) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn record_part_declarations(&self, _upload: &StoredUpload) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn settle_ready(&self, _upload: &StoredUpload) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn delete_upload(&self, _upload: &StoredUpload) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn begin_completion(
        &self,
        _upload: &StoredUpload,
        _completion_intent_hash: &str,
    ) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn stage_part_blocks(&self, _upload: &StoredUpload) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn commit_admission(
        &self,
        _plan: &aex_session_dynamodb::plan::TransactionPlan,
    ) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn finish_completion(
        &self,
        _upload: UploadId,
        _completion_intent_hash: &str,
    ) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }
}

fn cursor_keys() -> CursorKeyRing {
    CursorKeyRing::new(
        CursorKey::new("cur-1", vec![9u8; 32]).expect("a strong key"),
        Vec::new(),
    )
    .expect("a ring")
}

/// The physical table name the composition root supplies.
const CUSTODY_TABLE: &str = "dev-eu-west-1-regional-secret-custody";

fn router(custody: FakeCustody) -> (axum::Router, Vec<RouteId>) {
    composed(custody, FakeRegistry::default())
}

fn composed(custody: FakeCustody, registry: FakeRegistry) -> (axum::Router, Vec<RouteId>) {
    build(Arc::new(custody), Arc::new(registry)).0
}

fn build(
    custody: Arc<FakeCustody>,
    registry: Arc<FakeRegistry>,
) -> ((axum::Router, Vec<RouteId>), Arc<FakeCustody>) {
    build_with_sessions(custody, registry, Arc::new(FakeSessions::default()))
}

fn build_with_sessions(
    custody: Arc<FakeCustody>,
    registry: Arc<FakeRegistry>,
    sessions: Arc<FakeSessions>,
) -> ((axum::Router, Vec<RouteId>), Arc<FakeCustody>) {
    build_with_authorities(
        custody,
        registry,
        sessions,
        Arc::new(FakeOperations::default()),
    )
}

fn build_with_operations(
    custody: Arc<FakeCustody>,
    registry: Arc<FakeRegistry>,
    operations: Arc<FakeOperations>,
) -> ((axum::Router, Vec<RouteId>), Arc<FakeCustody>) {
    build_with_authorities(
        custody,
        registry,
        Arc::new(FakeSessions::default()),
        operations,
    )
}

/// The physical `session-authority` name the mount fixture binds.
const SESSION_TABLE: &str = "dev-eu-west-1-session-authority";

const RUNTIME_ACTIVITY_TABLE: &str = "dev-eu-west-1-runtime-activity";

/// A `DynamoDB` client whose transport refuses every request.
///
/// The mount assertions here exercise routing and the absence sweep, never a
/// command's durable effect. A client that answers nothing keeps that honest:
/// a test that started depending on a stored row would fail rather than pass
/// against an invented one.
fn offline_dynamodb() -> aws_sdk_dynamodb::Client {
    let (http_client, _receiver) = aws_smithy_http_client::test_util::capture_request(None);
    aws_sdk_dynamodb::Client::from_conf(
        aws_sdk_dynamodb::Config::builder()
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
            .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
                "AKIDTESTTESTTESTTEST",
                "test-secret",
                None,
                None,
                "aex-tests",
            ))
            .http_client(http_client)
            .build(),
    )
}

/// The physical `regional-content` table name this suite binds.
const CONTENT_TABLE: &str = "dev-eu-west-1-regional-content";

/// An S3 client bound to a capturing transport.
///
/// The mount test proves which routes the router offers and which it refuses; it
/// never drives a handler to the provider. Pointing the real adapters at an
/// offline transport keeps the composition identical to production, which is the
/// point of the test — a second, hand-written set of fakes could drift from the
/// adapters the process actually builds.
fn offline_s3() -> aws_sdk_s3::Client {
    let (http_client, _receiver) = aws_smithy_http_client::test_util::capture_request(None);
    aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("eu-west-1"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "AKIDTESTTESTTESTTEST",
                "test-secret",
                None,
                None,
                "aex-tests",
            ))
            .http_client(http_client)
            .build(),
    )
}

fn build_with_authorities(
    custody: Arc<FakeCustody>,
    registry: Arc<FakeRegistry>,
    sessions: Arc<FakeSessions>,
    operations: Arc<FakeOperations>,
) -> ((axum::Router, Vec<RouteId>), Arc<FakeCustody>) {
    build_with_workspace(
        custody,
        registry,
        sessions,
        operations,
        Arc::new(FakeWorkspaceProjection::default()),
        Arc::new(FakePlacements::default()),
    )
}

/// The workspace router, over an explicit projection.
///
/// Separate from the default builder because the workspace cases are about what
/// the projection holds: an absent row, a short set and a complete one are three
/// different answers, and each has to be constructible on its own.
fn build_with_workspace(
    custody: Arc<FakeCustody>,
    registry: Arc<FakeRegistry>,
    sessions: Arc<FakeSessions>,
    operations: Arc<FakeOperations>,
    projection: Arc<FakeWorkspaceProjection>,
    placements: Arc<FakePlacements>,
) -> ((axum::Router, Vec<RouteId>), Arc<FakeCustody>) {
    let shared = shared_with_workspace(
        Arc::clone(&custody),
        registry,
        sessions,
        operations,
        projection,
        placements,
    );
    let mounted = mount_unary(
        Arc::new(Dispatcher::new(shared)),
        Arc::new(Admit),
        aex_wire::dispatch::RequestLimits::DEFAULT,
    )
    .expect("the served set mounts");
    ((mounted.router, mounted.routes), custody)
}

fn shared_with_workspace(
    custody: Arc<FakeCustody>,
    registry: Arc<FakeRegistry>,
    sessions: Arc<FakeSessions>,
    operations: Arc<FakeOperations>,
    projection: Arc<FakeWorkspaceProjection>,
    placements: Arc<FakePlacements>,
) -> Arc<Shared> {
    Arc::new(Shared {
        catalog: Arc::new(aex_session_app::testing::ScriptedPorts::idle()),
        deployment: aex_session_app::testing::deployment_facts(),
        live_files: Arc::new(NoLiveFiles),
        live_transfers: Arc::new(
            session_stream_api::session::live_transfer::LiveTransferDynamoStore::new(
                offline_dynamodb(),
                SESSION_TABLE,
            ),
        ),
        workspace: projection as Arc<dyn WorkspaceProjection>,
        placements: placements as Arc<dyn AuthorizationProjection>,
        api_url: aex_wire::types::HttpsUrl::parse("https://eu-west-1.aex.dev")
            .expect("a regional host"),
        custody: custody as Arc<dyn SecretCustodyStore>,
        custody_reads: aex_secret_custody_dynamodb::store::CustodyStore::new(
            offline_dynamodb(),
            CUSTODY_TABLE,
        ),
        custody_table: CUSTODY_TABLE.to_owned(),
        plane: aex_secret_domain::context::Plane::Dev,
        region: aex_wire::types::Region::EuWest1,
        registry: registry as Arc<dyn RegistryStore>,
        // Both content authorities are bound to an offline transport. Every
        // route this suite drives — the five point reads and the five listings —
        // reads the registry only, so a handler that reached content would fail
        // at the wire rather than pass on an invented answer.
        content: Arc::new(aex_content_dynamodb::store::ContentStore::new(
            offline_dynamodb(),
            CONTENT_TABLE,
        )) as Arc<dyn ContentMetadataStore>,
        content_objects: Arc::new(aex_content_aws::object_store::S3ContentObjects::new(
            offline_s3(),
            aex_content_aws::object_store::BucketBinding {
                bucket: "aex-dev-eu-west-1-content".to_owned(),
                expected_owner: "000000000000".to_owned(),
                kms_key_id: "arn:aws:kms:eu-west-1:000000000000:key/content".to_owned(),
            },
        )) as Arc<dyn ContentObjectStore>,
        receipts: Arc::new(aex_registry_dynamodb::store::RegistryDynamoStore::new(
            offline_dynamodb(),
            "aex-dev-regional-registry",
        )),
        registry_table: "aex-dev-regional-registry".to_owned(),
        work_table: "aex-dev-regional-work".to_owned(),
        content_kms_key_id: "arn:aws:kms:eu-west-1:000000000000:key/content".to_owned(),
        content_encryption_context: Vec::new(),
        registry_entries: 1_000,
        registry_value_bytes: 65_536,
        sessions: sessions as Arc<dyn SessionQueries>,
        operations: operations as Arc<dyn OperationApiStore>,
        commands: aex_session_dynamodb::app_authority::SessionCommandReads::new(
            offline_dynamodb(),
            SESSION_TABLE,
        ),
        tables: aex_session_dynamodb::plan::RegionalTables::composed("dev", "eu-west-1"),
        authority: offline_dynamodb(),
        usage: Arc::new(FakeUsage::default()) as Arc<dyn UsageProjectionReads>,
        cursor_keys: Arc::new(cursor_keys()),
        runtime_activity: aex_runtime_activity_dynamodb::store::RuntimeActivityDynamoStore::new(
            offline_dynamodb(),
            RUNTIME_ACTIVITY_TABLE,
        ),
    })
}

/// A registry holding one public workspace-file pointer.
fn populated_registry() -> FakeRegistry {
    FakeRegistry {
        pointers: BTreeMap::from([(
            kind_key(RegistryKind::File),
            vec![pointer(RegistryKind::File, "notes.md")],
        )]),
        ..FakeRegistry::default()
    }
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Option<String>, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let etag = response
        .headers()
        .get(axum::http::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).expect("the response body is JSON")
    };
    (status, etag, json)
}

async fn post(router: &axum::Router, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("the response body is JSON")
    };
    (status, json)
}

// --- the served set --------------------------------------------------------------

#[test]
fn the_served_set_is_a_subset_of_the_owned_set_and_never_a_second_list() {
    let served = Routes::served();
    let owned = RouteOwner::SessionApi.routes();
    assert!(!served.is_empty(), "the deployable answers something");
    for id in &served {
        assert!(
            owned.contains(id),
            "`{id}` is served but not owned by this deployable"
        );
    }
    assert_eq!(
        served,
        {
            let mut sorted = served.clone();
            sorted.sort_unstable_by_key(|id| *id as usize);
            sorted
        },
        "the served set keeps `RouteId` order"
    );
}

#[test]
fn the_three_workspace_reads_are_owned_and_served() {
    let served = Routes::served();
    for id in [
        RouteId::WorkspaceCurrentGet,
        RouteId::WorkspaceLimitGet,
        RouteId::WorkspaceLimitsList,
    ] {
        assert_eq!(route_owner(id), Some(RouteOwner::SessionApi), "`{id}`");
        assert!(
            served.contains(&id),
            "`{id}` has a complete handler and an authority behind it"
        );
    }
}

#[test]
fn the_served_set_exactly_matches_the_generated_actual_mount_authority() {
    let registry: serde_json::Value = serde_json::from_str(include_str!(
        "../../../api/generated/registries/routes.json"
    ))
    .expect("generated route registry");
    // Both halves of the merged deployable name the same artifact, so the
    // transport is what selects the unary half. See
    // `aex_regional_http::router::route_owner`.
    let generated: Vec<RouteId> = registry["routes"]
        .as_array()
        .expect("route rows")
        .iter()
        .filter(|route| {
            route["servedArtifact"] == "session-stream-api" && route["transport"] != "ndjson"
        })
        .map(|route| {
            RouteId::parse(route["operationId"].as_str().expect("operation id"))
                .expect("generated operation id")
        })
        .collect();
    assert_eq!(Routes::served(), generated);

    let session_export = registry["routes"]
        .as_array()
        .expect("route rows")
        .iter()
        .find(|route| route["operationId"] == "session_telemetry_export_create")
        .expect("session telemetry export route");
    assert_eq!(
        session_export["servedArtifact"], "regional-observation-api",
        "session telemetry export admission is mounted by its observation owner"
    );
}

#[tokio::test]
async fn the_router_answers_exactly_the_served_set() {
    let (_, mounted) = router(FakeCustody::default());
    assert_eq!(mounted, Routes::served());
}

#[tokio::test]
async fn registry_file_download_is_mounted_and_requires_replay_identity_before_authority_reads() {
    let registry = FakeRegistry {
        fails: true,
        ..FakeRegistry::default()
    };
    let (router, mounted) = composed(FakeCustody::default(), registry);
    assert!(mounted.contains(&RouteId::RegistryFilesDownloadCreate));
    assert!(!route(RouteId::RegistryFilesDownloadCreate).deferred);

    let (status, body) = post(&router, "/api/workspace/files/notes.md/downloads", "{}").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], ErrorCode::InvalidRequest.as_str());
}

/// Every owned route the contract defers answers the published refusal.
///
/// This replaces an assertion that it was *absent*. A bare `404` cannot be told
/// apart from a mistyped path, a wrong base URL or a wrong region, and that is
/// the ambiguity a caller meets in the first ten minutes of an integration. The
/// arm carries no reason text: the ledger's prose is an engineering note, and a
/// stale one published to a customer is worse than none.
#[tokio::test]
async fn every_deferred_route_answers_the_published_refusal() {
    let (router, mounted) = router(FakeCustody::default());
    let deferred: Vec<RouteId> = RouteOwner::SessionApi
        .routes()
        .into_iter()
        .filter(|id| !mounted.contains(id))
        .collect();
    assert!(!deferred.is_empty(), "the deployable still owes routes");
    for id in deferred {
        assert!(route(id).deferred, "`{id}` is unserved and not deferred");
        let descriptor = route(id);
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(descriptor.method.as_str())
                    .uri(deferred_path(id))
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");
        assert_eq!(
            response.status(),
            StatusCode::NOT_IMPLEMENTED,
            "`{id}` answered {}",
            response.status()
        );
        let body = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        let envelope: serde_json::Value = serde_json::from_slice(&body).expect("an envelope");
        assert_eq!(
            envelope["error"]["code"],
            ErrorCode::NotImplemented.as_str()
        );
        assert_eq!(envelope["error"]["retryable"], false);
        assert!(
            envelope["error"]["requestId"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "`{id}` refused without a diagnostic identity"
        );
    }
}

/// A concrete path for a deferred template, bound positionally from the table.
fn deferred_path(id: RouteId) -> String {
    route(id)
        .template
        .split('/')
        .map(|segment| {
            match segment
                .strip_prefix('{')
                .and_then(|rest| rest.strip_suffix('}'))
            {
                Some("limitId") => "session.subagent_concurrency",
                Some("name") => "fixture",
                Some("kind") => "files",
                Some(_) => "01jxt21q00e40r2081040g2081",
                None => segment,
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// A wrong method on a deferred template is the router's `405`, never a `404`.
#[tokio::test]
async fn a_wrong_method_on_a_deferred_path_is_a_method_refusal() {
    let (router, _) = router(FakeCustody::default());
    let response = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(deferred_path(RouteId::SessionCancel))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

/// Create is mounted only after private replay-key election, exact registered
/// file materialization, provider-native readiness, durable root AgentStarted,
/// final active-account fencing, and an exact response receipt are composed.
#[test]
fn session_create_is_mounted_only_after_exact_generation_readiness_is_composed() {
    assert!(Routes::served().contains(&RouteId::SessionCreate));
    assert!(!route(RouteId::SessionCreate).deferred);
}

// --- durable operations ----------------------------------------------------------

#[tokio::test]
async fn operation_get_projects_the_typed_result_and_hides_internal_gc() {
    let session = sample::<SessionId>(40);
    let public = stored_operation(
        41,
        OperationKind::SessionSuspend,
        OperationStatus::Succeeded,
        Some(session),
    );
    let internal = stored_operation(42, OperationKind::ContentGc, OperationStatus::Running, None);
    let operations = Arc::new(FakeOperations {
        rows: std::sync::Mutex::new(vec![public.clone(), internal.clone()]),
        ..FakeOperations::default()
    });
    let ((router, mounted), _) = build_with_operations(
        Arc::new(FakeCustody::default()),
        Arc::new(FakeRegistry::default()),
        operations,
    );
    assert!(mounted.contains(&RouteId::RegionalOperationGet));

    let (status, _, body) = get(&router, &format!("/api/operations/{}", public.record.id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let decoded: models::Operation =
        serde_json::from_value(body.clone()).expect("the published operation schema");
    assert_eq!(decoded.workspace_id, workspace());
    assert_eq!(decoded.session_id, Some(session));
    assert_eq!(decoded.kind, models::OperationKind::SessionSuspend);
    assert_eq!(decoded.status, models::OperationStatus::Succeeded);
    assert!(matches!(
        decoded.result,
        Some(models::OperationResult::SessionSuspend(
            models::SessionSuspendResult {
                changed: true,
                session_revision: 9,
                ..
            }
        ))
    ));

    let (status, _, body) = get(&router, &format!("/api/operations/{}", internal.record.id)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], ErrorCode::NotFound.as_str());
}

fn operation_list_fixture(
    session: SessionId,
    other_session: SessionId,
) -> (Arc<FakeOperations>, OperationId, OperationId) {
    let first = stored_operation(
        45,
        OperationKind::SessionSuspend,
        OperationStatus::Succeeded,
        Some(session),
    );
    let second = stored_operation(
        46,
        OperationKind::SessionSuspend,
        OperationStatus::Succeeded,
        Some(session),
    );
    let first_id = first.record.id;
    let second_id = second.record.id;
    let operations = Arc::new(FakeOperations {
        rows: std::sync::Mutex::new(vec![
            first.clone(),
            second.clone(),
            stored_operation(
                47,
                OperationKind::SessionResume,
                OperationStatus::Succeeded,
                Some(session),
            ),
            stored_operation(
                48,
                OperationKind::SessionSuspend,
                OperationStatus::Running,
                Some(session),
            ),
            stored_operation(
                49,
                OperationKind::SessionSuspend,
                OperationStatus::Succeeded,
                Some(other_session),
            ),
            stored_operation(
                50,
                OperationKind::ContentGc,
                OperationStatus::Succeeded,
                None,
            ),
        ]),
        page_size: 1,
        ..FakeOperations::default()
    });
    (operations, first_id, second_id)
}

#[tokio::test]
async fn operation_list_filters_exactly_and_binds_the_cursor_to_every_filter() {
    let session = sample::<SessionId>(43);
    let other_session = sample::<SessionId>(44);
    let (operations, first_id, second_id) = operation_list_fixture(session, other_session);
    let ((router, mounted), _) = build_with_operations(
        Arc::new(FakeCustody::default()),
        Arc::new(FakeRegistry::default()),
        Arc::clone(&operations),
    );
    assert!(mounted.contains(&RouteId::RegionalOperationsList));
    let query = format!(
        "/api/operations?sessionId={session}&kind=session_suspend&status=succeeded&limit=1"
    );

    let (status, _, body) = get(&router, &query).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let first_page: models::OperationPage =
        serde_json::from_value(body).expect("the published page schema");
    assert_eq!(first_page.items.len(), 1);
    assert_eq!(first_page.items[0].id, first_id);
    let cursor = first_page.next_cursor.expect("another exact match remains");

    let (status, _, body) = get(&router, &format!("{query}&cursor={}", cursor.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let second_page: models::OperationPage =
        serde_json::from_value(body).expect("the published page schema");
    assert_eq!(second_page.items.len(), 1);
    assert_eq!(second_page.items[0].id, second_id);
    let empty_cursor = second_page
        .next_cursor
        .expect("unexamined physical rows remain behind the exact matches");

    let (status, _, body) = get(
        &router,
        &format!("{query}&cursor={}", empty_cursor.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let empty_page: models::OperationPage =
        serde_json::from_value(body).expect("the published page schema");
    assert!(
        empty_page.items.is_empty(),
        "the next physical row is filtered"
    );
    assert!(
        empty_page.next_cursor.is_some(),
        "a filtered empty page must not signal a false end while physical rows remain"
    );

    let (status, _, body) = get(
        &router,
        &format!(
            "/api/operations?sessionId={session}&kind=session_suspend&status=running&limit=1&cursor={}",
            cursor.as_str()
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], ErrorCode::InvalidCursor.as_str());

    let (status, _, body) = get(
        &router,
        &format!(
            "/api/operations?sessionId={session}&kind=session_resume&status=succeeded&limit=1&cursor={}",
            cursor.as_str()
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], ErrorCode::InvalidCursor.as_str());

    let (status, _, body) = get(
        &router,
        &format!(
            "/api/operations?sessionId={other_session}&kind=session_suspend&status=succeeded&limit=1&cursor={}",
            cursor.as_str()
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], ErrorCode::InvalidCursor.as_str());

    let filters = operations.filters.lock().expect("an uncontended fixture");
    assert_eq!(
        filters.len(),
        3,
        "invalid cursors never reach the authority"
    );
    assert!(filters.iter().all(|filter| {
        filter.session == Some(session)
            && filter.kind == Some(OperationKind::SessionSuspend)
            && filter.status == Some(OperationStatus::Succeeded)
    }));
}

#[tokio::test]
async fn operation_cancel_refuses_current_lifecycle_work_and_hides_internal_work() {
    let session = sample::<SessionId>(51);
    let running = stored_operation(
        52,
        OperationKind::SessionResume,
        OperationStatus::Running,
        Some(session),
    );
    let queued = stored_operation(
        53,
        OperationKind::SessionResume,
        OperationStatus::Queued,
        Some(session),
    );
    let fixed = stored_operation(
        54,
        OperationKind::SessionSuspend,
        OperationStatus::Running,
        Some(session),
    );
    let internal = stored_operation(55, OperationKind::ContentGc, OperationStatus::Running, None);
    let telemetry = stored_operation(
        56,
        OperationKind::TelemetryExport,
        OperationStatus::Queued,
        Some(session),
    );
    let operations = Arc::new(FakeOperations {
        rows: std::sync::Mutex::new(vec![
            running.clone(),
            queued.clone(),
            fixed.clone(),
            internal.clone(),
            telemetry.clone(),
        ]),
        ..FakeOperations::default()
    });
    let ((router, mounted), _) = build_with_operations(
        Arc::new(FakeCustody::default()),
        Arc::new(FakeRegistry::default()),
        Arc::clone(&operations),
    );
    assert!(mounted.contains(&RouteId::RegionalOperationCancel));

    let cancel = |operation: OperationId| format!("/api/operations/{operation}/cancellations");
    for operation in [running, queued, fixed, telemetry] {
        let (status, body) = post(&router, &cancel(operation.record.id), "{}").await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(
            body["error"]["code"],
            ErrorCode::OperationNotCancelable.as_str()
        );
    }
    assert_eq!(
        *operations.writes.lock().expect("an uncontended fixture"),
        0,
        "current lifecycle commands own their own effect fences"
    );

    let (status, body) = post(&router, &cancel(internal.record.id), "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], ErrorCode::NotFound.as_str());
}

// --- provider credential reads ----------------------------------------------------

#[tokio::test]
async fn a_credential_read_answers_the_persisted_fingerprint_and_never_the_secret() {
    let stored = stored_credential();
    let custody = FakeCustody {
        credentials: vec![stored.clone()],
        ..FakeCustody::default()
    };
    let (router, _) = router(custody);
    let (status, etag, body) = get(
        &router,
        &format!("/api/workspace/provider-credentials/{}", stored.credential),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let decoded: models::ProviderCredential =
        serde_json::from_value(body.clone()).expect("the published schema");
    assert_eq!(decoded.fingerprint, stored.fingerprint);
    assert_eq!(decoded.revision, 1);
    assert_eq!(decoded.provider, models::ProviderId::Openai);
    assert!(
        !body.to_string().contains(stored.secret_name.as_str()),
        "the referenced workspace secret must not reach the wire: {body}"
    );
    assert!(etag.is_some(), "the route declares `etag: returns`");
}

#[tokio::test]
async fn an_absent_credential_answers_the_declared_code() {
    let (router, _) = router(FakeCustody::default());
    let absent: ProviderCredentialId = sample(9);
    let (status, _, body) = get(
        &router,
        &format!("/api/workspace/provider-credentials/{absent}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::ProviderCredentialNotFound.as_str()),
        "the route declares its own not-found code"
    );
}

#[tokio::test]
async fn a_credential_listing_applies_the_declared_provider_filter() {
    let mut other = stored_credential();
    other.credential = sample(7);
    other.provider = models::ProviderId::Anthropic;
    let custody = FakeCustody {
        credentials: vec![stored_credential(), other],
        ..FakeCustody::default()
    };
    let (router, _) = router(custody);

    let (status, _, body) = get(&router, "/api/workspace/provider-credentials").await;
    assert_eq!(status, StatusCode::OK);
    let all: models::ProviderCredentialPage =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(all.items.len(), 2);

    let (_, _, body) = get(
        &router,
        "/api/workspace/provider-credentials?provider=anthropic",
    )
    .await;
    let filtered: models::ProviderCredentialPage =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items[0].provider, models::ProviderId::Anthropic);
}

// --- provider credential revocation -------------------------------------------------

fn revocations_path(credential: ProviderCredentialId) -> String {
    format!("/api/workspace/provider-credentials/{credential}/revocations")
}

#[tokio::test]
async fn a_revocation_fences_the_binding_and_publishes_the_committed_row() {
    let stored = stored_credential();
    let ((router, _), custody) = build(
        Arc::new(FakeCustody {
            credentials: vec![stored.clone()],
            ..FakeCustody::default()
        }),
        Arc::new(FakeRegistry::default()),
    );

    let (status, body) = post(&router, &revocations_path(stored.credential), "{}").await;
    assert_eq!(status, StatusCode::OK);
    let decoded: models::ProviderCredential =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(decoded.state, models::ProviderCredentialState::Revoked);
    assert_eq!(
        decoded.revision,
        stored.revision + 1,
        "a fenced binding publishes the revision the condition committed against"
    );
    assert!(decoded.revoked_at.is_some());

    // The body being right is not enough: a revocation that stopped conditioning
    // on the observed state would still publish this answer.
    let plans = custody.committed.lock().expect("an uncontended fixture");
    assert_eq!(plans.len(), 1, "exactly one transaction");
    let plan = &plans[0];
    assert_eq!(plan.participants, vec!["custody.provider_credential"]);
    assert_eq!(plan.conditions.len(), 1);
    assert!(
        plan.conditions[0].contains("revision = :expectedRevision"),
        "the fence must condition on the observed revision: {}",
        plan.conditions[0]
    );
    assert!(
        plan.conditions[0].contains(":ready"),
        "only a ready binding may be fenced: {}",
        plan.conditions[0]
    );
    assert!(plan.updates[0].contains("revokedAt = :now"));
    assert!(
        plan.client_request_token.starts_with("aex-"),
        "the transaction carries a deterministic deduplication identity"
    );
}

/// The route declares an `Idempotency-Key` and needs no durable receipt: the
/// scope subject is the credential and the body is empty, so a replay can only
/// carry the same intent and is answered from the stored row (RS-31).
#[tokio::test]
async fn a_replayed_revocation_answers_the_stored_row_and_writes_nothing() {
    let mut already = stored_credential();
    already.state = CredentialState::Revoked;
    already.revoked_at = Some(moment("2026-08-01T13:00:00.000Z"));
    let ((router, _), custody) = build(
        Arc::new(FakeCustody {
            credentials: vec![already.clone()],
            ..FakeCustody::default()
        }),
        Arc::new(FakeRegistry::default()),
    );

    let (status, body) = post(&router, &revocations_path(already.credential), "{}").await;
    assert_eq!(status, StatusCode::OK);
    let decoded: models::ProviderCredential =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(decoded.state, models::ProviderCredentialState::Revoked);
    assert_eq!(
        decoded.revision, already.revision,
        "a replay must not advance the revision a concurrent reader is fencing on"
    );
    assert!(
        custody
            .committed
            .lock()
            .expect("an uncontended fixture")
            .is_empty(),
        "a replay writes nothing"
    );
}

#[tokio::test]
async fn revoking_an_absent_binding_answers_the_declared_code() {
    let (router, _) = router(FakeCustody::default());
    let absent: ProviderCredentialId = sample(9);
    let (status, body) = post(&router, &revocations_path(absent), "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::ProviderCredentialNotFound.as_str())
    );
}

/// The deployable holds `TransactWriteItems` on `regional-secret-custody` and is
/// deliberately not granted `UpdateItem`, so the revocation has to reach the
/// authority as a transaction. A bare conditional update would pass every local
/// test and be denied in production.
#[tokio::test]
async fn the_revocation_reaches_the_authority_as_a_transaction() {
    let stored = stored_credential();
    let ((router, _), custody) = build(
        Arc::new(FakeCustody {
            credentials: vec![stored.clone()],
            ..FakeCustody::default()
        }),
        Arc::new(FakeRegistry::default()),
    );
    let (status, _) = post(&router, &revocations_path(stored.credential), "{}").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        custody
            .committed
            .lock()
            .expect("an uncontended fixture")
            .len(),
        1,
        "the only write path this deployable is granted is a transaction"
    );
}

/// The `provider-credentials` fragment is split across two deployables, so this
/// trait implementation carries methods for the other half. They are unreachable
/// through the router, and that is what makes the split safe rather than merely
/// conventional.
#[test]
fn the_other_half_of_the_provider_credentials_fragment_is_unreachable_here() {
    let served = Routes::served();
    let theirs = RouteOwner::SecretApi.routes_in(RouteGroup::ProviderCredentials);
    assert!(!theirs.is_empty(), "the group has a secret-api half");
    for id in theirs {
        assert!(!served.contains(&id), "`{id}` is the secret edge's");
    }
}

/// Every served route's declared success status is what the router actually
/// answers, because the handler never names one.
///
/// The handler's *return type* is what the generated dispatcher maps onto a
/// status — `Accepted` is the route's declared `202`, `Created` its `201`,
/// `NoContent` its `204` — so a handler that wanted a different status would
/// have to return a type its trait method does not declare. This asserts the
/// declared status stays inside that closed set; it deliberately no longer
/// asserts `200`, which was only ever true while every served route was a read.
#[test]
fn no_served_route_lets_a_handler_choose_a_status() {
    for id in Routes::served() {
        let declared = route(id).success_status;
        assert!(
            matches!(declared, 200 | 201 | 202 | 204),
            "`{id}` declares {declared}, which no handler return type can render"
        );
    }
}

/// Every mounted lifecycle mutation answers the durable-operation shape.
#[test]
fn the_mounted_session_mutations_are_durable_operation_admissions() {
    for id in [
        RouteId::SessionCancel,
        RouteId::SessionSuspend,
        RouteId::SessionResume,
        RouteId::SessionTerminate,
    ] {
        assert!(Routes::served().contains(&id), "`{id}` must be mounted");
        assert_eq!(
            route(id).success_status,
            202,
            "`{id}` admits a durable operation rather than returning a resource"
        );
    }
}

fn unused_custody_revision() -> CustodyRevision {
    CustodyRevision::FIRST
}

#[test]
fn the_fixture_module_stays_honest() {
    // `CustodyRevision` is imported for the store trait's signature surface;
    // this keeps the import load-bearing rather than silently unused.
    assert_eq!(unused_custody_revision(), CustodyRevision::FIRST);
}

// --- the registry listings ---------------------------------------------------------

/// The one public registry listing and the collection it must read.
const REGISTRY_LISTINGS: &[(RouteId, &str, RegistryKind)] = &[(
    RouteId::RegistryFilesList,
    "/api/workspace/files",
    RegistryKind::File,
)];

/// The one public point read whose representation includes its value document.
const REGISTRY_POINTS: &[(RouteId, &str)] =
    &[(RouteId::RegistryFilesGet, "/api/workspace/files/notes.md")];

#[tokio::test]
async fn canonical_session_point_read_is_complete_tagged_and_reachable() {
    let mut session = aex_session_domain::testing::session_fixture();
    session.workspace = workspace();
    let expected = aex_session_app::public_session(&session).expect("a complete public session");
    let expected_tag = entity_tag("Session", &expected).expect("a session entity tag");
    let ((router, mounted), _) = build_with_sessions(
        Arc::new(FakeCustody::default()),
        Arc::new(FakeRegistry::default()),
        Arc::new(FakeSessions::default()),
    );
    assert!(mounted.contains(&RouteId::SessionGet));

    let (status, etag, body) = get(&router, &format!("/api/sessions/{}", session.id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(etag.as_deref(), Some(expected_tag.as_str()));
    let projected: models::Session = serde_json::from_value(body).expect("strict wire session");
    assert_eq!(projected, expected);

    let ((deleted_router, _), _) = build_with_sessions(
        Arc::new(FakeCustody::default()),
        Arc::new(FakeRegistry::default()),
        Arc::new(FakeSessions {
            deleted: true,
            ..FakeSessions::default()
        }),
    );
    let (status, _, body) = get(&deleted_router, &format!("/api/sessions/{}", session.id)).await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert_eq!(body["error"]["code"], ErrorCode::SessionDeleted.as_str());
}

#[tokio::test]
async fn session_listing_keeps_one_authenticated_snapshot_across_filtered_pages() {
    let mut first_deleting = aex_session_domain::testing::session_fixture();
    first_deleting.id = sample(81);
    first_deleting.workspace = workspace();
    first_deleting.deletion.session = first_deleting.id;
    first_deleting.status = aex_session_domain::SessionStatus::Deleting;
    first_deleting.created_at = moment("2026-08-01T12:00:00.000Z");

    let mut second_deleting = first_deleting.clone();
    second_deleting.id = sample(82);
    second_deleting.deletion.session = second_deleting.id;
    second_deleting.created_at = moment("2026-08-01T12:01:00.000Z");

    let mut idle = first_deleting.clone();
    idle.id = sample(83);
    idle.deletion.session = idle.id;
    idle.status = aex_session_domain::SessionStatus::Idle;
    idle.created_at = moment("2026-08-01T12:02:00.000Z");

    let sessions = Arc::new(FakeSessions {
        sessions: vec![first_deleting.clone(), second_deleting.clone(), idle],
        ..FakeSessions::default()
    });
    let ((router, mounted), _) = build_with_sessions(
        Arc::new(FakeCustody::default()),
        Arc::new(FakeRegistry::default()),
        Arc::clone(&sessions),
    );
    assert!(mounted.contains(&RouteId::SessionsList));

    let (status, _, first_body) = get(&router, "/api/sessions?status=deleting&limit=1").await;
    assert_eq!(status, StatusCode::OK, "{first_body}");
    let first: models::SessionListPage =
        serde_json::from_value(first_body).expect("a strict first session page");
    assert_eq!(
        first.items,
        vec![aex_session_app::public_session_list_item(&first_deleting)]
    );
    let cursor = first
        .next_cursor
        .expect("the second deleting session remains");

    let (status, _, second_body) = get(
        &router,
        &format!("/api/sessions?status=deleting&limit=1&cursor={cursor}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second_body}");
    let second: models::SessionListPage =
        serde_json::from_value(second_body).expect("a strict second session page");
    assert_eq!(
        second.items,
        vec![aex_session_app::public_session_list_item(&second_deleting)]
    );
    assert!(second.next_cursor.is_none());

    let calls = sessions
        .session_page_calls
        .lock()
        .expect("an uncontended fixture");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0.status, Some(SessionListStatus::Deleting));
    assert_eq!(calls[1].0.status, Some(SessionListStatus::Deleting));
    assert_eq!(
        calls[0].1, calls[1].1,
        "resume must reuse the first snapshot"
    );
    assert!(calls[0].2.is_none());
    assert!(calls[1].2.is_some());
}

#[tokio::test]
async fn message_listing_publishes_only_complete_sealed_messages() {
    let (session, _run, _agent, open) = aex_session_domain::testing::running_session();
    let mut sealed = aex_session_domain::seal(&open, moment("2026-08-01T12:02:00.000Z")).message;
    sealed.parts = vec![aex_session_domain::MessagePart::Text {
        text: "complete".to_owned(),
    }];
    let ((router, mounted), _) = build_with_sessions(
        Arc::new(FakeCustody::default()),
        Arc::new(FakeRegistry::default()),
        Arc::new(FakeSessions {
            messages: vec![sealed.clone()],
            ..FakeSessions::default()
        }),
    );
    assert!(mounted.contains(&RouteId::SessionMessagesList));

    let (status, etag, body) =
        get(&router, &format!("/api/sessions/{}/messages", session.id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(etag.is_none());
    let page: models::MessagePage = serde_json::from_value(body).expect("strict message page");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, sealed.id);
    assert_eq!(
        page.items[0].content,
        vec![models::MessagePart::Text(models::MessagePartText {
            text: "complete".to_owned(),
        })]
    );
}

/// Every point read is mounted, returns its stored tag, and publishes a value.
///
/// This test was its own inverse until the value moved onto the pointer row: it
/// asserted that the point read answered a bare `404`, because the shared
/// projection could only produce a collection row. It is inverted rather than
/// deleted, so the thing it protected — a point response publishing a collection
/// projection as if it were the complete resource — is still under test, from
/// the other side. After the D-6 model split that response is not representable:
/// the point model has no `None` to publish and the row model has no field to
/// fill.
#[tokio::test]
async fn every_registry_point_read_publishes_its_complete_value() {
    for (id, path) in REGISTRY_POINTS {
        let (router, mounted) = composed(FakeCustody::default(), populated_registry());
        assert!(mounted.contains(id), "{id} is mounted");

        let (status, etag, body) = get(&router, path).await;
        assert_eq!(status, StatusCode::OK, "{id}: {body}");
        assert!(
            etag.as_deref()
                .is_some_and(|tag| tag.contains("registry-4")),
            "{id} returns the tag the row stores, not one re-derived on the way out: {etag:?}"
        );
        let value = body
            .get("value")
            .and_then(serde_json::Value::as_object)
            .unwrap_or_else(|| panic!("{id} publishes a value that is present and typed: {body}"));
        assert!(!value.is_empty(), "{id} publishes a non-empty value");
    }
}

/// A point read of a name that is not there is a typed refusal, not a bare one.
#[tokio::test]
async fn a_registry_point_read_of_an_absent_name_is_a_typed_refusal() {
    let (router, _) = composed(FakeCustody::default(), populated_registry());
    let (status, etag, body) = get(&router, "/api/workspace/files/nothing-here").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(etag.is_none(), "an absent resource has no tag");
    assert_eq!(
        body["error"]["code"].as_str(),
        Some("not_found"),
        "the mounted route answers the envelope, never a bare 404: {body}"
    );
}

#[tokio::test]
async fn every_registry_listing_reads_its_own_collection_and_publishes_its_rows() {
    for (id, path, _) in REGISTRY_LISTINGS {
        let (router, _) = composed(FakeCustody::default(), populated_registry());
        let (status, _, body) = get(&router, path).await;
        assert_eq!(status, StatusCode::OK, "{id}");
        let items = body
            .get("items")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("{id} publishes an items array"));
        assert_eq!(items.len(), 1, "{id} reads exactly its own collection");
        let row = &items[0];
        assert_eq!(
            row.get("revision").and_then(serde_json::Value::as_u64),
            Some(4),
            "{id}"
        );
        assert_eq!(
            row.get("state").and_then(serde_json::Value::as_str),
            Some("current"),
            "{id}"
        );
        assert_eq!(
            row.get("sizeBytes").and_then(serde_json::Value::as_str),
            Some("128"),
            "{id} publishes an exact size, never a float"
        );
        assert!(
            row.get("value").is_none(),
            "{id} is a collection row and must omit the value it cannot read"
        );
    }
}

#[tokio::test]
async fn a_registry_listing_asks_the_authority_for_its_own_kind_and_no_other() {
    for (id, path, kind) in REGISTRY_LISTINGS {
        let registry = Arc::new(populated_registry());
        let ((router, _), _) = build(Arc::new(FakeCustody::default()), Arc::clone(&registry));
        let (status, _, _) = get(&router, path).await;
        assert_eq!(status, StatusCode::OK, "{id}");
        assert_eq!(
            registry
                .asked
                .lock()
                .expect("an uncontended fixture")
                .as_slice(),
            &[*kind],
            "{id} must read one collection, and it must be its own"
        );
    }
}

#[tokio::test]
async fn a_registry_continuation_resumes_the_file_collection() {
    let with_more = FakeRegistry {
        next: Some(PagePosition {
            pk: "WS#1".to_owned(),
            sk: "REG#file#notes.md".to_owned(),
            index_pk: None,
            index_sk: None,
        }),
        ..populated_registry()
    };
    let (router, _) = composed(FakeCustody::default(), with_more);
    let (status, _, body) = get(&router, "/api/workspace/files").await;
    assert_eq!(status, StatusCode::OK);
    let cursor = body
        .get("nextCursor")
        .and_then(serde_json::Value::as_str)
        .expect("a page with more names a continuation")
        .to_owned();

    let (status, _, _) = get(&router, &format!("/api/workspace/files?cursor={cursor}")).await;
    assert_eq!(status, StatusCode::OK, "its own collection resumes");
}

#[tokio::test]
async fn an_empty_registry_is_an_empty_page_and_never_a_missing_collection() {
    for (id, path, _) in REGISTRY_LISTINGS {
        let (router, _) = composed(FakeCustody::default(), FakeRegistry::default());
        let (status, _, body) = get(&router, path).await;
        assert_eq!(status, StatusCode::OK, "{id}");
        assert_eq!(
            body.get("items").and_then(serde_json::Value::as_array),
            Some(&Vec::new()),
            "{id}"
        );
        assert!(body.get("nextCursor").is_none(), "{id}");
    }
}

#[tokio::test]
async fn an_unreadable_registry_fails_the_listing_rather_than_publishing_a_short_page() {
    let registry = FakeRegistry {
        pointers: BTreeMap::new(),
        fails: true,
        ..FakeRegistry::default()
    };
    let (router, _) = composed(FakeCustody::default(), registry);
    let (status, _, body) = get(&router, "/api/workspace/files").await;
    // Contention maps onto `conflict`, which this route does not declare, so the
    // dispatch boundary replaces it with `internal_error` rather than letting an
    // undeclared code reach the wire (C-58). What matters here is that a failed
    // authority read is never a successful empty page.
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"]["code"].as_str(), Some("internal_error"));
    assert!(
        route(RouteId::RegistryFilesList)
            .errors
            .iter()
            .all(|code| !code.retryable()),
        "the listing declares no transient code, which is why the refusal is internal"
    );
}

// --- usage -----------------------------------------------------------------

/// A usage projection that answers whatever the case needs.
///
/// `unavailable` is the interesting mode: it is how the "one failed partition
/// fails the whole query" rule is proved without an engine.
#[derive(Debug, Default)]
struct FakeUsage {
    /// Whether the generation pointer has ever been written.
    generation: Option<Generation>,
    /// Whether every read fails the way a throttled table fails.
    unavailable: bool,
}

#[async_trait::async_trait]
impl UsageProjectionReads for FakeUsage {
    async fn current_generation(&self) -> Result<Option<Generation>, QueryError> {
        if self.unavailable {
            return Err(QueryError::Unavailable {
                reason: "ThrottlingException".to_owned(),
            });
        }
        Ok(self.generation)
    }

    async fn coverage(
        &self,
        _generation: Generation,
        _workspace: &aex_usage_domain::wire_pending::WorkspaceId,
        _category: aex_usage_domain::meter::PublicCategory,
    ) -> Result<Option<CoverageRow>, QueryError> {
        Ok(None)
    }

    async fn coverage_eventual(
        &self,
        _generation: Generation,
        _workspace: &aex_usage_domain::wire_pending::WorkspaceId,
        _category: aex_usage_domain::meter::PublicCategory,
    ) -> Result<Option<CoverageRow>, QueryError> {
        Ok(None)
    }

    async fn aggregates(
        &self,
        _request: &AggregateRequest<'_>,
    ) -> Result<AggregatePageRows, QueryError> {
        Ok(AggregatePageRows {
            rows: Vec::new(),
            next: None,
        })
    }

    async fn coarse(&self, _request: &CoarseRequest<'_>) -> Result<CoarsePageRows, QueryError> {
        Ok(CoarsePageRows {
            rows: Vec::new(),
            next: None,
        })
    }
}

fn usage_router(usage: Arc<FakeUsage>) -> axum::Router {
    let shared = Arc::new(Shared {
        catalog: Arc::new(aex_session_app::testing::ScriptedPorts::idle()),
        deployment: aex_session_app::testing::deployment_facts(),
        live_files: Arc::new(NoLiveFiles),
        live_transfers: Arc::new(
            session_stream_api::session::live_transfer::LiveTransferDynamoStore::new(
                offline_dynamodb(),
                SESSION_TABLE,
            ),
        ),
        // The usage cases never reach a workspace read, so the projection here
        // is the empty one: a case that started depending on a workspace row
        // would fail rather than pass against an invented one.
        workspace: Arc::new(FakeWorkspaceProjection::default()) as Arc<dyn WorkspaceProjection>,
        placements: Arc::new(FakePlacements::default()) as Arc<dyn AuthorizationProjection>,
        api_url: aex_wire::types::HttpsUrl::parse("https://eu-west-1.aex.dev")
            .expect("a regional host"),
        custody: Arc::new(FakeCustody::default()) as Arc<dyn SecretCustodyStore>,
        custody_reads: aex_secret_custody_dynamodb::store::CustodyStore::new(
            offline_dynamodb(),
            CUSTODY_TABLE,
        ),
        custody_table: CUSTODY_TABLE.to_owned(),
        plane: aex_secret_domain::context::Plane::Dev,
        region: aex_wire::types::Region::EuWest1,
        registry: Arc::new(FakeRegistry::default()) as Arc<dyn RegistryStore>,
        content: Arc::new(aex_content_dynamodb::store::ContentStore::new(
            offline_dynamodb(),
            "aex-dev-regional-content",
        )),
        content_objects: Arc::new(aex_content_aws::object_store::S3ContentObjects::new(
            offline_s3(),
            aex_content_aws::object_store::BucketBinding {
                bucket: "aex-dev-eu-west-1-content".to_owned(),
                expected_owner: "000000000000".to_owned(),
                kms_key_id: "arn:aws:kms:eu-west-1:000000000000:key/content".to_owned(),
            },
        )),
        receipts: Arc::new(aex_registry_dynamodb::store::RegistryDynamoStore::new(
            offline_dynamodb(),
            "aex-dev-regional-registry",
        )),
        registry_table: "aex-dev-regional-registry".to_owned(),
        work_table: "aex-dev-regional-work".to_owned(),
        content_kms_key_id: "arn:aws:kms:eu-west-1:000000000000:key/content".to_owned(),
        content_encryption_context: Vec::new(),
        registry_entries: 1_000,
        registry_value_bytes: 65_536,
        sessions: Arc::new(FakeSessions::default()) as Arc<dyn SessionQueries>,
        operations: Arc::new(FakeOperations::default()) as Arc<dyn OperationApiStore>,
        commands: aex_session_dynamodb::app_authority::SessionCommandReads::new(
            offline_dynamodb(),
            SESSION_TABLE,
        ),
        tables: aex_session_dynamodb::plan::RegionalTables::composed("dev", "eu-west-1"),
        authority: offline_dynamodb(),
        usage: usage as Arc<dyn UsageProjectionReads>,
        cursor_keys: Arc::new(cursor_keys()),
        runtime_activity: aex_runtime_activity_dynamodb::store::RuntimeActivityDynamoStore::new(
            offline_dynamodb(),
            RUNTIME_ACTIVITY_TABLE,
        ),
    });
    mount_unary(
        Arc::new(Dispatcher::new(shared)),
        Arc::new(Admit),
        aex_wire::dispatch::RequestLimits::DEFAULT,
    )
    .expect("the served set mounts")
    .router
}

async fn usage_query(usage: Arc<FakeUsage>, body: serde_json::Value) -> (u16, serde_json::Value) {
    let response = usage_router(usage)
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/billing/usage/query")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("a body");
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn usage_body(limit: Option<u32>, bucket: &str, gte: &str, lt: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "bucket": bucket,
        "timeRange": { "gte": gte, "lt": lt }
    });
    if let Some(limit) = limit {
        body["limit"] = serde_json::json!(limit);
    }
    body
}

#[tokio::test]
async fn a_usage_page_above_the_item_ceiling_is_refused_rather_than_clamped() {
    // A caller silently given fewer items than it asked for cannot tell a short
    // page from the end of a collection, and this is a money read.
    let (status, body) = usage_query(
        Arc::new(FakeUsage::default()),
        usage_body(
            Some(101),
            "day",
            "2026-08-01T00:00:00.000Z",
            "2026-08-02T00:00:00.000Z",
        ),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_request");
    assert_eq!(body["error"]["message"], "page limit");

    let (status, _) = usage_query(
        Arc::new(FakeUsage {
            generation: Some(Generation::FIRST),
            unavailable: false,
        }),
        usage_body(
            Some(100),
            "day",
            "2026-08-01T00:00:00.000Z",
            "2026-08-02T00:00:00.000Z",
        ),
    )
    .await;
    assert_eq!(status, 200, "the ceiling itself is admissible");
}

#[tokio::test]
async fn a_misaligned_or_over_wide_usage_range_is_refused_with_invalid_query() {
    let cases = [
        // A part-day at day grain: clamping would report a wider interval than
        // was asked for.
        (
            "day",
            "2026-08-01T06:00:00.000Z",
            "2026-08-02T00:00:00.000Z",
        ),
        // Over the 400-day span cap.
        (
            "day",
            "2025-01-01T00:00:00.000Z",
            "2026-08-01T00:00:00.000Z",
        ),
        // An inverted range answers nothing rather than everything.
        (
            "day",
            "2026-08-02T00:00:00.000Z",
            "2026-08-01T00:00:00.000Z",
        ),
    ];
    for (bucket, gte, lt) in cases {
        let (status, body) = usage_query(
            Arc::new(FakeUsage {
                generation: Some(Generation::FIRST),
                unavailable: false,
            }),
            usage_body(None, bucket, gte, lt),
        )
        .await;
        assert_eq!(status, 400, "{gte}..{lt}: {body}");
        assert_eq!(body["error"]["code"], "invalid_query", "{gte}..{lt}");
    }
}

#[tokio::test]
async fn a_failed_projection_read_fails_the_whole_usage_query() {
    // No partial page and no per-category degradation: a page missing a
    // category is a bill missing a line.
    let (status, body) = usage_query(
        Arc::new(FakeUsage {
            generation: None,
            unavailable: true,
        }),
        usage_body(
            None,
            "day",
            "2026-08-01T00:00:00.000Z",
            "2026-08-02T00:00:00.000Z",
        ),
    )
    .await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["code"], "usage_unavailable");
}

#[tokio::test]
async fn a_usage_projection_that_was_never_cut_over_says_so_rather_than_answering_zero() {
    // An absent generation pointer is not a synonym for the first generation.
    // Answering an empty page would be a confidently wrong bill of zero.
    let (status, body) = usage_query(
        Arc::new(FakeUsage::default()),
        usage_body(
            None,
            "day",
            "2026-08-01T00:00:00.000Z",
            "2026-08-02T00:00:00.000Z",
        ),
    )
    .await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["code"], "usage_unavailable");
}

#[tokio::test]
async fn an_over_budget_usage_total_is_refused_rather_than_paged() {
    // A partial total is a wrong number rather than a short answer, so there is
    // no cursor that could continue it.
    let (status, body) = usage_query(
        Arc::new(FakeUsage {
            generation: Some(Generation::FIRST),
            unavailable: false,
        }),
        usage_body(
            None,
            "total",
            "2026-01-01T06:00:00.000Z",
            "2026-06-01T06:00:00.000Z",
        ),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid_query");
}

// --- the three workspace reads ---------------------------------------------------

/// The router, over a workspace projection holding exactly what a case needs.
fn workspace_router(
    projection: FakeWorkspaceProjection,
    placements: FakePlacements,
) -> axum::Router {
    build_with_workspace(
        Arc::new(FakeCustody::default()),
        Arc::new(FakeRegistry::default()),
        Arc::new(FakeSessions::default()),
        Arc::new(FakeOperations::default()),
        Arc::new(projection),
        Arc::new(placements),
    )
    .0
    .0
}

/// A projection holding the complete registry, as every bootstrapped workspace
/// does by construction.
fn complete_projection() -> FakeWorkspaceProjection {
    FakeWorkspaceProjection {
        profile: Some(stored_profile(None)),
        limits: aex_wire::limits::LimitId::ALL
            .iter()
            .copied()
            .map(|id| (id, stored_limit(id)))
            .collect(),
        bundle: Some(stored_bundle(aex_wire::limits::LimitId::ALL.len())),
    }
}

/// A short set is never a page.
///
/// This is the failure the cluster exists to remove. The registry is closed and
/// every workspace set is complete by construction, so eleven of twelve limits
/// is not "the first page" and not "the limits this workspace has" — it is a
/// workspace whose materialisation did not finish. A `200` carrying it would be
/// read by every client as the complete answer, and whatever enforces the
/// missing limit would then be deciding what an absent row means, which is the
/// decision eager materialisation exists to make unnecessary.
#[tokio::test]
async fn a_partial_limit_set_is_an_activation_refusal_and_never_a_short_page() {
    let full = aex_wire::limits::LimitId::ALL.len();
    for short in [0, 1, full - 1] {
        let router = workspace_router(
            FakeWorkspaceProjection {
                bundle: Some(stored_bundle(short)),
                ..FakeWorkspaceProjection::default()
            },
            FakePlacements::default(),
        );
        let (status, _, body) = get(&router, "/api/workspace/limits").await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "a {short}-of-{full} set answered {status}: {body}"
        );
        assert_eq!(
            body["error"]["code"], "workspace_activation_required",
            "{body}"
        );
        assert_eq!(
            body["error"]["retryable"], true,
            "activation clears, so a client must not give up on it: {body}"
        );
    }

    // The complete set is a `200` carrying every registered limit, in registry
    // order, with no continuation.
    let router = workspace_router(complete_projection(), FakePlacements::default());
    let (status, _, body) = get(&router, "/api/workspace/limits").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["items"].as_array().map(Vec::len),
        Some(full),
        "the published page is the whole registry: {body}"
    );
    assert!(
        body.get("nextCursor").is_none(),
        "a closed registry-sized collection never mints a continuation: {body}"
    );
    let ids: Vec<&str> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["id"].as_str().expect("a limit id"))
        .collect();
    let expected: Vec<&str> = aex_wire::limits::LimitId::ALL
        .iter()
        .map(|id| id.as_str())
        .collect();
    assert_eq!(ids, expected, "the page is not in registry order");
}

/// An absent bundle is the same activation refusal, not an empty page.
#[tokio::test]
async fn an_unmaterialised_workspace_is_told_so_rather_than_shown_no_limits() {
    let router = workspace_router(
        FakeWorkspaceProjection::default(),
        FakePlacements::default(),
    );
    let (status, _, body) = get(&router, "/api/workspace/limits").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["error"]["code"], "workspace_activation_required",
        "{body}"
    );
}

/// `not_found` on the point read has exactly one meaning, decided before any
/// read: the path segment names no registered limit.
///
/// The other case — a registered limit whose row is absent — must never be a
/// `404`. The two are indistinguishable to a client, and one of them claims the
/// limit does not exist, which is a lie about a published contract.
#[tokio::test]
async fn an_unregistered_limit_id_is_not_found_and_an_absent_row_never_is() {
    let router = workspace_router(complete_projection(), FakePlacements::default());
    let (status, _, body) = get(&router, "/api/workspace/limits/session.subagent_depth").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found", "{body}");

    let registered = aex_wire::limits::LimitId::ALL[0];
    let (status, _, body) = get(
        &router,
        &format!("/api/workspace/limits/{}", registered.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], registered.as_str(), "{body}");
    assert_eq!(body["source"], "default", "{body}");

    // The same registered id, against a workspace whose set was never
    // materialised. Retryable activation, never `404`.
    let empty = workspace_router(
        FakeWorkspaceProjection::default(),
        FakePlacements::default(),
    );
    let (status, _, body) = get(
        &empty,
        &format!("/api/workspace/limits/{}", registered.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["error"]["code"], "workspace_activation_required",
        "{body}"
    );
}

/// The workspace read publishes the account state from the profile row, and
/// reports a running deletion separately from it.
#[tokio::test]
async fn the_current_workspace_carries_the_account_state_and_its_own_status() {
    let router = workspace_router(
        complete_projection(),
        FakePlacements {
            placement: Some(stored_placement("active")),
        },
    );
    let (status, _, body) = get(&router, "/api/workspace").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "active", "{body}");
    assert_eq!(body["apiUrl"], "https://eu-west-1.aex.dev", "{body}");
    assert_eq!(body["slug"], "fixture", "{body}");
    assert_eq!(
        body["operationalState"]["inheritedFrom"], "account",
        "{body}"
    );
    assert_eq!(
        body["operationalState"]["state"]["status"], "active",
        "{body}"
    );
    assert_eq!(body["operationalState"]["state"]["revision"], 7, "{body}");

    // A paused account under a dispute hold: the reason is its own, and no
    // restoring amount is named, because paying restores nothing here.
    let router = workspace_router(
        FakeWorkspaceProjection {
            profile: Some(stored_profile(Some("dispute_hold"))),
            ..complete_projection()
        },
        FakePlacements {
            placement: Some(stored_placement("paused")),
        },
    );
    let (status, _, body) = get(&router, "/api/workspace").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["status"], "active",
        "a paused account is not a deleting workspace: {body}"
    );
    let state = &body["operationalState"]["state"];
    assert_eq!(state["status"], "paused", "{body}");
    assert_eq!(state["reason"], "dispute_hold", "{body}");
    assert!(
        state.get("minimumRestoreCents").is_none(),
        "a dispute hold has no paying remedy, so naming an amount would be a false one: {body}"
    );

    // The one hold a top-up clears names the flat $20.
    let router = workspace_router(
        FakeWorkspaceProjection {
            profile: Some(stored_profile(Some("top_up_required"))),
            ..complete_projection()
        },
        FakePlacements {
            placement: Some(stored_placement("paused")),
        },
    );
    let (_, _, body) = get(&router, "/api/workspace").await;
    assert_eq!(
        body["operationalState"]["state"]["minimumRestoreCents"], "2000",
        "{body}"
    );

    // A running deletion is the workspace status, published beside the account
    // state rather than folded into it. The admission path collapses `deleting`
    // onto paused because that is the right answer for admission; publishing
    // that collapse would lose the distinction entirely.
    let router = workspace_router(
        complete_projection(),
        FakePlacements {
            placement: Some(stored_placement("deleting")),
        },
    );
    let (status, _, body) = get(&router, "/api/workspace").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "deleting", "{body}");
    assert_eq!(
        body["operationalState"]["state"]["status"], "active",
        "{body}"
    );
}

/// A workspace whose descriptive rows are not there yet says so, and never
/// answers `401` at a caller whose credential the edge just verified.
#[tokio::test]
async fn a_cold_read_of_an_unactivated_workspace_is_not_an_unknown_credential() {
    for (projection, placements) in [
        // No placement row.
        (complete_projection(), FakePlacements::default()),
        // A placement, no profile.
        (
            FakeWorkspaceProjection {
                profile: None,
                ..complete_projection()
            },
            FakePlacements {
                placement: Some(stored_placement("active")),
            },
        ),
    ] {
        let router = workspace_router(projection, placements);
        let (status, _, body) = get(&router, "/api/workspace").await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "a verified credential was told it was unknown: {body}"
        );
        assert_eq!(
            body["error"]["code"], "workspace_activation_required",
            "{body}"
        );
    }

    // A reason outside the durable vocabulary is a corrupt projected row. It is
    // still not an answer, so it is still not `Active` — the region says it
    // could not establish the state, which is the code this route is the first
    // regional one to declare.
    let router = workspace_router(
        FakeWorkspaceProjection {
            profile: Some(stored_profile(Some("credit_exhausted"))),
            ..complete_projection()
        },
        FakePlacements {
            placement: Some(stored_placement("paused")),
        },
    );
    let (status, _, body) = get(&router, "/api/workspace").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "account_state_unavailable", "{body}");
}

/// The region publishes exactly what the shared mapping produces — no regional
/// post-processing, no second vocabulary.
///
/// This is one half of the cross-plane identity. The other half lives beside
/// the central handler, and asserts the same equality there. Together they say
/// that one `AccountProfile` yields one JSON document on either plane, which is
/// the property that stops a workspace being told it is active by its region
/// while the dashboard calls it paused.
///
/// The two answers may differ in *staleness* — the region reads a projection —
/// but never in vocabulary, discriminator or derivation.
#[tokio::test]
async fn the_region_publishes_byte_identical_json_to_the_shared_mapping() {
    for reason in [
        None,
        Some("top_up_required"),
        Some("payment_hold"),
        Some("dispute_hold"),
        Some("account_closed"),
    ] {
        // The same facts the projected profile row carries, handed straight to
        // the one mapping in `aex-control-domain`.
        let expected =
            aex_control_domain::account_operational_state(&aex_control_domain::AccountProfile {
                state: if reason.is_some() {
                    aex_control_domain::AccountState::PausedTopUpRequired
                } else {
                    aex_control_domain::AccountState::Active
                },
                reason: reason.map(str::to_owned),
                revision: 7,
                changed_at: moment("2026-08-02T00:00:00.000Z").to_datetime(),
            })
            .expect("the shared mapping projects every declared cause");
        let expected = serde_json::to_string(&expected).expect("the mapping's own encoding");

        let router = workspace_router(
            FakeWorkspaceProjection {
                profile: Some(stored_profile(reason)),
                ..complete_projection()
            },
            FakePlacements {
                placement: Some(stored_placement(if reason.is_some() {
                    "paused"
                } else {
                    "active"
                })),
            },
        );
        let (status, _, body) = get(&router, "/api/workspace").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let published = serde_json::to_string(&body["operationalState"]["state"])
            .expect("the published state re-encodes");
        assert_eq!(
            published, expected,
            "the region derived its own answer for {reason:?}"
        );
    }
}
