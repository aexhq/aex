//! The `DynamoDB`-backed [`JournalStore`] and [`EffectStore`].
//!
//! Three properties are carried by this module rather than by review.
//!
//! - **A page that observes a gap returns no entries.** The fold is contiguous, so half a
//!   page is worse than none: it would let an agent act on a prefix of its own history.
//! - **A page whose body does not hash to its recorded entry id quarantines.** That is a
//!   fork, and folding either side of one silently forks the agent's whole future.
//! - **A redelivered decision is a success, not a duplicate.** The journal put is
//!   conditional on `attribute_not_exists`, so the second attempt loses that action and
//!   comes back as [`ConditionFailure::IdempotentReplay`].

use aex_brain_app::ports::{
    AgentHead, BoxFuture, CommitError, CommitReceipt, ConditionFailure, DecisionContext,
    DispatchTicket, EffectStore, FenceGuard, JournalCursor, JournalPage, JournalStore, ReadBudget,
    SessionAuthority, StoreError,
};
use aex_brain_domain::commit::DecisionCommit;
use aex_brain_domain::effect::{DispatchEvidence, DurableEffect};
use aex_brain_domain::ids::{AgentKey, ContentHash, EffectId, JournalSeq, Timestamp};
use aex_session_dynamodb::attr::{Item, Row, n, s, stamp};
use aex_session_dynamodb::plan::key as item_key;
use aws_sdk_dynamodb::Client;

use crate::plan::{self, BrainTables};
use crate::{control, effect, keys, translate};

const DISPATCH_ORDER: &[aex_session_dynamodb::plan::Participant] = &[
    aex_session_dynamodb::plan::Participant::SESSION_HEAD_GUARD,
    aex_session_dynamodb::plan::Participant::AGENT_CONTROL,
    aex_session_dynamodb::plan::Participant::AGENT_EFFECT,
];

/// The named participant order of the pre-dispatch transaction.
///
/// Published for request/cancellation-shape assertions. The error decoder consumes this
/// exact positional order, so adding a guard without updating the names would misclassify
/// the losing authority.
#[must_use]
pub const fn dispatch_order() -> &'static [aex_session_dynamodb::plan::Participant] {
    DISPATCH_ORDER
}

/// The `DynamoDB` half of the Brain store.
///
/// One type implements [`JournalStore`], [`EffectStore`] and
/// [`LeaseStore`](aex_brain_app::ports::LeaseStore) because all three address the
/// same two partitions with the same client and the same table configuration. Three types
/// would be three copies of that configuration and three chances for them to disagree about
/// which table an agent lives in.
#[derive(Debug, Clone)]
pub struct BrainStore {
    client: Client,
    tables: BrainTables,
    session_telemetry: Option<aex_session_telemetry_aws::SessionTelemetryWriter>,
}

impl BrainStore {
    /// Binds the store to a client and a composition.
    #[must_use]
    pub const fn new(client: Client, tables: BrainTables) -> Self {
        Self {
            client,
            tables,
            session_telemetry: None,
        }
    }

    /// Adds the customer session-telemetry writer.
    ///
    /// This capability is optional at the adapter boundary so unit and local
    /// stores remain pure `DynamoDB` compositions. Production always binds it.
    #[must_use]
    pub fn with_session_telemetry(
        mut self,
        session_telemetry: aex_session_telemetry_aws::SessionTelemetryWriter,
    ) -> Self {
        self.session_telemetry = Some(session_telemetry);
        self
    }

    /// The physical tables this store was built with.
    #[must_use]
    pub const fn tables(&self) -> &BrainTables {
        &self.tables
    }

    /// The client, shared by the lease path.
    #[must_use]
    pub const fn client(&self) -> &Client {
        &self.client
    }

