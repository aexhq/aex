//! The production `aex-session-app` adapters: the committer and its readers.
//!
//! Before this module there was **no** implementation of
//! [`AuthorityCommitter`] anywhere in the tree — not even a test double — so
//! every one of `aex-session-app`'s finished use cases was unreachable and the
//! crate appeared in exactly one `Cargo.toml`, as an optional dependency of
//! this one. That is the gap this closes.
//!
//! Three rules shape it.
//!
//! **A condition is never dropped, weakened, merged or reordered.**
//! [`compile_application_transaction`] groups the plan's guards by logical item
//! and hands anything this crate's canonical codecs do not own to
//! [`SessionAuthorityExternal`], which either renders it as exactly one
//! provider action or refuses. There is no arm that skips a guard it does not
//! recognise.
//!
//! **A hint is dispatched strictly after the commit succeeds, and this
//! deployable dispatches none.** `AEX_OPERATION_QUEUE_URL` is in
//! `session-stream-api`'s forbidden configuration set, so the API cannot send a
//! queue message at all. The durable work row is the delivery, projected by the
//! existing streams pipe, with the scheduled due scan as the backstop (D-9).
//! [`HintSink`] exists so that fact is a type rather than a comment.
//!
//! **An unknown outcome is typed.** A transport failure on a transaction is
//! [`CommitError::Ambiguous`] carrying every target the plan would have
//! written, never a definite failure and never a blind retry (D-10 rows 6-8).

