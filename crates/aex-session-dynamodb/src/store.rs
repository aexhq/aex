//! The `session-authority` port implementations.
//!
//! Reads are strongly consistent without exception: an authority that answers
//! from a replica cannot fence anything, and every read here feeds a condition
//! that will later be committed against. Writes go through
//! [`crate::plan::TransactionPlan`], so no method in this module can issue an
//! unconditional authority write.

use aex_wire::idempotency::IdempotencyKey;
use aex_wire::ids::{AgentId, ApprovalId, OperationId, RunId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;

use crate::attr::{CodecError, Item};
use crate::codec;
use crate::error::{
    Idempotence, Resolution, StoreError, classify, decode_cancellation,
    decode_cancellation_with_resolution,
};
use crate::keys;
use crate::paging::{CursorBinding, CursorError, CursorKey, PageBudget, PagePosition};
use crate::plan::{RegionalTables, TransactionPlan, key};
use crate::replay::{IdempotencyScope, Receipt, ReceiptStore, key_digest};
use crate::transactions::{
    AdmissionForeign, Foreign, TerminalForeign, compile_admission, compile_decision,
    compile_fanout_page, compile_lifecycle, compile_terminal,
};
use crate::wire_pending::{
    AdmissionPlan, AgentControl, AgentDecisionPlan, Approval, FanoutPagePlan, JournalEntry,
    LifecyclePlan, Run, SessionHead, StoredOperation, TerminalPlan,
};

/// One page of decoded rows plus its continuation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The rows.
    pub items: Vec<T>,
    /// Where to resume, when the collection continues.
    pub next: Option<aex_wire::cursor::Cursor>,
}

// TODO(cross-stream): `aex-session-app` publishes no `SessionAuthority`. Its commit-side
// port is `aex_session_app::ports::AuthorityCommitter`, which takes an
// `aex_session_app::plan::SessionTransaction` and returns `ports::CommitOutcome`; its
// read side is `ports::SessionReader`.
/// The session authority.
#[async_trait]
pub trait SessionAuthority: Send + Sync + 'static {
    /// Commits the one public admission transaction.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming the participant that lost, and
    /// [`StoreError::CommitAmbiguous`] when the outcome is unknown — which the
    /// caller resolves by reading the receipt and never by writing again.
    async fn admit_message_and_run(
        &self,
        plan: &AdmissionPlan,
        foreign: AdmissionForeign,
    ) -> Result<(), StoreError>;

    /// Commits the run terminal barrier.
    ///
    /// # Errors
    ///
    /// As above.
    async fn commit_run_terminal(
        &self,
        plan: &TerminalPlan,
        foreign: TerminalForeign,
    ) -> Result<(), StoreError>;

    /// Commits a trash, restore, purge admission or purge completion.
    ///
    /// # Errors
    ///
    /// As above.
    async fn commit_lifecycle(
        &self,
        plan: &LifecyclePlan,
        foreign: Vec<Foreign>,
    ) -> Result<(), StoreError>;

    /// Reads one session head.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure. An absent head is
    /// `Ok(None)`; a purged head is a decoded head with `lifecycle = purged`,
    /// because one point read has to serve `200`, `410 session_deleting` and
    /// `410 session_deleted`.
    async fn load_head(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<SessionHead>, StoreError>;

    /// Reads one run.
    ///
    /// # Errors
    ///
    /// As [`SessionAuthority::load_head`].
    async fn load_run(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        run: RunId,
    ) -> Result<Option<Run>, StoreError>;
}