    /// The `session-authority` table this store addresses.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.tables.session_authority
    }

    /// Proves both tables this store addresses answer a real read.
    ///
    /// A successfully constructed SDK client is not evidence of anything: it resolves no
    /// endpoint, signs nothing and contacts no service, so a readiness check built on one
    /// reports ready against a deleted table. This performs the read the serving path
    /// performs — a strongly consistent `GetItem` on `session-authority` and then on
    /// `regional-work` — against [`keys::health_probe_agent`] and
    /// [`keys::HEALTH_PROBE_WORK_ID`], identities no clock can mint.
    ///
    /// **A response carrying no item is the success.** It proves the region resolved, the
    /// credentials signed, the table exists under its configured name and the task role may
    /// read it, without depending on any tenant's row existing.
    ///
    /// Both tables are read because they fail independently: `regional-work` holds the due
    /// index the backstop scans, and a task that reaches only one of the two can serve only
    /// part of its work.
    ///
    /// # Errors
    ///
    /// [`StoreError::Transport`] naming which of the two tables refused.
    pub async fn probe(&self) -> Result<(), StoreError> {
        let control =
            keys::control(&keys::health_probe_agent()).map_err(|error| store_key_error(&error))?;
        self.client
            .get_item()
            .table_name(&self.tables.session_authority)
            .set_key(Some(item_key(&control.pk, &control.sk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| transport("probe_session_authority", &error))?;
        let work = aex_work_dynamodb::keys::work(keys::HEALTH_PROBE_WORK_ID)
            .map_err(|error| store_key_error(&keys::BrainKeyError::Component(error)))?;
        self.client
            .get_item()
            .table_name(&self.tables.regional_work)
            .set_key(Some(item_key(&work.pk, &work.sk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| transport("probe_regional_work", &error))?;
        Ok(())
    }

    async fn get_control(&self, key: &AgentKey) -> Result<Option<Item>, StoreError> {
        let control = keys::control(key).map_err(|error| store_key_error(&error))?;
        let output = self
            .client
            .get_item()
            .table_name(self.table())
            .set_key(Some(item_key(&control.pk, &control.sk)))
            // Strongly consistent: an authority read that may be stale is not an authority
            // read, and every decision this feeds conditions on the values it returns.
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| transport("load_head", &error))?;
        Ok(output.item)
    }

    async fn query_effects(&self, key: &AgentKey) -> Result<Vec<DurableEffect>, StoreError> {
        let partition = keys::agent_partition(key).map_err(|error| store_key_error(&error))?;
        let mut open = Vec::new();
        let mut start_key = None;
        // Paginated to exhaustion: settled effects share the prefix and are
        // never deleted, so a long-lived agent's open set can sit past the
        // service's 1 MB page boundary. Stopping at one page would drop open
        // effects from the claim-path set, and dispatch would run against a
        // fold that cannot see them.
        loop {
            let output = self
                .client
                .query()
                .table_name(self.table())
                .consistent_read(true)
                .key_condition_expression("pk = :pk AND begins_with(sk, :prefix)")
                .expression_attribute_values(":pk", s(partition.clone()))
                .expression_attribute_values(":prefix", s(keys::effect_prefix()))
                .set_exclusive_start_key(start_key)
                .send()
                .await
                .map_err(|error| transport("load_open", &error))?;
            for item in output.items() {
                let decoded = effect::decode(item).map_err(|error| StoreError::Undecodable {
                    location: "agent effect".to_owned(),
                    reason: error.to_string(),
                })?;
                if !decoded.state.is_settled() {
                    open.push(decoded);
                }
            }
            start_key = output.last_evaluated_key().cloned();
            if start_key.is_none() {
                return Ok(open);
            }
        }
    }
}

impl JournalStore for BrainStore {
    fn load_head<'a>(
        &'a self,
        key: &'a AgentKey,
    ) -> BoxFuture<'a, Result<Option<AgentHead>, StoreError>> {
        Box::pin(async move {
            let Some(item) = self.get_control(key).await? else {
                return Ok(None);
            };
            let open = self
                .query_effects(key)
                .await?
                .into_iter()
                .map(|effect| effect.id)
                .collect();
            control::decode(&item, *key, open)
                .map(Some)
                .map_err(|error| StoreError::Undecodable {
                    location: "agent control".to_owned(),
                    reason: error.to_string(),
                })
        })
    }

    fn read_page<'a>(
        &'a self,
        key: &'a AgentKey,
        from: JournalSeq,
        budget: ReadBudget,
        after: Option<JournalCursor>,
    ) -> BoxFuture<'a, Result<JournalPage, StoreError>> {
        Box::pin(async move {
            let partition = keys::agent_partition(key).map_err(|error| store_key_error(&error))?;
            let query_from = after.as_ref().map_or(from, JournalCursor::start);
            let mut query = self
                .client
                .query()
                .table_name(self.table())
                .consistent_read(true)
                .key_condition_expression("pk = :pk AND sk BETWEEN :from AND :to")
                .expression_attribute_values(":pk", s(partition.clone()))
                .expression_attribute_values(":from", s(keys::journal_sort_key(query_from)))
                .expression_attribute_values(
                    ":to",
                    s(format!("{}\u{ffff}", keys::journal_prefix())),
                )
                .limit(i32::try_from(budget.max_entries).unwrap_or(i32::MAX));
            if let Some(cursor) = after {
                query =
                    query.set_exclusive_start_key(Some(encode_cursor(&cursor, &partition, from)?));
            }
            let output = query
                .send()
                .await
                .map_err(|error| transport("read_page", &error))?;
            decode_page(
                output.items(),
                output.last_evaluated_key(),
                &partition,
                query_from,
                from,
                budget,
            )
        })
    }

    fn commit<'a>(
        &'a self,
        context: &'a DecisionContext,
        commit: &'a DecisionCommit,
    ) -> BoxFuture<'a, Result<CommitReceipt, CommitError>> {
        Box::pin(async move {
            let compiled =
                plan::compile(&self.tables, context, commit).map_err(commit_plan_error)?;
            let participants = compiled.participants().to_vec();
            let request = compiled
                .compile(&self.client)
                .map_err(|error| commit_store_error(&error))?;
            match request.send().await {
                Ok(_) => {
                    self.write_session_telemetry(commit).await;
                    Ok(CommitReceipt {
                        revision: commit.control.next_revision,
                        tail: commit.control.next_tail,
                        wakes: commit.wakes.iter().map(|wake| wake.id).collect(),
                        committed_at: context.now,
                    })
                }
                Err(error) => {
                    let mapped = match error.as_service_error() {
                        Some(service) => {
                            aex_session_dynamodb::error::decode_cancellation(service, &participants)
                        }
                        None => aex_session_dynamodb::error::classify(
                            &error,
                            aex_session_dynamodb::error::Idempotence::Write(
                                aex_session_dynamodb::error::Resolution::TargetItem,
                            ),
                        ),
                    };
                    Err(commit_store_error(&mapped))
                }
            }
        })
    }
}