use aex_internal_contracts::RunId;
use aex_operation_domain::Operation;
use aex_operation_domain::cursor::ContinuationCursor;
use aex_operation_domain::operation::OperationVersion;
use aex_session_app::plan::{
    Condition, ConditionId, Hint, ItemKey, SessionTransaction, TableFamily, Write,
};
use aex_session_app::ports::{
    AccountStateReader, AgentCancelPage, AgentCancelTarget, AgentPage, AuthorityCommitter, Clock,
    CommitError, CommitOutcome, MessageAdmissionSnapshot, PageBudget, PortError,
    RootAdmissionState, SessionReader, VersionedOperation,
};
use aex_session_domain::{
    AccountProjection, AccountRevision, AccountState, AgentControl, AgentRevision,
    CancellationEpoch, IdempotencyIdentity, IdempotencyReceipt, JournalPage, JournalSeq,
    PauseReason, Run, Session,
};
use aex_wire::ids::{AgentId, OperationId, OrganizationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;

use crate::application_plan::{
    AuthorityBinding, CompiledApplicationPlan, Expression, ExternalActionCompiler, LogicalAction,
    SessionBinding, WorkspaceBinding, compile_application_transaction,
    compile_workspace_transaction, conditional_check, conditional_put, cross_tenant, term_eq_u64,
};
use crate::attr::{Item, Row, b, s, stamp};
use crate::error::StoreError;
use crate::plan::{IMMUTABLE, Participant, RegionalTables, TransactionPlan, key};
use crate::wire_pending::StoredOperation;

/// The `finishReason` a cancelled agent row carries.
///
/// The physical `agent_control` row is `aex-brain-store-dynamodb`'s and its
/// finish vocabulary is that crate's `FinishReason`. `cancelled` is the spelling
/// its decoder accepts, so a row settled from this side stays decodable there
/// rather than becoming a value the owning stream refuses.
const FINISH_CANCELLED: &str = "cancelled";

/// The `status` phase a cancelled agent row carries.
const PHASE_CANCELLED: &str = "cancelled";

/// One agent settlement, as the plan named it.
#[derive(Debug, Clone, Copy)]
struct AgentSettlement {
    session: SessionId,
    agent: AgentId,
    from: AgentRevision,
    to: AgentRevision,
    at: Timestamp,
}

// ---------------------------------------------------------------------------
// The external compiler
// ---------------------------------------------------------------------------

/// Compiles the session-authority families the canonical codec does not own.
///
/// `compile_owned` in [`crate::application_plan`] handles the session head, the
/// message and the run. Everything else a B1 command writes — the durable
/// operation row and one settled agent — arrives here.
///
/// The tenant arrives **both** ways, and that is deliberate.
/// [`ExternalActionCompiler`] hands over the binding the regional edge
/// asserted, and this compiler also holds the one it was constructed against.
/// Two sources for one fact could disagree — so a disagreement is a
/// `cross_tenant` refusal rather than a silent choice between two tenants,
/// which is the only answer that cannot pick the wrong one.
#[derive(Debug, Clone, Copy)]
pub struct SessionAuthorityExternal {
    /// The binding this compiler was constructed against.
    ///
    /// Held as well as received, so that a root compiler handing down a
    /// different one is a refusal rather than a silent choice between two
    /// tenants — see the check in `compile_action`.
    binding: SessionBinding,
}

impl SessionAuthorityExternal {
    /// Binds the foreign compiler to the tenant the request was authorized for.
    #[must_use]
    pub const fn new(binding: SessionBinding) -> Self {
        Self { binding }
    }

    fn operation_write(
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        operation: &Operation,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        if operation.workspace != binding.workspace
            || operation
                .session
                .is_some_and(|session| Some(session) != binding.session)
        {
            return Err(cross_tenant());
        }
        let mut expression = Expression::default();
        let mut resuming = false;
        let mut observed: Option<OperationVersion> = None;
        for (id, condition) in &action.conditions {
            match condition {
                Condition::OperationCursorAt { expected, .. } => {
                    match expected {
                        // A step names the cursor it advances *from*, so a
                        // duplicate delivery fails here and writes nothing.
                        Some(cursor) => {
                            resuming = true;
                            expression.and_literal("attribute_exists(pk)");
                            term_eq_bytes(
                                &mut expression,
                                &format!("c{}", id.0),
                                "continuationCursor",
                                encoded(cursor)?,
                            );
                        }
                        // `None` is the admission of a continued operation: the
                        // row must not exist yet.
                        None => expression.and_literal(IMMUTABLE),
                    }
                }
                Condition::OperationVersion { expected, .. } => {
                    observed = Some(*expected);
                    term_eq_u64(
                        &mut expression,
                        &format!("c{}", id.0),
                        "version",
                        expected.0,
                    );
                }
                other => return Err(unsupported_condition(other)),
            }
        }
        // The row a step writes has a second writer: the already-served public
        // cancellation runs its own optimistic loop over `version` on this same
        // item. A step that did not name the version it observed would have to
        // guess one, and every guess either resets the counter or lets two
        // writers believe they hold the row.
        let version = match (resuming, observed) {
            (true, Some(expected)) => expected.next(),
            (true, None) => {
                return Err(StoreError::Invalid {
                    detail: "a resumed operation step must carry `Condition::OperationVersion`: \
                             the row's optimistic version has a second writer and guessing it \
                             would corrupt the cancellation authority's loop"
                        .to_owned(),
                });
            }
            (false, Some(_)) => {
                return Err(StoreError::Invalid {
                    detail: "an operation admission cannot name a version it observed: the row \
                             does not exist yet and is born at the first version"
                        .to_owned(),
                });
            }
            // Every non-resuming operation write is an admission, and an
            // admission is an insert. That is what makes row 3 of the
            // unknown-outcome matrix — "my write already landed" — a condition
            // failure the caller can resolve rather than a silent overwrite of
            // somebody else's envelope.
            (false, None) => {
                expression.and_literal(IMMUTABLE);
                OperationVersion::FIRST
            }
        };
        let item = crate::codec::encode_operation(&StoredOperation {
            record: operation.clone(),
            version: version.0,
        })?;
        output.put(
            Participant::SESSION_OPERATION,
            conditional_put(&tables.session_authority, item, expression)?,
        )?;
        Ok(())
    }

    fn agent_cancel(
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        settlement: AgentSettlement,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        let AgentSettlement {
            session,
            agent,
            from,
            to,
            at,
        } = settlement;
        if Some(session) != binding.session {
            return Err(cross_tenant());
        }
        let mut expression = Expression::default();
        expression.and_literal("attribute_exists(pk)");
        for (id, condition) in &action.conditions {
            match condition {
                Condition::AgentRevision {
                    expected,
                    agent: guarded,
                    ..
                } => {
                    if *guarded != agent {
                        return Err(StoreError::Invalid {
                            detail: "an agent guard and its write name different agents".to_owned(),
                        });
                    }
                    term_eq_u64(
                        &mut expression,
                        &format!("c{}", id.0),
                        "revision",
                        expected.0,
                    );
                }
                other => return Err(unsupported_condition(other)),
            }
        }
        if from.next() != to {
            return Err(StoreError::Invalid {
                detail: "an agent settlement must advance the revision by exactly one".to_owned(),
            });
        }
        let physical = crate::keys::agent_control(session, agent);
        let mut names = expression.names.clone();
        let mut values = expression.values.clone();
        names.insert("#status".to_owned(), "status".to_owned());
        names.insert("#finishReason".to_owned(), "finishReason".to_owned());
        names.insert("#revision".to_owned(), "revision".to_owned());
        names.insert("#updatedAt".to_owned(), "updatedAt".to_owned());
        values.insert(":cancelledPhase".to_owned(), s(PHASE_CANCELLED));
        values.insert(":cancelledFinish".to_owned(), s(FINISH_CANCELLED));
        values.insert(":nextRevision".to_owned(), crate::attr::n(to.0));
        values.insert(":settledAt".to_owned(), stamp(at));
        let rendered = expression.rendered()?;
        // An update, not a put. `aex_session_domain::AgentControl` has no row
        // codec anywhere in the tree and the physical row belongs to
        // `aex-brain-store-dynamodb`, so a whole-record write from here would
        // drop every attribute that stream owns. This names exactly the four
        // attributes the domain cancellation establishes and removes the claim
        // the cancelled agent no longer holds.
        output.update(
            Participant::AGENT_CONTROL,
            aws_sdk_dynamodb::types::Update::builder()
                .table_name(&tables.session_authority)
                .set_key(Some(key(&physical.pk, &physical.sk)))
                .update_expression(
                    "SET #status = :cancelledPhase, #finishReason = :cancelledFinish, \
                     #revision = :nextRevision, #updatedAt = :settledAt \
                     REMOVE claimOwner, leaseExpiresAt",
                )
                .condition_expression(rendered)
                .set_expression_attribute_names(Some(names))
                .set_expression_attribute_values(Some(values)),
        )?;
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the exact root revision, journal fence and immutable entry facts form one closed atomic update"
    )]
    fn root_run_admission(
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        session: SessionId,
        agent: AgentId,
        from_revision: AgentRevision,
        to_revision: AgentRevision,
        from_tail: JournalSeq,
        to_tail: JournalSeq,
        entry_identity: aex_session_domain::EntryIdentity,
        at: Timestamp,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        if Some(session) != binding.session
            || from_revision.next() != to_revision
            || from_tail.next() != to_tail
        {
            return Err(StoreError::Invalid {
                detail: "a root run admission crosses its session or does not advance exactly one revision and journal position".to_owned(),
            });
        }
        let mut expression = Expression::default();
        expression.and_literal("attribute_exists(pk)");
        let mut saw_revision = false;
        let mut saw_tail = false;
        let mut saw_idle = false;
        for (id, condition) in &action.conditions {
            match condition {
                Condition::AgentRevision {
                    session: guarded_session,
                    agent: guarded_agent,
                    expected,
                } if (*guarded_session, *guarded_agent, *expected)
                    == (session, agent, from_revision) =>
                {
                    saw_revision = true;
                    term_eq_u64(
                        &mut expression,
                        &format!("c{}", id.0),
                        "revision",
                        expected.0,
                    );
                }
                Condition::JournalTail {
                    session: guarded_session,
                    agent: guarded_agent,
                    expected,
                } if (*guarded_session, *guarded_agent, *expected)
                    == (session, agent, from_tail) =>
                {
                    saw_tail = true;
                    term_eq_u64(
                        &mut expression,
                        &format!("c{}", id.0),
                        "journalTail",
                        expected.0,
                    );
                }
                Condition::RootAgentIdle {
                    session: guarded_session,
                    agent: guarded_agent,
                } if (*guarded_session, *guarded_agent) == (session, agent) => {
                    saw_idle = true;
                    let prefix = format!("c{}", id.0);
                    let name = format!("#{prefix}");
                    let value = format!(":{prefix}");
                    expression.names.insert(name.clone(), "status".to_owned());
                    expression.values.insert(value.clone(), s("idle"));
                    expression.terms.push(format!("{name} = {value}"));
                    expression.and_literal("attribute_not_exists(claimOwner)");
                    expression.and_literal("attribute_not_exists(finishReason)");
                }
                other => return Err(unsupported_condition(other)),
            }
        }
        if !(saw_revision && saw_tail && saw_idle) {
            return Err(StoreError::Invalid {
                detail: "a root run admission requires exact revision, journal-tail and idle-owner guards".to_owned(),
            });
        }
        let rendered = expression.rendered()?;
        let mut names = expression.names;
        let mut values = expression.values;
        names.insert("#status".to_owned(), "status".to_owned());
        values.insert(":running".to_owned(), s("awaiting_model"));
        values.insert(":nextRevision".to_owned(), crate::attr::n(to_revision.0));
        values.insert(":nextTail".to_owned(), crate::attr::n(to_tail.0));
        values.insert(
            ":nextTailHash".to_owned(),
            s(hex::encode(entry_identity.as_bytes())),
        );
        values.insert(":hasJournal".to_owned(), crate::attr::boolean(true));
        values.insert(":now".to_owned(), stamp(at));
        let physical = crate::keys::agent_control(session, agent);
        output.update(
            Participant::AGENT_ROOT_CONTROL,
            aws_sdk_dynamodb::types::Update::builder()
                .table_name(&tables.session_authority)
                .set_key(Some(key(&physical.pk, &physical.sk)))
                .condition_expression(rendered)
                .update_expression(
                    "SET #status = :running, revision = :nextRevision, journalTail = :nextTail, \
                     journalTailHash = :nextTailHash, hasJournal = :hasJournal, updatedAt = :now",
                )
                .set_expression_attribute_names(Some(names))
                .set_expression_attribute_values(Some(values)),
        )?;
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the narrow Brain-control transition names both sides of its two monotonic fences"
    )]
    fn root_cancellation_request(
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        session: SessionId,
        agent: AgentId,
        from_revision: AgentRevision,
        to_revision: AgentRevision,
        from_cancellation: CancellationEpoch,
        to_cancellation: CancellationEpoch,
        reason: aex_session_app::plan::RootStopReason,
        at: Timestamp,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        if Some(session) != binding.session
            || from_revision.next() != to_revision
            || from_cancellation.next() != to_cancellation
        {
            return Err(StoreError::Invalid {
                detail: "a root cancellation must stay in its session and advance both fences exactly once"
                    .to_owned(),
            });
        }
        let mut expression = Expression::default();
        expression.and_literal("attribute_exists(pk)");
        expression.and_literal("attribute_not_exists(finishReason)");
        expression.and_literal("cancelEpoch = :fromCancellation");
        let mut saw_revision = false;
        for (id, condition) in &action.conditions {
            match condition {
                Condition::AgentRevision {
                    session: guarded_session,
                    agent: guarded_agent,
                    expected,
                } if (*guarded_session, *guarded_agent, *expected)
                    == (session, agent, from_revision) =>
                {
                    saw_revision = true;
                    term_eq_u64(
                        &mut expression,
                        &format!("c{}", id.0),
                        "revision",
                        expected.0,
                    );
                }
                other => return Err(unsupported_condition(other)),
            }
        }
        if !saw_revision {
            return Err(StoreError::Invalid {
                detail: "a root cancellation requires the exact control revision it observed"
                    .to_owned(),
            });
        }
        expression.values.insert(
            ":fromCancellation".to_owned(),
            crate::attr::n(from_cancellation.0),
        );
        let rendered = expression.rendered()?;
        let physical = crate::keys::agent_control(session, agent);
        output.update(
            Participant::AGENT_ROOT_CONTROL,
            aws_sdk_dynamodb::types::Update::builder()
                .table_name(&tables.session_authority)
                .set_key(Some(key(&physical.pk, &physical.sk)))
                .condition_expression(rendered)
                .update_expression(
                    "SET stopRequested = :requested, cancelEpoch = :nextCancellation, \
                     stopReason = :stopReason, revision = :nextRevision, updatedAt = :now",
                )
                .set_expression_attribute_names(Some(expression.names))
                .set_expression_attribute_values(Some({
                    let mut values = expression.values;
                    values.insert(":requested".to_owned(), crate::attr::boolean(true));
                    values.insert(":stopReason".to_owned(), s(reason.as_str()));
                    values.insert(
                        ":nextCancellation".to_owned(),
                        crate::attr::n(to_cancellation.0),
                    );
                    values.insert(":nextRevision".to_owned(), crate::attr::n(to_revision.0));
                    values.insert(":now".to_owned(), stamp(at));
                    values
                })),
        )?;
        Ok(())
    }

    fn journal_append(
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        session: SessionId,
        page: &JournalPage,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        if Some(session) != binding.session || page.entries.len() != 1 {
            return Err(StoreError::Invalid {
                detail: "an admission appends exactly one journal record in its bound session"
                    .to_owned(),
            });
        }
        let entry = &page.entries[0];
        if page.first != entry.seq || page.agent != entry.agent {
            return Err(StoreError::Invalid {
                detail: "a journal page disagrees with its one entry".to_owned(),
            });
        }
        let aex_session_domain::JournalBody::Inline(body) = &entry.body else {
            return Err(StoreError::Invalid {
                detail: "the MVP Brain journal admits inline records only".to_owned(),
            });
        };
        let kind = serde_json::to_value(entry.kind)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| StoreError::Invalid {
                detail: "a journal kind has no canonical wire spelling".to_owned(),
            })?;
        let item = crate::codec::encode_journal(
            session,
            entry.agent,
            &crate::wire_pending::JournalEntry {
                seq: entry.seq.0,
                entry_id: hex::encode(entry.identity.as_bytes()),
                kind,
                body: crate::wire_pending::Body::Inline(body.clone()),
                body_bytes: u64::try_from(body.len()).unwrap_or(u64::MAX),
                occurred_at: entry.recorded_at,
            },
        );
        let mut expression = Expression::default();
        if !action.conditions.is_empty() {
            return Err(StoreError::Invalid {
                detail: "an immutable journal append carries no mutable predicate".to_owned(),
            });
        }
        expression.and_literal(IMMUTABLE);
        output.put(
            Participant::AGENT_JOURNAL,
            conditional_put(&tables.session_authority, item, expression)?,
        )?;
        Ok(())
    }

    fn read_only(
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        let (participant, physical, agent_guard) = match action.target.family {
            TableFamily::SessionAuthority => {
                let (session, agent) = agent_of(action)?;
                if Some(session) != binding.session {
                    return Err(cross_tenant());
                }
                (
                    Participant::AGENT_ROOT_CONTROL,
                    crate::keys::agent_control(session, agent),
                    true,
                )
            }
            TableFamily::OperationAuthority => {
                let operation = operation_of(action)?;
                (
                    Participant::SESSION_OPERATION,
                    crate::keys::operation(operation),
                    false,
                )
            }
            _ => {
                return Err(StoreError::Invalid {
                    detail: format!(
                        "a read-only guard group on `{:?}` has no compiler in the session \
                         authority adapter",
                        action.target.family
                    ),
                });
            }
        };
        let mut expression = Expression::default();
        if agent_guard {
            expression.and_literal("attribute_exists(pk)");
        }
        for (id, condition) in &action.conditions {
            match condition {
                Condition::AgentRevision { expected, .. } => {
                    term_eq_u64(
                        &mut expression,
                        &format!("c{}", id.0),
                        "revision",
                        expected.0,
                    );
                }
                Condition::JournalTail { expected, .. } => {
                    term_eq_u64(
                        &mut expression,
                        &format!("c{}", id.0),
                        "journalTail",
                        expected.0,
                    );
                }
                Condition::JournalTailHash { expected, .. } => {
                    crate::application_plan::term_eq_string(
                        &mut expression,
                        &format!("c{}", id.0),
                        "journalTailHash",
                        hex::encode(expected),
                    );
                }
                Condition::OperationCursorAt { expected, .. } => match expected {
                    Some(cursor) => {
                        expression.and_literal("attribute_exists(pk)");
                        term_eq_bytes(
                            &mut expression,
                            &format!("c{}", id.0),
                            "continuationCursor",
                            encoded(cursor)?,
                        );
                    }
                    None => expression.and_literal(IMMUTABLE),
                },
                Condition::OperationFence { fence, .. } => {
                    term_eq_u64(&mut expression, &format!("c{}", id.0), "fence", fence.0);
                }
                Condition::OperationVersion { expected, .. } => {
                    term_eq_u64(
                        &mut expression,
                        &format!("c{}", id.0),
                        "version",
                        expected.0,
                    );
                }
                other => return Err(unsupported_condition(other)),
            }
        }
        output.condition_check(
            participant,
            conditional_check(&tables.session_authority, &physical, expression)?,
        )?;
        Ok(())
    }
}

