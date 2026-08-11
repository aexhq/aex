//! Total compilation of an application transaction into distinct `DynamoDB` actions.
//!
//! Logical guards are grouped by item. A group with a write becomes one
//! conditional write; a read-only group becomes one `ConditionCheck`. The
//! compiler therefore cannot emit the provider-invalid shape "check and write
//! the same item" and its action ceiling is the number of distinct items, not
//! the number of domain predicates.

use std::collections::{BTreeMap, HashMap};

use aex_session_app::plan::{
    Condition, ConditionId, Hint, ItemKey, SessionTransaction, TableFamily, TransactionIntent,
    Write,
};
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::types::builders::{ConditionCheckBuilder, PutBuilder};

use crate::attr::{n, s};
use crate::error::StoreError;
use crate::plan::{IMMUTABLE, Participant, RegionalTables, TransactionPlan, key};

/// One logical item after all of its guards have been grouped.
#[derive(Debug)]
pub struct LogicalAction<'a> {
    /// The domain-owned item identity.
    pub target: ItemKey,
    /// Predicates in original plan order, with their stable ids.
    pub conditions: Vec<(ConditionId, &'a Condition)>,
    /// At most one write; application validation rejects a duplicate target.
    pub write: Option<&'a Write>,
}

/// Compiles item families the session adapter does not compile itself.
///
/// One call must append exactly one action. The root compiler checks that
/// contract, so a foreign adapter cannot silently drop a guard or split
/// atomicity. The asserted [`SessionBinding`] is handed over with the action so
/// a foreign compiler can enforce the same tenant check the owned writes do.
pub trait ExternalActionCompiler: Sync {
    /// Appends one conditional write or read-only condition check.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the owned logical action cannot be rendered
    /// as exactly one provider action.
    fn compile_action(
        &self,
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError>;
}

/// Which adapter compiles each table family a plan may address.
///
/// A cross-family transaction is only atomic if every family in it renders, and
/// the failure mode this replaces was silent: a write whose family had no
/// compiler fell through to whatever single external compiler the caller
/// happened to pass, which in practice was a refusing test double. Registration
/// makes the gap a named composition error instead.
///
/// The session adapter owns `session-authority`, so it ships the compilers for
/// the two families whose rows live there — [`TableFamily::Idempotency`] and
/// [`TableFamily::Outbox`] — pre-registered. Every other family belongs to the
/// adapter that owns its table and must be registered by the composition.
pub struct FamilyCompilers<'a> {
    owners: BTreeMap<TableFamily, &'a dyn ExternalActionCompiler>,
}

/// The receipt compiler, promoted so [`FamilyCompilers::new`] can register a
/// reference to it without allocating.
static IDEMPOTENCY: IdempotencyCompiler = IdempotencyCompiler::session();

/// The outbox compiler, promoted for the same reason.
static OUTBOX: OutboxCompiler = OutboxCompiler;

impl<'a> FamilyCompilers<'a> {
    /// The families the session adapter compiles on its own behalf.
    #[must_use]
    pub fn new() -> Self {
        let mut owners = BTreeMap::<TableFamily, &'a dyn ExternalActionCompiler>::new();
        owners.insert(TableFamily::Idempotency, &IDEMPOTENCY);
        owners.insert(TableFamily::Outbox, &OUTBOX);
        Self { owners }
    }

    /// Registers the adapter that owns `family`, replacing any earlier one.
    #[must_use]
    pub fn with(mut self, family: TableFamily, compiler: &'a dyn ExternalActionCompiler) -> Self {
        self.owners.insert(family, compiler);
        self
    }
}

impl Default for FamilyCompilers<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl ExternalActionCompiler for FamilyCompilers<'_> {
    fn compile_action(
        &self,
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        let family = action.target.family;
        let Some(compiler) = self.owners.get(&family) else {
            return Err(StoreError::Invalid {
                detail: format!(
                    "the {family:?} family has no registered compiler, so this logical item cannot \
                     be rendered as a provider action"
                ),
            });
        };
        compiler.compile_action(tables, binding, action, output)
    }
}

/// Compiles the `Idempotency` family onto `session-authority`.
///
/// The receipt row shape is the one in [`crate::replay`], shared by every
/// regional table that holds a receipt, so a session transaction's receipt and a
/// custody transaction's receipt are the same row read by the same reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdempotencyCompiler {
    authority: ReceiptAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiptAuthority {
    Session,
    Registry,
}

impl IdempotencyCompiler {
    /// Stores receipts beside the session authority.
    #[must_use]
    pub const fn session() -> Self {
        Self {
            authority: ReceiptAuthority::Session,
        }
    }

    /// Stores receipts beside the named-registry authority.
    #[must_use]
    pub const fn registry() -> Self {
        Self {
            authority: ReceiptAuthority::Registry,
        }
    }
}

