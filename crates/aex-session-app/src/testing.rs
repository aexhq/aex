//! Scripted ports that record every call.
//!
//! The call log is what makes "no external call inside the transaction"
//! checkable: after a use case returns, the log contains reads and nothing else,
//! and the committer is absent from [`crate::ports::AppContext`] entirely, so a
//! commit could not have happened.

use std::cell::RefCell;
use std::num::NonZeroU64;

use aex_content_domain::{
    ContentDigest, ContentOutcome, ContentRoot, PageDigest, TreeNode, TreeView,
};
use aex_operation_domain::Operation;
use aex_secret_domain::{SecretName, SessionCustody, TrueIdle, WorkspaceSecret};
use aex_session_domain::testing::{materialized_state, moment, session_fixture};
use aex_session_domain::{
    AccountProjection, AccountRevision, AccountState, AgentControl, EffectiveLimits,
    IdempotencyIdentity, IdempotencyReceipt, JournalPage, JournalSeq, ReservationId, Run, Session,
    create_root,
};
use aex_wire::ids::{
    AgentId, GenerationId, OperationId, OrganizationId, PrefixedId as _, RunId, SessionId,
    UploadId, Uuid7, WorkspaceId,
};
use aex_wire::limits::LimitId;
use aex_wire::types::Timestamp;
use aex_workspace_domain::{RegistryPointer, RegistrySelector, Upload};

use crate::ports::{
    AccountStateReader, AgentCancelPage, AgentCancelTarget, AgentPage, AppContext, Clock,
    ContentReader, ContinuityReader, IdFactory, LimitsReader, LiveEntry, LiveListQuery,
    LiveListing, LiveWorkspaceReader, PageBudget, PortError, RegistryReader, ReservationAuthority,
    ReservationGrant, ReservationRequest, SecretCustodyReader, SessionReader, SessionSnapshot,
    VersionedOperation, WorkspaceContinuity,
};

/// One recorded port interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PortCall {
    /// A read.
    Read(&'static str),
    /// A write. A use case must never produce one.
    Write(&'static str),
}

impl PortCall {
    /// Whether the call mutated anything.
    #[must_use]
    pub const fn is_write(self) -> bool {
        matches!(self, Self::Write(_))
    }
}

/// The shared call log.
#[derive(Debug, Default)]
pub struct CallLog(RefCell<Vec<PortCall>>);

impl CallLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self(RefCell::new(Vec::new()))
    }

    /// Records one call.
    pub fn record(&self, call: PortCall) {
        self.0.borrow_mut().push(call);
    }

    /// Every call, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<PortCall> {
        self.0.borrow().clone()
    }

    /// Whether any call mutated something.
    #[must_use]
    pub fn has_write(&self) -> bool {
        self.0.borrow().iter().any(|call| call.is_write())
    }
}

// `CallLog` is only ever used inside one test thread, but the port traits are
// declared `Send + Sync` because a real adapter is. The unsafe-free way to
// satisfy that is a mutex; the log is uncontended, so the cost is irrelevant.
#[derive(Debug, Default)]
struct SyncLog(std::sync::Mutex<Vec<PortCall>>);

impl SyncLog {
    fn record(&self, call: PortCall) {
        if let Ok(mut guard) = self.0.lock() {
            guard.push(call);
        }
    }

    fn calls(&self) -> Vec<PortCall> {
        self.0.lock().map(|guard| guard.clone()).unwrap_or_default()
    }
}

/// A clock that never moves.
#[derive(Debug)]
pub struct FixedClock(pub Timestamp);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// An id factory that counts.
#[derive(Debug, Default)]
pub struct CountingIds(std::sync::atomic::AtomicU64);

impl IdFactory for CountingIds {
    fn next_uuid_v7(&self) -> Uuid7 {
        let next = self
            .0
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .wrapping_add(1);
        Uuid7::compose(
            1_700_000_000_000,
            [u8::try_from(next % 251).unwrap_or(0); 10],
        )
    }
}

