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
    AccountStateReader, AgentPage, AppContext, Clock, ContentReader, ContinuityReader, IdFactory,
    LimitsReader, LiveWorkspaceReader, PageBudget, PortError, RegistryReader, ReservationAuthority,
    ReservationGrant, ReservationRequest, SecretCustodyReader, SessionReader, SessionSnapshot,
    WorkspaceContinuity,
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
    operation: Option<Operation>,
    run: Option<Run>,
    receipt: Option<IdempotencyReceipt>,
    custody: Option<SessionCustody>,
    secrets: Vec<WorkspaceSecret>,
    true_idle: TrueIdle,
    account: AccountProjection,
    limits: EffectiveLimits,
}

impl ScriptedPorts {
    /// Scripts a live, idle session with a root agent and no operations.
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
        }
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

    /// Scripts an already admitted operation.
    #[must_use]
    pub fn with_operation(mut self, operation: Operation) -> Self {
        self.operation = Some(operation);
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
            secrets: self,
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
    ) -> Result<SessionSnapshot, PortError> {
        self.log.record(PortCall::Read("load_session"));
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
    ) -> Result<Option<Operation>, PortError> {
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
        Ok(Vec::new())
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
            generation: None,
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
