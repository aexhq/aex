//! The generated server traits, implemented over the real adapters.
//!
//! Two rules shape everything here.
//!
//! **A handler never names a status and never invents a code.** The response
//! type it returns is the status the route declares, and every refusal is one of
//! the codes the route's descriptor lists — `dispatch::declared` refuses the rest
//! at the boundary, so a code that slipped through would be `internal_error`
//! rather than a lie.
//!
//! **A route this deployable does not own is [`not_served`].** Two authoring
//! fragments are split across two deployables, so implementing a trait means
//! implementing methods for the other half too. Those arms are unreachable
//! through the router — [`Routes::served`] never offers them — and the
//! composition test proves it.

use std::sync::Arc;

use aex_content_aws::object_store::ContentObjectStore;
use aex_content_domain::identity::RegistryKind;
use aex_content_dynamodb::store::ContentMetadataStore;
use aex_operation_domain::operation::{OperationKind, OperationStatus};
use aex_regional_http::context::RequestContext;
use aex_regional_http::cursor::{
    CursorBinding, CursorKeyRing, CursorRequestBinding, Order, SnapshotToken, SortTuple,
};
use aex_regional_http::mount::{UnaryDispatch, not_served};
use aex_regional_http::projection::{
    self, ProjectionError, authority_failure, entity_tag, position_tuple, tuple_position,
};
use aex_regional_http::router::RouteOwner;
use aex_registry_dynamodb::store::{PointerPage, RegistryStore};
use aex_runtime_activity_dynamodb::store::RuntimeActivityDynamoStore;
use aex_secret_custody_dynamodb::ProviderCredentialReads;
use aex_secret_custody_dynamodb::codec::{CredentialState, ProviderCredential as StoredCredential};
use aex_secret_custody_dynamodb::expressions;
use aex_secret_custody_dynamodb::store::CustodyStore;
use aex_secret_custody_dynamodb::store::SecretCustodyStore;
use aex_session_app::SessionReader as _;
use aex_session_app::{
    LifecycleAdmissionOutcome, LifecycleCommand, MessageAdmissionOutcome, SendMessage,
    admit_lifecycle_operation, admit_message, message_receipt_scope, replay_message_receipt,
};
use aex_session_dynamodb::app_authority::{
    ApiHintSink, AuthorizedAccount, DynamoAuthorityCommitter, RequestClock,
    SessionAuthorityExternal, SessionCommandReads,
};
use aex_session_dynamodb::application_plan::{FamilyCompilers, SessionBinding};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::{PageBudget, PagePosition};
use aex_session_dynamodb::plan::{Participant, RegionalTables, TransactionPlan};
use aex_session_dynamodb::projection::{AuthorizationProjection, WorkspaceProjection};
use aex_session_dynamodb::store::{
    OperationApiStore, OperationCancelOutcome, OperationFilter, SessionListFilter,
    SessionListStatus, SessionQueries, SessionScoped,
};
use aex_session_dynamodb::wire_pending::StoredOperation;
use aex_usage_query_dynamodb::store::UsageProjectionReads;
use aex_wire::cursor::Cursor;
use aex_wire::dispatch::{RawRequest, RawResponse, RequestLimits};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::{IntentDigest, ReplayIdentity};
use aex_wire::ids::{OperationId, ProviderCredentialId, ResourceName, SessionId, WorkspaceId};
use aex_wire::limits::LimitId;
use aex_wire::models;
use aex_wire::routes::{RouteId, route};
use aex_wire::server::{
    AcceptKind, Accepted, Created, NoContent, ProviderCredentialsApi, RegionalOperationsApi,
    RegistryApi, RequestContext as WireContext, RouteGroup, SessionsApi, WithETag, WorkspaceApi,
    dispatch_files, dispatch_provider_credentials, dispatch_regional_operations, dispatch_registry,
    dispatch_sessions, dispatch_usage, dispatch_workspace,
};
use aex_wire::types::Timestamp;
use aex_work_dynamodb::WorkApplicationCompiler;

/// The adapters and start-up bindings every request shares.
///
/// One value built once by the composition root. Nothing per-request lives here,
/// which is what lets a handler be constructed for one request by cloning two
/// `Arc`s.
pub struct Shared {
    /// Build-bound, signature-verified model admission authority.
    pub catalog: Arc<dyn aex_session_app::ModelQualifier>,
    /// Release-derived Hands image and plane capability facts.
    pub deployment: aex_session_app::DeploymentFacts,
    /// Exact-generation authenticated guest transport for ephemeral live files.
    pub live_files: Arc<dyn aex_brain_hands::LiveFileBackend>,
    /// Durable transfer election, ownership and replay authority.
    pub live_transfers: Arc<dyn crate::session::live_transfer::LiveTransferStore>,
    /// The ciphertext-metadata authority.
    ///
    /// Metadata only: this deployable holds no decrypt key, so the one thing it
    /// writes here is a revocation fence.
    pub custody: Arc<dyn SecretCustodyStore>,
    /// The concrete custody store, for the two rows a custody read joins.
    ///
    /// Separate from `custody` above, which is the `dyn` metadata authority the
    /// served secret routes use: reconstructing a `SessionCustody` needs the
    /// binding listing, and that is an inherent method on the concrete store.
    pub custody_reads: CustodyStore,
    /// The physical `regional-secret-custody` table name.
    ///
    /// Carried because a conditional expression names its own table, and the
    /// physical name is composed by the infrastructure stream rather than
    /// guessed here.
    pub custody_table: String,
    /// The only plaintext-bearing capability in this process.
    ///
    /// Kept behind its narrow port so the other session handlers cannot reach
    /// the secret key, branch-key store, or plaintext admission operation.
    pub credential_registration:
        Arc<dyn crate::session::secret_registration::ProviderCredentialRegistration>,
    /// The named-registry authority.
    pub registry: Arc<dyn RegistryStore>,
    /// The content-descriptor authority, for the payload a registered value
    /// names.
    pub content: Arc<dyn ContentMetadataStore>,
    /// The one content object adapter, and therefore the one presigner.
    ///
    /// The upload routes, the registry mutation path and the registry download
    /// route reach S3 through this and nothing else builds a second one.
    /// Registry payloads are always object-placed (D-11).
    pub content_objects: Arc<dyn ContentObjectStore>,
    /// The `regional-registry` receipt reader the replay combinator uses.
    pub receipts: Arc<dyn aex_session_dynamodb::replay::ReceiptStore>,
    /// The physical `regional-registry` table name.
    pub registry_table: String,
    /// The physical `regional-work` table name.
    pub work_table: String,
    /// The content CMK every object is sealed under, recorded on a descriptor.
    pub content_kms_key_id: String,
    /// The canonical encryption context every content object is bound to.
    pub content_encryption_context: Vec<u8>,
    /// Effective `registry.entries` per `(workspace, kind)`.
    ///
    /// The compiled default until the effective-limits authority lands; it
    /// overrides here, at the one check point.
    pub registry_entries: u64,
    /// Effective `registry.value_bytes`.
    pub registry_value_bytes: u64,
    /// The strongly consistent, read-only session-authority surface.
    pub sessions: Arc<dyn SessionQueries>,
    /// The cold workspace-description surface: profile and effective limits.
    ///
    /// Deliberately a separate port from the placement reader below. A display
    /// name or a limit read must not widen the capability the admission path
    /// uses on every request.
    pub workspace: Arc<dyn WorkspaceProjection>,
    /// The placement reader, for the one workspace fact the request context
    /// does not carry: whether a deletion is running.
    pub placements: Arc<dyn AuthorizationProjection>,
    /// The regional host this deployable answers on.
    ///
    /// One value per plane and region, resolved at start-up. It is not a
    /// per-workspace attribute: a projected one could disagree with the host
    /// that served the request, and changing a display URL would become a
    /// fleet-wide row rewrite.
    pub api_url: aex_wire::types::HttpsUrl,
    /// The durable-operation point, list and conditional cancellation authority.
    pub operations: Arc<dyn OperationApiStore>,
    /// Asynchronous continuation boundary for committed lifecycle operations.
    ///
    /// The API holds only Lambda invoke authority. Runtime/provider calls and
    /// deletion fan-out remain isolated in `session-operation-worker`.
    pub operation_worker: Arc<dyn crate::session::operation_worker::OperationWorkerInvoker>,
    /// The eventually consistent command-path view of the session authority.
    ///
    /// Separate from `sessions` on purpose: the read routes above are strongly
    /// consistent, and a command re-asserts every value it read as a condition,
    /// so it pays neither the capacity nor the latency of a strong read (D-11).
    pub commands: SessionCommandReads,
    /// The physical regional table names a command transaction compiles against.
    pub tables: RegionalTables,
    /// The client the committer submits its one transaction on.
    pub authority: aws_sdk_dynamodb::Client,
    /// The read-only usage projection.
    ///
    /// A port with no method that accepts a row, so this deployable cannot be
    /// given write authority over the billing projection by mistake.
    pub usage: Arc<dyn UsageProjectionReads>,
    /// The signing ring every continuation is minted and verified under.
    pub cursor_keys: Arc<CursorKeyRing>,
    /// The runtime-activity authority, which owns workspace continuity and the
    /// true-idle verdict.
    ///
    /// Read through `RuntimeContinuity` rather than interpreted here: the idle
    /// window and the lifecycle states belong to `aex-runtime-control`, and a
    /// second reading of them would let two authorities disagree about whether
    /// a session is idle.
    pub runtime_activity: RuntimeActivityDynamoStore,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Shared").finish_non_exhaustive()
    }
}