/// Every port, scripted from one fixture and recording every call.
#[derive(Debug)]
pub struct ScriptedPorts {
    log: SyncLog,
    snapshot: SessionSnapshot,
    operation: Option<VersionedOperation>,
    run: Option<Run>,
    receipt: Option<IdempotencyReceipt>,
    custody: Option<SessionCustody>,
    secrets: Vec<WorkspaceSecret>,
    true_idle: TrueIdle,
    account: AccountProjection,
    limits: EffectiveLimits,
    limits_revision: u64,
    credential: Option<crate::ports::ProviderCredentialBinding>,
    pointers: Vec<aex_workspace_domain::RegistryPointer>,
    sealed_root: aex_content_domain::ContentRoot,
    deployment: crate::ports::DeploymentFacts,
    qualification: Result<crate::ports::QualifiedModel, crate::ports::QualificationRefusal>,
    cancel_targets: Vec<AgentCancelTarget>,
    live_entries: Vec<LiveEntry>,
    generation: Option<GenerationId>,
}

impl ScriptedPorts {
    /// Scripts a live, idle session with a root agent and no operations.
    ///
    /// # Panics
    ///
    /// Panics if the compile-time provider-credential fixture name is invalid.
    #[must_use]
    pub fn idle() -> Self {
        let session = session_fixture();
        let root = create_root(
            AgentId::from_uuid7(Uuid7::compose(1, [4; 10])),
            &session,
            materialized_state(),
            moment(0),
        )
        .agent;
        Self {
            log: SyncLog::default(),
            account: AccountProjection {
                organization: session.organization,
                revision: AccountRevision(1),
                state: AccountState::Active,
                observed_at: moment(0),
            },
            snapshot: SessionSnapshot {
                session,
                root_agent: root,
                materialized: Vec::new(),
            },
            operation: None,
            run: None,
            receipt: None,
            custody: None,
            secrets: Vec::new(),
            true_idle: TrueIdle::idle(moment(0)),
            limits: [(LimitId::SessionMaterializedAgents, 8)]
                .into_iter()
                .collect(),
            limits_revision: 4,
            credential: Some(crate::ports::ProviderCredentialBinding {
                credential: aex_wire::ids::PrefixedId::from_uuid7(Uuid7::compose(1, [11; 10])),
                provider: aex_wire::provider::ProviderId::Openai,
                secret_name: aex_secret_domain::SecretName::parse("openai-key")
                    .expect("a fixture secret name is valid"),
                source_generation: 1,
                revision: 1,
                state: crate::ports::CredentialState::Ready,
            }),
            pointers: Vec::new(),
            sealed_root: aex_content_domain::ContentRoot {
                digest: [9; 32],
                entries: 3,
                logical_bytes: 300,
            },
            deployment: deployment_facts(),
            qualification: Ok(crate::ports::QualifiedModel {
                provider: aex_wire::provider::ProviderId::Openai,
                model: "gpt-test".to_owned(),
                catalog_revision: FIXTURE_CATALOG_REVISION.to_owned(),
            }),
            cancel_targets: Vec::new(),
            live_entries: Vec::new(),
            generation: None,
        }
    }

    /// Scripts the generation `ContinuityReader` reports as in force.
    #[must_use]
    pub const fn with_generation(mut self, generation: GenerationId) -> Self {
        self.generation = Some(generation);
        self
    }

    /// Scripts the live workspace entries a listing and a stat observe.
    ///
    /// Ordered by path on the way in, because the port contract is that a
    /// listing arrives ascending and a cursor resumes strictly after a path.
    #[must_use]
    pub fn with_live_entries(mut self, entries: impl IntoIterator<Item = LiveEntry>) -> Self {
        self.live_entries = entries.into_iter().collect();
        self.live_entries.sort();
        self
    }

    /// Scripts the workspace as having no such provider-credential binding.
    #[must_use]
    pub fn without_provider_credential(mut self) -> Self {
        self.credential = None;
        self
    }

    /// Scripts the named binding as revoked.
    #[must_use]
    pub fn with_revoked_provider_credential(mut self) -> Self {
        if let Some(credential) = self.credential.as_mut() {
            credential.state = crate::ports::CredentialState::Revoked;
        }
        self
    }

    /// Scripts the registry as holding exactly these pointers.
    #[must_use]
    pub fn with_registry_pointers(
        mut self,
        pointers: Vec<aex_workspace_domain::RegistryPointer>,
    ) -> Self {
        self.pointers = pointers;
        self
    }

