//! The generated server traits, implemented over the real adapters.
//!
//! Two rules shape everything here.
//!
//! **A handler never names a status and never invents a code.** The response
//! type it returns is the status the route declares, and every refusal is one of
//! the codes the route's descriptor lists â€” `dispatch::declared` refuses the rest
//! at the boundary, so a code that slipped through would be `internal_error`
//! rather than a lie.
//!
//! **A route this deployable does not own is [`not_served`].** Two authoring
//! fragments are split across two deployables, so implementing a trait means
//! implementing methods for the other half too. Those arms are unreachable
//! through the router â€” [`Routes::served`] never offers them â€” and the
//! composition test proves it.

use std::convert::Infallible;
use std::sync::Arc;

use aex_content_aws::object_store::ContentObjectStore;
use aex_content_domain::identity::RegistryKind;
use aex_content_dynamodb::store::ContentMetadataStore;
use aex_operation_domain::operation::OperationKind;
use aex_regional_http::context::RequestContext;
use aex_regional_http::cursor::{
    CursorBinding, CursorKeyRing, CursorRequestBinding, Order, SnapshotToken, SortTuple,
};
use aex_regional_http::mount::{DispatchResponse, ResponseStream, UnaryDispatch, not_served};
use aex_regional_http::projection::{
    self, ProjectionError, authority_failure, entity_tag, position_tuple, tuple_position,
};
use aex_regional_http::router::RouteOwner;
use aex_registry_dynamodb::store::{PointerPage, RegistryStore};
use aex_runtime_activity_dynamodb::store::RuntimeActivityDynamoStore;
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
use aex_session_dynamodb::plan::{Participant, RegionalTables};
use aex_session_dynamodb::projection::WorkspaceProjection;
use aex_session_dynamodb::store::{
    OperationApiStore, SessionListFilter, SessionListStatus, SessionQueries, SessionScoped,
};
use aex_wire::cursor::Cursor;
use aex_wire::dispatch::{RawRequest, RequestLimits};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::{IntentDigest, ReplayIdentity};
use aex_wire::ids::{
    ContentHash, MessageId, PrefixedId as _, ResourceName, SessionId, WorkspaceId,
};
use aex_wire::models;
use aex_wire::routes::{RouteId, route};
use aex_wire::server::{
    AcceptKind, Created, NdjsonStream, NoContent, RegistryApi, RequestContext as WireContext,
    RouteGroup, SessionsApi, WithETag, dispatch_registry, dispatch_sessions, dispatch_uploads,
};
use aex_wire::types::{DecimalU128, HttpsUrl, Timestamp};
use aex_work_dynamodb::WorkApplicationCompiler;
use bytes::Bytes;

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
    /// Bounded remote and exact-generation sandbox MCP qualification.
    pub mcp_qualifier: Arc<dyn super::mcp_readiness::SessionMcpQualifier>,
    /// Write-only session provider-key encryption and custody.
    pub provider_keys: Arc<dyn aex_session_app::ProviderCredentialReader>,
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
    /// Read/presign-only access to immutable AEX-generated OTLP segments.
    pub session_telemetry: aex_session_telemetry_aws::SessionTelemetryReader,
    /// The cold workspace-description surface: profile and effective limits.
    ///
    /// Deliberately a separate port from the placement reader below. A display
    /// name or a limit read must not widen the capability the admission path
    /// uses on every request.
    pub workspace: Arc<dyn WorkspaceProjection>,
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

use super::registry::RegistryWriteState;
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