/// One request's view of the deployable.
///
/// The regional context is carried because the wire context does not name a
/// workspace for an `Account` principal, and every authority read here is
/// workspace-scoped. Building it per request costs two `Arc` clones.
pub struct Routes {
    pub(super) shared: Arc<Shared>,
    pub(super) cx: RequestContext,
}

impl Routes {
    /// Binds the shared adapters to one verified request.
    #[must_use]
    pub const fn new(shared: Arc<Shared>, cx: RequestContext) -> Self {
        Self { shared, cx }
    }

    /// The adapters this request may reach.
    #[must_use]
    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    /// The verified request this handler is answering.
    #[must_use]
    pub const fn context(&self) -> &RequestContext {
        &self.cx
    }

    /// Every route whose handler is complete, in `RouteId` order.
    ///
    /// This is the narrowing RS-22 permits and RS-18 requires, and it really is
    /// derived: the owned unary partition minus whatever the deferral ledger
    /// defers, with no list of route names anywhere in this crate. Landing a
    /// route is two lines in `routes-meta.yaml` plus the handler itself.
    /// Everything the ledger defers is mounted as the generated refusal arm and
    /// answers `501 not_implemented`.
    #[must_use]
    pub fn served() -> Vec<RouteId> {
        RouteOwner::SessionApi
            .routes()
            .into_iter()
            .filter(|id| !route(*id).deferred)
            .collect()
    }
}

impl std::fmt::Debug for Routes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Routes")
            .field("route", &self.cx.route)
            .finish_non_exhaustive()
    }
}

/// The page budget a list request asks for.
///
/// A limit above the ceiling is refused rather than clamped: a caller that is
/// silently given fewer items than it asked for cannot tell a short page from
/// the end of a collection.
fn budget(limit: Option<u32>) -> WireResult<PageBudget> {
    PageBudget::new(limit.unwrap_or(0))
        .map_err(|_| WireError::new(ErrorCode::InvalidRequest).with_message("page limit"))
}

impl Routes {
    fn cursor_binding(&self, route: RouteId, snapshot: &str) -> WireResult<CursorBinding> {
        self.cursor_binding_for_session(route, snapshot, None)
    }

    fn cursor_binding_for_session(
        &self,
        route: RouteId,
        snapshot: &str,
        session_id: Option<SessionId>,
    ) -> WireResult<CursorBinding> {
        self.cursor_binding_for_query(route, snapshot, session_id, [0; 32])
    }

    fn cursor_binding_for_session_epoch(
        &self,
        route: RouteId,
        collection: &str,
        session_id: SessionId,
        epoch: aex_operation_domain::DeletionEpoch,
    ) -> WireResult<CursorBinding> {
        let snapshot = format!("{collection}:{session_id}:deletion-epoch:{}", epoch.0);
        self.cursor_binding_for_session(route, &snapshot, Some(session_id))
    }

    async fn resume_session_collection(
        &self,
        route: RouteId,
        collection: &str,
        session_id: SessionId,
        cursor: Option<&Cursor>,
    ) -> WireResult<(
        Option<PagePosition>,
        Option<aex_operation_domain::DeletionEpoch>,
    )> {
        let Some(cursor) = cursor else {
            return Ok((None, None));
        };
        let parent = match self
            .shared
            .sessions
            .load_session(self.cx.auth.workspace_id, session_id)
            .await
            .map_err(|error| authority_failure(&error))?
        {
            SessionScoped::Missing => return Err(WireError::new(ErrorCode::NotFound)),
            SessionScoped::Deleted => return Err(WireError::new(ErrorCode::SessionDeleted)),
            SessionScoped::Active(parent) => parent,
        };
        let binding = self.cursor_binding_for_session_epoch(
            route,
            collection,
            session_id,
            parent.deletion.epoch,
        )?;
        Ok((
            self.resume(Some(cursor), &binding)?,
            Some(parent.deletion.epoch),
        ))
    }

    fn cursor_binding_for_query(
        &self,
        route: RouteId,
        snapshot: &str,
        session_id: Option<SessionId>,
        query_hash: [u8; 32],
    ) -> WireResult<CursorBinding> {
        Ok(CursorBinding {
            route,
            principal_scope: self.cx.auth.credential_binding,
            region: self.cx.auth.placement,
            workspace_id: self.cx.auth.workspace_id,
            session_id,
            query_hash,
            order: Order::Ascending,
            snapshot: SnapshotToken::new(snapshot)
                .map_err(|error| WireError::from(ProjectionError::Cursor(error)))?,
        })
    }

    fn resume(
        &self,
        cursor: Option<&Cursor>,
        binding: &CursorBinding,
    ) -> WireResult<Option<PagePosition>> {
        let Some(cursor) = cursor else {
            return Ok(None);
        };
        let tuple = aex_regional_http::cursor::decode(
            &self.shared.cursor_keys,
            cursor,
            binding,
            self.now()?,
        )
        .map_err(|error| WireError::from(ProjectionError::Cursor(error)))?;
        Ok(Some(tuple_position(&tuple).map_err(WireError::from)?))
    }

    fn continuation(
        &self,
        next: Option<&PagePosition>,
        binding: &CursorBinding,
    ) -> WireResult<Option<Cursor>> {
        let Some(position) = next else {
            return Ok(None);
        };
        let tuple: SortTuple = position_tuple(position).map_err(WireError::from)?;
        let cursor = aex_regional_http::cursor::encode(
            self.shared.cursor_keys.current(),
            binding,
            &tuple,
            self.now()?,
        )
        .map_err(|error| WireError::from(ProjectionError::Cursor(error)))?;
        Ok(Some(cursor))
    }

    pub(super) fn now(&self) -> WireResult<Timestamp> {
        self.cx
            .now()
            .map_err(|_| WireError::new(ErrorCode::InternalError))
    }