impl BrainStore {
    async fn write_session_telemetry(&self, commit: &DecisionCommit) {
        let Some(writer) = self.session_telemetry.as_ref() else {
            return;
        };
        let Ok(session) = translate::session(commit.guard.key.session) else {
            return;
        };
        for event in &commit.session_events {
            let outcome = match event.outcome.as_str() {
                "succeeded" => aex_session_telemetry_aws::SessionOutcome::Succeeded,
                "failed" => aex_session_telemetry_aws::SessionOutcome::Failed,
                "timed_out" => aex_session_telemetry_aws::SessionOutcome::TimedOut,
                "cancelled" => aex_session_telemetry_aws::SessionOutcome::Cancelled,
                "interrupted" => aex_session_telemetry_aws::SessionOutcome::Interrupted,
                _ => continue,
            };
            let Ok(at) = translate::at(event.at, "session telemetry timestamp") else {
                continue;
            };
            // DynamoDB is authoritative. Telemetry is deliberately post-commit
            // and fail-open: an S3/KMS refusal never changes the product result.
            let _ = writer
                .write(
                    session,
                    aex_session_telemetry_aws::SessionEvent::message_completed(
                        event.event_seq,
                        outcome,
                        at,
                    ),
                )
                .await;
        }
    }
}

impl EffectStore for BrainStore {
    fn mark_dispatch_started<'a>(
        &'a self,
        guard: &'a FenceGuard,
        authority: &'a SessionAuthority,
        id: &'a EffectId,
        attempt: u16,
        at: Timestamp,
    ) -> BoxFuture<'a, Result<DispatchTicket, CommitError>> {
        Box::pin(async move {
            let key = guard.key();
            let session = translate::session(key.session)
                .map_err(|error| CommitError::Store(translate_error(&error)))?;
            let control_key =
                keys::control(&key).map_err(|error| CommitError::Store(store_key_error(&error)))?;
            let effect_key = keys::effect(&key, *id)
                .map_err(|error| CommitError::Store(store_key_error(&error)))?;
            let now = translate::at(at, "at")
                .map_err(|error| CommitError::Store(translate_error(&error)))?;
            let material = format!(
                "{}:{}:{}:{}:{}:{}:{}:{}:{}",
                key.session.0.as_hyphenated(),
                key.agent.0.as_hyphenated(),
                id.to_hex(),
                guard.fence().0,
                attempt,
                guard.cancel_epoch().0,
                authority.deletion_epoch,
                authority.workspace,
                authority.organization,
            );
            let digest = blake3::hash(material.as_bytes()).to_hex().to_string();
            let mut plan = aex_session_dynamodb::plan::TransactionPlan::new(format!(
                "brain-dispatch-{}",
                &digest[..21]
            ));
            // Session lifecycle, current control ownership and effect takeover are one
            // transaction. No separate session read is needed at dispatch time: the head
            // condition checks the typed facts returned by claim at the same serialization
            // point that mints the ticket.
            plan.condition_check(
                aex_session_dynamodb::plan::Participant::SESSION_HEAD_GUARD,
                plan::session_head_guard(self.table(), session, guard.cancel_epoch(), authority),
            )
            .map_err(|error| commit_store_error(&error))?;
            // The effect's prepare-time `agentFence` is deliberately not a condition: after
            // a crash it belongs to the predecessor, while current control ownership belongs
            // to the successor responsible for the same prepared identity.
            plan.condition_check(
                aex_session_dynamodb::plan::Participant::AGENT_CONTROL,
                aws_sdk_dynamodb::types::ConditionCheck::builder()
                    .table_name(self.table())
                    .set_key(Some(item_key(&control_key.pk, &control_key.sk)))
                    .condition_expression("fence = :fence AND claimOwner = :owner")
                    .expression_attribute_values(":fence", n(guard.fence().0))
                    .expression_attribute_values(
                        ":owner",
                        s(guard.as_ref().owner.0.as_hyphenated().to_string()),
                    ),
            )
            .map_err(|error| commit_store_error(&error))?;
            plan.update(
                aex_session_dynamodb::plan::Participant::AGENT_EFFECT,
                aws_sdk_dynamodb::types::Update::builder()
                    .table_name(self.table())
                    .set_key(Some(item_key(&effect_key.pk, &effect_key.sk)))
                    .condition_expression(
                        "#state = :prepared AND effectId = :id AND attempt = :attempt",
                    )
                    .update_expression(
                        "SET #state = :next, dispatchStartedAt = :now, \
                         agentFence = :fence",
                    )
                    .expression_attribute_names("#state", "state")
                    .expression_attribute_values(":prepared", s("prepared"))
                    .expression_attribute_values(":id", s(id.to_hex()))
                    .expression_attribute_values(":fence", n(guard.fence().0))
                    .expression_attribute_values(":next", s("dispatched"))
                    .expression_attribute_values(":attempt", n(u64::from(attempt)))
                    .expression_attribute_values(":now", stamp(now)),
            )
            .map_err(|error| commit_store_error(&error))?;
            debug_assert_eq!(plan.participants(), DISPATCH_ORDER);
            let participants = plan.participants().to_vec();
            let request = plan
                .compile(&self.client)
                .map_err(|error| commit_store_error(&error))?;
            request
                .send()
                .await
                .map_err(|error| dispatch_transaction_error(*id, &participants, &error))?;
            Ok(DispatchTicket::mint(
                guard,
                authority.workspace,
                authority.organization,
                *id,
                attempt,
                at,
            ))
        })
    }

    fn mark_response_started<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<(), CommitError>> {
        Box::pin(async move {
            let key = ticket.key();
            let effect_key = keys::effect(&key, ticket.effect())
                .map_err(|error| CommitError::Store(store_key_error(&error)))?;
            let now = translate::at(ticket.issued_at(), "at")
                .map_err(|error| CommitError::Store(translate_error(&error)))?;
            // Every attribute `effect::decode` reads back is written here. The operation
            // binding is closed: an external operation is mutually exclusive with the
            // detached tool's id-plus-executor pair.
            if evidence.external_operation.is_some() && evidence.detached_tool.is_some() {
                return Err(CommitError::Store(StoreError::Undecodable {
                    location: format!("effect {} response evidence", ticket.effect()),
                    reason: "external and detached-tool operation bindings are mutually exclusive"
                        .to_owned(),
                }));
            }
            let mut update = "SET #state = :next, responseStartedAt = :now, \
                              dispatchStage = :stage, dispatchProof = :proof"
                .to_owned();
            let mut request = self
                .client
                .update_item()
                .table_name(self.table())
                .set_key(Some(item_key(&effect_key.pk, &effect_key.sk)))
                .condition_expression("#state = :dispatched")
                .expression_attribute_names("#state", "state")
                .expression_attribute_values(":dispatched", s("dispatched"))
                .expression_attribute_values(":next", s("responding"))
                .expression_attribute_values(":now", stamp(now))
                .expression_attribute_values(":stage", s(format!("{:?}", evidence.stage)))
                .expression_attribute_values(":proof", s(format!("{:?}", evidence.proof)));
            if let Some(operation) = evidence.external_operation.as_ref() {
                update.push_str(", externalOperationId = :externalOperation");
                request = request
                    .expression_attribute_values(":externalOperation", s(operation.0.clone()));
            }
            if let Some(operation) = evidence.detached_tool.as_ref() {
                update.push_str(
                    ", detachedOperationId = :detachedOperation, detachedExecutor = :detachedExecutor",
                );
                request = request
                    .expression_attribute_values(":detachedOperation", s(operation.id.0.clone()))
                    .expression_attribute_values(
                        ":detachedExecutor",
                        s(format!("{:?}", operation.executor)),
                    );
            }
            if let Some(provider_request) = evidence.provider_request_id.as_ref() {
                update.push_str(", providerRequestId = :providerRequestId");
                request = request.expression_attribute_values(
                    ":providerRequestId",
                    s(provider_request.as_str()),
                );
            }
            if let Some(receipt) = evidence.receipt {
                update.push_str(", receiptHash = :receipt");
                request = request.expression_attribute_values(":receipt", s(receipt.to_hex()));
            }
            request
                .update_expression(update)
                .send()
                .await
                .map_err(|error| {
                    effect_condition(ticket.effect(), "mark_response_started", &error)
                })?;
            Ok(())
        })
    }

    fn load_open<'a>(
        &'a self,
        key: &'a AgentKey,
    ) -> BoxFuture<'a, Result<Vec<DurableEffect>, StoreError>> {
        Box::pin(async move { self.query_effects(key).await })
    }
}

