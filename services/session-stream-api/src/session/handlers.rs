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

use aex_content_domain::identity::RegistryKind;
use aex_operation_domain::operation::{OperationKind, OperationStatus};
use aex_regional_http::context::RequestContext;
use aex_regional_http::cursor::{CursorBinding, CursorKeyRing, Order, SnapshotToken, SortTuple};
use aex_regional_http::mount::{UnaryDispatch, not_served};
use aex_regional_http::projection::{
    self, ProjectionError, authority_failure, entity_tag, position_tuple, tuple_position,
};
use aex_regional_http::router::RouteOwner;
use aex_registry_dynamodb::store::{PointerPage, RegistryStore};
use aex_secret_custody_dynamodb::codec::{CredentialState, ProviderCredential as StoredCredential};
use aex_secret_custody_dynamodb::expressions;
use aex_secret_custody_dynamodb::store::SecretCustodyStore;
use aex_session_app::plan::Planned;
use aex_session_app::ports::AuthorityCommitter as _;
use aex_session_app::{SessionCommand, restore_session, stop_session, trash_session};
use aex_session_dynamodb::app_authority::{
    ApiHintSink, AuthorizedAccount, DynamoAuthorityCommitter, RequestClock, SessionCommandReads,
};
use aex_session_dynamodb::application_plan::SessionBinding;
use aex_session_dynamodb::paging::{PageBudget, PagePosition};
use aex_session_dynamodb::plan::{Participant, RegionalTables, TransactionPlan};
use aex_session_dynamodb::store::{
    OperationApiStore, OperationCancelOutcome, OperationFilter, SessionQueries, SessionScoped,
};
use aex_session_dynamodb::wire_pending::{Approval, ApprovalStatus, StoredOperation};
use aex_wire::cursor::Cursor;
use aex_wire::dispatch::{RawRequest, RawResponse, RequestLimits};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{
    OperationId, ProviderCredentialId, ResourceName, RunId, SessionId, WorkspaceId,
};
use aex_wire::models;
use aex_wire::routes::{RouteId, route};
use aex_wire::server::{
    AcceptKind, Accepted, ApprovalsApi, Created, NoContent, ProviderCredentialsApi,
    RegionalOperationsApi, RegistryApi, RequestContext as WireContext, RouteGroup, SecretsApi,
    SessionsApi, WithETag, dispatch_approvals, dispatch_provider_credentials,
    dispatch_regional_operations, dispatch_registry, dispatch_secrets, dispatch_sessions,
};
use aex_wire::types::Timestamp;

/// The adapters and start-up bindings every request shares.
///
/// One value built once by the composition root. Nothing per-request lives here,
/// which is what lets a handler be constructed for one request by cloning two
/// `Arc`s.
pub struct Shared {
    /// The ciphertext-metadata authority.
    ///
    /// Metadata only: this deployable holds no decrypt key, so the one thing it
    /// writes here is a revocation fence.
    pub custody: Arc<dyn SecretCustodyStore>,
    /// The physical `regional-secret-custody` table name.
    ///
    /// Carried because a conditional expression names its own table, and the
    /// physical name is composed by the infrastructure stream rather than
    /// guessed here.
    pub custody_table: String,
    /// The named-registry authority.
    pub registry: Arc<dyn RegistryStore>,
    /// The strongly consistent, read-only session-authority surface.
    pub sessions: Arc<dyn SessionQueries>,
    /// The durable-operation point, list and conditional cancellation authority.
    pub operations: Arc<dyn OperationApiStore>,
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
    shared: Arc<Shared>,
    cx: RequestContext,
}

impl Routes {
    /// Binds the shared adapters to one verified request.
    #[must_use]
    pub const fn new(shared: Arc<Shared>, cx: RequestContext) -> Self {
        Self { shared, cx }
    }

    /// Every route whose handler is complete, in `RouteId` order.
    ///
    /// This is the narrowing RS-22 permits and RS-18 requires. It is derived from
    /// the owned partition rather than written out, so a route that leaves this
    /// list has to leave the owned set too.
    #[must_use]
    pub fn served() -> Vec<RouteId> {
        RouteOwner::SessionApi
            .routes()
            .into_iter()
            .filter(|id| SERVED.contains(id))
            .collect()
    }
}