fn operation_of(action: &LogicalAction<'_>) -> Result<OperationId, StoreError> {
    action
        .conditions
        .iter()
        .find_map(|(_, condition)| match condition {
            Condition::OperationCursorAt { operation, .. }
            | Condition::OperationFence { operation, .. }
            | Condition::OperationVersion { operation, .. } => Some(*operation),
            _ => None,
        })
        .ok_or_else(|| StoreError::Invalid {
            detail: "an operation guard group has no operation identity".to_owned(),
        })
}

fn agent_of(action: &LogicalAction<'_>) -> Result<(SessionId, AgentId), StoreError> {
    let mut found = None;
    for (_, condition) in &action.conditions {
        let candidate = match condition {
            Condition::AgentRevision { session, agent, .. }
            | Condition::JournalTail { session, agent, .. }
            | Condition::JournalTailHash { session, agent, .. } => (*session, *agent),
            _ => continue,
        };
        if found.is_some_and(|existing| existing != candidate) {
            return Err(StoreError::Invalid {
                detail: "one root-readiness guard group names different agents".to_owned(),
            });
        }
        found = Some(candidate);
    }
    found.ok_or_else(|| StoreError::Invalid {
        detail: "a root-readiness guard group has no agent identity".to_owned(),
    })
}

fn encoded(cursor: &ContinuationCursor) -> Result<Vec<u8>, StoreError> {
    cursor.encode().map_err(|error| StoreError::Invalid {
        detail: format!("a cursor guard could not be encoded: {error}"),
    })
}

fn term_eq_bytes(output: &mut Expression, prefix: &str, attribute: &str, value: Vec<u8>) {
    let name = format!("#{prefix}");
    let value_name = format!(":{prefix}");
    output.names.insert(name.clone(), attribute.to_owned());
    output.values.insert(value_name.clone(), b(value));
    output.terms.push(format!("{name} = {value_name}"));
}

fn unsupported_condition(condition: &Condition) -> StoreError {
    StoreError::Invalid {
        detail: format!(
            "condition `{}` reached the session authority adapter, which does not own its item \
             family",
            condition_tag(condition)
        ),
    }
}

const fn condition_tag(condition: &Condition) -> &'static str {
    match condition {
        Condition::SessionRevision { .. } => "SessionRevision",
        Condition::SessionStatusIn { .. } => "SessionStatusIn",
        Condition::SessionActiveRun { .. } => "SessionActiveRun",
        Condition::WorkAdmission { .. } => "WorkAdmission",
        Condition::DeletionState { .. } => "DeletionState",
        Condition::CancellationEpoch { .. } => "CancellationEpoch",
        Condition::MutationGuardFree { .. } => "MutationGuardFree",
        Condition::MutationGuardHeldBy { .. } => "MutationGuardHeldBy",
        Condition::AgentRevision { .. } => "AgentRevision",
        Condition::AgentFence { .. } => "AgentFence",
        Condition::JournalTail { .. } => "JournalTail",
        Condition::RootAgentIdle { .. } => "RootAgentIdle",
        Condition::JournalTailHash { .. } => "JournalTailHash",
        Condition::RunNonTerminal { .. } => "RunNonTerminal",
        Condition::AccountActiveAtLeast { .. } => "AccountActiveAtLeast",
        Condition::AccountRevisionAtLeast { .. } => "AccountRevisionAtLeast",
        Condition::ProviderCredentialReady { .. } => "ProviderCredentialReady",
        Condition::AuthorizationEpochAtLeast { .. } => "AuthorizationEpochAtLeast",
        Condition::RegistryEtag { .. } => "RegistryEtag",
        Condition::UploadState { .. } => "UploadState",
        Condition::ContentOwned { .. } => "ContentOwned",
        Condition::GrantUnexpired { .. } => "GrantUnexpired",
        Condition::OperationFence { .. } => "OperationFence",
        Condition::OperationCursorAt { .. } => "OperationCursorAt",
        Condition::OperationVersion { .. } => "OperationVersion",
        Condition::SecretRevocationEpoch { .. } => "SecretRevocationEpoch",
        Condition::CustodyRevision { .. } => "CustodyRevision",
        Condition::ItemAbsent(_) => "ItemAbsent",
        Condition::ItemPresent(_) => "ItemPresent",
    }
}