/// Turns one query page into a [`JournalPage`], enforcing contiguity and fork detection.
///
/// Separated from the client call so both properties are assertable without a service: they
/// are the two rules that decide whether an agent may act at all.
///
/// # Errors
///
/// [`StoreError::JournalGap`] when the page is not contiguous from `from`,
/// [`StoreError::JournalForked`] when a stored body does not hash to its recorded entry id,
/// [`StoreError::ReadBudgetExhausted`] when the response exceeds either page bound, and
/// [`StoreError::Undecodable`] when a row is not a journal entry.
pub fn decode_page(
    items: &[Item],
    last_evaluated_key: Option<&Item>,
    partition: &str,
    query_from: JournalSeq,
    from: JournalSeq,
    budget: ReadBudget,
) -> Result<JournalPage, StoreError> {
    let mut entries = Vec::with_capacity(items.len().min(budget.max_entries));
    let mut expected = from;
    let mut bytes = 0_usize;
    for item in items {
        if entries.len() >= budget.max_entries {
            return Err(StoreError::ReadBudgetExhausted {
                entries: entries.len().saturating_add(1),
                bytes,
            });
        }
        let row = Row::bind(item, aex_session_dynamodb::codec::JOURNAL_ENTRY)
            .map_err(|error| undecodable("journal entry", &error))?;
        let seq = JournalSeq(
            row.u64("seq")
                .map_err(|error| undecodable("journal entry", &error))?,
        );
        validate_row_key(&row, partition, seq)?;
        if seq != expected {
            return Err(StoreError::JournalGap { missing: expected });
        }
        let body = row
            .bytes(aex_session_dynamodb::codec::BODY_INLINE)
            .map_err(|error| undecodable("journal entry", &error))?;
        let recorded = row
            .string("entryId")
            .map_err(|error| undecodable("journal entry", &error))?;
        let observed = ContentHash::of(body);
        if observed.to_hex() != recorded {
            return Err(StoreError::JournalForked {
                seq,
                stored: parse_hash(recorded).unwrap_or(observed),
                read: observed,
            });
        }
        bytes = bytes.saturating_add(body.len());
        if bytes > budget.max_bytes {
            return Err(StoreError::ReadBudgetExhausted {
                entries: entries.len(),
                bytes,
            });
        }
        let record = aex_brain_domain::journal::decode(body)
            .map_err(|error| undecodable("journal entry", &error))?;
        let recorded_at = row
            .timestamp("occurredAt")
            .map_err(|error| undecodable("journal entry", &error))?;
        entries.push(aex_brain_domain::journal::JournalEntry {
            envelope: aex_brain_domain::wire_pending::JournalEnvelope {
                seq,
                content_hash: observed,
                recorded_at: translate::from_wire(recorded_at),
            },
            record,
        });
        expected = expected.next();
    }
    let next = last_evaluated_key
        // The API defines an absent or empty LEK as EOF. Every non-empty map is evidence
        // that pagination must continue and is validated as the exact table key below.
        .filter(|key| !key.is_empty())
        .map(|key| decode_cursor(key, partition, query_from, expected))
        .transpose()?;
    Ok(JournalPage {
        entries,
        hydrated_bytes: bytes,
        next,
    })
}