// TODO(cross-stream): `aex-session-app` publishes no journal store port. Journal reads are
// `aex_session_app::ports::SessionReader`, and journal writes are ordinary writes inside an
// `aex_session_app::plan::SessionTransaction`.
/// The agent journal, consumed by Brain through `aex-brain-store-aws`.
#[async_trait]
pub trait AgentJournalStore: Send + Sync + 'static {
    /// Commits one agent decision.
    ///
    /// # Errors
    ///
    /// As [`SessionAuthority::admit_message_and_run`].
    async fn commit_decision(
        &self,
        plan: &AgentDecisionPlan,
        next_wake: Option<Foreign>,
    ) -> Result<(), StoreError>;

    /// Commits one bounded fanout page.
    ///
    /// # Errors
    ///
    /// As above, plus [`StoreError::PreconditionFailed`] on the budget
    /// condition when the effective agent limit is exhausted.
    async fn commit_fanout_page(
        &self,
        plan: &FanoutPagePlan,
        wakes: Vec<Foreign>,
    ) -> Result<(), StoreError>;

    /// Reads one agent's control item.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn load_control(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        agent: AgentId,
    ) -> Result<Option<AgentControl>, StoreError>;

    /// Reads a bounded page of journal entries from `from_seq`.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn read_journal(
        &self,
        session: SessionId,
        agent: AgentId,
        from_seq: u64,
        budget: PageBudget,
    ) -> Result<Vec<JournalEntry>, StoreError>;
}

/// One bounded page of decoded rows plus the position a continuation resumes
/// from.
///
/// `next` is `Some` exactly when the authority reported more rows behind this
/// page. The position is returned rather than a token because the regional edge
/// owns the one signed cursor codec: a second minting path here would be a
/// second, subtly different continuation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionPage<T> {
    /// The rows, in key order.
    pub items: Vec<T>,
    /// Where the next page starts, when there is one.
    pub next: Option<PagePosition>,
}

/// The narrow durable-operation authority used by continuation workers and
/// operation point reads.
///
/// It is deliberately separate from [`SessionAuthority`]: a worker that only
/// needs to reload an operation before a fenced step must not acquire the
/// session transaction surface as a side effect.
#[async_trait]
pub trait OperationAuthority: Send + Sync + 'static {
    /// Strongly reads one operation under its asserted tenant.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or strict decode failure.
    async fn load(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<StoredOperation>, StoreError>;
}

/// Durable-operation reader and fenced step committer.
#[derive(Debug, Clone)]
pub struct OperationStore {
    client: Client,
    table: String,
}

impl OperationStore {
    /// Binds the reader to the physical `session-authority` table.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    /// The physical authority table.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Commits exactly the operation/work pair for an observed cancellation
    /// step and preserves its named cancellation reasons.
    ///
    /// # Errors
    ///
    /// Every [`StoreError`]. A transport failure is commit-ambiguous and must
    /// be resolved by strongly reading the operation and work target rows; the
    /// caller must never issue the write blindly a second time.
    pub async fn commit_cancelled_step(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        if plan.participants()
            != [
                crate::plan::Participant::SESSION_OPERATION,
                crate::plan::Participant::WORK_WAKE_DONE,
            ]
        {
            return Err(StoreError::Invalid {
                detail: "a cancelled operation step must update operation then fenced work"
                    .to_owned(),
            });
        }
        let request = plan.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(service) = error.as_service_error() {
                    return Err(decode_cancellation_with_resolution(
                        service,
                        plan.participants(),
                        Resolution::TargetItem,
                    ));
                }
                Err(classify(&error, Idempotence::Write(Resolution::TargetItem)))
            }
        }
    }
}

#[async_trait]
impl OperationAuthority for OperationStore {
    async fn load(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<StoredOperation>, StoreError> {
        let operation_key = keys::operation(operation);
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(&operation_key.pk, &operation_key.sk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        match output.item {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_operation(&item, workspace)?)),
        }
    }
}