fn session_telemetry_failure(
    error: &aex_session_telemetry_aws::SessionTelemetryError,
) -> WireError {
    match error {
        aex_session_telemetry_aws::SessionTelemetryError::InvalidSegmentId
        | aex_session_telemetry_aws::SessionTelemetryError::InvalidPageSize => {
            WireError::new(ErrorCode::InvalidRequest)
        }
        aex_session_telemetry_aws::SessionTelemetryError::NotFound => {
            WireError::new(ErrorCode::NotFound)
        }
        _ => WireError::new(ErrorCode::UpstreamError),
    }
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

    async fn require_live_session(&self, session_id: SessionId) -> WireResult<()> {
        match self
            .shared
            .sessions
            .load_session(self.cx.auth.workspace_id, session_id)
            .await
            .map_err(|error| authority_failure(&error))?
        {
            SessionScoped::Missing => Err(WireError::new(ErrorCode::NotFound)),
            SessionScoped::Deleted => Err(WireError::new(ErrorCode::SessionDeleted)),
            SessionScoped::Active(_) => Ok(()),
        }
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
            credentials: Arc::clone(&self.shared.provider_keys),
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
                WireError::new(ErrorCode::InvalidRequest)
                    .with_message("the session provider key is unavailable")
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
    ) -> WireResult<models::SessionCommandReceipt> {
        let command = self.lifecycle_command(session_id, route, kind)?;
        let bindings = self.bindings()?;
        let outcome = admit_lifecycle_operation(&bindings.context(), &command)
            .await
            .map_err(|error| app_failure(&error))?;
        self.admit_lifecycle(session_id, outcome).await
    }

    async fn admit_cancellation_route(
        &self,
        session_id: SessionId,
    ) -> WireResult<models::SessionCommandReceipt> {
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
    ) -> WireResult<models::SessionCommandReceipt> {
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
        Ok(models::SessionCommandReceipt {
            accepted_at: projected.created_at,
            operation_id: projected.id,
            session_id,
        })
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
    credentials: Arc<dyn aex_session_app::ProviderCredentialReader>,
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
            credentials: self.credentials.as_ref(),
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

fn sessions_query_hash(status: Option<models::SessionStatus>) -> [u8; 32] {
    use sha2::Digest as _;

    let mut digest = sha2::Sha256::new();
    digest.update(b"aex.sessions.list.filters.v1\0");
    digest.update(status.map_or("*", models::SessionStatus::as_str).as_bytes());
    digest.finalize().into()
}

fn frame_stream<T: serde::Serialize>(frames: &[T]) -> WireResult<ResponseStream> {
    let mut encoded = Vec::with_capacity(frames.len());
    for frame in frames {
        let mut bytes =
            serde_json::to_vec(frame).map_err(|_| WireError::new(ErrorCode::InternalError))?;
        bytes.push(b'\n');
        encoded.push(Ok::<Bytes, Infallible>(Bytes::from(bytes)));
    }
    Ok(Box::pin(futures::stream::iter(encoded)))
}

async fn retained_telemetry_frames(
    reader: &aex_session_telemetry_aws::SessionTelemetryReader,
    session: SessionId,
    after: u128,
    limit: usize,
) -> Result<Vec<models::TelemetryFrame>, aex_session_telemetry_aws::SessionTelemetryError> {
    let mut continuation = None;
    let mut frames = Vec::new();
    while frames.len() < limit {
        let page = reader
            .list(
                session,
                continuation,
                aex_session_telemetry_aws::MAX_PAGE_ITEMS,
            )
            .await?;
        for segment in page.segments {
            if u128::from(segment.last_sequence) <= after {
                continue;
            }
            frames.extend(
                reader
                    .read_frames(session, &segment.id)
                    .await?
                    .into_iter()
                    .filter(|frame| frame.sequence.get() > after),
            );
            if frames.len() >= limit {
                break;
            }
        }
        continuation = page.next;
        if continuation.is_none() || frames.len() >= limit {
            break;
        }
    }
    frames.sort_by_key(|frame| frame.sequence.get());
    frames.dedup_by_key(|frame| frame.sequence.get());
    frames.truncate(limit);
    Ok(frames)
}

fn system_timestamp() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default();
    Timestamp::from_unix_millis(millis)
        .unwrap_or_else(|_| Timestamp::from_unix_millis(0).expect("epoch is representable"))
}

fn encode_ndjson_frames<T: serde::Serialize>(frames: &[T]) -> Bytes {
    let mut bytes = Vec::new();
    for frame in frames {
        if serde_json::to_writer(&mut bytes, frame).is_err() {
            continue;
        }
        bytes.push(b'\n');
    }
    Bytes::from(bytes)
}

fn retained_telemetry_stream(
    reader: aex_session_telemetry_aws::SessionTelemetryReader,
    session: SessionId,
    after: u128,
) -> ResponseStream {
    Box::pin(futures::stream::unfold(
        (reader, session, after),
        |(reader, session, mut after)| async move {
            let frames = match retained_telemetry_frames(&reader, session, after, 100).await {
                Ok(frames) if !frames.is_empty() => frames,
                Ok(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    vec![models::TelemetryFrame {
                        body: None,
                        kind: models::TelemetryKind::Heartbeat,
                        occurred_at: system_timestamp(),
                        preview: None,
                        sequence: DecimalU128::new(after),
                        span_id: None,
                        trace_id: None,
                        truncated: false,
                    }]
                }
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let body = aex_wire::CanonicalJson::from_value(&serde_json::json!({
                        "reason": "retained_stream_unavailable",
                    }))
                    .ok();
                    vec![models::TelemetryFrame {
                        body,
                        kind: models::TelemetryKind::Gap,
                        occurred_at: system_timestamp(),
                        preview: None,
                        sequence: DecimalU128::new(after),
                        span_id: None,
                        trace_id: None,
                        truncated: false,
                    }]
                }
            };
            if let Some(sequence) = frames
                .iter()
                .filter(|frame| {
                    !matches!(
                        frame.kind,
                        models::TelemetryKind::Heartbeat | models::TelemetryKind::Gap
                    )
                })
                .map(|frame| frame.sequence.get())
                .max()
            {
                after = after.max(sequence);
            }
            Some((
                Ok::<Bytes, Infallible>(encode_ndjson_frames(&frames)),
                (reader, session, after),
            ))
        },
    ))
}