fn validate_row_key(row: &Row<'_>, partition: &str, seq: JournalSeq) -> Result<(), StoreError> {
    let row_partition = row
        .string(aex_session_dynamodb::attr::PK)
        .map_err(|error| undecodable("journal entry key", &error))?;
    let row_sort = row
        .string(aex_session_dynamodb::attr::SK)
        .map_err(|error| undecodable("journal entry key", &error))?;
    if row_partition != partition || parse_journal_sort_key(row_sort) != Some(seq) {
        return Err(StoreError::Undecodable {
            location: "journal entry key".to_owned(),
            reason: "the row key does not match its agent partition and sequence".to_owned(),
        });
    }
    Ok(())
}

fn decode_cursor(
    key: &Item,
    partition: &str,
    query_from: JournalSeq,
    next: JournalSeq,
) -> Result<JournalCursor, StoreError> {
    if key.len() != 2 {
        return Err(invalid_cursor(
            "the native continuation must contain exactly pk and sk",
        ));
    }
    let pk = string_key_part(key, aex_session_dynamodb::attr::PK)?;
    let sk = string_key_part(key, aex_session_dynamodb::attr::SK)?;
    let Some(evaluated) = parse_journal_sort_key(sk) else {
        return Err(invalid_cursor("the continuation sk is not a journal key"));
    };
    if pk != partition || evaluated.next() != next || query_from > evaluated {
        return Err(invalid_cursor(
            "the continuation does not match the requested agent and decoded page tail",
        ));
    }
    Ok(JournalCursor::new(
        [
            (aex_session_dynamodb::attr::PK, pk),
            (aex_session_dynamodb::attr::SK, sk),
        ],
        query_from,
        next,
    ))
}