    /// The command envelope for a session lifecycle mutation.
    ///
    /// These routes are keyed by `Aex-Operation-Id` rather than by an
    /// ordinary idempotency key, so the operation identity is caller-minted and
    /// the edge has already parsed it. An absent one is an invalid request, not
    /// a server-minted identity: a server-minted identity could never be
    /// replayed, which is the whole point of the header.
    fn lifecycle_command(
        &self,
        session_id: SessionId,
        route: RouteId,
        kind: OperationKind,
    ) -> WireResult<LifecycleCommand> {
        let operation = self.cx.operation_id.ok_or_else(|| {
            WireError::new(ErrorCode::InvalidRequest).with_message("operation id")
        })?;
        Ok(LifecycleCommand {
            workspace: self.cx.auth.workspace_id,
            session: session_id,
            operation,
            intent: intent_of(route, self.cx.auth.workspace_id, session_id),
            kind,
        })
    }

    /// The six ports this deployable supplies, plus the five it refuses.
    pub(super) fn bindings(&self) -> WireResult<CommandBindings> {
        let now = self.now()?;
        Ok(CommandBindings {
            clock: RequestClock(now),
            ids: crate::session::app_ports::RequestIds,
            unowned: crate::session::app_ports::UnownedPorts,
            registry: crate::session::app_ports::RegistryReads::new(Arc::clone(
                &self.shared.registry,
            )),
            limits: crate::session::app_ports::LimitReads::new(Arc::clone(&self.shared.workspace)),
            reads: self.shared.commands.clone(),
            credentials: ProviderCredentialReads::new(
                self.shared.custody_reads.clone(),
                self.cx.auth.workspace_id,
            ),
            accounts: AuthorizedAccount {
                organization: self.cx.auth.organization_id,
                revision: self.cx.auth.epochs.account,
                paused: self.cx.auth.account_state
                    == aex_regional_http::context::AccountState::Paused,
                observed_at: now,
            },
            catalog: Arc::clone(&self.shared.catalog),
            deployment: self.shared.deployment.clone(),
        })
    }

    fn message_command(
        &self,
        session: SessionId,
        request: models::MessageSendRequest,
    ) -> WireResult<SendMessage> {
        let replay = self.cx.idempotency.as_ref().ok_or_else(|| {
            WireError::new(ErrorCode::InvalidRequest)
                .with_message("this route requires an `Idempotency-Key`")
        })?;
        let intent = IntentDigest::from_bytes(replay.intent);
        Ok(SendMessage {
            workspace: self.cx.auth.workspace_id,
            session,
            identity: aex_session_domain::IdempotencyIdentity::Key(Box::new(ReplayIdentity {
                principal: self.cx.auth.principal,
                route: RouteId::SessionMessageSend,
                key: replay.key.clone(),
                intent,
            })),
            request,
        })
    }

    async fn admit_message_route(
        &self,
        session: SessionId,
        request: models::MessageSendRequest,
    ) -> WireResult<models::MessageSendResult> {
        let command = self.message_command(session, request)?;
        let bindings = self.bindings()?;
        let outcome = admit_message(&bindings.context(), &command)
            .await
            .map_err(|error| app_failure(&error))?;
        match outcome {
            MessageAdmissionOutcome::Replayed { response, .. } => Ok(*response),
            MessageAdmissionOutcome::Planned(planned) => {
                let response = models::MessageSendResult {
                    message: aex_session_app::projection::public_message(&planned.projected.0)
                        .map_err(|_| WireError::new(ErrorCode::InternalError))?,
                    session: aex_session_app::public_session(&planned.projected.1)
                        .map_err(|_| WireError::new(ErrorCode::InternalError))?,
                };
                self.commit_message(&command, &planned.plan, response).await
            }
        }
    }

    async fn commit_message(
        &self,
        command: &SendMessage,
        plan: &aex_session_app::SessionTransaction,
        attempted: models::MessageSendResult,
    ) -> WireResult<models::MessageSendResult> {
        let binding = SessionBinding {
            workspace: command.workspace,
            organization: self.cx.auth.organization_id,
            session: command.session,
        };
        let committer = DynamoAuthorityCommitter::new(
            self.shared.authority.clone(),
            self.shared.tables.clone(),
            binding,
            self.now()?,
            ApiHintSink,
        );
        let work = WorkApplicationCompiler;
        let credentials = aex_secret_custody_dynamodb::ProviderCredentialAdmissionCompiler;
        let compilers = FamilyCompilers::new()
            .with(aex_session_app::TableFamily::SecretCustody, &credentials)
            .with(aex_session_app::TableFamily::WorkAuthority, &work);
        match committer
            .commit_replayable_resolving(
                plan,
                &compilers,
                aex_session_dynamodb::Resolution::IdempotencyReceipt,
            )
            .await
        {
            Ok(()) => Ok(attempted),
            Err(error) => self.recover_message(&error, command).await,
        }
    }

    async fn recover_message(
        &self,
        failure: &StoreError,
        command: &SendMessage,
    ) -> WireResult<models::MessageSendResult> {
        if !matches!(
            failure,
            StoreError::PreconditionFailed { .. } | StoreError::CommitAmbiguous { .. }
        ) {
            return Err(authority_failure(failure));
        }

        let scope = message_receipt_scope(command.session);
        let receipt = self
            .shared
            .commands
            .load_receipt(command.workspace, &scope, &command.identity, self.now()?)
            .await
            .map_err(|error| app_failure(&aex_session_app::AppError::Port(error)))?;
        if let Some(receipt) = receipt {
            return match replay_message_receipt(&receipt, command)
                .map_err(|error| app_failure(&error))?
            {
                MessageAdmissionOutcome::Replayed { response, .. } => Ok(*response),
                MessageAdmissionOutcome::Planned(_) => {
                    Err(WireError::new(ErrorCode::InternalError))
                }
            };
        }

        let StoreError::PreconditionFailed { participant, .. } = failure else {
            return Err(WireError::new(ErrorCode::CommitOutcomeUnknown));
        };
        Err(
            if *participant == Participant::CUSTODY_PROVIDER_CREDENTIAL {
                WireError::new(ErrorCode::ProviderCredentialRevoked)
            } else if *participant == Participant::AUTHZ_PLACEMENT {
                WireError::new(ErrorCode::AccountPaused)
            } else if matches!(
                *participant,
                Participant::SESSION_HEAD
                    | Participant::AGENT_ROOT_CONTROL
                    | Participant::WORK_ROOT_WAKE
                    | Participant::WORK_DEDUPE
            ) {
                WireError::new(ErrorCode::SessionNotIdle)
            } else if *participant == Participant::SESSION_IDEMPOTENCY {
                // A strongly addressed receipt should exist after losing its
                // immutable-put election. If it does not, the transaction outcome
                // is not safe to describe as a definite customer conflict.
                WireError::new(ErrorCode::CommitOutcomeUnknown)
            } else {
                WireError::new(ErrorCode::InternalError)
            },
        )
    }

    async fn admit_lifecycle_route(
        &self,
        session_id: SessionId,
        route: RouteId,
        kind: OperationKind,
    ) -> WireResult<Accepted> {
        let command = self.lifecycle_command(session_id, route, kind)?;
        let bindings = self.bindings()?;
        let outcome = admit_lifecycle_operation(&bindings.context(), &command)
            .await
            .map_err(|error| app_failure(&error))?;
        self.admit_lifecycle(session_id, outcome).await
    }