#[derive(Debug, PartialEq, Eq)]
enum AssistantObservation {
    Preview {
        message_id: Option<MessageId>,
        text: String,
    },
    Gap {
        message_id: Option<MessageId>,
    },
    Committed {
        message_id: MessageId,
    },
}

fn assistant_observation(frame: &models::TelemetryFrame) -> Option<AssistantObservation> {
    let body = frame.body.as_ref().map(aex_wire::CanonicalJson::to_value);
    let event = body
        .as_ref()
        .and_then(|value| value.get("event"))
        .and_then(serde_json::Value::as_str);
    let message_id = body
        .as_ref()
        .and_then(|value| value.get("scope"))
        .and_then(|scope| scope.get("messageId"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| MessageId::parse(value).ok());
    match event {
        Some("aex.session.assistant.preview") => {
            frame
                .preview
                .as_ref()
                .map(|text| AssistantObservation::Preview {
                    message_id,
                    text: text.clone(),
                })
        }
        Some("aex.session.assistant.gap") => Some(AssistantObservation::Gap { message_id }),
        Some("aex.session.assistant.committed") => message_id
            .map(|message_id| AssistantObservation::Committed { message_id })
            .or(Some(AssistantObservation::Gap { message_id: None })),
        _ if frame.kind == models::TelemetryKind::Gap => {
            Some(AssistantObservation::Gap { message_id })
        }
        _ => None,
    }
}

async fn assistant_message_frame(
    sessions: &Arc<dyn SessionQueries>,
    workspace: WorkspaceId,
    session: SessionId,
    frame: &models::TelemetryFrame,
) -> WireResult<Option<models::MessageStreamFrame>> {
    let sequence = frame.sequence;
    match assistant_observation(frame) {
        Some(AssistantObservation::Preview { message_id, text }) => {
            Ok(Some(models::MessageStreamFrame {
                sequence,
                kind: models::MessageStreamKind::Preview,
                message_id,
                text: Some(text),
                message: None,
            }))
        }
        Some(AssistantObservation::Gap { message_id }) => Ok(Some(models::MessageStreamFrame {
            sequence,
            kind: models::MessageStreamKind::Gap,
            message_id,
            text: None,
            message: None,
        })),
        Some(AssistantObservation::Committed { message_id }) => {
            let message = match sessions
                .load_message(workspace, session, message_id)
                .await
                .map_err(|error| authority_failure(&error))?
            {
                SessionScoped::Active(Some(message)) => message,
                SessionScoped::Active(None) => {
                    return Ok(Some(models::MessageStreamFrame {
                        sequence,
                        kind: models::MessageStreamKind::Gap,
                        message_id: Some(message_id),
                        text: None,
                        message: None,
                    }));
                }
                SessionScoped::Missing => return Err(WireError::new(ErrorCode::NotFound)),
                SessionScoped::Deleted => return Err(WireError::new(ErrorCode::SessionDeleted)),
            };
            Ok(Some(models::MessageStreamFrame {
                sequence,
                kind: models::MessageStreamKind::Reconcile,
                message_id: Some(message_id),
                text: None,
                message: Some(projection::session_message(&message).map_err(WireError::from)?),
            }))
        }
        None => Ok(None),
    }
}

fn assistant_message_stream(
    reader: aex_session_telemetry_aws::SessionTelemetryReader,
    sessions: Arc<dyn SessionQueries>,
    workspace: WorkspaceId,
    session: SessionId,
    after: u128,
) -> ResponseStream {
    Box::pin(futures::stream::unfold(
        (reader, sessions, workspace, session, after),
        |(reader, sessions, workspace, session, mut after)| async move {
            let frames = match retained_telemetry_frames(&reader, session, after, 100).await {
                Ok(retained) if !retained.is_empty() => {
                    after = retained
                        .iter()
                        .map(|frame| frame.sequence.get())
                        .max()
                        .unwrap_or(after)
                        .max(after);
                    let mut public = Vec::new();
                    let mut failed = false;
                    for frame in &retained {
                        match assistant_message_frame(&sessions, workspace, session, frame).await {
                            Ok(Some(frame)) => public.push(frame),
                            Ok(None) => {}
                            Err(_) => {
                                failed = true;
                                break;
                            }
                        }
                    }
                    if failed {
                        vec![models::MessageStreamFrame {
                            sequence: DecimalU128::new(after),
                            kind: models::MessageStreamKind::Gap,
                            message_id: None,
                            text: None,
                            message: None,
                        }]
                    } else if public.is_empty() {
                        vec![models::MessageStreamFrame {
                            sequence: DecimalU128::new(after),
                            kind: models::MessageStreamKind::Heartbeat,
                            message_id: None,
                            text: None,
                            message: None,
                        }]
                    } else {
                        public
                    }
                }
                Ok(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    vec![models::MessageStreamFrame {
                        sequence: DecimalU128::new(after),
                        kind: models::MessageStreamKind::Heartbeat,
                        message_id: None,
                        text: None,
                        message: None,
                    }]
                }
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    vec![models::MessageStreamFrame {
                        sequence: DecimalU128::new(after),
                        kind: models::MessageStreamKind::Gap,
                        message_id: None,
                        text: None,
                        message: None,
                    }]
                }
            };
            Some((
                Ok::<Bytes, Infallible>(encode_ndjson_frames(&frames)),
                (reader, sessions, workspace, session, after),
            ))
        },
    ))
}