/// The routes this deployable can answer completely today.
///
/// Everything else it owns is absent from the router. `references/rewrite/regional-services.md`
/// records, per fragment, exactly what each remaining route is waiting for.
const SERVED: &[RouteId] = &[
    RouteId::ProviderCredentialGet,
    RouteId::ProviderCredentialRevoke,
    RouteId::ProviderCredentialsList,
    RouteId::RegionalOperationCancel,
    RouteId::RegionalOperationGet,
    RouteId::RegionalOperationsList,
    RouteId::RegistryFilesList,
    RouteId::RegistryInstructionsList,
    RouteId::RegistryMcpServersList,
    RouteId::RegistrySkillsList,
    RouteId::RegistryToolsList,
    RouteId::SecretGet,
    RouteId::SecretsList,
    RouteId::SessionApprovalGet,
    RouteId::SessionApprovalsList,
    RouteId::SessionRestore,
    RouteId::SessionRunGet,
    RouteId::SessionRunsList,
    RouteId::SessionStop,
    RouteId::SessionTrash,
];

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

    fn now(&self) -> WireResult<Timestamp> {
        self.cx
            .now()
            .map_err(|_| WireError::new(ErrorCode::InternalError))
    }

    /// The command envelope for a whole-session mutation.
    ///
    /// These three routes are keyed by `Aex-Operation-Id` rather than by an
    /// ordinary idempotency key, so the operation identity is caller-minted and
    /// the edge has already parsed it. An absent one is an invalid request, not
    /// a server-minted identity: a server-minted identity could never be
    /// replayed, which is the whole point of the header.
    fn session_command(&self, session_id: SessionId, route: RouteId) -> WireResult<SessionCommand> {
        let operation = self.cx.operation_id.ok_or_else(|| {
            WireError::new(ErrorCode::InvalidRequest).with_message("operation id")
        })?;
        Ok(SessionCommand {
            workspace: self.cx.auth.workspace_id,
            session: session_id,
            operation,
            intent: intent_of(route, self.cx.auth.workspace_id, session_id),
        })
    }

    /// The four ports this deployable supplies, plus the seven it refuses.
    fn bindings(&self) -> WireResult<CommandBindings> {
        let now = self.now()?;
        Ok(CommandBindings {
            clock: RequestClock(now),
            ids: crate::session::app_ports::RequestIds,
            unowned: crate::session::app_ports::UnownedPorts,
            reads: self.shared.commands.clone(),
            accounts: AuthorizedAccount {
                organization: self.cx.auth.organization_id,
                revision: self.cx.auth.epochs.account,
                paused: self.cx.auth.account_state
                    == aex_regional_http::context::AccountState::Paused,
                observed_at: now,
            },
        })
    }

    /// Submits one planned command and projects the operation it admitted.
    async fn admit(
        &self,
        session_id: SessionId,
        planned: Planned<aex_operation_domain::Operation>,
    ) -> WireResult<Accepted> {
        let committer = DynamoAuthorityCommitter::new(
            self.shared.authority.clone(),
            self.shared.tables.clone(),
            SessionBinding {
                workspace: self.cx.auth.workspace_id,
                organization: self.cx.auth.organization_id,
                session: session_id,
            },
            self.now()?,
            ApiHintSink,
        );
        let projected = match committer.commit(&planned.plan).await {
            Ok(_) => planned.projected,
            Err(failure) => self.recover(&failure, &planned.projected).await?,
        };
        let operation = projected
            .public()
            .map_err(|_| WireError::new(ErrorCode::InternalError))?
            .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
        Ok(Accepted(operation))
    }

    /// Resolves a refused commit against the durable facts (D-10).
    ///
    /// Asserted on the rows, never on a status code. Rows 1-6 need no extra
    /// read at all; only the two transport-ambiguous rows pay for the strongly
    /// consistent point read below, and `OperationStore` is already the
    /// strongly consistent operation authority, so there is no second reader to
    /// keep in step.
    async fn recover(
        &self,
        failure: &aex_session_app::CommitError,
        attempted: &aex_operation_domain::Operation,
    ) -> WireResult<aex_operation_domain::Operation> {
        let answer = aex_session_app::ProviderAnswer::of(failure);
        let needs_read = matches!(
            answer,
            aex_session_app::ProviderAnswer::Ambiguous(_)
                | aex_session_app::ProviderAnswer::ConditionFailed(_)
        );
        let stored = if needs_read {
            self.shared
                .operations
                .load(self.cx.auth.workspace_id, attempted.id)
                .await
                .map_err(|error| authority_failure(&error))?
                .map(|stored| stored.record)
        } else {
            None
        };
        let resolution = aex_session_app::resolve(
            &answer,
            aex_session_app::Attempted {
                workspace: self.cx.auth.workspace_id,
                operation: attempted.id,
                intent: attempted.intent,
            },
            &aex_session_app::Observed {
                operation: stored,
                // The command read the head before planning, so a session that
                // was absent would have refused before any commit.
                session_present: true,
            },
        );
        match resolution {
            // Rows 1, 3 and the unlatched half of 7: the write landed and the
            // caller is shown the stored envelope, not a second write.
            aex_session_app::Resolution::Replay(operation) => Ok(*operation),
            aex_session_app::Resolution::NotFound => Err(WireError::new(ErrorCode::NotFound)),
            aex_session_app::Resolution::IdempotencyConflict => {
                Err(WireError::new(ErrorCode::OperationIdempotencyConflict))
            }
            aex_session_app::Resolution::GuardMoved(_) => {
                Err(WireError::new(ErrorCode::PreconditionFailed))
            }
            // Row 7. The identity is stable, so re-submitting it unchanged is
            // the caller's next step and the code says exactly that.
            aex_session_app::Resolution::Resubmit => {
                Err(WireError::new(ErrorCode::CommitOutcomeUnknown))
            }
            // Row 8. Latched and stuck. The operation may never become
            // `Failed`, so the customer is told the outcome is unknown and the
            // condition is made loud for an operator.
            aex_session_app::Resolution::Quarantine => {
                eprintln!(
                    "session-stream-api: operation {} is latched and its commit outcome is \
                     unknown; it is a manual-review candidate",
                    attempted.id
                );
                Err(WireError::new(ErrorCode::CommitOutcomeUnknown))
            }
            aex_session_app::Resolution::Retry => Err(commit_failure(failure)),
        }
    }
}