    async fn admit_cancellation_route(&self, session_id: SessionId) -> WireResult<Accepted> {
        const ROOT_FENCE_ATTEMPTS: usize = 3;

        let command = self.lifecycle_command(
            session_id,
            RouteId::SessionCancel,
            OperationKind::SessionCancel,
        )?;
        for attempt in 0..ROOT_FENCE_ATTEMPTS {
            let bindings = self.bindings()?;
            let outcome = admit_lifecycle_operation(&bindings.context(), &command)
                .await
                .map_err(|error| app_failure(&error))?;
            match self.admit_lifecycle(session_id, outcome).await {
                Ok(accepted) => return Ok(accepted),
                Err(error)
                    if error.code == ErrorCode::PreconditionFailed
                        && attempt + 1 < ROOT_FENCE_ATTEMPTS =>
                {
                    // Brain may have advanced the root revision between the
                    // eventual planning reads and the conditional stop-latch
                    // write. No operation row was elected in that case. Re-read
                    // the exact operation first, then the current root, and
                    // rebuild the same caller operation against its new fence.
                }
                Err(error) if error.code == ErrorCode::PreconditionFailed => {
                    return Err(WireError::new(ErrorCode::CommitOutcomeUnknown));
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the bounded cancellation fence loop returns on every attempt")
    }

    /// Submits one lifecycle admission and projects its durable operation.
    async fn admit_lifecycle(
        &self,
        session_id: SessionId,
        outcome: LifecycleAdmissionOutcome,
    ) -> WireResult<Accepted> {
        let projected = match outcome {
            LifecycleAdmissionOutcome::Replayed(operation) => operation,
            LifecycleAdmissionOutcome::Planned(planned) => {
                self.commit_lifecycle(session_id, &planned.plan, &planned.projected)
                    .await?
            }
        };
        crate::session::operation_worker::invoke_pending(
            self.shared.operation_worker.as_ref(),
            self.cx.auth.workspace_id,
            &projected,
        )
        .await
        .map_err(|_| WireError::new(ErrorCode::CommitOutcomeUnknown))?;
        let operation = projected
            .public()
            .map_err(|_| WireError::new(ErrorCode::InternalError))?
            .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
        Ok(Accepted(operation))
    }

    async fn commit_lifecycle(
        &self,
        session_id: SessionId,
        plan: &aex_session_app::SessionTransaction,
        attempted: &aex_operation_domain::Operation,
    ) -> WireResult<aex_operation_domain::Operation> {
        let binding = SessionBinding {
            workspace: self.cx.auth.workspace_id,
            organization: self.cx.auth.organization_id,
            session: session_id,
        };
        let committer = DynamoAuthorityCommitter::new(
            self.shared.authority.clone(),
            self.shared.tables.clone(),
            binding,
            self.now()?,
            ApiHintSink,
        );
        let operations = SessionAuthorityExternal::new(binding);
        let work = WorkApplicationCompiler;
        let compilers = FamilyCompilers::new()
            .with(
                aex_session_app::TableFamily::OperationAuthority,
                &operations,
            )
            .with(aex_session_app::TableFamily::WorkAuthority, &work);
        match committer
            .commit_replayable_resolving(
                plan,
                &compilers,
                aex_session_dynamodb::Resolution::TargetItem,
            )
            .await
        {
            Ok(()) => Ok(attempted.clone()),
            Err(error) => self.recover_lifecycle(&error, attempted).await,
        }
    }

    async fn recover_lifecycle(
        &self,
        failure: &StoreError,
        attempted: &aex_operation_domain::Operation,
    ) -> WireResult<aex_operation_domain::Operation> {
        if matches!(
            failure,
            StoreError::PreconditionFailed { .. } | StoreError::CommitAmbiguous { .. }
        ) {
            let stored = self
                .shared
                .operations
                .load(self.cx.auth.workspace_id, attempted.id)
                .await
                .map_err(|error| authority_failure(&error))?;
            if let Some(stored) = stored {
                let observed = stored.record;
                if observed.workspace == attempted.workspace
                    && observed.session == attempted.session
                    && observed.scope == attempted.scope
                    && observed.kind == attempted.kind
                    && observed.intent == attempted.intent
                {
                    return Ok(observed);
                }
                return Err(WireError::new(ErrorCode::OperationIdempotencyConflict));
            }
            if matches!(failure, StoreError::PreconditionFailed { .. }) {
                match self
                    .shared
                    .commands
                    .load_session(self.cx.auth.workspace_id, session_id_of(attempted)?)
                    .await
                {
                    Err(aex_session_app::PortError::Deleting { operation, .. })
                        if attempted.kind == OperationKind::SessionDelete =>
                    {
                        return Err(if operation == attempted.id {
                            WireError::new(ErrorCode::CommitOutcomeUnknown)
                        } else {
                            WireError::new(ErrorCode::DeletionInProgress)
                        });
                    }
                    Err(aex_session_app::PortError::Deleting { .. }) => {
                        return Err(WireError::new(ErrorCode::SessionDeleting));
                    }
                    Err(aex_session_app::PortError::Deleted { .. }) => {
                        return Err(WireError::new(ErrorCode::SessionDeleted));
                    }
                    Err(aex_session_app::PortError::NotFound { .. }) => {
                        return Err(WireError::new(ErrorCode::NotFound));
                    }
                    Err(error) => return Err(app_failure(&aex_session_app::AppError::Port(error))),
                    Ok(_) if attempted.kind == OperationKind::SessionDelete => {
                        // A competing whole-session command moved a guard but
                        // no delete operation was elected. The caller must
                        // retry the same operation identity after observing
                        // the newer session state; `session_delete` does not
                        // publish a generic precondition response.
                        return Err(WireError::new(ErrorCode::CommitOutcomeUnknown));
                    }
                    Ok(_) => {}
                }
            }
            return Err(if matches!(failure, StoreError::CommitAmbiguous { .. }) {
                WireError::new(ErrorCode::CommitOutcomeUnknown)
            } else {
                WireError::new(ErrorCode::PreconditionFailed)
            });
        }
        Err(authority_failure(failure))
    }
}

fn session_id_of(operation: &aex_operation_domain::Operation) -> WireResult<SessionId> {
    match (operation.session, operation.scope) {
        (Some(session), aex_operation_domain::OperationScope::Session(scoped))
            if session == scoped =>
        {
            Ok(session)
        }
        _ => Err(WireError::new(ErrorCode::InternalError)),
    }
}

/// Everything an `AppContext` borrows, owned for the length of one request.
pub(super) struct CommandBindings {
    clock: RequestClock,
    ids: crate::session::app_ports::RequestIds,
    unowned: crate::session::app_ports::UnownedPorts,
    registry: crate::session::app_ports::RegistryReads,
    limits: crate::session::app_ports::LimitReads,
    reads: SessionCommandReads,
    accounts: AuthorizedAccount,
    credentials: ProviderCredentialReads,
    catalog: Arc<dyn aex_session_app::ModelQualifier>,
    deployment: aex_session_app::DeploymentFacts,
}

impl CommandBindings {
    pub(super) const fn ids(&self) -> &crate::session::app_ports::RequestIds {
        &self.ids
    }

    pub(super) fn context(&self) -> aex_session_app::AppContext<'_> {
        aex_session_app::AppContext {
            clock: &self.clock,
            ids: &self.ids,
            sessions: &self.reads,
            accounts: &self.accounts,
            registry: &self.registry,
            credentials: &self.credentials,
            catalog: Some(self.catalog.as_ref()),
            deployment: Some(&self.deployment),
            limits: &self.limits,
            live: &self.unowned,
        }
    }
}

/// What was asked for, for a whole-session command with an empty body.
///
/// Stop, trash and restore carry no request payload, so the complete statement
/// of intent is the route and the resource it names. Admission compares this
/// against the stored envelope, so the same operation id against the same route
/// and session replays, and the same id against a different one conflicts.
fn intent_of(
    route: RouteId,
    workspace: WorkspaceId,
    session: SessionId,
) -> aex_wire::idempotency::IntentDigest {
    use sha2::Digest as _;

    let mut digest = sha2::Sha256::new();
    digest.update(b"aex.regional.session.command.v1");
    digest.update([0_u8]);
    digest.update(route.as_str().as_bytes());
    digest.update([0_u8]);
    digest.update(workspace.to_string().as_bytes());
    digest.update([0_u8]);
    digest.update(session.to_string().as_bytes());
    aex_wire::idempotency::IntentDigest::from_bytes(digest.finalize().into())
}

/// Maps one application refusal onto the stable public code it declared.
fn app_failure(error: &aex_session_app::AppError) -> WireError {
    if let aex_session_app::AppError::Port(aex_session_app::PortError::Unowned { kind, seam }) =
        error
    {
        // A mounted route reached a port no adapter owns. That is a composition
        // defect rather than a customer condition, and it has to be loud in the
        // log even though the customer sees only `internal_error`.
        eprintln!("session-stream-api: a mounted route reached the unowned `{kind}` port: {seam}");
    }
    WireError::new(error.code())
}

fn public_operation(stored: &StoredOperation) -> WireResult<Option<models::Operation>> {
    stored
        .record
        .public()
        .map_err(|_| WireError::new(ErrorCode::InternalError))
}

fn operation_query_hash(query: &models::RegionalOperationsListQuery) -> WireResult<[u8; 32]> {
    use sha2::Digest as _;

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Normalized<'a> {
        kind: Option<&'a str>,
        session_id: Option<String>,
        status: Option<&'a str>,
    }

    let normalized = Normalized {
        kind: query.kind.map(models::OperationKind::as_str),
        session_id: query.session_id.map(|session| session.to_string()),
        status: query.status.map(models::OperationStatus::as_str),
    };
    let bytes = aex_wire::canonical::to_jcs_bytes(&normalized)
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    let mut digest = sha2::Sha256::new();
    digest.update(b"aex.regional.operations.list.filters.v1\0");
    digest.update(bytes);
    Ok(digest.finalize().into())
}

fn sessions_query_hash(status: Option<models::SessionStatus>) -> [u8; 32] {
    use sha2::Digest as _;

    let mut digest = sha2::Sha256::new();
    digest.update(b"aex.sessions.list.filters.v1\0");
    digest.update(status.map_or("*", models::SessionStatus::as_str).as_bytes());
    digest.finalize().into()
}

const fn session_list_status(status: models::SessionStatus) -> SessionListStatus {
    match status {
        models::SessionStatus::Idle => SessionListStatus::Idle,
        models::SessionStatus::Running => SessionListStatus::Running,
        models::SessionStatus::Suspending => SessionListStatus::Suspending,
        models::SessionStatus::Suspended => SessionListStatus::Suspended,
        models::SessionStatus::Resuming => SessionListStatus::Resuming,
        models::SessionStatus::Terminating => SessionListStatus::Terminating,
        models::SessionStatus::Terminated => SessionListStatus::Terminated,
        models::SessionStatus::Deleting => SessionListStatus::Deleting,
    }
}

impl RegionalOperationsApi for Routes {
    async fn regional_operation_cancel(
        &self,
        _cx: &WireContext,
        operation_id: OperationId,
        _body: models::EmptyRequest,
    ) -> WireResult<models::Operation> {
        match self
            .shared
            .operations
            .request_cancel(self.cx.auth.workspace_id, operation_id, self.now()?)
            .await
            .map_err(|error| authority_failure(&error))?
        {
            OperationCancelOutcome::Accepted(stored) => {
                public_operation(&stored)?.ok_or_else(|| WireError::new(ErrorCode::InternalError))
            }
            OperationCancelOutcome::NotFound => Err(WireError::new(ErrorCode::NotFound)),
            OperationCancelOutcome::NotCancelable => {
                Err(WireError::new(ErrorCode::OperationNotCancelable))
            }
        }
    }