const fn write_tag(write: &Write) -> &'static str {
    match write {
        Write::PutSessionHead(_) => "PutSessionHead",
        Write::PutSessionDeletionHead(_) => "PutSessionDeletionHead",
        Write::PutMessage(_) => "PutMessage",
        Write::PutSealedMessage(_) => "PutSealedMessage",
        Write::PutRun(_) => "PutRun",
        Write::PutAgentControl(_) => "PutAgentControl",
        Write::AdmitRootRun { .. } => "AdmitRootRun",
        Write::RequestRootCancellation { .. } => "RequestRootCancellation",
        Write::CancelAgent { .. } => "CancelAgent",
        Write::AppendJournalPage { .. } => "AppendJournalPage",
        Write::PutApproval(_) => "PutApproval",
        Write::PutIdempotencyReceipt(_) => "PutIdempotencyReceipt",
        Write::PutSessionReceiptDirectory { .. } => "PutSessionReceiptDirectory",
        Write::PutActiveSession(_) => "PutActiveSession",
        Write::DeleteActiveSession { .. } => "DeleteActiveSession",
        Write::PutOperation(_) => "PutOperation",
        Write::PutSessionOperationEdge { .. } => "PutSessionOperationEdge",
        Write::RedactOperationResult(_) => "RedactOperationResult",
        Write::PutWorkItem(_) => "PutWorkItem",
        Write::PutAgentWake(_) => "PutAgentWake",
        Write::PutAgentWakeDedupe(_) => "PutAgentWakeDedupe",
        Write::CompleteWorkItem(_) => "CompleteWorkItem",
        Write::PutMessageAdmittedEvent(_) => "PutMessageAdmittedEvent",
        Write::PutPin(_) => "PutPin",
        Write::DeletePin(_) => "DeletePin",
        Write::PutRegistryPointer(_) => "PutRegistryPointer",
        Write::PutUpload(_) => "PutUpload",
        Write::PutGrant(_) => "PutGrant",
        Write::PutCustody(_) => "PutCustody",
        Write::PutSecret(_) => "PutSecret",
        Write::PutTombstone(_) => "PutTombstone",
        Write::DeleteItem(_) => "DeleteItem",
    }
}