    /// Scripts the seal as retaining nothing.
    #[must_use]
    pub const fn with_empty_seal(mut self) -> Self {
        self.sealed_root = aex_content_domain::ContentRoot {
            digest: [0; 32],
            entries: 0,
            logical_bytes: 0,
        };
        self
    }

    /// Scripts this plane as having no managed egress connector.
    #[must_use]
    pub const fn without_public_internet_egress(mut self) -> Self {
        self.deployment.public_internet_egress = false;
        self
    }

    /// Scripts the catalog as refusing the pair.
    #[must_use]
    pub fn with_qualification_refusal(
        mut self,
        refusal: crate::ports::QualificationRefusal,
    ) -> Self {
        self.qualification = Err(refusal);
        self
    }

    /// Scripts the workspace's effective limits.
    #[must_use]
    pub fn with_limits(mut self, limits: EffectiveLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Scripts `count` active agents, in canonical order.
    ///
    /// Ordered by identity so a page boundary is deterministic: the paged-stop
    /// property depends on the same page being re-read after a failed step.
    #[must_use]
    pub fn with_active_agents(mut self, count: u16) -> Self {
        self.cancel_targets = (0..count)
            .map(|index| AgentCancelTarget {
                agent: AgentId::from_uuid7(Uuid7::compose(
                    1_700_000_000_000 + u64::from(index),
                    [7; 10],
                )),
                revision: aex_session_domain::AgentRevision(1),
                active: true,
            })
            .collect();
        self
    }

    /// Marks every scripted agent from `settled` onwards as already terminal.
    #[must_use]
    pub fn with_settled_prefix(mut self, settled: usize) -> Self {
        for target in self.cancel_targets.iter_mut().take(settled) {
            target.active = false;
        }
        self
    }

    /// The scripted cancellation targets, in canonical order.
    #[must_use]
    pub fn cancel_targets(&self) -> &[AgentCancelTarget] {
        &self.cancel_targets
    }

    /// The same fixture with a paused account.
    #[must_use]
    pub fn paused(mut self) -> Self {
        self.account.state = AccountState::Paused {
            reason: aex_session_domain::PauseReason::TopUpRequired,
        };
        self
    }

    /// Replaces the scripted session.
    #[must_use]
    pub fn with_session(mut self, session: Session) -> Self {
        self.snapshot.session = session;
        self
    }

    /// Scripts an already admitted operation at the version a fresh admission
    /// leaves it at.
    #[must_use]
    pub fn with_operation(mut self, operation: Operation) -> Self {
        self.operation = Some(VersionedOperation {
            operation,
            version: aex_operation_domain::operation::OperationVersion::FIRST,
        });
        self
    }

    /// Scripts an already admitted operation at an exact stored version.
    ///
    /// Present because a resumed step conditions on the version it read, so a
    /// property that never varied the version could not tell a step that names
    /// its version from one that hard-codes the first.
    #[must_use]
    pub fn with_operation_at(
        mut self,
        operation: Operation,
        version: aex_operation_domain::operation::OperationVersion,
    ) -> Self {
        self.operation = Some(VersionedOperation { operation, version });
        self
    }

    /// Scripts a stored run.
    #[must_use]
    pub fn with_run(mut self, run: Run) -> Self {
        self.run = Some(run);
        self
    }

    /// Scripts an existing idempotency receipt.
    #[must_use]
    pub fn with_receipt(mut self, receipt: IdempotencyReceipt) -> Self {
        self.receipt = Some(receipt);
        self
    }

    /// Scripts the workspace secrets returned by the custody reader.
    #[must_use]
    pub fn with_secrets(mut self, secrets: Vec<WorkspaceSecret>) -> Self {
        self.secrets = secrets;
        self
    }

    /// Scripts the session custody row returned by the custody reader.
    #[must_use]
    pub fn with_custody(mut self, custody: SessionCustody) -> Self {
        self.custody = Some(custody);
        self
    }

    /// Scripts the runtime authority's true-idle verdict.
    #[must_use]
    pub fn with_true_idle(mut self, true_idle: TrueIdle) -> Self {
        self.true_idle = true_idle;
        self
    }

    /// The scripted session.
    #[must_use]
    pub const fn session(&self) -> &Session {
        &self.snapshot.session
    }

    /// The scripted root agent.
    #[must_use]
    pub const fn root_agent(&self) -> &AgentControl {
        &self.snapshot.root_agent
    }

    /// Every call the ports recorded, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<PortCall> {
        self.log.calls()
    }