    async fn regional_operation_get(
        &self,
        _cx: &WireContext,
        operation_id: OperationId,
    ) -> WireResult<models::Operation> {
        let stored = self
            .shared
            .operations
            .load(self.cx.auth.workspace_id, operation_id)
            .await
            .map_err(|error| authority_failure(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        public_operation(&stored)?.ok_or_else(|| WireError::new(ErrorCode::NotFound))
    }

    async fn regional_operations_list(
        &self,
        _cx: &WireContext,
        query: models::RegionalOperationsListQuery,
    ) -> WireResult<models::OperationPage> {
        let filter = OperationFilter {
            session: query.session_id,
            kind: query.kind.map(OperationKind::from_public),
            status: query.status.map(OperationStatus::from_public),
        };
        let binding = self.cursor_binding_for_query(
            RouteId::RegionalOperationsList,
            "operations",
            query.session_id,
            operation_query_hash(&query)?,
        )?;
        let after = self.resume(query.cursor.as_ref(), &binding)?;
        let page = self
            .shared
            .operations
            .page(
                self.cx.auth.workspace_id,
                &filter,
                budget(query.limit)?,
                after.as_ref(),
            )
            .await
            .map_err(|error| authority_failure(&error))?;
        if page.isolated > 0 {
            // The GSI-hit/base-miss race is legitimate — an eventually
            // consistent index racing a purge — but it must stay visible: a
            // growing count is an index-integrity signal, not noise.
            eprintln!(
                "regional-session-api: an operation listing isolated {} stale locator(s) for workspace {}",
                page.isolated, self.cx.auth.workspace_id
            );
        }
        let items = page
            .items
            .iter()
            .map(|stored| {
                public_operation(stored)?.ok_or_else(|| WireError::new(ErrorCode::InternalError))
            })
            .collect::<WireResult<Vec<_>>>()?;
        Ok(models::OperationPage {
            items,
            next_cursor: self.continuation(page.next.as_ref(), &binding)?,
        })
    }
}

impl SessionsApi for Routes {
    async fn session_cancel(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        _body: models::EmptyRequest,
    ) -> WireResult<Accepted> {
        self.admit_cancellation_route(session_id).await
    }

    async fn session_create(
        &self,
        _cx: &WireContext,
        body: models::SessionCreateRequest,
    ) -> WireResult<Created<models::Session>> {
        Box::pin(crate::session::create_route::create_session(self, body))
            .await
            .map(Created)
    }

    async fn session_delete(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        _body: models::EmptyRequest,
    ) -> WireResult<Accepted> {
        self.admit_lifecycle_route(
            session_id,
            RouteId::SessionDelete,
            OperationKind::SessionDelete,
        )
        .await
    }

    async fn session_get(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
    ) -> WireResult<WithETag<models::Session>> {
        let stored = match self
            .shared
            .sessions
            .load_session(self.cx.auth.workspace_id, session_id)
            .await
            .map_err(|error| authority_failure(&error))?
        {
            SessionScoped::Missing => return Err(WireError::new(ErrorCode::NotFound)),
            SessionScoped::Deleted => return Err(WireError::new(ErrorCode::SessionDeleted)),
            SessionScoped::Active(stored) => stored,
        };
        let value = aex_session_app::public_session(&stored)
            .map_err(|_| WireError::new(ErrorCode::InternalError))?;
        let etag = entity_tag("Session", &value).map_err(WireError::from)?;
        Ok(WithETag { value, etag })
    }

    async fn session_message_send(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        body: models::MessageSendRequest,
    ) -> WireResult<Created<models::MessageSendResult>> {
        self.admit_message_route(session_id, body)
            .await
            .map(Created)
    }

    async fn session_messages_list(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        query: models::SessionMessagesListQuery,
    ) -> WireResult<models::MessagePage> {
        let budget = budget(query.limit)?;
        let (after, expected_deletion_epoch) = self
            .resume_session_collection(
                RouteId::SessionMessagesList,
                "session.sealed-messages",
                session_id,
                query.cursor.as_ref(),
            )
            .await?;
        let page = match self
            .shared
            .sessions
            .page_messages(
                self.cx.auth.workspace_id,
                session_id,
                expected_deletion_epoch,
                budget,
                after.as_ref(),
            )
            .await
            .map_err(|error| authority_failure(&error))?
        {
            SessionScoped::Missing => return Err(WireError::new(ErrorCode::NotFound)),
            SessionScoped::Deleted => return Err(WireError::new(ErrorCode::SessionDeleted)),
            SessionScoped::Active(page) => page,
        };
        let binding = self.cursor_binding_for_session_epoch(
            RouteId::SessionMessagesList,
            "session.sealed-messages",
            session_id,
            page.deletion_epoch,
        )?;
        if query.cursor.is_some() {
            self.resume(query.cursor.as_ref(), &binding)?;
        }
        let next = self.continuation(page.next.as_ref(), &binding)?;
        projection::session_message_page(&page.items, next).map_err(WireError::from)
    }

    async fn session_resume(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        _body: models::EmptyRequest,
    ) -> WireResult<Accepted> {
        self.admit_lifecycle_route(
            session_id,
            RouteId::SessionResume,
            OperationKind::SessionResume,
        )
        .await
    }

    async fn session_suspend(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        _body: models::EmptyRequest,
    ) -> WireResult<Accepted> {
        self.admit_lifecycle_route(
            session_id,
            RouteId::SessionSuspend,
            OperationKind::SessionSuspend,
        )
        .await
    }

    async fn session_terminate(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        _body: models::EmptyRequest,
    ) -> WireResult<Accepted> {
        self.admit_lifecycle_route(
            session_id,
            RouteId::SessionTerminate,
            OperationKind::SessionTerminate,
        )
        .await
    }

    async fn sessions_list(
        &self,
        _cx: &WireContext,
        query: models::SessionsListQuery,
    ) -> WireResult<models::SessionListPage> {
        let query_hash = sessions_query_hash(query.status);
        let request_binding = CursorRequestBinding {
            route: RouteId::SessionsList,
            principal_scope: self.cx.auth.credential_binding,
            region: self.cx.auth.placement,
            workspace_id: self.cx.auth.workspace_id,
            session_id: None,
            query_hash,
            order: Order::Ascending,
        };
        let (snapshot, after) = if let Some(cursor) = query.cursor.as_ref() {
            let resumed = aex_regional_http::cursor::decode_resume(
                &self.shared.cursor_keys,
                cursor,
                &request_binding,
                self.now()?,
            )
            .map_err(|error| WireError::from(ProjectionError::Cursor(error)))?;
            let snapshot = Timestamp::parse(resumed.snapshot.as_str())
                .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?;
            let after = tuple_position(&resumed.tuple).map_err(WireError::from)?;
            (snapshot, Some(after))
        } else {
            (self.now()?, None)
        };
        let binding = self.cursor_binding_for_query(
            RouteId::SessionsList,
            &snapshot.to_wire(),
            None,
            query_hash,
        )?;
        let filter = SessionListFilter {
            status: query.status.map(session_list_status),
        };
        let page = self
            .shared
            .sessions
            .page_sessions(
                self.cx.auth.workspace_id,
                &filter,
                snapshot,
                budget(query.limit)?,
                after.as_ref(),
            )
            .await
            .map_err(|error| authority_failure(&error))?;
        if page.isolated > 0 {
            eprintln!(
                "regional-session-api: a session listing isolated {} stale locator(s) for workspace {}",
                page.isolated, self.cx.auth.workspace_id
            );
        }
        Ok(models::SessionListPage {
            items: page
                .items
                .iter()
                .map(aex_session_app::public_session_list_item)
                .collect(),
            next_cursor: self.continuation(page.next.as_ref(), &binding)?,
        })
    }
}

impl ProviderCredentialsApi for Routes {
    async fn provider_credential_get(
        &self,
        _cx: &WireContext,
        provider_credential_id: ProviderCredentialId,
    ) -> WireResult<WithETag<models::ProviderCredential>> {
        let stored = self
            .shared
            .custody
            .load_provider_credential(self.cx.auth.workspace_id, provider_credential_id)
            .await
            .map_err(|error| authority_failure(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::ProviderCredentialNotFound))?;
        let value = projection::provider_credential(&stored);
        let etag = entity_tag("ProviderCredential", &value).map_err(WireError::from)?;
        Ok(WithETag { value, etag })
    }

    async fn provider_credential_register(
        &self,
        _cx: &WireContext,
        body: models::ProviderCredentialRegisterRequest,
    ) -> WireResult<Created<models::ProviderCredential>> {
        self.shared
            .credential_registration
            .register(&self.cx, body)
            .await
    }

    /// Fences one BYOK binding.
    ///
    /// Two things are load bearing.
    ///
    /// **The update is committed as a one-action transaction, not as a bare
    /// conditional update.** This deployable is granted `GetItem`, `Query` and
    /// `TransactWriteItems` on `regional-secret-custody` and is deliberately not
    /// granted `UpdateItem`, so the same expression issued directly would be
    /// denied in production while passing every local test. Routing it through
    /// the one transaction compiler also keeps the "no unconditional authority
    /// write" check on the path.
    ///
    /// **Idempotency is carried by the terminal state (RS-31).** The scope
    /// subject is the credential and the body is `EmptyRequest`, so one scope
    /// plus one key can only ever carry this one intent: an
    /// `idempotency_conflict` is unreachable rather than undetected, and a
    /// replay answers from the stored row without a second write.
    async fn provider_credential_revoke(
        &self,
        _cx: &WireContext,
        provider_credential_id: ProviderCredentialId,
        _body: models::EmptyRequest,
    ) -> WireResult<models::ProviderCredential> {
        let workspace = self.cx.auth.workspace_id;
        let stored = self
            .shared
            .custody
            .load_provider_credential(workspace, provider_credential_id)
            .await
            .map_err(|error| authority_failure(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::ProviderCredentialNotFound))?;

        if stored.state == CredentialState::Revoked {
            return Ok(projection::provider_credential(&stored));
        }

        // Computed before the expression consumes the observed row, so the
        // published revision is the one the condition committed against rather
        // than a number re-derived after the fact.
        let next_revision = stored
            .revision
            .checked_add(1)
            .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
        let now = self.now()?;
        let builder =
            expressions::revoke_provider_credential(&self.shared.custody_table, &stored, now)
                .map_err(|error| authority_failure(&error))?;
        let mut plan = TransactionPlan::new(revocation_token(workspace, &stored));
        plan.update(Participant::CUSTODY_PROVIDER_CREDENTIAL, builder)
            .map_err(|error| authority_failure(&error))?;
        self.shared
            .custody
            .commit(&plan)
            .await
            .map_err(|error| authority_failure(&error))?;

        // The committed row is the observed one with the state fenced, the
        // revision advanced and both instants stamped, so the answer is built
        // from what the transaction wrote rather than from a second read that
        // could observe a later state.
        Ok(projection::provider_credential(&StoredCredential {
            state: CredentialState::Revoked,
            revision: next_revision,
            revoked_at: Some(now),
            updated_at: now,
            ..stored
        }))
    }

    async fn provider_credentials_list(
        &self,
        _cx: &WireContext,
        query: models::ProviderCredentialsListQuery,
    ) -> WireResult<models::ProviderCredentialPage> {
        let binding =
            self.cursor_binding(RouteId::ProviderCredentialsList, "provider-credentials")?;
        let after = self.resume(query.cursor.as_ref(), &binding)?;
        let page = self
            .shared
            .custody
            .page_provider_credentials(
                self.cx.auth.workspace_id,
                budget(query.limit)?,
                after.as_ref(),
            )
            .await
            .map_err(|error| authority_failure(&error))?;
        let next = self.continuation(page.next.as_ref(), &binding)?;
        // The declared `provider` filter is applied after the page is read, so a
        // filtered page can be shorter than the budget while still naming a
        // continuation. That is why the cursor is minted from the authority's
        // own position and never from the filtered item count.
        let items: Vec<_> = match query.provider {
            None => page.items,
            Some(provider) => page
                .items
                .into_iter()
                .filter(|row| row.provider == provider)
                .collect(),
        };
        Ok(projection::provider_credential_page(&items, next))
    }
}

impl Routes {
    /// Reads one registry listing and mints its continuation.
    ///
    /// Every listing this deployable serves is the same read: one
    /// `(workspace, kind)` partition in name order, resumed from a signed cursor
    /// and continued by one. The five public methods differ only in the kind
    /// they name and the model they project onto, so the read is written once.
    ///
    /// The snapshot token is the registry's own name, which is what stops a
    /// cursor minted over `skills` from resuming a read over `tools`: the
    /// binding is authenticated, so a re-pointed cursor fails its MAC rather
    /// than paging the wrong collection.
    async fn registry_page(
        &self,
        route: RouteId,
        kind: RegistryKind,
        cursor: Option<&Cursor>,
        limit: Option<u32>,
    ) -> WireResult<(PointerPage, Option<Cursor>)> {
        let binding = self.cursor_binding(route, registry_snapshot(kind))?;
        let after = self.resume(cursor, &binding)?;
        let page = self
            .shared
            .registry
            .list_pointers(
                self.cx.auth.workspace_id,
                kind,
                budget(limit)?,
                after.as_ref(),
            )
            .await
            .map_err(|error| authority_failure(&error))?;
        let next = self.continuation(page.next.as_ref(), &binding)?;
        Ok((page, next))
    }
}

/// The transport deduplication identity of one revocation.
///
/// It is derived from the binding and the **observed** revision, so two attempts
/// to fence the same observed state are one transaction inside the provider's
/// deduplication window and a later attempt against a moved row is not. The
/// durable identity is the terminal state itself (RS-31); this token only stops
/// a retry of the same attempt from being counted twice.
fn revocation_token(workspace: WorkspaceId, stored: &StoredCredential) -> String {
    use sha2::Digest as _;

    let mut digest = sha2::Sha256::new();
    digest.update(b"aex.provider_credential.revoke.v1");
    digest.update(workspace.to_string());
    digest.update(stored.credential.to_string());
    digest.update(stored.revision.to_be_bytes());
    let digest: [u8; 32] = digest.finalize().into();
    format!("aex-{}", hex::encode(&digest[..16]))
}

/// The snapshot token a registry listing's cursor is bound to.
///
/// One token per registry, so a continuation cannot cross from one collection
/// into another even though all five share a table and a key template.
const fn registry_snapshot(kind: RegistryKind) -> &'static str {
    match kind {
        RegistryKind::File => "registry:files",
        RegistryKind::Skill => "registry:skills",
        RegistryKind::Tool => "registry:tools",
        RegistryKind::Instruction => "registry:instructions",
        RegistryKind::McpServer => "registry:mcp-servers",
    }
}

/// The named registry: fifteen routes served, one absent.
///
/// The point reads became servable when the value moved onto the pointer row:
/// the complete non-payload value is a canonical JSON document stored beside the
/// digest, so a point read is one `GetItem` and needs no content data key —
/// there is nothing to decrypt because no registry response ever publishes
/// payload bytes (D-2, D-3). `registry_files_download_create` is the one route
/// still absent; it needs the content grant and presigning composition, and it
/// closes with cluster D's second landing.
impl RegistryApi for Routes {
    async fn registry_files_delete(
        &self,
        _cx: &WireContext,
        name: ResourceName,
    ) -> WireResult<NoContent> {
        self.registry_delete(RegistryKind::File, &name).await
    }