impl ExternalActionCompiler for IdempotencyCompiler {
    fn compile_action(
        &self,
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        let Some(Write::PutIdempotencyReceipt(receipt)) = action.write else {
            return Err(StoreError::Invalid {
                detail: "the idempotency family carries receipt writes only".to_owned(),
            });
        };
        validate_receipt_binding(binding, receipt)?;
        let row = crate::codec::receipt_of(receipt).map_err(|error| StoreError::Invalid {
            detail: format!("an idempotency receipt could not be projected: {error}"),
        })?;
        let item = crate::codec::encode_receipt(binding.workspace, &row).map_err(|error| {
            StoreError::Invalid {
                detail: format!("an idempotency receipt key is unusable: {error}"),
            }
        })?;
        let mut expression = compile_conditions(&action.conditions)?;
        // The conditional put *is* the concurrency election (A D-6): the loser
        // of a race writes nothing at all, because `TransactWriteItems` is
        // all-or-nothing. Applying it here rather than trusting the plan to
        // carry `ItemAbsent` means a use case cannot forget the election.
        if !action
            .conditions
            .iter()
            .any(|(_, condition)| matches!(condition, Condition::ItemAbsent(_)))
        {
            if receipt.expires_at.is_some() {
                // A short-lived bearer response may be replayed only while its
                // signature is live. Once its explicit fence passes, the old
                // row is still present until asynchronous TTL reclamation; the
                // next elected request replaces it atomically instead of
                // becoming permanently ambiguous behind a stale row.
                expression
                    .and_literal("attribute_not_exists(pk) OR #receiptExpiresAt <= :receiptNow");
                expression
                    .names
                    .insert("#receiptExpiresAt".to_owned(), "expiresAt".to_owned());
                expression.values.insert(
                    ":receiptNow".to_owned(),
                    crate::attr::stamp(receipt.created_at),
                );
            } else {
                expression.and_literal(IMMUTABLE);
            }
        }
        let (participant, table) = match self.authority {
            ReceiptAuthority::Session => {
                (Participant::SESSION_IDEMPOTENCY, &tables.session_authority)
            }
            ReceiptAuthority::Registry => {
                (Participant::REGISTRY_IDEMPOTENCY, &tables.regional_registry)
            }
        };
        output.put(participant, conditional_put(table, item, expression)?)?;
        Ok(())
    }
}

fn validate_receipt_binding(
    binding: AuthorityBinding,
    receipt: &aex_session_domain::IdempotencyReceipt,
) -> Result<(), StoreError> {
    let expected_key = aex_session_domain::ReceiptKey::of(receipt.key.scope(), &receipt.identity)
        .map_err(|error| StoreError::Invalid {
        detail: format!("an idempotency receipt key is invalid: {error}"),
    })?;
    if receipt.key != expected_key || receipt.intent != receipt.identity.intent() {
        return Err(StoreError::Invalid {
            detail: "an idempotency receipt drifted from its replay key or canonical intent"
                .to_owned(),
        });
    }
    let principal = match &receipt.identity {
        aex_session_domain::IdempotencyIdentity::Key(identity) => &identity.principal,
        aex_session_domain::IdempotencyIdentity::Operation(identity) => &identity.principal,
    };
    if principal.organization() != Some(binding.organization)
        || principal
            .workspace()
            .is_some_and(|workspace| workspace != binding.workspace)
    {
        return Err(StoreError::Invalid {
            detail: "an idempotency receipt crosses its authenticated tenant binding".to_owned(),
        });
    }
    Ok(())
}

/// Compiles the `Outbox` family onto `session-authority`.
pub struct OutboxCompiler;

impl ExternalActionCompiler for OutboxCompiler {
    fn compile_action(
        &self,
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        let Some(Write::PutOutboxEvent(event)) = action.write else {
            return Err(StoreError::Invalid {
                detail: "the outbox family carries outbox-event writes only".to_owned(),
            });
        };
        if Some(event.session) != binding.session {
            return Err(cross_tenant());
        }
        let item = crate::codec::encode_outbox_event(binding.workspace, event);
        let mut expression = compile_conditions(&action.conditions)?;
        // A terminal barrier emits its outbox event exactly once; a replayed
        // barrier must lose the row rather than rewrite it, or a delivered event
        // could be resurrected as pending.
        if !action
            .conditions
            .iter()
            .any(|(_, condition)| matches!(condition, Condition::ItemAbsent(_)))
        {
            expression.and_literal(IMMUTABLE);
        }
        output.put(
            Participant::SESSION_TERMINAL_EVENT,
            conditional_put(&tables.session_authority, item, expression)?,
        )?;
        Ok(())
    }
}

/// A compiled provider plan plus the logical guard ids represented by each
/// physical action and the hints that become publishable only after commit.
#[derive(Debug)]
pub struct CompiledApplicationPlan {
    /// The provider transaction.
    pub transaction: TransactionPlan,
    /// Parallel to `transaction.actions()`.
    pub condition_groups: Vec<Vec<ConditionId>>,
    /// Never dispatched before the transaction succeeds.
    pub after_commit: Vec<Hint>,
}

/// Tenant and path identity the regional assertion already authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionBinding {
    /// Owning workspace.
    pub workspace: aex_wire::ids::WorkspaceId,
    /// Owning organization.
    pub organization: aex_wire::ids::OrganizationId,
    /// Exact session path parameter.
    pub session: aex_wire::ids::SessionId,
}

/// Tenant identity for an authority transaction that is not session-scoped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceBinding {
    /// Owning workspace.
    pub workspace: aex_wire::ids::WorkspaceId,
    /// Owning organization.
    pub organization: aex_wire::ids::OrganizationId,
}