impl ExternalActionCompiler for SessionAuthorityExternal {
    // MERGE ARTIFACT, not this cluster's change: `binding` was added to the
    // trait by the idempotency-identity work and this impl was merged without
    // it, so `aex-session-dynamodb` did not compile at `4b9d9b89`. It is
    // accepted and asserted here rather than ignored: every action this compiler
    // renders is a session-authority row, so the plan's asserted tenant must be
    // the one whose rows it is about to write. The owning lane should confirm
    // this is the check it intended.
    fn compile_action(
        &self,
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        // The root compiler hands the asserted binding to every foreign
        // compiler so none of them has to hold its own. This one was
        // constructed with a binding as well, and two bindings that disagree
        // would mean the tenant check below ran against a tenant the request
        // was not authorized for. Refusing is the only answer that cannot
        // silently pick the wrong one.
        if binding != self.binding.into() {
            return Err(cross_tenant());
        }
        match action.write {
            None => Self::read_only(tables, binding, action, output),
            Some(Write::PutOperation(operation)) => {
                Self::operation_write(tables, binding, action, operation, output)
            }
            Some(Write::CancelAgent {
                session,
                agent,
                from_revision,
                to_revision,
                at,
            }) => Self::agent_cancel(
                tables,
                binding,
                action,
                AgentSettlement {
                    session: *session,
                    agent: *agent,
                    from: *from_revision,
                    to: *to_revision,
                    at: *at,
                },
                output,
            ),
            Some(Write::AdmitRootRun {
                session,
                agent,
                from_revision,
                to_revision,
                from_tail,
                to_tail,
                entry_identity,
                at,
            }) => Self::root_run_admission(
                tables,
                binding,
                action,
                *session,
                *agent,
                *from_revision,
                *to_revision,
                *from_tail,
                *to_tail,
                *entry_identity,
                *at,
                output,
            ),
            Some(Write::RequestRootCancellation {
                session,
                agent,
                from_revision,
                to_revision,
                from_cancellation,
                to_cancellation,
                reason,
                at,
            }) => Self::root_cancellation_request(
                tables,
                binding,
                action,
                *session,
                *agent,
                *from_revision,
                *to_revision,
                *from_cancellation,
                *to_cancellation,
                *reason,
                *at,
                output,
            ),
            Some(Write::AppendJournalPage { session, page }) => {
                Self::journal_append(tables, binding, action, *session, page, output)
            }
            Some(other) => Err(StoreError::Invalid {
                detail: format!(
                    "write `{}` has no compiler in the session authority adapter",
                    write_tag(other)
                ),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// The committer
// ---------------------------------------------------------------------------

/// Where an after-commit hint goes.
///
/// A trait rather than an inline call so that "this deployable dispatches no
/// hint" is a type somebody has to choose, not an omission somebody has to
/// notice. See [`ApiHintSink`].
pub trait HintSink: Send + Sync {
    /// Dispatches one hint. Called strictly after the commit succeeded.
    fn dispatch(&self, hint: &Hint);
}

/// The transaction committer's API hint sink: no I/O at all, by construction.
///
/// `AEX_OPERATION_QUEUE_URL` and `AEX_CONTENT_QUEUE_URL` are in
/// `session-stream-api`'s forbidden configuration set, so the API is
/// structurally unable to send a queue message. Lifecycle routes explicitly
/// invoke the isolated operation-worker Lambda only after this committer has
/// returned an authoritative commit or replay result. Keeping this sink inert
/// prevents any pre-commit notification from escaping the transaction boundary.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiHintSink;

impl HintSink for ApiHintSink {
    fn dispatch(&self, _hint: &Hint) {}
}

/// Submits one [`SessionTransaction`] as one `TransactWriteItems`.
pub struct DynamoAuthorityCommitter<S: HintSink> {
    client: Client,
    tables: RegionalTables,
    binding: AuthorityBinding,
    now: Timestamp,
    hints: S,
}

impl<S: HintSink> std::fmt::Debug for DynamoAuthorityCommitter<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DynamoAuthorityCommitter")
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}

impl<S: HintSink> DynamoAuthorityCommitter<S> {
    /// Binds a committer to one verified request.
    ///
    /// The instant is the edge's receipt time rather than a clock read here:
    /// the plan was built against it, and a second clock would let the row's
    /// `committedAt` disagree with the operation the caller is shown.
    #[must_use]
    pub const fn new(
        client: Client,
        tables: RegionalTables,
        binding: SessionBinding,
        now: Timestamp,
        hints: S,
    ) -> Self {
        Self {
            client,
            tables,
            binding: AuthorityBinding {
                workspace: binding.workspace,
                organization: binding.organization,
                session: Some(binding.session),
            },
            now,
            hints,
        }
    }

    /// Binds a committer to a workspace-scoped authority request.
    #[must_use]
    pub const fn new_workspace(
        client: Client,
        tables: RegionalTables,
        binding: WorkspaceBinding,
        now: Timestamp,
        hints: S,
    ) -> Self {
        Self {
            client,
            tables,
            binding: AuthorityBinding {
                workspace: binding.workspace,
                organization: binding.organization,
                session: None,
            },
            now,
            hints,
        }
    }

    fn compile_with(
        &self,
        plan: &SessionTransaction,
        external: &impl ExternalActionCompiler,
    ) -> Result<CompiledApplicationPlan, StoreError> {
        match self.binding.session {
            Some(session) => compile_application_transaction(
                &self.tables,
                plan,
                SessionBinding {
                    workspace: self.binding.workspace,
                    organization: self.binding.organization,
                    session,
                },
                external,
            ),
            None => compile_workspace_transaction(
                &self.tables,
                plan,
                WorkspaceBinding {
                    workspace: self.binding.workspace,
                    organization: self.binding.organization,
                },
                external,
            ),
        }
    }

    /// Commits a replay-elected multi-family plan without erasing the
    /// participant that lost its condition.
    ///
    /// The shared replay combinator needs that participant to distinguish a
    /// receipt race from a grant collision. Transport ambiguity resolves only
    /// through the receipt; this method never retries an unknown write.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when compilation refuses the participant set or
    /// the provider cannot establish a committed outcome.
    pub async fn commit_replayable(
        &self,
        plan: &SessionTransaction,
        external: &impl ExternalActionCompiler,
    ) -> Result<(), StoreError> {
        self.commit_replayable_resolving(
            plan,
            external,
            crate::error::Resolution::IdempotencyReceipt,
        )
        .await
    }

    /// Commits a multi-family plan whose ambiguous outcome is resolved by the
    /// caller-selected durable authority.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when compilation refuses the participant set or
    /// the provider cannot establish a committed outcome under `resolution`.
    pub async fn commit_replayable_resolving(
        &self,
        plan: &SessionTransaction,
        external: &impl ExternalActionCompiler,
        resolution: crate::error::Resolution,
    ) -> Result<(), StoreError> {
        let compiled = self.compile_with(plan, external)?;
        let request = compiled.transaction.compile(&self.client)?;
        match request.send().await {
            Ok(_) => {
                for hint in &compiled.after_commit {
                    self.hints.dispatch(hint);
                }
                Ok(())
            }
            Err(error) => Err(match error.as_service_error() {
                Some(service) => {
                    crate::error::decode_cancellation(service, compiled.transaction.participants())
                }
                None => {
                    crate::error::classify(&error, crate::error::Idempotence::Write(resolution))
                }
            }),
        }
    }
}

#[async_trait::async_trait]
impl<S: HintSink> AuthorityCommitter for DynamoAuthorityCommitter<S> {
    async fn commit(&self, plan: &SessionTransaction) -> Result<CommitOutcome, CommitError> {
        // A use case that decided to do nothing — an exact operation replay —
        // returns a plan with no guard and no write. Submitting it is a
        // provider error ("a transaction plan with no action is always a
        // planning bug"), and inventing an action to carry it would write on a
        // replay. Reporting the commit as already made is the truth: the
        // original admission made it.
        if plan.conditions.is_empty() && plan.writes.is_empty() {
            return Ok(CommitOutcome {
                committed_at: self.now,
            });
        }
        let session = self.binding.session.ok_or_else(|| {
            CommitError::PlanRejected(aex_session_app::plan::PlanError::MissingRequiredCondition(
                ConditionId(u16::MAX),
            ))
        })?;
        let session_binding = SessionBinding {
            workspace: self.binding.workspace,
            organization: self.binding.organization,
            session,
        };
        let operations = SessionAuthorityExternal::new(session_binding);
        let families = crate::application_plan::FamilyCompilers::new()
            .with(TableFamily::OperationAuthority, &operations);
        let compiled = self
            .compile_with(plan, &families)
            .map_err(|error| store_to_commit(&error, plan))?;

        let request = compiled
            .transaction
            .compile(&self.client)
            .map_err(|error| store_to_commit(&error, plan))?;
        match request.send().await {
            Ok(_) => {
                for hint in &compiled.after_commit {
                    self.hints.dispatch(hint);
                }
                Ok(CommitOutcome {
                    committed_at: self.now,
                })
            }
            Err(error) => {
                let Some(service) = error.as_service_error() else {
                    // Transport ambiguity. The write may or may not have
                    // landed, so the only honest answer is the ambiguous one
                    // with every target named (D-10 rows 7-8).
                    return Err(CommitError::Ambiguous {
                        targets: targets_of(plan),
                    });
                };
                Err(decode(service, &compiled.condition_groups, plan))
            }
        }
    }
}

fn targets_of(plan: &SessionTransaction) -> Vec<ItemKey> {
    plan.writes.iter().map(Write::target).collect()
}

/// Maps a provider cancellation onto the plan's own condition identities.
///
/// Positional: `condition_groups[i]` are the logical guards compiled into
/// physical action *i*, so reason *i* names them exactly. Every failing action
/// is collected, not just the first — a caller that is told only about the
/// first failed guard cannot distinguish "the revision moved" from "the
/// revision moved *and* an agent moved".
fn decode(
    error: &aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError,
    condition_groups: &[Vec<ConditionId>],
    plan: &SessionTransaction,
) -> CommitError {
    use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError as Failure;

    let Failure::TransactionCanceledException(cancelled) = error else {
        return match error {
            Failure::ProvisionedThroughputExceededException(_)
            | Failure::RequestLimitExceeded(_) => CommitError::Throttled,
            Failure::InternalServerError(_) => CommitError::Ambiguous {
                targets: targets_of(plan),
            },
            _ => CommitError::Unavailable,
        };
    };
    let reasons = cancelled.cancellation_reasons();
    if reasons.len() != condition_groups.len() {
        // A reason vector that does not line up with the plan makes every
        // positional mapping a guess. Refusing is the only answer that cannot
        // attribute a failure to the wrong guard.
        return CommitError::Ambiguous {
            targets: targets_of(plan),
        };
    }
    let mut failed = Vec::new();
    let mut throttled = false;
    let mut conflicted = false;
    for (index, reason) in reasons.iter().enumerate() {
        match reason.code().unwrap_or("None") {
            "None" => {}
            "ConditionalCheckFailed" => {
                if let Some(group) = condition_groups.get(index) {
                    failed.extend(group.iter().copied());
                }
            }
            "TransactionConflict" => conflicted = true,
            "ProvisionedThroughputExceeded" | "ThrottlingError" => throttled = true,
            _ => {
                return CommitError::Ambiguous {
                    targets: targets_of(plan),
                };
            }
        }
    }
    if !failed.is_empty() {
        return CommitError::ConditionFailed { failed };
    }
    if throttled || conflicted {
        return CommitError::Throttled;
    }
    CommitError::Unavailable
}

fn store_to_commit(error: &StoreError, plan: &SessionTransaction) -> CommitError {
    match error {
        StoreError::Throttled { .. } | StoreError::Contended => CommitError::Throttled,
        StoreError::CommitAmbiguous { .. } => CommitError::Ambiguous {
            targets: targets_of(plan),
        },
        StoreError::Unavailable { .. } => CommitError::Unavailable,
        // Everything else is a planning fault: the plan named something this
        // adapter cannot render. It is never a customer error, and it must not
        // masquerade as a condition failure.
        _ => CommitError::PlanRejected(aex_session_app::plan::PlanError::MissingRequiredCondition(
            ConditionId(u16::MAX),
        )),
    }
}

// ---------------------------------------------------------------------------
// The readers
// ---------------------------------------------------------------------------

/// A clock frozen at one request's edge receipt time.
///
/// The edge already stamped the request, and every row this command writes has
/// to agree with the operation the caller is shown. A second clock read inside
/// the command path would let them differ by the request's own duration.
#[derive(Debug, Clone, Copy)]
pub struct RequestClock(pub Timestamp);

impl Clock for RequestClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// The account projection the regional edge already established.
///
/// Zero extra reads: the edge resolved the account state from the
/// `regional-authz-projection` row during admission, and re-reading it in the
/// command path would buy nothing a condition does not already buy (D-11, P-4).
#[derive(Debug, Clone, Copy)]
pub struct AuthorizedAccount {
    /// Which organization.
    pub organization: OrganizationId,
    /// The account-state revision the edge observed.
    pub revision: u64,
    /// Whether the account may spend.
    pub paused: bool,
    /// When the edge observed it.
    pub observed_at: Timestamp,
}

#[async_trait::async_trait]
impl AccountStateReader for AuthorizedAccount {
    async fn projection(
        &self,
        organization: OrganizationId,
    ) -> Result<AccountProjection, PortError> {
        if organization != self.organization {
            // The command named an organization the request was not authorized
            // for. That is a tenant-isolation failure, and it is correctness,
            // not a permission check: answering it would let one tenant's pause
            // gate be evaluated against another's account.
            return Err(PortError::Corrupt {
                kind: "account projection",
                reason: "the command names an organization the request was not authorized for",
            });
        }
        Ok(AccountProjection {
            organization,
            revision: AccountRevision(self.revision),
            state: if self.paused {
                AccountState::Paused {
                    reason: PauseReason::TopUpRequired,
                }
            } else {
                AccountState::Active
            },
            observed_at: self.observed_at,
        })
    }
}

/// The command-path view of the session authority.
///
/// Deliberately **not** [`crate::store::SessionReads`], which reads strongly for
/// served read routes. Mutable planning inputs are eventual because every value
/// is re-asserted as a transaction condition. The known operation identity is
/// different: it is the replay election authority and is read strongly before
/// any mutable dependency, so an accepted retry cannot be hidden by lag and
/// re-planned against later account or session state.
#[derive(Debug, Clone)]
pub struct SessionCommandReads {
    client: Client,
    table: String,
}

impl SessionCommandReads {
    /// Binds the reader to the physical `session-authority` table.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    async fn get(&self, pk: &str, sk: &str) -> Result<Option<Item>, PortError> {
        self.get_with(pk, sk, false).await
    }

    async fn get_strong(&self, pk: &str, sk: &str) -> Result<Option<Item>, PortError> {
        self.get_with(pk, sk, true).await
    }

    async fn get_with(
        &self,
        pk: &str,
        sk: &str,
        consistent: bool,
    ) -> Result<Option<Item>, PortError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(pk, sk)))
            .consistent_read(consistent)
            .send()
            .await
            .map_err(|_| PortError::Unavailable {
                kind: "session authority",
            })?;
        Ok(output.item)
    }