fn encode_cursor(
    cursor: &JournalCursor,
    partition: &str,
    from: JournalSeq,
) -> Result<Item, StoreError> {
    let parts = cursor.parts();
    if parts.len() != 2 || cursor.next() != from {
        return Err(invalid_cursor(
            "the supplied continuation does not contain the expected native key",
        ));
    }
    let pk = parts
        .get(aex_session_dynamodb::attr::PK)
        .ok_or_else(|| invalid_cursor("the supplied continuation has no pk"))?;
    let sk = parts
        .get(aex_session_dynamodb::attr::SK)
        .ok_or_else(|| invalid_cursor("the supplied continuation has no sk"))?;
    let evaluated = parse_journal_sort_key(sk);
    if pk != partition
        || evaluated.map(JournalSeq::next) != Some(from)
        || evaluated.is_none_or(|seq| cursor.start() > seq)
    {
        return Err(invalid_cursor(
            "the supplied continuation addresses another agent or sequence",
        ));
    }
    Ok(Item::from([
        (aex_session_dynamodb::attr::PK.to_owned(), s(pk.clone())),
        (aex_session_dynamodb::attr::SK.to_owned(), s(sk.clone())),
    ]))
}

fn string_key_part<'a>(key: &'a Item, name: &str) -> Result<&'a str, StoreError> {
    match key.get(name) {
        Some(aws_sdk_dynamodb::types::AttributeValue::S(value)) => Ok(value),
        _ => Err(invalid_cursor(
            "a native continuation component is absent or is not a string",
        )),
    }
}