    /// Whether any port call mutated anything.
    #[must_use]
    pub fn recorded_a_write(&self) -> bool {
        self.log.calls().iter().any(|call| call.is_write())
    }

    /// A context bound to these ports.
    ///
    /// Note what is absent: there is no committer, so a use case built against
    /// this context cannot commit even by accident.
    #[must_use]
    pub fn context<'a>(&'a self, clock: &'a FixedClock, ids: &'a CountingIds) -> AppContext<'a> {
        AppContext {
            clock,
            ids,
            sessions: self,
            registry: self,
            content: self,
            content_writer: self,
            secrets: self,
            catalog: self,
            deployment: &self.deployment,
            limits: self,
            accounts: self,
            reservations: self,
            continuity: self,
            live: self,
        }
    }
}

#[async_trait::async_trait]
impl SessionReader for ScriptedPorts {
    async fn load_session(
        &self,
        _workspace: WorkspaceId,
        _session: SessionId,
    ) -> Result<Session, PortError> {
        self.log.record(PortCall::Read("load_session"));
        Ok(self.snapshot.session.clone())
    }

    async fn load_snapshot(
        &self,
        _workspace: WorkspaceId,
        _session: SessionId,
    ) -> Result<SessionSnapshot, PortError> {
        self.log.record(PortCall::Read("load_snapshot"));
        Ok(self.snapshot.clone())
    }

    async fn load_run(&self, _session: SessionId, run: RunId) -> Result<Run, PortError> {
        self.log.record(PortCall::Read("load_run"));
        self.run
            .clone()
            .map(|value| Run { id: run, ..value })
            .ok_or(PortError::NotFound { kind: "run" })
    }

    async fn load_agent(
        &self,
        _session: SessionId,
        _agent: AgentId,
    ) -> Result<AgentControl, PortError> {
        self.log.record(PortCall::Read("load_agent"));
        Ok(self.snapshot.root_agent.clone())
    }

    async fn list_agents(
        &self,
        _session: SessionId,
        _budget: PageBudget,
    ) -> Result<AgentPage, PortError> {
        self.log.record(PortCall::Read("list_agents"));
        Ok(AgentPage {
            agents: self.snapshot.materialized.clone(),
            more: false,
        })
    }

    async fn list_agent_cancel_targets(
        &self,
        _session: SessionId,
        from: Option<AgentId>,
        budget: PageBudget,
    ) -> Result<AgentCancelPage, PortError> {
        self.log.record(PortCall::Read("list_agent_cancel_targets"));
        let start = from.map_or(0, |first| {
            self.cancel_targets
                .iter()
                .position(|target| target.agent >= first)
                .unwrap_or(self.cancel_targets.len())
        });
        let limit = usize::from(budget.limit);
        let end = start.saturating_add(limit).min(self.cancel_targets.len());
        Ok(AgentCancelPage {
            targets: self.cancel_targets[start..end].to_vec(),
            next: self.cancel_targets.get(end).map(|target| target.agent),
        })
    }

    async fn load_journal_page(
        &self,
        _session: SessionId,
        agent: AgentId,
        from: JournalSeq,
        _budget: PageBudget,
    ) -> Result<JournalPage, PortError> {
        self.log.record(PortCall::Read("load_journal_page"));
        Ok(JournalPage {
            agent,
            first: from,
            entries: Vec::new(),
        })
    }

    async fn load_receipt(
        &self,
        _identity: &IdempotencyIdentity,
    ) -> Result<Option<IdempotencyReceipt>, PortError> {
        self.log.record(PortCall::Read("load_receipt"));
        Ok(self.receipt.clone())
    }