/// The read-only `session-authority` surface the finite regional API composes.
///
/// Separate from [`SessionAuthority`] because it is a different capability
/// statement: there is no method here that commits anything, so a deployable
/// that holds only this port cannot write the session table however it is
/// wired. Both surfaces share [`crate::codec`] and [`crate::keys`], so the two
/// cannot disagree about row shape.
#[async_trait]
pub trait SessionQueries: Send + Sync + 'static {
    /// Reads one session head.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure. An absent head is
    /// `Ok(None)`; a purged head decodes with `lifecycle = purged`, because one
    /// point read has to serve `200`, `410 session_deleting` and
    /// `410 session_deleted`.
    async fn read_head(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<SessionHead>, StoreError>;

    /// Reads one approval.
    ///
    /// # Errors
    ///
    /// As [`SessionQueries::read_head`].
    async fn load_approval(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        approval: ApprovalId,
    ) -> Result<Option<Approval>, StoreError>;

    /// Lists one session's approvals, oldest first, from a continuation.
    ///
    /// # Errors
    ///
    /// As [`SessionQueries::read_head`].
    async fn page_approvals(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<PositionPage<Approval>, StoreError>;
}

/// The read-only adapter.
///
/// It holds a client and a table name and nothing else — in particular no
/// cursor key, because it mints no token, and no transaction compiler, because
/// it commits nothing.
#[derive(Debug, Clone)]
pub struct SessionReads {
    client: Client,
    table: String,
}

impl SessionReads {
    /// Binds a reader to a client and the physical `session-authority` name.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    /// The physical table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    async fn get(&self, pk: &str, sk: &str) -> Result<Option<Item>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(pk, sk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        Ok(output.item)
    }
}

fn approval_for_session(
    item: &Item,
    workspace: WorkspaceId,
    session: SessionId,
) -> Result<Approval, StoreError> {
    let approval = codec::decode_approval(item, workspace)?;
    if approval.binding.session != session {
        return Err(StoreError::Corrupt(CodecError::Malformed {
            item_type: codec::APPROVAL,
            attribute: "sessionId",
            reason: "does not match the session partition queried".to_owned(),
        }));
    }
    Ok(approval)
}