    /// Strongly loads one session head for a recovery/settlement decision.
    ///
    /// Command admission deliberately uses the eventually consistent
    /// [`SessionReader`] implementation and reasserts every observed fact in
    /// its transaction. Background completion is different: after an
    /// ambiguous or raced write it must observe its own latest checkpoint
    /// before deciding whether another write is necessary.
    pub async fn load_session_strong(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Session, PortError> {
        self.load_session_with(workspace, session, true).await
    }

    async fn load_session_with(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        consistent: bool,
    ) -> Result<Session, PortError> {
        let physical = crate::keys::head(session);
        let item = self
            .get_with(&physical.pk, &physical.sk, consistent)
            .await?
            .ok_or(PortError::NotFound { kind: "session" })?;
        if crate::authority_codec::is_session_tombstone(&item, workspace, session).map_err(
            |error| PortError::Corrupt {
                kind: "session",
                reason: match error {
                    crate::attr::CodecError::WrongTenant { .. } => {
                        "the stored tombstone crosses the asserted tenant"
                    }
                    _ => "the stored tombstone is malformed",
                },
            },
        )? {
            return Err(PortError::Deleted { kind: "session" });
        }
        if item
            .get(crate::attr::ITEM_TYPE)
            .and_then(|value| value.as_s().ok())
            .map(String::as_str)
            == Some(crate::codec::SESSION_DELETION_HEAD)
        {
            let deleting =
                crate::authority_codec::decode_session_deletion_head(&item, workspace, session)
                    .map_err(|_| PortError::Corrupt {
                        kind: "session",
                        reason: "the stored deletion coordination head is malformed",
                    })?;
            return Err(PortError::Deleting {
                kind: "session",
                operation: deleting.operation,
            });
        }
        crate::authority_codec::decode_session(&item, workspace).map_err(|_| PortError::Corrupt {
            kind: "session",
            reason: "the stored session head does not decode into the domain vocabulary",
        })
    }
}

#[async_trait::async_trait]
impl SessionReader for SessionCommandReads {
    async fn load_session(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Session, PortError> {
        self.load_session_with(workspace, session, false).await
    }

    async fn load_message_snapshot(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<MessageAdmissionSnapshot, PortError> {
        let head = self.load_session(workspace, session).await?;
        let physical = crate::keys::agent_control(session, head.root_agent);
        let item = self
            .get(&physical.pk, &physical.sk)
            .await?
            .ok_or(PortError::NotFound { kind: "root agent" })?;
        let control =
            crate::codec::decode_control(&item, workspace).map_err(|_| PortError::Corrupt {
                kind: "root agent",
                reason: "the stored Brain control row does not decode",
            })?;
        if control.session != session || control.agent != head.root_agent {
            return Err(PortError::Corrupt {
                kind: "root agent",
                reason: "the stored Brain control row disagrees with the session head",
            });
        }
        if head
            .generation
            .is_some_and(|generation| generation != control.generation)
        {
            return Err(PortError::Corrupt {
                kind: "root agent",
                reason: "the stored Brain control row names a different generation",
            });
        }
        Ok(MessageAdmissionSnapshot {
            session: head,
            root: RootAdmissionState {
                agent: control.agent,
                revision: AgentRevision(control.revision),
                journal_tail: JournalSeq(control.journal_tail),
                limits_revision: control.limits_revision,
                max_run_duration_ms: control.max_run_duration_ms,
                idle: control.status == "idle" && control.claim_owner.is_none(),
            },
        })
    }

    async fn load_run(&self, session: SessionId, run: RunId) -> Result<Run, PortError> {
        let physical = crate::keys::run(session, run);
        let item = self
            .get(&physical.pk, &physical.sk)
            .await?
            .ok_or(PortError::NotFound { kind: "run" })?;
        let workspace = Row::bind(&item, crate::codec::RUN)
            .and_then(|row| row.id::<WorkspaceId>("workspaceId"))
            .map_err(|_| PortError::Corrupt {
                kind: "run",
                reason: "the stored run names no workspace",
            })?;
        crate::authority_codec::decode_domain_run(&item, workspace).map_err(|_| {
            PortError::Corrupt {
                kind: "run",
                reason: "the stored run does not decode into the domain vocabulary",
            }
        })
    }

    async fn load_agent(
        &self,
        _session: SessionId,
        _agent: AgentId,
    ) -> Result<AgentControl, PortError> {
        Err(AGENT_CONTROL_SEAM)
    }

    async fn list_agents(
        &self,
        _session: SessionId,
        _budget: PageBudget,
    ) -> Result<AgentPage, PortError> {
        Err(AGENT_CONTROL_SEAM)
    }

    async fn list_agent_cancel_targets(
        &self,
        session: SessionId,
        from: Option<AgentId>,
        budget: PageBudget,
    ) -> Result<AgentCancelPage, PortError> {
        let partition = crate::keys::session_partition(session);
        let start = from.map_or_else(
            || "AGENT#".to_owned(),
            |first| crate::keys::agent_index(session, first).sk,
        );
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk AND sk BETWEEN :from AND :to")
            .expression_attribute_values(":pk", s(partition))
            .expression_attribute_values(":from", s(start))
            .expression_attribute_values(":to", s("AGENT$"))
            .limit(i32::from(budget.limit))
            .consistent_read(false)
            .send()
            .await
            .map_err(|_| PortError::Unavailable {
                kind: "agent index",
            })?;
        let rows = output.items.unwrap_or_default();
        let mut targets = Vec::with_capacity(rows.len());
        for item in &rows {
            targets.push(decode_cancel_target(item)?);
        }
        // A `LastEvaluatedKey` is the provider's own statement that more rows
        // remain; deriving "more" from a full page would report a next position
        // that does not exist whenever the collection divides evenly.
        let next = output
            .last_evaluated_key
            .as_ref()
            .and_then(|key| key.get(crate::attr::SK))
            .and_then(|value| value.as_s().ok())
            .and_then(|sort| sort.strip_prefix("AGENT#"))
            .and_then(|id| id.parse::<AgentId>().ok());
        Ok(AgentCancelPage { targets, next })
    }

    async fn load_journal_page(
        &self,
        _session: SessionId,
        _agent: AgentId,
        _from: JournalSeq,
        _budget: PageBudget,
    ) -> Result<JournalPage, PortError> {
        Err(PortError::Unowned {
            kind: "journal page",
            seam: "`aex_session_domain::JournalPage` has no row decoder; the journal rows are \
                   `aex-brain-store-dynamodb`'s `JournalEntry`",
        })
    }

    async fn load_receipt(
        &self,
        workspace: WorkspaceId,
        scope: &str,
        identity: &IdempotencyIdentity,
        now: Timestamp,
    ) -> Result<Option<IdempotencyReceipt>, PortError> {
        let expected = aex_session_domain::ReceiptKey::of(scope, identity).map_err(|_| {
            PortError::Corrupt {
                kind: "idempotency receipt key",
                reason: "the application supplied an unusable receipt scope",
            }
        })?;
        let physical =
            crate::keys::receipt(workspace, scope, expected.key_sha256()).map_err(|_| {
                PortError::Corrupt {
                    kind: "idempotency receipt key",
                    reason: "the application receipt key cannot enter the physical key template",
                }
            })?;
        let Some(item) = self.get_strong(&physical.pk, &physical.sk).await? else {
            return Ok(None);
        };
        let stored = crate::codec::decode_receipt(&item).map_err(|_| PortError::Corrupt {
            kind: "idempotency receipt",
            reason: "the stored receipt does not decode",
        })?;
        if !crate::codec::receipt_is_live(&stored, now) {
            return Ok(None);
        }
        if stored.scope != scope || stored.key_sha256 != expected.key_sha256() {
            return Err(PortError::Corrupt {
                kind: "idempotency receipt",
                reason: "the stored receipt disagrees with the strongly addressed key",
            });
        }
        let crate::replay::ReceiptBody::Inline(bytes) = stored.response else {
            return Err(PortError::Corrupt {
                kind: "idempotency receipt",
                reason: "session command responses must fit the inline replay envelope",
            });
        };
        let (kind, id) = match stored.response_kind.as_str() {
            "session" => {
                let response: aex_wire::models::Session =
                    serde_json::from_slice(&bytes).map_err(|_| PortError::Corrupt {
                        kind: "session create receipt",
                        reason: "the stored response is malformed",
                    })?;
                (
                    aex_session_domain::ResourceKind::Session,
                    response.id.to_string(),
                )
            }
            "message" => {
                let response: aex_wire::models::MessageSendResult = serde_json::from_slice(&bytes)
                    .map_err(|_| PortError::Corrupt {
                        kind: "session message receipt",
                        reason: "the stored response is malformed",
                    })?;
                (
                    aex_session_domain::ResourceKind::Message,
                    response.message.id.to_string(),
                )
            }
            _ => {
                return Err(PortError::Corrupt {
                    kind: "idempotency receipt",
                    reason: "a session command receipt carries an unrelated response kind",
                });
            }
        };
        Ok(Some(IdempotencyReceipt {
            key: expected,
            identity: identity.clone(),
            intent: stored.intent,
            outcome: aex_session_domain::ReceiptOutcome::Resource {
                kind,
                id: aex_session_domain::ResourceId(id),
                response: aex_session_domain::ResponseBody::Inline(bytes),
            },
            created_at: stored.committed_at,
            expires_at: Some(stored.expires_at),
        }))
    }

    async fn load_operation(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<VersionedOperation>, PortError> {
        let physical = crate::keys::operation(operation);
        // Operation identity is the lifecycle command's replay authority. It
        // must be checked strongly before account/session state: otherwise an
        // exact accepted retry could miss the row, observe a later pause or
        // session transition, and refuse instead of replaying.
        let Some(item) = self.get_strong(&physical.pk, &physical.sk).await? else {
            return Ok(None);
        };
        crate::codec::decode_operation(&item, workspace)
            .map(|stored| {
                Some(VersionedOperation {
                    operation: stored.record,
                    version: OperationVersion(stored.version),
                })
            })
            .map_err(|_| PortError::Corrupt {
                kind: "operation",
                reason: "the stored operation does not decode into the domain vocabulary",
            })
    }
}

/// The one seam every unimplementable agent read names.
const AGENT_CONTROL_SEAM: PortError = PortError::Unowned {
    kind: "agent control",
    seam: "the physical `agent_control` row is `aex-brain-store-dynamodb`'s `AgentHead`; \
           `aex_session_domain::AgentControl` is a third vocabulary that no adapter persists and \
           whose `AgentStatus` has no stored spelling",
};

/// Decodes the three facts a cancellation needs from one agent index row.
///
/// Strict: a row that cannot answer all three is a decode failure, never a
/// target that is assumed inactive. Assuming inactive is exactly the silent
/// truncation the paged stop exists to remove.
fn decode_cancel_target(item: &Item) -> Result<AgentCancelTarget, PortError> {
    let row = Row::bind(item, crate::codec::AGENT_INDEX).map_err(|_| PortError::Corrupt {
        kind: "agent index",
        reason: "the row is not an agent index entry",
    })?;
    let agent = row
        .id::<AgentId>("agentId")
        .map_err(|_| PortError::Corrupt {
            kind: "agent index",
            reason: "the row names no agent",
        })?;
    let revision = row.u64("revision").map_err(|_| PortError::Corrupt {
        kind: "agent index",
        reason: "the row carries no revision, so no settlement could be conditioned on it",
    })?;
    let finished = row
        .opt_string("finishReason")
        .map_err(|_| PortError::Corrupt {
            kind: "agent index",
            reason: "the row carries a malformed finish reason",
        })?
        .is_some();
    Ok(AgentCancelTarget {
        agent,
        revision: AgentRevision(revision),
        active: !finished,
    })
}

#[cfg(test)]
mod tests {
    use aex_operation_domain::cursor::ContinuationCursor;
    use aex_session_app::plan::{Condition, SessionTransaction, TransactionIntent, Write};
    use aex_session_domain::testing::running_session;

    use super::SessionAuthorityExternal;
    use crate::application_plan::{SessionBinding, compile_application_transaction};
    use crate::plan::RegionalTables;

    fn tables() -> RegionalTables {
        RegionalTables::composed("dev", "eu-west-1")
    }

    #[test]
    fn an_admitted_operation_compiles_to_one_conditional_insert() {
        let (session, _run, _agent, _message) = running_session();
        let operation = aex_operation_domain::Operation {
            id: aex_session_domain::testing::id(41),
            workspace: session.workspace,
            session: Some(session.id),
            kind: aex_operation_domain::OperationKind::SessionTerminate,
            status: aex_operation_domain::operation::OperationStatus::Succeeded,
            intent: aex_wire::idempotency::IntentDigest::from_bytes([4; 32]),
            scope: aex_operation_domain::operation::OperationScope::Session(session.id),
            progress: None,
            cursor: None,
            cancel_requested: false,
            result: Some(aex_operation_domain::operation::OperationResult::receipt()),
            error: None,
            created_at: aex_session_domain::testing::moment(1),
            started_at: Some(aex_session_domain::testing::moment(1)),
            updated_at: aex_session_domain::testing::moment(1),
            committed_at: Some(aex_session_domain::testing::moment(1)),
            terminal_at: Some(aex_session_domain::testing::moment(1)),
        };
        let plan = SessionTransaction {
            intent: TransactionIntent::TerminateSession,
            conditions: vec![Condition::SessionRevision {
                session: session.id,
                expected: session.revision,
            }],
            writes: vec![
                Write::PutOperation(Box::new(operation)),
                Write::PutSessionHead(Box::new(session.clone())),
            ],
            after_commit: Vec::new(),
        };
        let binding = SessionBinding {
            workspace: session.workspace,
            organization: session.organization,
            session: session.id,
        };
        let compiled = compile_application_transaction(
            &tables(),
            &plan,
            binding,
            &SessionAuthorityExternal::new(binding),
        )
        .expect("compiles");
        assert_eq!(compiled.transaction.len(), 2);
        let operation_put = compiled.transaction.actions()[1]
            .put()
            .or_else(|| compiled.transaction.actions()[0].put())
            .expect("the operation is one conditional put");
        assert!(
            operation_put
                .condition_expression()
                .expect("a condition")
                .contains("attribute_not_exists(pk)"),
            "an admission is an insert, or a lost acknowledgement silently overwrites"
        );
    }

    #[test]
    fn a_cross_tenant_operation_write_fails_before_request_construction() {
        let (session, _run, _agent, _message) = running_session();
        let mut operation = aex_operation_domain::Operation {
            id: aex_session_domain::testing::id(42),
            workspace: session.workspace,
            session: Some(session.id),
            kind: aex_operation_domain::OperationKind::SessionTerminate,
            status: aex_operation_domain::operation::OperationStatus::Succeeded,
            intent: aex_wire::idempotency::IntentDigest::from_bytes([4; 32]),
            scope: aex_operation_domain::operation::OperationScope::Session(session.id),
            progress: None,
            cursor: None,
            cancel_requested: false,
            result: None,
            error: None,
            created_at: aex_session_domain::testing::moment(1),
            started_at: None,
            updated_at: aex_session_domain::testing::moment(1),
            committed_at: None,
            terminal_at: None,
        };
        operation.workspace = aex_session_domain::testing::id(88);
        let plan = SessionTransaction {
            intent: TransactionIntent::TerminateSession,
            conditions: Vec::new(),
            writes: vec![Write::PutOperation(Box::new(operation))],
            after_commit: Vec::new(),
        };
        let binding = SessionBinding {
            workspace: session.workspace,
            organization: session.organization,
            session: session.id,
        };
        let error = compile_application_transaction(
            &tables(),
            &plan,
            binding,
            &SessionAuthorityExternal::new(binding),
        )
        .expect_err("tenant drift");
        assert!(
            matches!(error, crate::error::StoreError::Invalid { .. }),
            "{error}"
        );
    }

    #[test]
    fn a_settled_agent_compiles_to_one_conditional_update_that_touches_four_attributes() {
        let (session, _run, _agent, _message) = running_session();
        let agent = aex_session_domain::testing::id(51);
        let plan = SessionTransaction {
            intent: TransactionIntent::CancelSession,
            conditions: vec![Condition::AgentRevision {
                session: session.id,
                agent,
                expected: aex_session_domain::AgentRevision(3),
            }],
            writes: vec![Write::CancelAgent {
                session: session.id,
                agent,
                from_revision: aex_session_domain::AgentRevision(3),
                to_revision: aex_session_domain::AgentRevision(4),
                at: aex_session_domain::testing::moment(9),
            }],
            after_commit: Vec::new(),
        };
        let binding = SessionBinding {
            workspace: session.workspace,
            organization: session.organization,
            session: session.id,
        };
        let compiled = compile_application_transaction(
            &tables(),
            &plan,
            binding,
            &SessionAuthorityExternal::new(binding),
        )
        .expect("compiles");
        assert_eq!(compiled.transaction.len(), 1);
        let update = compiled.transaction.actions()[0]
            .update()
            .expect("a settlement is an update, never a whole-record put");
        let expression = update.update_expression();
        assert!(
            expression.contains("REMOVE claimOwner, leaseExpiresAt"),
            "{expression}"
        );
        assert!(
            update
                .expression_attribute_names()
                .expect("names")
                .values()
                .any(|attribute| attribute == "revision"),
            "a settlement is conditioned on the revision it observed"
        );
        assert!(
            update
                .condition_expression()
                .expect("a condition")
                .contains("attribute_exists(pk)"),
            "a settlement never creates the row it settles"
        );
        assert_eq!(compiled.condition_groups[0].len(), 1);
    }

    #[test]
    fn root_cancellation_advances_both_fences_and_sets_only_the_stop_latch() {
        let (session, _run, _agent, _message) = running_session();
        let agent = session.root_agent;
        let plan = SessionTransaction {
            intent: TransactionIntent::CancelSession,
            conditions: vec![Condition::AgentRevision {
                session: session.id,
                agent,
                expected: aex_session_domain::AgentRevision(3),
            }],
            writes: vec![Write::RequestRootCancellation {
                session: session.id,
                agent,
                from_revision: aex_session_domain::AgentRevision(3),
                to_revision: aex_session_domain::AgentRevision(4),
                from_cancellation: aex_session_domain::CancellationEpoch(7),
                to_cancellation: aex_session_domain::CancellationEpoch(8),
                reason: aex_session_app::plan::RootStopReason::SessionCancelled,
                at: aex_session_domain::testing::moment(9),
            }],
            after_commit: Vec::new(),
        };
        let binding = SessionBinding {
            workspace: session.workspace,
            organization: session.organization,
            session: session.id,
        };
        let compiled = compile_application_transaction(
            &tables(),
            &plan,
            binding,
            &SessionAuthorityExternal::new(binding),
        )
        .expect("compiles");
        let update = compiled.transaction.actions()[0]
            .update()
            .expect("the Brain control is a narrow update");
        let change = update.update_expression();
        assert!(change.contains("stopRequested = :requested"), "{change}");
        assert!(change.contains("stopReason = :stopReason"), "{change}");
        assert!(
            change.contains("cancelEpoch = :nextCancellation"),
            "{change}"
        );
        assert!(!change.contains("#status ="), "{change}");
        let guard = update
            .condition_expression()
            .expect("both old fences are conditioned");
        assert!(guard.contains("cancelEpoch = :fromCancellation"), "{guard}");
        assert!(
            update
                .expression_attribute_names()
                .expect("the revision guard uses an attribute-name placeholder")
                .values()
                .any(|attribute| attribute == "revision"),
            "{guard}"
        );
        assert!(
            guard.contains("attribute_not_exists(finishReason)"),
            "{guard}"
        );
    }

    /// A continued stop parked at one agent, as the worker would read it back.
    fn resumed(
        session: &aex_session_domain::Session,
    ) -> (aex_operation_domain::Operation, ContinuationCursor) {
        let cursor = ContinuationCursor::new(
            aex_operation_domain::cursor::CursorPosition::SessionTerminate {
                stage: aex_operation_domain::cursor::LifecycleStage::AwaitProvider,
                generation: session
                    .lifecycle
                    .generation
                    .expect("sandbox fixture carries a generation"),
            },
            98,
            None,
        )
        .expect("builds");
        let operation = aex_operation_domain::Operation {
            id: aex_session_domain::testing::id(61),
            workspace: session.workspace,
            session: Some(session.id),
            kind: aex_operation_domain::OperationKind::SessionTerminate,
            status: aex_operation_domain::operation::OperationStatus::Running,
            intent: aex_wire::idempotency::IntentDigest::from_bytes([5; 32]),
            scope: aex_operation_domain::operation::OperationScope::Session(session.id),
            progress: None,
            cursor: Some(cursor.clone()),
            cancel_requested: false,
            result: None,
            error: None,
            created_at: aex_session_domain::testing::moment(1),
            started_at: Some(aex_session_domain::testing::moment(1)),
            updated_at: aex_session_domain::testing::moment(2),
            committed_at: Some(aex_session_domain::testing::moment(1)),
            terminal_at: None,
        };
        (operation, cursor)
    }

    fn step_plan(
        operation: &aex_operation_domain::Operation,
        conditions: Vec<Condition>,
    ) -> SessionTransaction {
        SessionTransaction {
            intent: TransactionIntent::SettleLifecycle,
            conditions,
            writes: vec![Write::PutOperation(Box::new(operation.clone()))],
            after_commit: Vec::new(),
        }
    }

    fn binding_of(session: &aex_session_domain::Session) -> SessionBinding {
        SessionBinding {
            workspace: session.workspace,
            organization: session.organization,
            session: session.id,
        }
    }

    #[test]
    fn a_resumed_step_conditions_on_the_version_it_read_and_advances_it_by_exactly_one() {
        let (session, _run, _agent, _message) = running_session();
        let (operation, cursor) = resumed(&session);
        let observed = aex_operation_domain::operation::OperationVersion(7);
        let plan = step_plan(
            &operation,
            vec![
                Condition::OperationCursorAt {
                    operation: operation.id,
                    expected: Some(Box::new(cursor)),
                },
                Condition::OperationVersion {
                    operation: operation.id,
                    expected: observed,
                },
            ],
        );
        let binding = binding_of(&session);
        let compiled = compile_application_transaction(
            &tables(),
            &plan,
            binding,
            &SessionAuthorityExternal::new(binding),
        )
        .expect("a resumed step compiles");

        assert_eq!(
            compiled.transaction.len(),
            1,
            "both guards and the write merge into one action on the operation item"
        );
        let put = compiled.transaction.actions()[0]
            .put()
            .expect("a step rewrites the whole envelope");
        let expression = put.condition_expression().expect("a condition");
        assert!(
            expression.contains("attribute_exists(pk)"),
            "a step never creates the row it advances: {expression}"
        );
        assert!(
            !expression.contains("attribute_not_exists(pk)"),
            "a step is not an insert: {expression}"
        );
        let names = put.expression_attribute_names().expect("names");
        assert!(
            names.values().any(|attribute| attribute == "version"),
            "the step must guard the version it observed"
        );
        assert!(
            names
                .values()
                .any(|attribute| attribute == "continuationCursor"),
            "the step must guard the cursor it advances from"
        );
        let written = put
            .item()
            .get("version")
            .and_then(|value| value.as_n().ok())
            .and_then(|text| text.parse::<u64>().ok())
            .expect("the stored row carries its version");
        assert_eq!(
            written,
            observed.next().0,
            "the row advances by exactly one, or the public cancellation's optimistic loop breaks"
        );
    }

    #[test]
    fn a_resumed_step_that_cannot_name_its_version_is_refused_rather_than_guessed_at() {
        let (session, _run, _agent, _message) = running_session();
        let (operation, cursor) = resumed(&session);
        let plan = step_plan(
            &operation,
            vec![Condition::OperationCursorAt {
                operation: operation.id,
                expected: Some(Box::new(cursor)),
            }],
        );
        let binding = binding_of(&session);
        let error = compile_application_transaction(
            &tables(),
            &plan,
            binding,
            &SessionAuthorityExternal::new(binding),
        )
        .expect_err("a version the plan cannot name must not be invented");
        assert!(
            format!("{error}").contains("OperationVersion"),
            "the refusal must name what is missing: {error}"
        );
    }

    #[test]
    fn an_admission_that_claims_an_observed_version_is_refused() {
        let (session, _run, _agent, _message) = running_session();
        let (mut operation, _cursor) = resumed(&session);
        operation.cursor = None;
        let plan = step_plan(
            &operation,
            vec![Condition::OperationVersion {
                operation: operation.id,
                expected: aex_operation_domain::operation::OperationVersion(3),
            }],
        );
        let binding = binding_of(&session);
        let error = compile_application_transaction(
            &tables(),
            &plan,
            binding,
            &SessionAuthorityExternal::new(binding),
        )
        .expect_err("an insert has no version to have observed");
        assert!(format!("{error}").contains("admission"), "{error}");
    }

    #[test]
    fn a_binding_that_disagrees_with_the_compiler_is_refused_before_any_action_is_built() {
        let (session, _run, _agent, _message) = running_session();
        let (operation, _cursor) = resumed(&session);
        let mut plan = step_plan(&operation, Vec::new());
        plan.intent = TransactionIntent::SettleLifecycle;
        let binding = binding_of(&session);
        let mut foreign = binding;
        foreign.workspace = aex_session_domain::testing::id(99);
        let error = compile_application_transaction(
            &tables(),
            &plan,
            binding,
            &SessionAuthorityExternal::new(foreign),
        )
        .expect_err("two disagreeing bindings must never both be trusted");
        assert!(
            matches!(error, crate::error::StoreError::Invalid { .. }),
            "{error}"
        );
    }

    #[test]
    fn a_write_with_no_compiler_is_refused_rather_than_skipped() {
        let (session, _run, _agent, _message) = running_session();
        let plan = SessionTransaction {
            intent: TransactionIntent::SettleLifecycle,
            conditions: Vec::new(),
            writes: vec![Write::RedactOperationResult(
                aex_session_domain::testing::id(7),
            )],
            after_commit: Vec::new(),
        };
        let binding = SessionBinding {
            workspace: session.workspace,
            organization: session.organization,
            session: session.id,
        };
        let error = compile_application_transaction(
            &tables(),
            &plan,
            binding,
            &SessionAuthorityExternal::new(binding),
        )
        .expect_err("an unknown write must never be silently dropped");
        assert!(
            format!("{error}").contains("RedactOperationResult"),
            "the refusal must name the arm: {error}"
        );
    }
}