    async fn load_operation(
        &self,
        _workspace: WorkspaceId,
        _operation: OperationId,
    ) -> Result<Option<VersionedOperation>, PortError> {
        self.log.record(PortCall::Read("load_operation"));
        Ok(self.operation.clone())
    }
}

#[async_trait::async_trait]
impl RegistryReader for ScriptedPorts {
    async fn read_many(
        &self,
        _workspace: WorkspaceId,
        _selectors: &[RegistrySelector],
    ) -> Result<Vec<RegistryPointer>, PortError> {
        self.log.record(PortCall::Read("read_many"));
        Ok(self.pointers.clone())
    }

    async fn read_upload(
        &self,
        _workspace: WorkspaceId,
        _upload: UploadId,
    ) -> Result<Upload, PortError> {
        self.log.record(PortCall::Read("read_upload"));
        Err(PortError::NotFound { kind: "upload" })
    }
}

#[async_trait::async_trait]
impl ContentReader for ScriptedPorts {
    async fn describe(
        &self,
        _workspace: WorkspaceId,
        _digest: ContentDigest,
    ) -> Result<ContentOutcome, PortError> {
        self.log.record(PortCall::Read("describe"));
        Err(PortError::NotFound { kind: "content" })
    }

    async fn load_page(
        &self,
        _workspace: WorkspaceId,
        _page: PageDigest,
    ) -> Result<TreeNode, PortError> {
        self.log.record(PortCall::Read("load_page"));
        Err(PortError::NotFound { kind: "page" })
    }
}

#[async_trait::async_trait]
impl SecretCustodyReader for ScriptedPorts {
    async fn read_secrets(
        &self,
        _workspace: WorkspaceId,
        names: &[SecretName],
    ) -> Result<Vec<WorkspaceSecret>, PortError> {
        self.log.record(PortCall::Read("read_secrets"));
        Ok(self
            .secrets
            .iter()
            .filter(|secret| names.contains(&secret.name))
            .cloned()
            .collect())
    }

    async fn read_custody(&self, _session: SessionId) -> Result<Option<SessionCustody>, PortError> {
        self.log.record(PortCall::Read("read_custody"));
        Ok(self.custody.clone())
    }

    async fn read_provider_credential(
        &self,
        _workspace: WorkspaceId,
        _credential: aex_wire::ids::ProviderCredentialId,
    ) -> Result<Option<crate::ports::ProviderCredentialBinding>, PortError> {
        self.log.record(PortCall::Read("read_provider_credential"));
        Ok(self.credential.clone())
    }
}

#[async_trait::async_trait]
impl LimitsReader for ScriptedPorts {
    async fn effective(
        &self,
        _workspace: WorkspaceId,
        _ids: &[LimitId],
    ) -> Result<EffectiveLimits, PortError> {
        self.log.record(PortCall::Read("effective"));
        Ok(self.limits.clone())
    }

    async fn bundle(
        &self,
        _workspace: WorkspaceId,
    ) -> Result<crate::ports::LimitsBundle, PortError> {
        self.log.record(PortCall::Read("limits_bundle"));
        Ok(crate::ports::LimitsBundle {
            revision: self.limits_revision,
            limits: self.limits.clone(),
        })
    }
}

#[async_trait::async_trait]
impl AccountStateReader for ScriptedPorts {
    async fn projection(
        &self,
        _organization: OrganizationId,
    ) -> Result<AccountProjection, PortError> {
        self.log.record(PortCall::Read("projection"));
        Ok(self.account)
    }
}

#[async_trait::async_trait]
impl ReservationAuthority for ScriptedPorts {
    async fn prepare(&self, request: ReservationRequest) -> Result<ReservationGrant, PortError> {
        // A reservation is a read of the finance authority from this crate's
        // point of view: it never writes the session's own tables.
        self.log.record(PortCall::Read("prepare_reservation"));
        Ok(ReservationGrant {
            reservation: ReservationId(Uuid7::compose(1, [5; 10])),
            granted_cents: request.max_spend_cents,
        })
    }
}

#[async_trait::async_trait]
impl ContinuityReader for ScriptedPorts {
    async fn continuity(&self, _session: SessionId) -> Result<WorkspaceContinuity, PortError> {
        self.log.record(PortCall::Read("continuity"));
        Ok(WorkspaceContinuity {
            generation: self.generation,
            intact: true,
        })
    }