const fn session_list_status(status: models::SessionStatus) -> SessionListStatus {
    match status {
        models::SessionStatus::Idle => SessionListStatus::Idle,
        models::SessionStatus::Running => SessionListStatus::Running,
        models::SessionStatus::Terminating => SessionListStatus::Terminating,
        models::SessionStatus::Terminated => SessionListStatus::Terminated,
        models::SessionStatus::Deleting => SessionListStatus::Deleting,
    }
}

impl SessionsApi for Routes {
    type FrameStream = ResponseStream;

    async fn session_cancel(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        _body: models::EmptyRequest,
    ) -> WireResult<models::SessionCommandReceipt> {
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
    ) -> WireResult<models::SessionCommandReceipt> {
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

    async fn session_messages_stream(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        query: models::SessionMessagesStreamQuery,
    ) -> WireResult<NdjsonStream<Self::FrameStream>> {
        self.require_live_session(session_id).await?;
        let after = query.after.map_or(0, DecimalU128::get);
        Ok(NdjsonStream(assistant_message_stream(
            self.shared.session_telemetry.clone(),
            self.shared.sessions.clone(),
            self.cx.auth.workspace_id,
            session_id,
            after,
        )))
    }

    async fn session_telemetry_download_create(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        body: models::TelemetryDownloadRequest,
    ) -> WireResult<Created<models::TelemetryDownloadGrant>> {
        self.require_live_session(session_id).await?;
        let from = body.from_sequence.map_or(0, DecimalU128::get);
        let to = body.to_sequence.map(DecimalU128::get);
        if to.is_some_and(|ceiling| ceiling <= from) {
            return Err(WireError::new(ErrorCode::InvalidRange));
        }
        let export = self
            .shared
            .session_telemetry
            .export_range(session_id, from, to)
            .await
            .map_err(|error| session_telemetry_failure(&error))?;
        let url = self
            .shared
            .session_telemetry
            .presign_export(&export)
            .await
            .map_err(|error| session_telemetry_failure(&error))?;
        let expires_at = Timestamp::from_unix_millis(
            self.now()?.unix_millis().saturating_add(
                i64::try_from(aex_session_telemetry_aws::DOWNLOAD_GRANT_TTL.as_millis())
                    .unwrap_or(i64::MAX),
            ),
        )
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
        Ok(Created(models::TelemetryDownloadGrant {
            expires_at,
            media_type: aex_session_telemetry_aws::TELEMETRY_EXPORT_MEDIA_TYPE.to_owned(),
            sha256: ContentHash::parse(&format!("sha256:{}", export.sha256))
                .map_err(|_| WireError::new(ErrorCode::InternalError))?,
            size_bytes: DecimalU128::new(u128::from(export.size_bytes)),
            url: HttpsUrl::parse(&url).map_err(|_| WireError::new(ErrorCode::UpstreamError))?,
        }))
    }

    async fn session_telemetry_replay(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        query: models::SessionTelemetryReplayQuery,
    ) -> WireResult<NdjsonStream<Self::FrameStream>> {
        self.require_live_session(session_id).await?;
        let after = query.after.map_or(0, DecimalU128::get);
        let requested = query.limit.unwrap_or(100).min(100);
        let frames = retained_telemetry_frames(
            &self.shared.session_telemetry,
            session_id,
            after,
            usize::try_from(requested).map_err(|_| WireError::new(ErrorCode::InvalidRequest))?,
        )
        .await
        .map_err(|error| session_telemetry_failure(&error))?;
        Ok(NdjsonStream(frame_stream(&frames)?))
    }

    async fn session_telemetry_stream(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        query: models::SessionTelemetryStreamQuery,
    ) -> WireResult<NdjsonStream<Self::FrameStream>> {
        self.require_live_session(session_id).await?;
        let after = query.after.map_or(0, DecimalU128::get);
        Ok(NdjsonStream(retained_telemetry_stream(
            self.shared.session_telemetry.clone(),
            session_id,
            after,
        )))
    }

    async fn session_terminate(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        _body: models::EmptyRequest,
    ) -> WireResult<models::SessionCommandReceipt> {
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
/// digest, so a point read is one `GetItem` and needs no content data key â€”
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
        if let models::BlobInput::Url(source) = &body.content {
            let pending = serde_json::json!({
                "source": {
                    "type": "url",
                    "url": source.url.as_str(),
                },
                "organizationId": self.cx.auth.organization_id.to_string(),
                "mediaType": body.media_type,
                "mode": body.mode.as_str(),
            });
            return self
                .registry_put(
                    RegistryKind::File,
                    &name,
                    &pending,
                    RegistryWriteState {
                        payload: None,
                        state: aex_workspace_domain::registry::RegistryState::Pending,
                        failure_code: None,
                        exact_current: None,
                    },
                    projection::registered_file,
                )
                .await;
        }
        let payload = self
            .admit_payload(RegistryKind::File, &name, &body.content)
            .await?;
        let read = models::RegisteredFileRead {
            content: payload.reference,
            media_type: body.media_type,
            mode: body.mode,
        };
        self.registry_put(
            RegistryKind::File,
            &name,
            &read,
            RegistryWriteState {
                payload: Some(payload.source),
                state: aex_workspace_domain::registry::RegistryState::Ready,
                failure_code: None,
                exact_current: None,
            },
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
    ) -> WireResult<DispatchResponse> {
        let wire = cx.to_wire(accept);
        match route(raw.route).fragment {
            "registry" => match dispatch_registry(self, &wire, raw, limits).await? {
                aex_wire::dispatch::DispatchOutcome::Unary(response) => {
                    Ok(DispatchResponse::Unary(response))
                }
                aex_wire::dispatch::DispatchOutcome::Ndjson(never) => match never.0 {},
            },
            "sessions" => match Box::pin(dispatch_sessions(self, &wire, raw, limits)).await? {
                aex_wire::dispatch::DispatchOutcome::Unary(response) => {
                    Ok(DispatchResponse::Unary(response))
                }
                aex_wire::dispatch::DispatchOutcome::Ndjson(stream) => {
                    Ok(DispatchResponse::Ndjson(stream.0))
                }
            },
            "uploads" => match dispatch_uploads(self, &wire, raw, limits).await? {
                aex_wire::dispatch::DispatchOutcome::Unary(response) => {
                    Ok(DispatchResponse::Unary(response))
                }
                aex_wire::dispatch::DispatchOutcome::Ndjson(never) => match never.0 {},
            },
            _ => return Err(not_served(raw.route)),
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
    ) -> WireResult<DispatchResponse> {
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

#[cfg(test)]
mod message_stream_tests {
    use super::{AssistantObservation, assistant_observation};
    use aex_wire::ids::{MessageId, PrefixedId as _, Uuid7};
    use aex_wire::models::{TelemetryFrame, TelemetryKind};
    use aex_wire::types::{DecimalU128, Timestamp};

    fn frame(
        kind: TelemetryKind,
        event: &str,
        message: Option<MessageId>,
        preview: Option<&str>,
    ) -> TelemetryFrame {
        TelemetryFrame {
            body: Some(
                aex_wire::CanonicalJson::from_value(&serde_json::json!({
                    "event": event,
                    "scope": { "messageId": message.map(|id| id.to_string()) },
                }))
                .expect("canonical event"),
            ),
            kind,
            occurred_at: Timestamp::from_unix_millis(1).expect("timestamp"),
            preview: preview.map(str::to_owned),
            sequence: DecimalU128::new(7),
            span_id: None,
            trace_id: None,
            truncated: false,
        }
    }

    #[test]
    fn retained_assistant_events_keep_message_identity_through_reconciliation() {
        let message = MessageId::from_uuid7(Uuid7::compose(1, [2; 10]));
        assert_eq!(
            assistant_observation(&frame(
                TelemetryKind::Assistant,
                "aex.session.assistant.preview",
                Some(message),
                Some("hello"),
            )),
            Some(AssistantObservation::Preview {
                message_id: Some(message),
                text: "hello".to_owned(),
            })
        );
        assert_eq!(
            assistant_observation(&frame(
                TelemetryKind::Assistant,
                "aex.session.assistant.committed",
                Some(message),
                None,
            )),
            Some(AssistantObservation::Committed {
                message_id: message,
            })
        );
    }

    #[test]
    fn assistant_and_exporter_loss_both_become_public_gaps() {
        let message = MessageId::from_uuid7(Uuid7::compose(1, [3; 10]));
        assert_eq!(
            assistant_observation(&frame(
                TelemetryKind::Gap,
                "aex.session.assistant.gap",
                Some(message),
                None,
            )),
            Some(AssistantObservation::Gap {
                message_id: Some(message),
            })
        );
        assert_eq!(
            assistant_observation(&frame(
                TelemetryKind::Gap,
                "aex.session.telemetry.exporter_gap",
                None,
                None,
            )),
            Some(AssistantObservation::Gap { message_id: None })
        );
    }
}