    async fn registry_files_get(
        &self,
        _cx: &WireContext,
        name: ResourceName,
    ) -> WireResult<WithETag<models::RegisteredFile>> {
        self.registry_get(RegistryKind::File, &name, projection::registered_file)
            .await
    }

    async fn registry_files_list(
        &self,
        _cx: &WireContext,
        query: models::RegistryFilesListQuery,
    ) -> WireResult<models::RegisteredFilePage> {
        let (page, next) = self
            .registry_page(
                RouteId::RegistryFilesList,
                RegistryKind::File,
                query.cursor.as_ref(),
                query.limit,
            )
            .await?;
        projection::registered_file_page(&page.rows, next).map_err(WireError::from)
    }

    async fn registry_files_put(
        &self,
        _cx: &WireContext,
        name: ResourceName,
        body: models::RegisteredFileValue,
    ) -> WireResult<WithETag<models::RegisteredFile>> {
        let payload = self
            .admit_payload(RegistryKind::File, &name, &body.content)
            .await?;
        let read = models::RegisteredFileRead {
            content: payload.reference,
            media_type: body.media_type,
            mode: body.mode,
            mount_path: body.mount_path,
        };
        self.registry_put(
            RegistryKind::File,
            &name,
            &read,
            Some(payload.source),
            projection::registered_file,
        )
        .await
    }