/// Everything an `AppContext` borrows, owned for the length of one request.
struct CommandBindings {
    clock: RequestClock,
    ids: crate::session::app_ports::RequestIds,
    unowned: crate::session::app_ports::UnownedPorts,
    reads: SessionCommandReads,
    accounts: AuthorizedAccount,
}

impl CommandBindings {
    fn context(&self) -> aex_session_app::AppContext<'_> {
        aex_session_app::AppContext {
            clock: &self.clock,
            ids: &self.ids,
            sessions: &self.reads,
            accounts: &self.accounts,
            // Seven ports another stream owns. Every one refuses rather than
            // inventing an answer; see `crate::session::app_ports`.
            registry: &self.unowned,
            content: &self.unowned,
            secrets: &self.unowned,
            limits: &self.unowned,
            reservations: &self.unowned,
            continuity: &self.unowned,
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

/// Maps one commit refusal onto the stable public code.
fn commit_failure(error: &aex_session_app::CommitError) -> WireError {
    match error {
        // A guard the command asserted did not hold: the request was built
        // against a session that has since moved. Never a server fault.
        aex_session_app::CommitError::ConditionFailed { .. } => {
            WireError::new(ErrorCode::PreconditionFailed)
        }
        aex_session_app::CommitError::Throttled => WireError::new(ErrorCode::RateLimited),
        aex_session_app::CommitError::Unavailable => WireError::new(ErrorCode::UpstreamError),
        // Rows 6-8 of the unknown-outcome matrix. The operation identity is
        // caller-minted and stable, so the caller resolves this by polling
        // `regional_operation_get` or by re-submitting the *same* identity
        // unchanged. It must never be reported as a definite failure.
        aex_session_app::CommitError::Ambiguous { .. } => {
            WireError::new(ErrorCode::CommitOutcomeUnknown)
        }
        aex_session_app::CommitError::PlanRejected(_) => WireError::new(ErrorCode::InternalError),
    }
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

fn approval(stored: &Approval) -> models::Approval {
    let (status, decision) = match stored.status {
        ApprovalStatus::Pending => (models::ApprovalStatus::Pending, None),
        ApprovalStatus::Approved => (
            models::ApprovalStatus::Approved,
            Some(models::ApprovalDecision::Approve),
        ),
        ApprovalStatus::Denied => (
            models::ApprovalStatus::Denied,
            Some(models::ApprovalDecision::Deny),
        ),
        ApprovalStatus::Cancelled => (models::ApprovalStatus::Cancelled, None),
        ApprovalStatus::Expired => (models::ApprovalStatus::Expired, None),
    };
    models::Approval {
        bound_call: models::ApprovalBoundCall {
            agent_id: stored.binding.agent,
            arguments_digest: stored.binding.argument_digest,
            config_digest: stored.binding.config_digest,
            expected_config_revision: stored.binding.expected_config_revision,
            expected_custody_revision: stored.binding.expected_custody,
            expected_generation_id: stored.binding.expected_generation,
            implementation_digest: stored.binding.implementation_digest,
            run_id: stored.binding.run,
            tool_call_id: stored.binding.tool_call,
            tool_name: stored.binding.tool.clone(),
        },
        created_at: stored.created_at,
        decision,
        expires_at: stored.expires_at,
        id: stored.approval,
        resolved_at: stored.resolved_at,
        session_id: stored.binding.session,
        status,
    }
}

impl ApprovalsApi for Routes {
    async fn session_approval_get(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        approval_id: aex_wire::ids::ApprovalId,
    ) -> WireResult<models::Approval> {
        match self
            .shared
            .sessions
            .load_approval(self.cx.auth.workspace_id, session_id, approval_id)
            .await
            .map_err(|error| authority_failure(&error))?
        {
            SessionScoped::Missing | SessionScoped::Active(None) => {
                Err(WireError::new(ErrorCode::NotFound))
            }
            SessionScoped::Deleted => Err(WireError::new(ErrorCode::SessionDeleted)),
            SessionScoped::Active(Some(stored)) => Ok(approval(&stored)),
        }
    }

    async fn session_approval_respond(
        &self,
        _cx: &WireContext,
        _session_id: SessionId,
        _approval_id: aex_wire::ids::ApprovalId,
        _body: models::ApprovalRespondRequest,
    ) -> WireResult<models::Approval> {
        Err(not_served(RouteId::SessionApprovalRespond))
    }

    async fn session_approvals_list(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        query: models::SessionApprovalsListQuery,
    ) -> WireResult<models::ApprovalPage> {
        let budget = budget(query.limit)?;
        let (after, expected_deletion_epoch) = self
            .resume_session_collection(
                RouteId::SessionApprovalsList,
                "session.approvals",
                session_id,
                query.cursor.as_ref(),
            )
            .await?;
        let page = match self
            .shared
            .sessions
            .page_approvals(
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
            RouteId::SessionApprovalsList,
            "session.approvals",
            session_id,
            page.deletion_epoch,
        )?;
        // The pre-decode parent read and the page's own before/query/after
        // fence can observe different live generations if trash+restore races
        // this request. Rechecking the MAC binding refuses that page instead
        // of silently resuming an old generation.
        if query.cursor.is_some() {
            self.resume(query.cursor.as_ref(), &binding)?;
        }
        Ok(models::ApprovalPage {
            items: page.items.iter().map(approval).collect(),
            next_cursor: self.continuation(page.next.as_ref(), &binding)?,
        })
    }
}

impl SessionsApi for Routes {
    async fn session_clone(
        &self,
        _cx: &WireContext,
        _session_id: SessionId,
        _body: models::SessionCloneRequest,
    ) -> WireResult<Accepted> {
        Err(not_served(RouteId::SessionClone))
    }

    async fn session_create(
        &self,
        _cx: &WireContext,
        _body: models::SessionCreateRequest,
    ) -> WireResult<Created<models::Session>> {
        Err(not_served(RouteId::SessionCreate))
    }

    async fn session_credential_rebind(
        &self,
        _cx: &WireContext,
        _session_id: SessionId,
        _body: models::SessionCredentialRebindRequest,
    ) -> WireResult<Accepted> {
        Err(not_served(RouteId::SessionCredentialRebind))
    }

    async fn session_get(
        &self,
        _cx: &WireContext,
        _session_id: SessionId,
    ) -> WireResult<WithETag<models::Session>> {
        Err(not_served(RouteId::SessionGet))
    }

    async fn session_message_send(
        &self,
        _cx: &WireContext,
        _session_id: SessionId,
        _body: models::MessageSendRequest,
    ) -> WireResult<Created<models::MessageSendResult>> {
        Err(not_served(RouteId::SessionMessageSend))
    }

    async fn session_messages_list(
        &self,
        _cx: &WireContext,
        _session_id: SessionId,
        _query: models::SessionMessagesListQuery,
    ) -> WireResult<models::MessagePage> {
        Err(not_served(RouteId::SessionMessagesList))
    }

    async fn session_persist(
        &self,
        _cx: &WireContext,
        _session_id: SessionId,
        _body: models::SessionPersistRequest,
    ) -> WireResult<Accepted> {
        Err(not_served(RouteId::SessionPersist))
    }

    async fn session_purge(
        &self,
        _cx: &WireContext,
        _session_id: SessionId,
        _body: models::SessionPurgeRequest,
    ) -> WireResult<Accepted> {
        Err(not_served(RouteId::SessionPurge))
    }

    async fn session_restore(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        _body: models::EmptyRequest,
    ) -> WireResult<Accepted> {
        let command = self.session_command(session_id, RouteId::SessionRestore)?;
        let bindings = self.bindings()?;
        let planned = restore_session(&bindings.context(), &command)
            .await
            .map_err(|error| app_failure(&error))?;
        self.admit(session_id, planned).await
    }

    async fn session_run_get(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        run_id: RunId,
    ) -> WireResult<models::Run> {
        match self
            .shared
            .sessions
            .load_run(self.cx.auth.workspace_id, session_id, run_id)
            .await
            .map_err(|error| authority_failure(&error))?
        {
            SessionScoped::Missing | SessionScoped::Active(None) => {
                Err(WireError::new(ErrorCode::NotFound))
            }
            SessionScoped::Deleted => Err(WireError::new(ErrorCode::SessionDeleted)),
            SessionScoped::Active(Some(run)) => Ok(projection::session_run(&run)),
        }
    }

    async fn session_runs_list(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        query: models::SessionRunsListQuery,
    ) -> WireResult<models::RunPage> {
        let budget = budget(query.limit)?;
        let (after, expected_deletion_epoch) = self
            .resume_session_collection(
                RouteId::SessionRunsList,
                "session.runs",
                session_id,
                query.cursor.as_ref(),
            )
            .await?;
        let page = match self
            .shared
            .sessions
            .page_runs(
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
            RouteId::SessionRunsList,
            "session.runs",
            session_id,
            page.deletion_epoch,
        )?;
        if query.cursor.is_some() {
            self.resume(query.cursor.as_ref(), &binding)?;
        }
        let next = self.continuation(page.next.as_ref(), &binding)?;
        Ok(projection::session_run_page(&page.items, next))
    }

    async fn session_stop(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        _body: models::EmptyRequest,
    ) -> WireResult<Accepted> {
        let command = self.session_command(session_id, RouteId::SessionStop)?;
        let bindings = self.bindings()?;
        let planned = stop_session(&bindings.context(), &command)
            .await
            .map_err(|error| app_failure(&error))?;
        self.admit(session_id, planned).await
    }

    async fn session_trash(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        _body: models::EmptyRequest,
    ) -> WireResult<Accepted> {
        let command = self.session_command(session_id, RouteId::SessionTrash)?;
        let bindings = self.bindings()?;
        let planned = trash_session(&bindings.context(), &command)
            .await
            .map_err(|error| app_failure(&error))?;
        self.admit(session_id, planned).await
    }

    async fn session_workspace_discard(
        &self,
        _cx: &WireContext,
        _session_id: SessionId,
        _body: models::SessionWorkspaceDiscardRequest,
    ) -> WireResult<Accepted> {
        Err(not_served(RouteId::SessionWorkspaceDiscard))
    }

    async fn sessions_list(
        &self,
        _cx: &WireContext,
        _query: models::SessionsListQuery,
    ) -> WireResult<models::SessionListPage> {
        Err(not_served(RouteId::SessionsList))
    }
}

impl SecretsApi for Routes {
    async fn secret_delete(&self, _cx: &WireContext, _name: ResourceName) -> WireResult<NoContent> {
        Err(not_served(RouteId::SecretDelete))
    }

    async fn secret_get(
        &self,
        _cx: &WireContext,
        name: ResourceName,
    ) -> WireResult<WithETag<models::SecretMetadata>> {
        let stored = self
            .shared
            .custody
            .load_secret(self.cx.auth.workspace_id, &name)
            .await
            .map_err(|error| authority_failure(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        // A tombstone projects to `SecretDeleted`, which maps to `not_found`:
        // a deleted record is absent, never a `deleted` state on the wire.
        let value = projection::secret_metadata(&stored).map_err(WireError::from)?;
        let etag = entity_tag("SecretMetadata", &value).map_err(WireError::from)?;
        Ok(WithETag { value, etag })
    }

    async fn secret_put(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
        _body: models::SecretPutRequest,
    ) -> WireResult<WithETag<models::SecretMetadata>> {
        Err(not_served(RouteId::SecretPut))
    }

    async fn secret_revoke(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
        _body: models::EmptyRequest,
    ) -> WireResult<models::SecretRevocation> {
        Err(not_served(RouteId::SecretRevoke))
    }

    async fn secrets_list(
        &self,
        _cx: &WireContext,
        query: models::SecretsListQuery,
    ) -> WireResult<models::SecretMetadataPage> {
        let binding = self.cursor_binding(RouteId::SecretsList, "secrets")?;
        let after = self.resume(query.cursor.as_ref(), &binding)?;
        let page = self
            .shared
            .custody
            .page_secrets(
                self.cx.auth.workspace_id,
                budget(query.limit)?,
                after.as_ref(),
            )
            .await
            .map_err(|error| authority_failure(&error))?;
        let next = self.continuation(page.next.as_ref(), &binding)?;
        projection::secret_metadata_page(&page.items, next).map_err(WireError::from)
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
        _body: models::ProviderCredentialRegisterRequest,
    ) -> WireResult<Created<models::ProviderCredential>> {
        Err(not_served(RouteId::ProviderCredentialRegister))
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

/// The named registry: five listings served, sixteen routes absent.
///
/// The five `*_get` and five `*_put` routes are not servable from this
/// deployable's adapters, and for the same reason: a registry pointer stores a
/// `sha256` and a size, while the item form of every registry model carries the
/// `value` itself. That value is a sealed body in content storage, so returning
/// it needs the content data key. A listing is complete without it, because the
/// wire model marks `value` "omitted in collection rows"; an item read is not.
/// RS-18 is why the other sixteen are absent rather than mounted and answering
/// half a resource.
impl RegistryApi for Routes {
    async fn registry_files_delete(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<NoContent> {
        Err(not_served(RouteId::RegistryFilesDelete))
    }

    async fn registry_files_get(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<WithETag<models::RegisteredFile>> {
        Err(not_served(RouteId::RegistryFilesGet))
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
        projection::registered_file_page(&page.pointers, next).map_err(WireError::from)
    }

    async fn registry_files_put(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
        _body: models::RegisteredFileValue,
    ) -> WireResult<WithETag<models::RegisteredFile>> {
        Err(not_served(RouteId::RegistryFilesPut))
    }

    async fn registry_instructions_delete(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<NoContent> {
        Err(not_served(RouteId::RegistryInstructionsDelete))
    }

    async fn registry_instructions_get(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<WithETag<models::RegisteredInstruction>> {
        Err(not_served(RouteId::RegistryInstructionsGet))
    }

    async fn registry_instructions_list(
        &self,
        _cx: &WireContext,
        query: models::RegistryInstructionsListQuery,
    ) -> WireResult<models::RegisteredInstructionPage> {
        let (page, next) = self
            .registry_page(
                RouteId::RegistryInstructionsList,
                RegistryKind::Instruction,
                query.cursor.as_ref(),
                query.limit,
            )
            .await?;
        projection::registered_instruction_page(&page.pointers, next).map_err(WireError::from)
    }

    async fn registry_instructions_put(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
        _body: models::RegisteredInstructionValue,
    ) -> WireResult<WithETag<models::RegisteredInstruction>> {
        Err(not_served(RouteId::RegistryInstructionsPut))
    }

    async fn registry_mcp_servers_delete(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<NoContent> {
        Err(not_served(RouteId::RegistryMcpServersDelete))
    }

    async fn registry_mcp_servers_get(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<WithETag<models::RegisteredMcpServer>> {
        Err(not_served(RouteId::RegistryMcpServersGet))
    }

    async fn registry_mcp_servers_list(
        &self,
        _cx: &WireContext,
        query: models::RegistryMcpServersListQuery,
    ) -> WireResult<models::RegisteredMcpServerPage> {
        let (page, next) = self
            .registry_page(
                RouteId::RegistryMcpServersList,
                RegistryKind::McpServer,
                query.cursor.as_ref(),
                query.limit,
            )
            .await?;
        projection::registered_mcp_server_page(&page.pointers, next).map_err(WireError::from)
    }

    async fn registry_mcp_servers_put(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
        _body: models::RegisteredMcpServerValue,
    ) -> WireResult<WithETag<models::RegisteredMcpServer>> {
        Err(not_served(RouteId::RegistryMcpServersPut))
    }

    async fn registry_skills_delete(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<NoContent> {
        Err(not_served(RouteId::RegistrySkillsDelete))
    }

    async fn registry_skills_get(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<WithETag<models::RegisteredSkill>> {
        Err(not_served(RouteId::RegistrySkillsGet))
    }

    async fn registry_skills_list(
        &self,
        _cx: &WireContext,
        query: models::RegistrySkillsListQuery,
    ) -> WireResult<models::RegisteredSkillPage> {
        let (page, next) = self
            .registry_page(
                RouteId::RegistrySkillsList,
                RegistryKind::Skill,
                query.cursor.as_ref(),
                query.limit,
            )
            .await?;
        projection::registered_skill_page(&page.pointers, next).map_err(WireError::from)
    }

    async fn registry_skills_put(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
        _body: models::RegisteredSkillValue,
    ) -> WireResult<WithETag<models::RegisteredSkill>> {
        Err(not_served(RouteId::RegistrySkillsPut))
    }

    async fn registry_tools_delete(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<NoContent> {
        Err(not_served(RouteId::RegistryToolsDelete))
    }

    async fn registry_tools_get(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<WithETag<models::RegisteredTool>> {
        Err(not_served(RouteId::RegistryToolsGet))
    }

    async fn registry_tools_list(
        &self,
        _cx: &WireContext,
        query: models::RegistryToolsListQuery,
    ) -> WireResult<models::RegisteredToolPage> {
        let (page, next) = self
            .registry_page(
                RouteId::RegistryToolsList,
                RegistryKind::Tool,
                query.cursor.as_ref(),
                query.limit,
            )
            .await?;
        projection::registered_tool_page(&page.pointers, next).map_err(WireError::from)
    }

    async fn registry_tools_put(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
        _body: models::RegisteredToolValue,
    ) -> WireResult<WithETag<models::RegisteredTool>> {
        Err(not_served(RouteId::RegistryToolsPut))
    }

    async fn registry_files_download_create(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
        _body: models::RegistryDownloadRequest,
    ) -> WireResult<Created<models::DownloadGrant>> {
        Err(not_served(RouteId::RegistryFilesDownloadCreate))
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
    ) -> WireResult<RawResponse> {
        let wire = cx.to_wire(accept);
        // The group comes from the generated table, so a new fragment is a
        // non-exhaustive-match compile error rather than a runtime 404.
        let outcome = match route(raw.route).fragment {
            "secrets" => dispatch_secrets(self, &wire, raw, limits).await?,
            "provider-credentials" => {
                dispatch_provider_credentials(self, &wire, raw, limits).await?
            }
            "operations" => dispatch_regional_operations(self, &wire, raw, limits).await?,
            "registry" => dispatch_registry(self, &wire, raw, limits).await?,
            "approvals" => dispatch_approvals(self, &wire, raw, limits).await?,
            "sessions" => dispatch_sessions(self, &wire, raw, limits).await?,
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