/// The authenticated tenant and optional resource path carried to every
/// family compiler.
///
/// A workspace route must not invent a session merely to enter the shared
/// transaction compiler. Session-owned actions require `session`; content and
/// registry actions bind only the tenant facts they actually own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorityBinding {
    /// Owning workspace.
    pub workspace: aex_wire::ids::WorkspaceId,
    /// Owning organization.
    pub organization: aex_wire::ids::OrganizationId,
    /// Exact session path, only for a session-scoped transaction.
    pub session: Option<aex_wire::ids::SessionId>,
}

impl From<SessionBinding> for AuthorityBinding {
    fn from(binding: SessionBinding) -> Self {
        Self {
            workspace: binding.workspace,
            organization: binding.organization,
            session: Some(binding.session),
        }
    }
}

impl From<WorkspaceBinding> for AuthorityBinding {
    fn from(binding: WorkspaceBinding) -> Self {
        Self {
            workspace: binding.workspace,
            organization: binding.organization,
            session: None,
        }
    }
}

/// Compiles every logical item exactly once.
///
/// Session, message and run rows use the canonical v1 codec here. Every other
/// family is delegated once to its owning adapter through
/// [`ExternalActionCompiler`].
///
/// # Errors
///
/// Returns [`StoreError`] when the logical plan is invalid, crosses the
/// asserted tenant/path binding, cannot be encoded, or cannot be rendered as
/// one distinct provider action per item.
pub fn compile_application_transaction(
    tables: &RegionalTables,
    input: &SessionTransaction,
    binding: SessionBinding,
    external: &impl ExternalActionCompiler,
) -> Result<CompiledApplicationPlan, StoreError> {
    compile_bound_transaction(tables, input, binding.into(), external)
}

/// Compiles an application transaction authorized at workspace scope.
///
/// # Errors
///
/// As [`compile_application_transaction`]. A session-owned action is rejected
/// because this binding deliberately carries no session identity.
pub fn compile_workspace_transaction(
    tables: &RegionalTables,
    input: &SessionTransaction,
    binding: WorkspaceBinding,
    external: &impl ExternalActionCompiler,
) -> Result<CompiledApplicationPlan, StoreError> {
    compile_bound_transaction(tables, input, binding.into(), external)
}

fn compile_bound_transaction(
    tables: &RegionalTables,
    input: &SessionTransaction,
    binding: AuthorityBinding,
    external: &impl ExternalActionCompiler,
) -> Result<CompiledApplicationPlan, StoreError> {
    input.validate().map_err(|error| StoreError::Invalid {
        detail: error.to_string(),
    })?;
    let actions = grouped(input);
    let mut output = TransactionPlan::new(transport_identity(input));
    let mut condition_groups = Vec::with_capacity(actions.len());
    for action in &actions {
        let before = output.len();
        if !compile_owned(tables, input.intent, action, binding, &mut output)? {
            external.compile_action(tables, binding, action, &mut output)?;
        }
        if output.len() != before + 1 {
            return Err(StoreError::Invalid {
                detail: "one logical item must compile to exactly one DynamoDB action".to_owned(),
            });
        }
        condition_groups.push(action.conditions.iter().map(|(id, _)| *id).collect());
    }
    Ok(CompiledApplicationPlan {
        transaction: output,
        condition_groups,
        after_commit: input.after_commit.clone(),
    })
}

fn grouped(input: &SessionTransaction) -> Vec<LogicalAction<'_>> {
    let mut positions = BTreeMap::<ItemKey, usize>::new();
    let mut actions = Vec::<LogicalAction<'_>>::new();
    for (index, condition) in input.conditions.iter().enumerate() {
        let target = condition.target();
        let position = *positions.entry(target.clone()).or_insert_with(|| {
            let position = actions.len();
            actions.push(LogicalAction {
                target,
                conditions: Vec::new(),
                write: None,
            });
            position
        });
        actions[position].conditions.push((
            ConditionId(u16::try_from(index).unwrap_or(u16::MAX)),
            condition,
        ));
    }
    for write in &input.writes {
        let target = write.target();
        let position = *positions.entry(target.clone()).or_insert_with(|| {
            let position = actions.len();
            actions.push(LogicalAction {
                target,
                conditions: Vec::new(),
                write: None,
            });
            position
        });
        actions[position].write = Some(write);
    }
    actions
}