    async fn registry_files_download_create(
        &self,
        _cx: &WireContext,
        name: ResourceName,
        body: models::RegistryDownloadRequest,
    ) -> WireResult<Created<models::DownloadGrant>> {
        self.registry_file_download_create(&name, &body)
            .await
            .map(Created)
    }
}

/// The three regional workspace reads.
///
/// # One predicate, spelled once
///
/// All three answer `workspace_activation_required` for the same fact: a
/// durable row this workspace needs does not exist here yet. That is the same
/// predicate `session_create` and the three live-file routes declare, and it has
/// to stay one predicate rather than becoming four — [`Routes::activation`] is
/// the whole of it.
///
/// What it is emphatically **not** is `404`. An absent row of a registered limit
/// is an operator fault that clears, and answering "no such limit" would be a
/// lie about the contract that a client would rightly stop retrying.
impl WorkspaceApi for Routes {
    async fn workspace_current_get(&self, _cx: &WireContext) -> WireResult<models::Workspace> {
        let workspace = self.cx.auth.workspace_id;
        // Two eventual point reads. The placement row is read for exactly one
        // fact the request context does not carry — whether a deletion is
        // running — because the admission projection collapses `deleting` onto
        // `Paused`, which is the right answer for admission and the wrong one
        // to publish as a workspace status.
        let (placement, profile) = futures::future::try_join(
            async {
                self.shared
                    .placements
                    .read_placement(workspace)
                    .await
                    .map_err(|error| Self::activation(&error))
            },
            async {
                self.shared
                    .workspace
                    .read_profile(workspace)
                    .await
                    .map_err(|error| {
                        // A profile row that is present and will not decode is
                        // not an un-activated workspace. Since the account
                        // fields became required, the likeliest cause is a row
                        // written before they existed — and the honest answer to
                        // "what is this account's state" is then that we could
                        // not establish it, never a guessed `Active`.
                        if matches!(error, StoreError::Invalid { .. }) {
                            WireError::new(ErrorCode::AccountStateUnavailable)
                        } else {
                            Self::activation(&error)
                        }
                    })
            },
        )
        .await?;
        let profile =
            profile.ok_or_else(|| WireError::new(ErrorCode::WorkspaceActivationRequired))?;

        let status = match placement.status.as_str() {
            "deleting" => models::WorkspaceStatus::Deleting,
            // `active` and `paused` are both live workspaces. The pause is the
            // *account's* state and is published as such below; folding it into
            // the workspace status would give one fact two homes.
            "active" | "paused" => models::WorkspaceStatus::Active,
            _ => return Err(WireError::new(ErrorCode::InternalError)),
        };
        Ok(models::Workspace {
            api_url: self.shared.api_url.clone(),
            created_at: profile.created_at,
            // The deletion operation id is a central fact; the placement row
            // carries whether a deletion runs, not which operation runs it.
            deletion_operation_id: None,
            id: workspace,
            name: profile.name.clone(),
            operational_state: models::WorkspaceOperationalState {
                inherited_from: models::OperationalStateSource::Account,
                organization_id: self.cx.auth.organization_id,
                state: account_state(&profile)?,
            },
            organization_id: self.cx.auth.organization_id,
            region: self.cx.auth.placement,
            slug: profile.slug.clone(),
            status,
        })
    }