    async fn true_idle(&self, _session: SessionId) -> Result<TrueIdle, PortError> {
        self.log.record(PortCall::Read("true_idle"));
        Ok(self.true_idle)
    }
}

#[async_trait::async_trait]
impl LiveWorkspaceReader for ScriptedPorts {
    async fn scan(
        &self,
        _session: SessionId,
        _generation: GenerationId,
    ) -> Result<TreeView, PortError> {
        self.log.record(PortCall::Read("scan"));
        TreeView::build(self.snapshot.session.workspace, &[]).map_err(|_| PortError::Corrupt {
            kind: "live tree",
            reason: "an empty tree always builds",
        })
    }

    async fn root(
        &self,
        _session: SessionId,
        _generation: GenerationId,
    ) -> Result<ContentRoot, PortError> {
        self.log.record(PortCall::Read("live_root"));
        Ok(aex_content_domain::empty_root(
            self.snapshot.session.workspace,
        ))
    }

    async fn list(
        &self,
        _session: SessionId,
        _generation: GenerationId,
        query: &LiveListQuery,
    ) -> Result<LiveListing, PortError> {
        self.log.record(PortCall::Read("live_list"));
        let prefix = query
            .path
            .clone()
            .unwrap_or_else(|| "/workspace".to_owned());
        let mut matching = self
            .live_entries
            .iter()
            .filter(|entry| {
                entry
                    .path
                    .starts_with(&format!("{}/", prefix.trim_end_matches('/')))
            })
            .filter(|entry| {
                query
                    .after
                    .as_ref()
                    .is_none_or(|after| entry.path.as_str() > after.as_str())
            });
        let limit = usize::from(query.limit);
        let entries: Vec<LiveEntry> = matching.by_ref().take(limit).cloned().collect();
        let next_after = matching
            .next()
            .and_then(|_| entries.last().map(|entry| entry.path.clone()));
        Ok(LiveListing {
            entries,
            next_after,
        })
    }

    async fn stat(
        &self,
        _session: SessionId,
        _generation: GenerationId,
        path: &str,
    ) -> Result<LiveEntry, PortError> {
        self.log.record(PortCall::Read("live_stat"));
        self.live_entries
            .iter()
            .find(|entry| entry.path == path)
            .cloned()
            .ok_or(PortError::NotFound {
                kind: "live workspace entry",
            })
    }
}

/// The spend ceiling the fixtures use.
///
/// # Panics
///
/// Never: the literal is non-zero.
#[must_use]
pub fn fixture_spend() -> NonZeroU64 {
    NonZeroU64::new(500).unwrap_or_else(|| unreachable!("500 is non-zero"))
}

/// The `mc1_<hex>` rendering every fixture qualifies under.
pub const FIXTURE_CATALOG_REVISION: &str =
    "mc1_0000000000000000000000000000000000000000000000000000000000000000";

/// A deployment that has egress and every published ecosystem.
///
/// # Panics
///
/// Panics only if the compiled catalog literal below is not a valid catalog,
/// which is a broken fixture rather than a reachable condition.
#[must_use]
pub fn deployment_facts() -> crate::ports::DeploymentFacts {
    use aex_runtime_control::catalog::{HandsImageCatalog, HandsImageCatalogEntry};
    use aex_runtime_control::generation::{ImageIdentifier, ImageVersion};
    use aex_runtime_control::shape::ShapeCapacity as _;

    let entries = aex_wire::types::ComputeSize::ALL
        .into_iter()
        .map(|size| {
            (
                size.as_str().to_owned(),
                HandsImageCatalogEntry {
                    image_arn: ImageIdentifier(format!("aex-hands-{}", size.as_str())),
                    image_version: ImageVersion("1".to_owned()),
                    artifact_digest: aex_wire::ids::ContentHash::from_bytes([7; 32]),
                    minimum_memory_mib: size.minimum_memory_mib(),
                    browser: false,
                },
            )
        })
        .collect();
    crate::ports::DeploymentFacts {
        public_internet_egress: true,
        images: HandsImageCatalog::from_entries(entries).expect("a fixture catalog is complete"),
        package_ecosystems: [
            aex_wire::models::PackageEcosystem::Apt,
            aex_wire::models::PackageEcosystem::Pip,
            aex_wire::models::PackageEcosystem::Npm,
        ]
        .into_iter()
        .collect(),
    }
}