fn parse_journal_sort_key(value: &str) -> Option<JournalSeq> {
    let digits = value.strip_prefix(keys::journal_prefix())?;
    if digits.len() != 20 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().map(JournalSeq)
}

fn invalid_cursor(reason: &str) -> StoreError {
    StoreError::Undecodable {
        location: "journal continuation".to_owned(),
        reason: reason.to_owned(),
    }
}

fn undecodable(location: &str, error: &impl core::fmt::Display) -> StoreError {
    StoreError::Undecodable {
        location: location.to_owned(),
        reason: error.to_string(),
    }
}

fn parse_hash(text: &str) -> Option<ContentHash> {
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(ContentHash(bytes))
}

pub(crate) fn store_key_error(error: &crate::keys::BrainKeyError) -> StoreError {
    undecodable("agent key", error)
}

pub(crate) fn translate_error(error: &crate::translate::TranslateError) -> StoreError {
    undecodable("timestamp", error)
}

pub(crate) fn transport<E, R>(
    operation: &str,
    error: &aws_sdk_dynamodb::error::SdkError<E, R>,
) -> StoreError
where
    E: aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    let mapped = aex_session_dynamodb::error::classify(
        error,
        aex_session_dynamodb::error::Idempotence::Read,
    );
    StoreError::Transport {
        reason: format!("{operation}: {mapped}"),
        retryable: mapped.retryable(),
    }
}

fn effect_condition<R>(
    id: EffectId,
    operation: &str,
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::update_item::UpdateItemError,
        R,
    >,
) -> CommitError {
    if error.as_service_error().is_some_and(
        aws_sdk_dynamodb::operation::update_item::UpdateItemError::is_conditional_check_failed_exception,
    ) {
        // The effect is not in the state this write required. That is never a retry: the
        // durable record already says what happened to this attempt.
        return CommitError::Condition(ConditionFailure::EffectStateMismatch { effect: id });
    }
    CommitError::Store(transport(operation, error))
}

fn dispatch_transaction_error<R>(
    id: EffectId,
    participants: &[aex_session_dynamodb::plan::Participant],
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError,
        R,
    >,
) -> CommitError {
    let mapped = match error.as_service_error() {
        Some(service) => aex_session_dynamodb::error::decode_cancellation(service, participants),
        None => aex_session_dynamodb::error::classify(
            error,
            aex_session_dynamodb::error::Idempotence::Write(
                aex_session_dynamodb::error::Resolution::TargetItem,
            ),
        ),
    };
    match mapped {
        aex_session_dynamodb::error::StoreError::PreconditionFailed {
            participant: aex_session_dynamodb::plan::Participant::SESSION_HEAD_GUARD,
            ..
        } => CommitError::Condition(ConditionFailure::CancelEpochAdvanced),
        aex_session_dynamodb::error::StoreError::PreconditionFailed {
            participant: aex_session_dynamodb::plan::Participant::AGENT_CONTROL,
            ..
        } => CommitError::Condition(ConditionFailure::StaleFence),
        aex_session_dynamodb::error::StoreError::PreconditionFailed {
            participant: aex_session_dynamodb::plan::Participant::AGENT_EFFECT,
            ..
        } => CommitError::Condition(ConditionFailure::EffectStateMismatch { effect: id }),
        other => commit_store_error(&other),
    }
}