    async fn workspace_limit_get(
        &self,
        _cx: &WireContext,
        limit_id: LimitId,
    ) -> WireResult<models::EffectiveWorkspaceLimit> {
        // `limit_id` parsed, so the identifier is registered. `not_found` was
        // decided before this call and can no longer happen here: an absent row
        // of a registered limit is an activation gap, not an unknown resource.
        let stored = self
            .shared
            .workspace
            .read_limit(self.cx.auth.workspace_id, limit_id)
            .await
            .map_err(|error| Self::activation(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::WorkspaceActivationRequired))?;
        Ok(models::EffectiveWorkspaceLimit {
            changed_at: stored.changed_at,
            effective_value: stored.effective_value,
            id: stored.id,
            revision: stored.revision,
            source: stored.source,
        })
    }

    async fn workspace_limits_list(
        &self,
        _cx: &WireContext,
    ) -> WireResult<models::EffectiveWorkspaceLimitPage> {
        // The bundle item, not a query over the member rows.
        //
        // A paged query can interleave two authority revisions inside one page,
        // which is the "partial answer presented as authoritative" failure this
        // whole cluster exists to remove. The bundle is written in the same
        // transaction as the members, so it is one complete revision or the
        // previous one, and never a mixture of both.
        let bundle = self
            .shared
            .workspace
            .read_limit_bundle(self.cx.auth.workspace_id)
            .await
            .map_err(|error| Self::activation(&error))?;
        if bundle.limits.len() != LimitId::ALL.len() {
            // Short is not a page here. The registry is closed and every
            // workspace's set is complete by construction, so fewer items than
            // the registry means the set was never finished — and a `200`
            // carrying it would be read as "these are all the limits there are".
            return Err(WireError::new(ErrorCode::WorkspaceActivationRequired));
        }
        Ok(models::EffectiveWorkspaceLimitPage {
            items: bundle.limits,
            // Never populated. The registry is a closed, registry-sized
            // collection and the whole of it is one item; `cursor` and `limit`
            // were removed from this route for the same reason.
            next_cursor: None,
        })
    }
}

impl Routes {
    /// The one activation predicate, over a store failure.
    ///
    /// `Misconfigured` is the projection's single constructor for "this region
    /// holds no usable record of that" — an absent row and a row contradicting
    /// its siblings are deliberately indistinguishable, so the difference is not
    /// probeable. On the admission path that collapses to `401`, which is right:
    /// telling an unknown credential which of the two it hit is telling it
    /// something. On a cold descriptive read it is wrong, because the caller's
    /// credential was already verified and calling it unknown is a false
    /// statement about the caller.
    fn activation(error: &StoreError) -> WireError {
        match error {
            StoreError::Misconfigured { .. } => {
                WireError::new(ErrorCode::WorkspaceActivationRequired)
            }
            other => authority_failure(other),
        }
    }
}

/// Projects the account fields the profile row carries onto the published state.
///
/// This calls the one mapping in `aex-control-domain`, which is also what
/// `central-identity-api` calls. There is no second derivation and there must
/// never be one: the same account read centrally and regionally may differ in
/// staleness, never in vocabulary, discriminator or derivation.
fn account_state(
    profile: &aex_session_dynamodb::wire_pending::WorkspaceProfile,
) -> WireResult<models::AccountOperationalState> {
    let changed_at = profile.account_changed_at.to_datetime();
    // Finance publishes a reason exactly when the account is paused, so the
    // presence of one *is* the discriminator. The placement row's `deleting`
    // collapse is not consulted: that is an admission projection of workspace
    // status, and this is the account's state.
    let state = if profile.account_pause_reason.is_some() {
        aex_control_domain::AccountState::PausedTopUpRequired
    } else {
        aex_control_domain::AccountState::Active
    };
    aex_control_domain::account_operational_state(&aex_control_domain::AccountProfile {
        state,
        reason: profile.account_pause_reason.clone(),
        revision: profile.account_revision,
        changed_at,
    })
    .map_err(|error| match error {
        aex_control_domain::AccountProjectionError::Unavailable => {
            WireError::new(ErrorCode::AccountStateUnavailable)
        }
        // A reason outside the durable vocabulary, or an unrepresentable
        // instant, is a corrupt projected row rather than a customer condition.
        // It is still not an answer, so it is still not `Active`.
        _ => WireError::new(ErrorCode::AccountStateUnavailable),
    })
}

#[async_trait::async_trait]
impl UnaryDispatch for Routes {
    fn owner(&self) -> RouteOwner {
        RouteOwner::SessionApi
    }

    fn served(&self) -> Vec<RouteId> {
        Self::served()
    }

    async fn dispatch(
        &self,
        cx: &RequestContext,
        accept: AcceptKind,
        raw: RawRequest<'_>,
        limits: RequestLimits,
    ) -> WireResult<RawResponse> {
        let wire = cx.to_wire(accept);
        // The group comes from the generated table, so a new fragment is a
        // non-exhaustive-match compile error rather than a runtime 404.
        let outcome = match route(raw.route).fragment {
            "provider-credentials" => {
                dispatch_provider_credentials(self, &wire, raw, limits).await?
            }
            "operations" => dispatch_regional_operations(self, &wire, raw, limits).await?,
            "registry" => dispatch_registry(self, &wire, raw, limits).await?,
            "sessions" => Box::pin(dispatch_sessions(self, &wire, raw, limits)).await?,
            "files" => dispatch_files(self, &wire, raw, limits).await?,
            "uploads" => aex_wire::server::dispatch_uploads(self, &wire, raw, limits).await?,
            "usage" => dispatch_usage(self, &wire, raw, limits).await?,
            "workspace" => dispatch_workspace(self, &wire, raw, limits).await?,
            _ => return Err(not_served(raw.route)),
        };
        match outcome {
            aex_wire::dispatch::DispatchOutcome::Unary(response) => Ok(response),
            // Neither group declares an NDJSON route, so `NoStream` is
            // uninhabited and this arm is unconstructible.
            aex_wire::dispatch::DispatchOutcome::Ndjson(never) => match never.0 {},
        }
    }
}

/// The mounted dispatcher: the shared adapters, plus a `Routes` per request.
///
/// [`Routes`] carries the request context because every authority read here is
/// workspace-scoped, so it cannot be the value `mount_unary` holds for the life
/// of the process. This is that value, and it costs one `Arc` clone per request.
///
/// It is published rather than written twice, once here and once in the `served`
/// target: two spellings of the composition would let the tested router and the
/// mounted router drift apart, which is the one thing the `served` target exists
/// to rule out.
#[derive(Debug)]
pub struct Dispatcher(Arc<Shared>);

impl Dispatcher {
    /// Binds the dispatcher to the shared adapters.
    #[must_use]
    pub const fn new(shared: Arc<Shared>) -> Self {
        Self(shared)
    }
}

#[async_trait::async_trait]
impl UnaryDispatch for Dispatcher {
    fn owner(&self) -> RouteOwner {
        RouteOwner::SessionApi
    }

    fn served(&self) -> Vec<RouteId> {
        Routes::served()
    }

    async fn dispatch(
        &self,
        cx: &RequestContext,
        accept: AcceptKind,
        raw: RawRequest<'_>,
        limits: RequestLimits,
    ) -> WireResult<RawResponse> {
        Routes::new(Arc::clone(&self.0), cx.clone())
            .dispatch(cx, accept, raw, limits)
            .await
    }
}

/// The groups this deployable draws served routes from.
#[must_use]
pub fn served_groups() -> Vec<RouteGroup> {
    let served = Routes::served();
    RouteGroup::ALL
        .iter()
        .copied()
        .filter(|group| group.routes().iter().any(|id| served.contains(id)))
        .collect()
}