fn compile_owned(
    tables: &RegionalTables,
    intent: TransactionIntent,
    action: &LogicalAction<'_>,
    binding: AuthorityBinding,
    output: &mut TransactionPlan,
) -> Result<bool, StoreError> {
    let Some(write) = action.write else {
        return compile_owned_check(tables, action, binding, output);
    };
    match write {
        Write::PutSessionHead(session) => {
            compile_session_write(tables, intent, action, session, binding, output)?;
            Ok(true)
        }
        Write::PutMessage(message) => {
            compile_message_write(tables, intent, action, message, binding, output)?;
            Ok(true)
        }
        Write::PutSealedMessage(message) => {
            compile_sealed_message_write(tables, intent, action, message, binding, output)?;
            Ok(true)
        }
        Write::PutRun(run) => {
            compile_run_write(tables, intent, action, run, binding, output)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn compile_session_write(
    tables: &RegionalTables,
    intent: TransactionIntent,
    action: &LogicalAction<'_>,
    session: &aex_session_domain::Session,
    binding: AuthorityBinding,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    if Some(session.id) != binding.session
        || session.workspace != binding.workspace
        || session.organization != binding.organization
    {
        return Err(cross_tenant());
    }
    let item = crate::authority_codec::encode_session(session)?;
    let mut expression = compile_conditions(&action.conditions)?;
    if intent == TransactionIntent::CreateSession {
        expression.and_literal(IMMUTABLE);
    }
    output.put(
        Participant::SESSION_HEAD,
        conditional_put(&tables.session_authority, item, expression)?,
    )?;
    Ok(())
}

fn compile_message_write(
    tables: &RegionalTables,
    intent: TransactionIntent,
    action: &LogicalAction<'_>,
    message: &aex_session_domain::Message,
    binding: AuthorityBinding,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    if Some(message.session) != binding.session {
        return Err(cross_tenant());
    }
    let item = crate::authority_codec::encode_domain_message(
        message,
        binding.workspace,
        binding.organization,
    )?;
    let mut expression = compile_conditions(&action.conditions)?;
    match intent {
        TransactionIntent::AdmitMessage => expression.and_literal(IMMUTABLE),
        TransactionIntent::CommitTerminal => {
            expression.and_literal("attribute_exists(pk) AND #messageState = :messageOpen");
            expression
                .names
                .insert("#messageState".to_owned(), "state".to_owned());
            expression
                .values
                .insert(":messageOpen".to_owned(), s("open"));
        }
        _ => {
            return Err(StoreError::Invalid {
                detail:
                    "a canonical message write is only valid at admission or the terminal barrier"
                        .to_owned(),
            });
        }
    }
    output.put(
        Participant::SESSION_MESSAGE,
        conditional_put(&tables.session_authority, item, expression)?,
    )?;
    Ok(())
}

fn compile_sealed_message_write(
    tables: &RegionalTables,
    intent: TransactionIntent,
    action: &LogicalAction<'_>,
    message: &aex_session_domain::Message,
    binding: AuthorityBinding,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    if Some(message.session) != binding.session {
        return Err(cross_tenant());
    }
    if !matches!(
        intent,
        TransactionIntent::AdmitMessage | TransactionIntent::CommitTerminal
    ) {
        return Err(StoreError::Invalid {
            detail:
                "a sealed-message projection is written only at admission or the terminal barrier"
                    .to_owned(),
        });
    }
    let item = crate::authority_codec::encode_sealed_message(
        message,
        binding.workspace,
        binding.organization,
    )?;
    let mut expression = compile_conditions(&action.conditions)?;
    expression.and_literal(IMMUTABLE);
    output.put(
        Participant::SESSION_SEALED_MESSAGE,
        conditional_put(&tables.session_authority, item, expression)?,
    )?;
    Ok(())
}

fn compile_run_write(
    tables: &RegionalTables,
    intent: TransactionIntent,
    action: &LogicalAction<'_>,
    run: &aex_session_domain::Run,
    binding: AuthorityBinding,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    if Some(run.session) != binding.session {
        return Err(cross_tenant());
    }
    let item =
        crate::authority_codec::encode_domain_run(run, binding.workspace, binding.organization)?;
    let mut expression = compile_conditions(&action.conditions)?;
    if intent == TransactionIntent::AdmitMessage {
        expression.and_literal(IMMUTABLE);
    }
    output.put(
        Participant::SESSION_RUN,
        conditional_put(&tables.session_authority, item, expression)?,
    )?;
    Ok(())
}

fn compile_owned_check(
    tables: &RegionalTables,
    action: &LogicalAction<'_>,
    binding: AuthorityBinding,
    output: &mut TransactionPlan,
) -> Result<bool, StoreError> {
    if action
        .conditions
        .iter()
        .all(|(_, condition)| is_head_condition(condition))
    {
        let session = action
            .conditions
            .iter()
            .find_map(|(_, condition)| condition_session(condition))
            .ok_or_else(|| StoreError::Invalid {
                detail: "a session-head guard group has no session identity".to_owned(),
            })?;
        if Some(session) != binding.session {
            return Err(cross_tenant());
        }
        output.condition_check(
            Participant::SESSION_HEAD_GUARD,
            conditional_check(
                &tables.session_authority,
                &crate::keys::head(session),
                compile_conditions(&action.conditions)?,
            )?,
        )?;
        return Ok(true);
    }
    if action
        .conditions
        .iter()
        .all(|(_, condition)| matches!(condition, Condition::RunNonTerminal { .. }))
    {
        compile_run_check(tables, action, binding, output)?;
        return Ok(true);
    }
    Ok(false)
}

fn compile_run_check(
    tables: &RegionalTables,
    action: &LogicalAction<'_>,
    binding: AuthorityBinding,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    let (session, run) = action
        .conditions
        .iter()
        .find_map(|(_, condition)| match condition {
            Condition::RunNonTerminal { session, run } => Some((*session, *run)),
            _ => None,
        })
        .ok_or_else(|| StoreError::Invalid {
            detail: "a run guard group has no physical identity".to_owned(),
        })?;
    if Some(session) != binding.session {
        return Err(cross_tenant());
    }
    output.condition_check(
        Participant::SESSION_RUN,
        conditional_check(
            &tables.session_authority,
            &crate::keys::run(session, run),
            compile_conditions(&action.conditions)?,
        )?,
    )?;
    Ok(())
}

#[derive(Default)]
pub(crate) struct Expression {
    pub(crate) terms: Vec<String>,
    pub(crate) names: HashMap<String, String>,
    pub(crate) values: HashMap<String, AttributeValue>,
}

impl Expression {
    pub(crate) fn and_literal(&mut self, value: &str) {
        self.terms.push(value.to_owned());
    }

    pub(crate) fn rendered(&self) -> Result<String, StoreError> {
        if self.terms.is_empty() {
            return Err(StoreError::Invalid {
                detail: "an authority action carries no condition".to_owned(),
            });
        }
        Ok(self
            .terms
            .iter()
            .map(|term| format!("({term})"))
            .collect::<Vec<_>>()
            .join(" AND "))
    }
}

fn compile_conditions(conditions: &[(ConditionId, &Condition)]) -> Result<Expression, StoreError> {
    let mut output = Expression::default();
    for (id, condition) in conditions {
        let prefix = format!("c{}", id.0);
        compile_condition(&mut output, &prefix, condition)?;
    }
    Ok(output)
}

fn compile_condition(
    output: &mut Expression,
    prefix: &str,
    condition: &Condition,
) -> Result<(), StoreError> {
    match condition {
        Condition::SessionRevision { expected, .. } => {
            term_eq_u64(output, prefix, "revision", expected.0);
        }
        Condition::SessionStatusIn { allowed, .. } => {
            compile_status_set(output, prefix, allowed)?;
        }
        Condition::SessionActiveRun { expected, .. } => match expected {
            Some(run) => term_eq_string(output, prefix, "activeRunId", run.to_string()),
            None => output
                .terms
                .push("attribute_not_exists(activeRunId)".to_owned()),
        },
        Condition::WorkAdmission { expected, .. } => term_eq_string(
            output,
            prefix,
            "workAdmission",
            work_admission(*expected).to_owned(),
        ),
        Condition::DeletionState {
            expected, epoch, ..
        } => {
            term_eq_string(
                output,
                &format!("{prefix}state"),
                "lifecycle",
                deletion_state(*expected).to_owned(),
            );
            term_eq_u64(output, &format!("{prefix}epoch"), "deletionEpoch", epoch.0);
        }
        Condition::CancellationEpoch { expected, .. } => {
            term_eq_u64(output, prefix, "cancelEpoch", expected.0);
        }
        Condition::MutationGuardFree { .. } => output
            .terms
            .push("attribute_not_exists(mutationGuardOperationId)".to_owned()),
        Condition::MutationGuardHeldBy { holder, .. } => term_eq_string(
            output,
            prefix,
            "mutationGuardOperationId",
            holder.to_string(),
        ),
        Condition::RunNonTerminal { .. } => compile_run_status(output, prefix),
        Condition::ItemAbsent(_) => output.and_literal(IMMUTABLE),
        Condition::ItemPresent(_) => output.and_literal("attribute_exists(pk)"),
        _ => {
            return Err(StoreError::Invalid {
                detail: "a condition reached an adapter that does not own its item family"
                    .to_owned(),
            });
        }
    }
    Ok(())
}

fn compile_status_set(
    output: &mut Expression,
    prefix: &str,
    allowed: &std::collections::BTreeSet<aex_session_domain::SessionStatus>,
) -> Result<(), StoreError> {
    if allowed.is_empty() {
        return Err(StoreError::Invalid {
            detail: "a session status guard cannot have an empty set".to_owned(),
        });
    }
    let name = format!("#{prefix}status");
    output.names.insert(name.clone(), "status".to_owned());
    let mut values = Vec::with_capacity(allowed.len());
    for (index, status) in allowed.iter().enumerate() {
        let key = format!(":{prefix}status{index}");
        output
            .values
            .insert(key.clone(), s(session_status(*status)));
        values.push(key);
    }
    output
        .terms
        .push(format!("{name} IN ({})", values.join(", ")));
    Ok(())
}

fn compile_run_status(output: &mut Expression, prefix: &str) {
    let name = format!("#{prefix}status");
    output.names.insert(name.clone(), "status".to_owned());
    let queued = format!(":{prefix}queued");
    let running = format!(":{prefix}running");
    output.values.insert(queued.clone(), s("queued"));
    output.values.insert(running.clone(), s("running"));
    output
        .terms
        .push(format!("{name} IN ({queued}, {running})"));
}

pub(crate) fn term_eq_u64(output: &mut Expression, prefix: &str, attribute: &str, value: u64) {
    let name = format!("#{prefix}");
    let value_name = format!(":{prefix}");
    output.names.insert(name.clone(), attribute.to_owned());
    output.values.insert(value_name.clone(), n(value));
    output.terms.push(format!("{name} = {value_name}"));
}

pub(crate) fn term_eq_string(
    output: &mut Expression,
    prefix: &str,
    attribute: &str,
    value: String,
) {
    let name = format!("#{prefix}");
    let value_name = format!(":{prefix}");
    output.names.insert(name.clone(), attribute.to_owned());
    output.values.insert(value_name.clone(), s(value));
    output.terms.push(format!("{name} = {value_name}"));
}

pub(crate) fn conditional_put(
    table: &str,
    item: crate::attr::Item,
    expression: Expression,
) -> Result<PutBuilder, StoreError> {
    let rendered = expression.rendered()?;
    Ok(aws_sdk_dynamodb::types::Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(rendered)
        .set_expression_attribute_names((!expression.names.is_empty()).then_some(expression.names))
        .set_expression_attribute_values(
            (!expression.values.is_empty()).then_some(expression.values),
        ))
}

pub(crate) fn conditional_check(
    table: &str,
    physical: &crate::keys::Key,
    expression: Expression,
) -> Result<ConditionCheckBuilder, StoreError> {
    let rendered = expression.rendered()?;
    Ok(aws_sdk_dynamodb::types::ConditionCheck::builder()
        .table_name(table)
        .set_key(Some(key(&physical.pk, &physical.sk)))
        .condition_expression(rendered)
        .set_expression_attribute_names((!expression.names.is_empty()).then_some(expression.names))
        .set_expression_attribute_values(
            (!expression.values.is_empty()).then_some(expression.values),
        ))
}

fn is_head_condition(condition: &Condition) -> bool {
    matches!(
        condition,
        Condition::SessionRevision { .. }
            | Condition::SessionStatusIn { .. }
            | Condition::SessionActiveRun { .. }
            | Condition::WorkAdmission { .. }
            | Condition::DeletionState { .. }
            | Condition::CancellationEpoch { .. }
            | Condition::MutationGuardFree { .. }
            | Condition::MutationGuardHeldBy { .. }
    )
}

fn condition_session(condition: &Condition) -> Option<aex_wire::ids::SessionId> {
    match condition {
        Condition::SessionRevision { session, .. }
        | Condition::SessionStatusIn { session, .. }
        | Condition::SessionActiveRun { session, .. }
        | Condition::WorkAdmission { session, .. }
        | Condition::DeletionState { session, .. }
        | Condition::CancellationEpoch { session, .. }
        | Condition::MutationGuardFree { session }
        | Condition::MutationGuardHeldBy { session, .. } => Some(*session),
        _ => None,
    }
}

fn transport_identity(input: &SessionTransaction) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(format!("{input:?}").as_bytes());
    format!("app-{}", &hex::encode(digest)[..32])
}

const fn session_status(status: aex_session_domain::SessionStatus) -> &'static str {
    match status {
        aex_session_domain::SessionStatus::Idle => "idle",
        aex_session_domain::SessionStatus::Running => "running",
        aex_session_domain::SessionStatus::AwaitingApproval => "awaiting_approval",
        aex_session_domain::SessionStatus::Trashed => "trashed",
        aex_session_domain::SessionStatus::Purging => "purging",
    }
}

const fn work_admission(value: aex_session_domain::WorkAdmission) -> &'static str {
    match value {
        aex_session_domain::WorkAdmission::Open => "open",
        aex_session_domain::WorkAdmission::Paused => "paused",
        aex_session_domain::WorkAdmission::Trashing => "trashing",
        aex_session_domain::WorkAdmission::Purging => "purging",
        aex_session_domain::WorkAdmission::ContinuityLost => "continuity_lost",
    }
}