fn commit_plan_error(error: crate::plan::PlanError) -> CommitError {
    match error {
        crate::plan::PlanError::Envelope(violation) => CommitError::Envelope(violation),
        crate::plan::PlanError::Key(key) => CommitError::Store(store_key_error(&key)),
        crate::plan::PlanError::Store(store) => commit_store_error(&store),
        crate::plan::PlanError::Boundary(reason) => CommitError::Store(StoreError::Undecodable {
            location: "root run boundary".to_owned(),
            reason,
        }),
    }
}

/// Maps the shared store vocabulary onto the Brain's commit failures.
///
/// This is only decodable because the plan named every participant: `DynamoDB` returns a
/// positional reason vector, and a position means nothing to a caller that never saw the
/// compiled request.
#[must_use]
pub fn commit_store_error(error: &aex_session_dynamodb::error::StoreError) -> CommitError {
    use aex_session_dynamodb::error::StoreError as Shared;
    match error {
        Shared::PreconditionFailed { participant, .. } => {
            CommitError::Condition(condition_for(*participant))
        }
        Shared::Throttled { retry_after } => CommitError::Throttled {
            retry_after: *retry_after,
        },
        Shared::ItemTooLarge { measured, .. } => {
            CommitError::Envelope(aex_brain_domain::commit::EnvelopeViolation::ItemTooLarge {
                bytes: *measured,
                which: "a compiled action".to_owned(),
            })
        }
        other => CommitError::Store(StoreError::Transport {
            reason: other.to_string(),
            retryable: other.retryable(),
        }),
    }
}

fn replayed() -> ConditionFailure {
    ConditionFailure::IdempotentReplay(Box::new(CommitReceipt {
        revision: aex_brain_domain::ids::AgentRevision::ZERO,
        tail: JournalSeq::ZERO,
        wakes: Vec::new(),
        committed_at: Timestamp::from_millis(0),
    }))
}

/// The precondition failure one losing participant means.
///
/// Named per participant rather than collapsed into one "conflict", because the four
/// answers are genuinely different: reload, replan, treat as success, or quarantine.
#[must_use]
pub fn condition_for(participant: aex_session_dynamodb::plan::Participant) -> ConditionFailure {
    use aex_session_dynamodb::plan::Participant;
    match participant {
        // The head guard fences the cancellation and deletion epochs and nothing else, so a
        // loss there is always "the session moved out from under this decision".
        Participant::SESSION_HEAD_GUARD => ConditionFailure::CancelEpochAdvanced,
        Participant::AGENT_CONTROL => ConditionFailure::StaleFence,
        Participant::WORK_WAKE_DONE => ConditionFailure::WakeStateMoved,
        // A journal put loses only to itself: the sort key is the sequence and the
        // condition is `attribute_not_exists`, so whatever beat it is the same decision
        // arriving twice. The caller treats that as success.
        // One wake outstanding per agent is likewise a durable claim: losing it means a
        // wake is already in flight, which is exactly the state the caller wanted.
        Participant::AGENT_JOURNAL | Participant::WORK_NEXT_WAKE | Participant::WORK_DEDUPE => {
            replayed()
        }
        Participant::AGENT_EFFECT => ConditionFailure::EffectStateMismatch {
            effect: EffectId([0; 16]),
        },
        other
            if other == crate::plan::participant::SESSION_BUDGET
                || other == crate::plan::participant::AGENT_BUDGET =>
        {
            ConditionFailure::BudgetExhausted
        }
        _ => ConditionFailure::ChildStateMismatch,
    }
}