/// The root agent a create writes for `session`.
#[must_use]
pub fn root_agent_of(session: &Session) -> aex_session_domain::AgentControl {
    aex_session_domain::AgentControl {
        id: session.root_agent,
        session: session.id,
        kind: aex_session_domain::AgentKind::Root,
        parent: None,
        depth: 0,
        status: aex_session_domain::AgentStatus::Idle,
        revision: aex_session_domain::AgentRevision::INITIAL,
        journal_tail: aex_session_domain::JournalSeq::INITIAL,
        last_entry: None,
        claim: None,
        join: None,
        budget: None,
        open_effects: aex_session_domain::OpenEffectSet::default(),
        pending_approval: None,
        queue_reason: None,
        generation: Some(session.pinned_runtime.generation()),
        terminal: None,
        created_at: session.created_at,
    }
}

/// The create receipt for `session`, under a fixed caller key.
///
/// # Panics
///
/// Panics only if a fixture literal is not a usable idempotency key or scope.
#[must_use]
pub fn create_receipt(session: &Session) -> aex_session_domain::IdempotencyReceipt {
    let identity = create_identity(session);
    aex_session_domain::IdempotencyReceipt {
        key: aex_session_domain::ReceiptKey::of(crate::CREATE_SCOPE, &identity)
            .expect("the create scope is usable"),
        intent: identity.intent(),
        identity,
        outcome: aex_session_domain::ReceiptOutcome::Resource {
            kind: aex_session_domain::ResourceKind::Session,
            id: aex_session_domain::ResourceId(session.id.to_string()),
            response: aex_session_domain::ResponseBody::of(
                &crate::projection::canonical_session_bytes(session)
                    .expect("a fixture session projects"),
            ),
        },
        created_at: session.created_at,
    }
}

/// The replay envelope a fixture create arrives under.
///
/// # Panics
///
/// Panics only if the fixture key literal is not a valid `Idempotency-Key`.
#[must_use]
pub fn create_identity(session: &Session) -> aex_session_domain::IdempotencyIdentity {
    create_identity_under("fixture-create-key", session)
}

/// The same envelope under an exact caller key.
///
/// # Panics
///
/// Panics only if `key` is not a valid `Idempotency-Key`.
#[must_use]
pub fn create_identity_under(
    key: &str,
    session: &Session,
) -> aex_session_domain::IdempotencyIdentity {
    aex_session_domain::IdempotencyIdentity::Key(Box::new(aex_wire::idempotency::ReplayIdentity {
        principal: aex_wire::idempotency::PrincipalScope::WorkspaceKey {
            key: aex_wire::ids::PrefixedId::from_uuid7(Uuid7::compose(1, [12; 10])),
            workspace: session.workspace,
            organization: session.organization,
        },
        route: aex_wire::routes::route(aex_wire::routes::RouteId::SessionCreate).id,
        key: aex_wire::idempotency::IdempotencyKey::parse(key).expect("a fixture key is valid"),
        intent: aex_wire::canonical::intent_digest(
            aex_wire::routes::route(aex_wire::routes::RouteId::SessionCreate).id,
            &aex_wire::routes::PathBinding::default(),
            None,
        ),
    }))
}

#[async_trait::async_trait]
impl crate::ports::ContentWriter for ScriptedPorts {
    async fn seal_registry_manifest(
        &self,
        _workspace: WorkspaceId,
        _entries: &[crate::ports::SealedRegistryEntry],
    ) -> Result<ContentRoot, PortError> {
        self.log.record(PortCall::Write("seal_registry_manifest"));
        Ok(self.sealed_root)
    }
}

impl crate::ports::ModelQualifier for ScriptedPorts {
    fn admit(
        &self,
        _provider: aex_wire::provider::ProviderId,
        _model: &str,
    ) -> Result<crate::ports::QualifiedModel, crate::ports::QualificationRefusal> {
        self.qualification.clone()
    }
}