const fn deletion_state(value: aex_operation_domain::DeletionState) -> &'static str {
    match value {
        aex_operation_domain::DeletionState::Live => "active",
        aex_operation_domain::DeletionState::Trashed => "trashed",
        aex_operation_domain::DeletionState::Purging => "purging",
        aex_operation_domain::DeletionState::Purged => "purged",
    }
}

pub(crate) fn cross_tenant() -> StoreError {
    StoreError::Invalid {
        detail: "a session authority write belongs to another tenant".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use aex_session_app::plan::{Condition, SessionTransaction, TransactionIntent, Write};
    use aex_session_domain::testing::running_session;

    use super::{
        AuthorityBinding, ExternalActionCompiler, FamilyCompilers, LogicalAction, SessionBinding,
        compile_application_transaction,
    };
    use crate::error::StoreError;
    use crate::plan::{RegionalTables, TransactionPlan};

    struct RefuseExternal;

    impl ExternalActionCompiler for RefuseExternal {
        fn compile_action(
            &self,
            _tables: &RegionalTables,
            _binding: AuthorityBinding,
            _action: &LogicalAction<'_>,
            _output: &mut TransactionPlan,
        ) -> Result<(), StoreError> {
            Err(StoreError::Invalid {
                detail: "unexpected external action".to_owned(),
            })
        }
    }

    #[test]
    fn run_start_compiles_to_two_distinct_items_not_three_logical_guards() {
        let (session, mut run, _agent, _message) = running_session();
        run.status = aex_session_domain::RunStatus::Running;
        run.started_at = Some(aex_session_domain::testing::moment(9));
        let input = SessionTransaction {
            intent: TransactionIntent::StartRun,
            conditions: vec![
                Condition::SessionActiveRun {
                    session: session.id,
                    expected: Some(run.id),
                },
                Condition::SessionRevision {
                    session: session.id,
                    expected: session.revision,
                },
                Condition::RunNonTerminal {
                    session: session.id,
                    run: run.id,
                },
            ],
            writes: vec![Write::PutRun(Box::new(run))],
            after_commit: Vec::new(),
        };
        let compiled = compile_application_transaction(
            &RegionalTables::composed("dev", "eu-west-1"),
            &input,
            SessionBinding {
                workspace: session.workspace,
                organization: session.organization,
                session: session.id,
            },
            &RefuseExternal,
        )
        .expect("compiles");
        assert_eq!(compiled.transaction.len(), 2);
        assert_eq!(compiled.condition_groups[0].len(), 2);
        assert_eq!(compiled.condition_groups[1].len(), 1);
        assert!(
            compiled.transaction.actions()[0]
                .condition_check()
                .is_some()
        );
        let run_write = compiled.transaction.actions()[1]
            .put()
            .expect("run is one conditional put");
        assert!(
            run_write
                .condition_expression()
                .expect("condition")
                .contains("IN")
        );
    }

    #[test]
    fn a_cross_tenant_canonical_write_fails_before_request_construction() {
        let (session, _run, _agent, _message) = running_session();
        let input = SessionTransaction {
            intent: TransactionIntent::AdmitMessage,
            conditions: Vec::new(),
            writes: vec![Write::PutSessionHead(Box::new(session.clone()))],
            after_commit: Vec::new(),
        };
        let another_workspace = aex_session_domain::testing::id(88);
        let error = compile_application_transaction(
            &RegionalTables::composed("dev", "eu-west-1"),
            &input,
            SessionBinding {
                workspace: another_workspace,
                organization: session.organization,
                session: session.id,
            },
            &RefuseExternal,
        )
        .expect_err("tenant drift");
        assert!(matches!(error, StoreError::Invalid { .. }), "{error}");
    }

    #[test]
    fn a_partial_session_create_is_rejected_before_request_construction() {
        let (session, _run, _agent, _message) = running_session();
        let input = SessionTransaction {
            intent: TransactionIntent::CreateSession,
            conditions: Vec::new(),
            writes: vec![Write::PutSessionHead(Box::new(session.clone()))],
            after_commit: Vec::new(),
        };
        let error = compile_application_transaction(
            &RegionalTables::composed("dev", "eu-west-1"),
            &input,
            SessionBinding {
                workspace: session.workspace,
                organization: session.organization,
                session: session.id,
            },
            &RefuseExternal,
        );
        assert!(
            matches!(error, Err(StoreError::Invalid { ref detail }) if detail == "session creation authority is wrong: a create writes exactly one root agent control record; a head whose root agent does not exist answers 404 to the reader the 201 just invited"),
            "{error:?}"
        );
    }

    /// A receipt filed under `scope` for the caller key `key`.
    fn receipt(scope: &str, key: &str, intent: u8) -> aex_session_domain::IdempotencyReceipt {
        use aex_session_domain::{
            IdempotencyIdentity, IdempotencyReceipt, ReceiptKey, ReceiptOutcome, ResourceId,
            ResourceKind, ResponseBody,
        };
        use aex_wire::idempotency::{IdempotencyKey, IntentDigest, ReplayIdentity};
        let identity = IdempotencyIdentity::Key(Box::new(ReplayIdentity {
            principal: aex_wire::idempotency::PrincipalScope::WorkspaceKey {
                key: aex_session_domain::testing::id(1),
                workspace: aex_session_domain::testing::id(2),
                organization: aex_session_domain::testing::id(3),
            },
            route: aex_wire::routes::RouteId::SessionMessageSend,
            key: IdempotencyKey::parse(key).expect("a key"),
            intent: IntentDigest::from_bytes([intent; 32]),
        }));
        IdempotencyReceipt {
            key: ReceiptKey::of(scope, &identity).expect("a usable receipt key"),
            identity,
            intent: IntentDigest::from_bytes([intent; 32]),
            outcome: ReceiptOutcome::Resource {
                kind: ResourceKind::Message,
                id: ResourceId("msg_1".to_owned()),
                response: ResponseBody::of(br#"{"id":"msg_1"}"#),
            },
            created_at: aex_session_domain::testing::moment(1),
            expires_at: None,
        }
    }

    #[test]
    fn a_receipt_compiles_to_one_conditional_put_the_session_adapter_owns() {
        let (session, _run, _agent, _message) = running_session();
        let input = SessionTransaction {
            intent: TransactionIntent::AdmitMessage,
            conditions: Vec::new(),
            writes: vec![Write::PutIdempotencyReceipt(Box::new(receipt(
                "session.message:ses_1",
                "k",
                7,
            )))],
            after_commit: Vec::new(),
        };
        let compiled = compile_application_transaction(
            &RegionalTables::composed("dev", "eu-west-1"),
            &input,
            SessionBinding {
                workspace: session.workspace,
                organization: session.organization,
                session: session.id,
            },
            &FamilyCompilers::new(),
        )
        .expect("the idempotency family is registered");
        assert_eq!(compiled.transaction.len(), 1);
        assert_eq!(
            compiled.transaction.participants(),
            [crate::plan::Participant::SESSION_IDEMPOTENCY]
        );
        let put = compiled.transaction.actions()[0]
            .put()
            .expect("a receipt is one conditional put");
        assert_eq!(
            put.condition_expression(),
            Some("(attribute_not_exists(pk))"),
            "the conditional put is the concurrency election"
        );
        let expected = crate::keys::receipt(
            session.workspace,
            "session.message:ses_1",
            receipt("session.message:ses_1", "k", 7).key.key_sha256(),
        )
        .expect("a receipt key");
        assert_eq!(
            put.item().get("pk").and_then(|value| value.as_s().ok()),
            Some(&expected.pk)
        );
    }

    fn rejected_receipt(
        receipt: aex_session_domain::IdempotencyReceipt,
    ) -> crate::error::StoreError {
        let (session, _run, _agent, _message) = running_session();
        let input = SessionTransaction {
            intent: TransactionIntent::AdmitMessage,
            conditions: Vec::new(),
            writes: vec![Write::PutIdempotencyReceipt(Box::new(receipt))],
            after_commit: Vec::new(),
        };
        compile_application_transaction(
            &RegionalTables::composed("dev", "eu-west-1"),
            &input,
            SessionBinding {
                workspace: session.workspace,
                organization: session.organization,
                session: session.id,
            },
            &FamilyCompilers::new(),
        )
        .expect_err("the forged receipt is rejected before request construction")
    }

    #[test]
    fn a_receipt_key_must_be_derived_from_its_own_replay_identity() {
        let mut forged = receipt("session.message:ses_1", "k1", 7);
        forged.key = receipt("session.message:ses_1", "k2", 7).key;
        assert!(matches!(
            rejected_receipt(forged),
            StoreError::Invalid { .. }
        ));
    }

    #[test]
    fn a_receipt_intent_must_equal_its_one_canonical_identity_intent() {
        let mut forged = receipt("session.message:ses_1", "k", 7);
        forged.intent = aex_wire::idempotency::IntentDigest::from_bytes([9; 32]);
        assert!(matches!(
            rejected_receipt(forged),
            StoreError::Invalid { .. }
        ));
    }

    #[test]
    fn a_receipt_principal_cannot_cross_the_authenticated_workspace() {
        let mut forged = receipt("session.message:ses_1", "k", 7);
        let aex_session_domain::IdempotencyIdentity::Key(identity) = &mut forged.identity else {
            panic!("the fixture is key-based");
        };
        identity.principal = aex_wire::idempotency::PrincipalScope::WorkspaceKey {
            key: aex_session_domain::testing::id(1),
            workspace: aex_session_domain::testing::id(99),
            organization: aex_session_domain::testing::id(3),
        };
        assert!(matches!(
            rejected_receipt(forged),
            StoreError::Invalid { .. }
        ));
    }

    #[test]
    fn two_keys_with_one_intent_are_two_receipts_and_one_key_with_two_intents_is_one() {
        // Keyed by the intent digest, the first pair collided on one item — one
        // caller's receipt silently overwrote the other's — and the second pair
        // addressed two different items, so a replay under a changed body found
        // nothing and executed twice instead of conflicting.
        let two_keys = SessionTransaction {
            intent: TransactionIntent::AdmitMessage,
            conditions: Vec::new(),
            writes: vec![
                Write::PutIdempotencyReceipt(Box::new(receipt("session.message:ses_1", "k1", 7))),
                Write::PutIdempotencyReceipt(Box::new(receipt("session.message:ses_1", "k2", 7))),
            ],
            after_commit: Vec::new(),
        };
        assert_eq!(
            two_keys.validate().expect("two distinct receipts").actions,
            2
        );

        let two_intents = SessionTransaction {
            writes: vec![
                Write::PutIdempotencyReceipt(Box::new(receipt("session.message:ses_1", "k", 7))),
                Write::PutIdempotencyReceipt(Box::new(receipt("session.message:ses_1", "k", 9))),
            ],
            ..two_keys
        };
        assert!(
            matches!(
                two_intents.validate(),
                Err(aex_session_app::plan::PlanError::DuplicateWriteTarget(_))
            ),
            "one key addresses one receipt, which is what makes the conflict reachable"
        );
    }

    #[test]
    fn an_outbox_event_compiles_to_one_conditional_put() {
        let (session, run, _agent, _message) = running_session();
        let event = aex_session_domain::OutboxEvent {
            schema_version: aex_internal_contracts::SchemaVersion::V1,
            session: session.id,
            run: run.id,
            status: aex_session_domain::RunStatus::Succeeded,
            session_revision: session.revision,
            usage_closure: aex_session_domain::UsageClosureId(aex_wire::ids::Uuid7::compose(
                1, [11; 10],
            )),
            at: aex_session_domain::testing::moment(9),
        };
        let input = SessionTransaction {
            intent: TransactionIntent::CommitTerminal,
            conditions: Vec::new(),
            writes: vec![Write::PutOutboxEvent(Box::new(event))],
            after_commit: Vec::new(),
        };
        let compiled = compile_application_transaction(
            &RegionalTables::composed("dev", "eu-west-1"),
            &input,
            SessionBinding {
                workspace: session.workspace,
                organization: session.organization,
                session: session.id,
            },
            &FamilyCompilers::new(),
        )
        .expect("the outbox family is registered");
        assert_eq!(compiled.transaction.len(), 1);
        assert_eq!(
            compiled.transaction.participants(),
            [crate::plan::Participant::SESSION_TERMINAL_EVENT]
        );
    }
}