#[async_trait]
impl SessionQueries for SessionReads {
    async fn read_head(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<SessionHead>, StoreError> {
        let key = keys::head(session);
        match self.get(&key.pk, &key.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_head(&item, workspace)?)),
        }
    }

    async fn load_approval(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        approval: ApprovalId,
    ) -> Result<Option<Approval>, StoreError> {
        let key = keys::approval(session, approval);
        match self.get(&key.pk, &key.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(approval_for_session(&item, workspace, session)?)),
        }
    }

    async fn page_approvals(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<PositionPage<Approval>, StoreError> {
        // One `begins_with` range inside the session partition. No index and no
        // filter expression: an approval that belongs to another session is in
        // another partition, so it is unreachable rather than filtered out.
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
            .expression_attribute_names("#pk", crate::attr::PK)
            .expression_attribute_names("#sk", crate::attr::SK)
            .expression_attribute_values(":pk", crate::attr::s(keys::session_partition(session)))
            .expression_attribute_values(":prefix", crate::attr::s(keys::approval_prefix()))
            .limit(budget.limit())
            .consistent_read(true)
            .set_exclusive_start_key(after.map(|position| position.to_exclusive_start(None, None)))
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;

        let mut items = Vec::new();
        for item in output.items.unwrap_or_default() {
            items.push(approval_for_session(&item, workspace, session)?);
        }
        let next = output
            .last_evaluated_key
            .as_ref()
            .map(|last| PagePosition::from_last_evaluated(last, None, None))
            .transpose()
            .map_err(|error| cursor_error(&error))?;
        Ok(PositionPage { items, next })
    }
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct SessionStore {
    client: Client,
    tables: RegionalTables,
    cursor_key: CursorKey,
}

impl SessionStore {
    /// Binds a store to a client, the physical table names and a cursor key.
    #[must_use]
    pub fn new(client: Client, tables: RegionalTables, cursor_key: CursorKey) -> Self {
        Self {
            client,
            tables,
            cursor_key,
        }
    }

    /// The physical `session-authority` table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.tables.session_authority
    }

    /// The regional table names this store compiles against.
    #[must_use]
    pub const fn tables(&self) -> &RegionalTables {
        &self.tables
    }

    async fn get(&self, pk: &str, sk: &str) -> Result<Option<Item>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(self.table())
            .set_key(Some(key(pk, sk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        Ok(output.item)
    }

    /// Commits a compiled plan and decodes a cancellation into a
    /// participant-named error.
    ///
    /// # Errors
    ///
    /// Every [`StoreError`]. A transport failure on a transaction is always
    /// [`StoreError::CommitAmbiguous`], never a silent retry.
    pub async fn commit(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        let request = plan.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(service) = error.as_service_error() {
                    return Err(decode_cancellation(service, plan.participants()));
                }
                Err(classify(
                    &error,
                    Idempotence::Write(Resolution::IdempotencyReceipt),
                ))
            }
        }
    }

    /// Lists one page of session heads over the sparse workspace index.
    ///
    /// `lifecycle` selects the index partition, so the query needs no filter
    /// expression and a purged session is physically absent rather than
    /// filtered out.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure, and
    /// [`StoreError::Invalid`] for a cursor that is not bound to this
    /// collection.
    pub async fn list_sessions(
        &self,
        binding: CursorBinding<'_>,
        lifecycle: &str,
        budget: PageBudget,
        cursor: Option<&aex_wire::cursor::Cursor>,
        now: Timestamp,
    ) -> Result<Page<SessionListEntry>, StoreError> {
        let start = self.resume(binding, cursor, now)?;
        let output = self
            .client
            .query()
            .table_name(self.table())
            .index_name(keys::workspace_index::NAME)
            .key_condition_expression("#pk = :pk")
            .expression_attribute_names("#pk", keys::workspace_index::PK)
            .expression_attribute_values(
                ":pk",
                crate::attr::s(keys::workspace_index::session_partition(
                    binding.workspace,
                    lifecycle,
                )),
            )
            .limit(budget.limit())
            .set_exclusive_start_key(start.map(|position| {
                position.to_exclusive_start(
                    Some(keys::workspace_index::PK),
                    Some(keys::workspace_index::SK),
                )
            }))
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;

        let mut items = Vec::new();
        for item in output.items.unwrap_or_default() {
            items.push(decode_list_entry(&item, binding.workspace)?);
        }
        let next = self.continuation(binding, output.last_evaluated_key.as_ref(), now)?;
        Ok(Page { items, next })
    }

    fn resume(
        &self,
        binding: CursorBinding<'_>,
        cursor: Option<&aex_wire::cursor::Cursor>,
        now: Timestamp,
    ) -> Result<Option<PagePosition>, StoreError> {
        cursor
            .map(|cursor| crate::paging::verify(&self.cursor_key, binding, cursor, now))
            .transpose()
            .map_err(|error| cursor_error(&error))
    }

    fn continuation(
        &self,
        binding: CursorBinding<'_>,
        last: Option<&Item>,
        now: Timestamp,
    ) -> Result<Option<aex_wire::cursor::Cursor>, StoreError> {
        let Some(last) = last else {
            return Ok(None);
        };
        let position = PagePosition::from_last_evaluated(
            last,
            Some(keys::workspace_index::PK),
            Some(keys::workspace_index::SK),
        )
        .map_err(|error| cursor_error(&error))?;
        crate::paging::mint(&self.cursor_key, binding, &position, now)
            .map(Some)
            .map_err(|error| cursor_error(&error))
    }
}

fn cursor_error(error: &CursorError) -> StoreError {
    StoreError::Invalid {
        detail: error.to_string(),
    }
}

/// One row of a session list, as the slim index projection carries it.
///
/// The projection deliberately does not carry a prompt, a resolved config or a
/// receipt, so this type has nowhere to put one even if a row grew an attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListEntry {
    /// Which session.
    pub session: SessionId,
    /// Its status.
    pub status: String,
    /// Its lifecycle.
    pub lifecycle: String,
    /// Its revision.
    pub revision: u64,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
}

/// Decodes one projected index row into a list entry.
///
/// # Errors
///
/// [`crate::attr::CodecError`] for a row that is not a session head, is missing
/// a projected attribute, or belongs to another workspace.
pub fn decode_list_entry(
    item: &Item,
    asserted: WorkspaceId,
) -> Result<SessionListEntry, crate::attr::CodecError> {
    let row = crate::attr::Row::bind(item, codec::SESSION_HEAD)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(SessionListEntry {
        session: row.id::<SessionId>("sessionId")?,
        status: row.enumerated("status", keys::STATUSES)?.to_owned(),
        lifecycle: row.enumerated("lifecycle", keys::LIFECYCLES)?.to_owned(),
        revision: row.u64("revision")?,
        created_at: row.timestamp("createdAt")?,
        updated_at: row.timestamp("updatedAt")?,
    })
}

