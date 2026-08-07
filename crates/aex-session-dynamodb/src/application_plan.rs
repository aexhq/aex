//! Total compilation of an application transaction into distinct `DynamoDB` actions.
//!
//! Logical guards are grouped by item. A group with a write becomes one
//! conditional write; a read-only group becomes one `ConditionCheck`. The
//! compiler therefore cannot emit the provider-invalid shape "check and write
//! the same item" and its action ceiling is the number of distinct items, not
//! the number of domain predicates.

use std::collections::{BTreeMap, HashMap};

use aex_session_app::plan::{
    Condition, ConditionId, Hint, ItemKey, SessionTransaction, TransactionIntent, Write,
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

/// Compiles item families owned by another regional adapter.
///
/// One call must append exactly one action. The root compiler checks that
/// contract, so a foreign adapter cannot silently drop a guard or split
/// atomicity.
pub trait ExternalActionCompiler {
    /// Appends one conditional write or read-only condition check.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the owned logical action cannot be rendered
    /// as exactly one provider action.
    fn compile_action(
        &self,
        tables: &RegionalTables,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError>;
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
    input.validate().map_err(|error| StoreError::Invalid {
        detail: error.to_string(),
    })?;
    let actions = grouped(input);
    let mut output = TransactionPlan::new(transport_identity(input));
    let mut condition_groups = Vec::with_capacity(actions.len());
    for action in &actions {
        let before = output.len();
        if !compile_owned(tables, input.intent, action, binding, &mut output)? {
            external.compile_action(tables, action, &mut output)?;
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
    binding: SessionBinding,
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
    binding: SessionBinding,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    if session.id != binding.session
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
    binding: SessionBinding,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    if message.session != binding.session {
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

fn compile_run_write(
    tables: &RegionalTables,
    intent: TransactionIntent,
    action: &LogicalAction<'_>,
    run: &aex_session_domain::Run,
    binding: SessionBinding,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    if run.session != binding.session {
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
    binding: SessionBinding,
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
        if session != binding.session {
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
    binding: SessionBinding,
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
    if session != binding.session {
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
struct Expression {
    terms: Vec<String>,
    names: HashMap<String, String>,
    values: HashMap<String, AttributeValue>,
}

impl Expression {
    fn and_literal(&mut self, value: &str) {
        self.terms.push(value.to_owned());
    }

    fn rendered(&self) -> Result<String, StoreError> {
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
        Condition::PersistRoot {
            expected, revision, ..
        } => {
            term_eq_string(
                output,
                &format!("{prefix}root"),
                "persistedRootDigest",
                hex::encode(expected.digest),
            );
            term_eq_u64(
                output,
                &format!("{prefix}revision"),
                "persistRevision",
                revision.0,
            );
        }
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

fn term_eq_u64(output: &mut Expression, prefix: &str, attribute: &str, value: u64) {
    let name = format!("#{prefix}");
    let value_name = format!(":{prefix}");
    output.names.insert(name.clone(), attribute.to_owned());
    output.values.insert(value_name.clone(), n(value));
    output.terms.push(format!("{name} = {value_name}"));
}

fn term_eq_string(output: &mut Expression, prefix: &str, attribute: &str, value: String) {
    let name = format!("#{prefix}");
    let value_name = format!(":{prefix}");
    output.names.insert(name.clone(), attribute.to_owned());
    output.values.insert(value_name.clone(), s(value));
    output.terms.push(format!("{name} = {value_name}"));
}

fn conditional_put(
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

fn conditional_check(
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
            | Condition::PersistRoot { .. }
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
        | Condition::MutationGuardHeldBy { session, .. }
        | Condition::PersistRoot { session, .. } => Some(*session),
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

fn cross_tenant() -> StoreError {
    StoreError::Invalid {
        detail: "a session authority write belongs to another tenant".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use aex_session_app::plan::{Condition, SessionTransaction, TransactionIntent, Write};
    use aex_session_domain::testing::running_session;

    use super::{
        ExternalActionCompiler, LogicalAction, SessionBinding, compile_application_transaction,
    };
    use crate::error::StoreError;
    use crate::plan::{RegionalTables, TransactionPlan};

    struct RefuseExternal;

    impl ExternalActionCompiler for RefuseExternal {
        fn compile_action(
            &self,
            _tables: &RegionalTables,
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
    fn create_session_is_one_immutable_canonical_put() {
        let (session, _run, _agent, _message) = running_session();
        let input = SessionTransaction {
            intent: TransactionIntent::CreateSession,
            conditions: Vec::new(),
            writes: vec![Write::PutSessionHead(Box::new(session.clone()))],
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
        assert_eq!(compiled.transaction.len(), 1);
        let put = compiled.transaction.actions()[0]
            .put()
            .expect("canonical session put");
        assert!(
            put.condition_expression()
                .is_some_and(|expression| expression.contains(crate::plan::IMMUTABLE))
        );
    }
}