#[async_trait]
impl SessionAuthority for SessionStore {
    async fn admit_message_and_run(
        &self,
        plan: &AdmissionPlan,
        foreign: AdmissionForeign,
    ) -> Result<(), StoreError> {
        let compiled = compile_admission(&self.tables, plan, foreign)?;
        self.commit(&compiled).await
    }

    async fn commit_run_terminal(
        &self,
        plan: &TerminalPlan,
        foreign: TerminalForeign,
    ) -> Result<(), StoreError> {
        let compiled = compile_terminal(&self.tables, plan, foreign)?;
        self.commit(&compiled).await
    }

    async fn commit_lifecycle(
        &self,
        plan: &LifecyclePlan,
        foreign: Vec<Foreign>,
    ) -> Result<(), StoreError> {
        let compiled = compile_lifecycle(&self.tables, plan, foreign)?;
        self.commit(&compiled).await
    }

    async fn load_head(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<SessionHead>, StoreError> {
        let key = keys::head(session);
        match self.get(&key.pk, &key.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_head(&item, workspace)?)),
        }
    }

    async fn load_run(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        run: RunId,
    ) -> Result<Option<Run>, StoreError> {
        let key = keys::run(session, run);
        match self.get(&key.pk, &key.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_run(&item, workspace)?)),
        }
    }
}

#[async_trait]
impl AgentJournalStore for SessionStore {
    async fn commit_decision(
        &self,
        plan: &AgentDecisionPlan,
        next_wake: Option<Foreign>,
    ) -> Result<(), StoreError> {
        let compiled = compile_decision(&self.tables, plan, next_wake)?;
        self.commit(&compiled).await
    }

    async fn commit_fanout_page(
        &self,
        plan: &FanoutPagePlan,
        wakes: Vec<Foreign>,
    ) -> Result<(), StoreError> {
        let compiled = compile_fanout_page(&self.tables, plan, wakes)?;
        self.commit(&compiled).await
    }

    async fn load_control(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        agent: AgentId,
    ) -> Result<Option<AgentControl>, StoreError> {
        let key = keys::agent_control(session, agent);
        match self.get(&key.pk, &key.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_control(&item, workspace)?)),
        }
    }

    async fn read_journal(
        &self,
        session: SessionId,
        agent: AgentId,
        from_seq: u64,
        budget: PageBudget,
    ) -> Result<Vec<JournalEntry>, StoreError> {
        let partition = keys::agent_partition(session, agent);
        let output = self
            .client
            .query()
            .table_name(self.table())
            .key_condition_expression("pk = :pk AND sk >= :from")
            .expression_attribute_values(":pk", crate::attr::s(partition))
            .expression_attribute_values(":from", crate::attr::s(keys::journal_sort_key(from_seq)))
            .limit(budget.limit())
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        let mut entries = Vec::new();
        for item in output.items.unwrap_or_default() {
            entries.push(codec::decode_journal(&item)?);
        }
        Ok(entries)
    }
}

#[async_trait]
impl ReceiptStore for SessionStore {
    async fn read_receipt(
        &self,
        workspace: WorkspaceId,
        scope: &IdempotencyScope<'_>,
        key: &IdempotencyKey,
        now: Timestamp,
    ) -> Result<Option<Receipt>, StoreError> {
        let rendered = scope.render();
        let receipt_key = keys::receipt(workspace, &rendered, &key_digest(key))?;
        let Some(item) = self.get(&receipt_key.pk, &receipt_key.sk).await? else {
            return Ok(None);
        };
        let receipt = codec::decode_receipt(&item)?;
        // Expiry is checked here rather than trusted to TTL: AWS reclaims a
        // TTL'd row within 48 hours, so a reader that trusted it would replay
        // an expired receipt for up to two days.
        if codec::receipt_is_live(&receipt, now) {
            Ok(Some(receipt))
        } else {
            Ok(None)
        }
    }
}
