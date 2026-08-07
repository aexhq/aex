//! Transaction plans, their named participants, and the one compiler that turns
//! a plan into a `TransactWriteItems` request.
//!
//! A plan action carries a stable [`Participant`] name. That is the whole
//! reason a cancellation is decodable: `DynamoDB` returns a positional reason
//! vector, and the position only means something if the plan remembers what it
//! put there.
//!
//! There is exactly one compiler in the workspace, and every regional `DynamoDB`
//! adapter uses it. Seven compilers would be seven chances to forget
//! `ReturnValuesOnConditionCheckFailure`, seven chances to omit a condition, and
//! seven different cancellation decodings.

use std::collections::HashMap;
use std::fmt;

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::operation::transact_write_items::builders::TransactWriteItemsFluentBuilder;
use aws_sdk_dynamodb::types::builders::{
    ConditionCheckBuilder, DeleteBuilder, PutBuilder, UpdateBuilder,
};
use aws_sdk_dynamodb::types::{
    AttributeValue, ConditionCheck, Delete, Put, ReturnValuesOnConditionCheckFailure,
    TransactWriteItem, Update,
};

use crate::attr::Item;
use crate::error::StoreError;
use crate::measure;

/// The stable name of one action inside a transaction plan.
///
/// Names are chosen by the planning domain and never derived from the table or
/// the index, so adding an action in front of another cannot silently retarget
/// an existing error mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Participant(&'static str);

impl Participant {
    /// Names one participant.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// The name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Participant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// The participants named by plan 05 §3. A peer domain may name more; these are
/// the ones whose failure semantics are pinned by the plan.
impl Participant {
    /// The projected workspace placement guard.
    pub const AUTHZ_PLACEMENT: Self = Self::new("authz.placement");
    /// The staged content body being committed.
    pub const CONTENT_COMMIT: Self = Self::new("content.commit");
    /// The pin that keeps an admitted message body reachable.
    pub const CONTENT_MESSAGE_PIN: Self = Self::new("content.message_pin");
    /// The session head.
    pub const SESSION_HEAD: Self = Self::new("session.head");
    /// The session head, as a read-only guard.
    pub const SESSION_HEAD_GUARD: Self = Self::new("session.head_guard");
    /// The admitted message.
    pub const SESSION_MESSAGE: Self = Self::new("session.message");
    /// The admitted run.
    pub const SESSION_RUN: Self = Self::new("session.run");
    /// The run's spend reservation.
    pub const SESSION_RESERVATION: Self = Self::new("session.reservation");
    /// The root agent's control item.
    pub const AGENT_ROOT_CONTROL: Self = Self::new("agent.root_control");
    /// An agent's control item.
    pub const AGENT_CONTROL: Self = Self::new("agent.control");
    /// An immutable journal entry.
    pub const AGENT_JOURNAL: Self = Self::new("agent.journal");
    /// A prepared or settled effect.
    pub const AGENT_EFFECT: Self = Self::new("agent.effect");
    /// The `run.admitted` native event.
    pub const SESSION_ADMITTED_EVENT: Self = Self::new("session.admitted_event");
    /// The run's terminal native event.
    pub const SESSION_TERMINAL_EVENT: Self = Self::new("session.terminal_event");
    /// A bounded preview native event.
    pub const SESSION_PREVIEW_EVENT: Self = Self::new("session.preview_event");
    /// The durable idempotency receipt.
    pub const SESSION_IDEMPOTENCY: Self = Self::new("session.idempotency");
    /// The durable operation record.
    pub const SESSION_OPERATION: Self = Self::new("session.operation");
    /// The session-partition edge to a durable operation.
    pub const SESSION_OPERATION_EDGE: Self = Self::new("session.operation_edge");
    /// The root agent's wake.
    pub const WORK_ROOT_WAKE: Self = Self::new("work.root_wake");
    /// An agent's next wake.
    pub const WORK_NEXT_WAKE: Self = Self::new("work.next_wake");
    /// The durable "one wake outstanding" claim.
    pub const WORK_DEDUPE: Self = Self::new("work.dedupe");
    /// The wake being retired by a fenced commit.
    pub const WORK_WAKE_DONE: Self = Self::new("work.wake_done");
    /// The compute-usage closure fact.
    pub const WORK_USAGE_CLOSURE: Self = Self::new("work.usage_closure");
    /// The storage-usage delta fact.
    pub const WORK_USAGE_STORAGE: Self = Self::new("work.usage_storage");
    /// The purge worker's wake.
    pub const WORK_PURGE: Self = Self::new("work.purge");
    /// One due shard's durable reconciliation position.
    pub const WORK_CURSOR: Self = Self::new("work.cursor");
    /// A parent agent's fanout page.
    pub const AGENT_FANOUT_PAGE: Self = Self::new("agent.fanout_page");
    /// The garbage-collection epoch.
    pub const CONTENT_GC_EPOCH: Self = Self::new("content.gc_epoch");
    /// A content descriptor.
    pub const CONTENT_DESCRIPTOR: Self = Self::new("content.descriptor");
    /// A garbage-collection candidate.
    pub const CONTENT_GC_CANDIDATE: Self = Self::new("content.gc_candidate");
    /// A download grant.
    pub const CONTENT_GRANT: Self = Self::new("content.grant");
    /// A download grant's pin.
    pub const CONTENT_GRANT_PIN: Self = Self::new("content.grant_pin");
    /// One grant-expiry shard's durable scan position.
    pub const CONTENT_GRANT_EXPIRY_CURSOR: Self = Self::new("content.grant_expiry_cursor");
    /// A registry pointer.
    pub const REGISTRY_POINTER: Self = Self::new("registry.pointer");
    /// A staged registry upload.
    pub const REGISTRY_UPLOAD: Self = Self::new("registry.upload");
    /// A workspace secret's metadata.
    pub const SECRET_METADATA: Self = Self::new("secret.metadata");
    /// A workspace secret's source generation.
    pub const SECRET_GENERATION: Self = Self::new("secret.generation");
    /// A workspace secret's lineage index entry.
    pub const SECRET_LINEAGE: Self = Self::new("secret.lineage");
    /// A session's custody head.
    pub const CUSTODY_HEAD: Self = Self::new("custody.head");
    /// One custody binding.
    pub const CUSTODY_BINDING: Self = Self::new("custody.binding");
    /// A rebind intent.
    pub const CUSTODY_REBIND: Self = Self::new("custody.rebind");
    /// A managed-call authorization.
    pub const CUSTODY_AUTHORIZATION: Self = Self::new("custody.authorization");
    /// A provider-credential binding directory entry.
    pub const CUSTODY_PROVIDER_CREDENTIAL: Self = Self::new("custody.provider_credential");
    /// A Hands generation head.
    pub const RUNTIME_GENERATION: Self = Self::new("runtime.generation");
    /// The session's current-generation pointer.
    pub const RUNTIME_CURRENT: Self = Self::new("runtime.current");
    /// A Hands lifecycle intent.
    pub const RUNTIME_INTENT: Self = Self::new("runtime.intent");
    /// A Hands lifecycle receipt.
    pub const RUNTIME_RECEIPT: Self = Self::new("runtime.receipt");
}

/// The physical names of the regional tables a plan may address.
///
/// The logical name is fixed by `migrations/regional`; the physical name is
/// `{plane}-{region}-{table}` and is composed by the infrastructure stream, so
/// it arrives as configuration and is never guessed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionalTables {
    /// `session-authority`.
    pub session_authority: String,
    /// `regional-work`.
    pub regional_work: String,
    /// `regional-content`.
    pub regional_content: String,
    /// `regional-registry`.
    pub regional_registry: String,
    /// `regional-secret-custody`.
    pub regional_secret_custody: String,
    /// `regional-secret-keystore`.
    pub regional_secret_keystore: String,
    /// `runtime-activity`.
    pub runtime_activity: String,
    /// `regional-authz-projection`.
    pub regional_authz_projection: String,
}

impl RegionalTables {
    /// Composes every physical name from a plane and a region.
    ///
    /// This mirrors what the infrastructure stream instantiates; a deployable
    /// that reads explicit names from configuration builds the struct directly.
    #[must_use]
    pub fn composed(plane: &str, region: &str) -> Self {
        let name = |logical: &str| format!("{plane}-{region}-{logical}");
        Self {
            session_authority: name("session-authority"),
            regional_work: name("regional-work"),
            regional_content: name("regional-content"),
            regional_registry: name("regional-registry"),
            regional_secret_custody: name("regional-secret-custody"),
            regional_secret_keystore: name("regional-secret-keystore"),
            runtime_activity: name("runtime-activity"),
            regional_authz_projection: name("regional-authz-projection"),
        }
    }
}

/// A composed transaction, ready to compile.
#[derive(Debug, Clone)]
pub struct TransactionPlan {
    client_request_token: String,
    participants: Vec<Participant>,
    actions: Vec<TransactWriteItem>,
}

/// The provider ceiling on one `TransactWriteItems` call.
///
/// This matches `aex-session-app`, so application validation and compiled
/// request validation enforce the same atomic envelope.
pub const MAX_ACTIONS: usize = 100;

impl TransactionPlan {
    /// Starts a plan whose transport deduplication identity is
    /// `client_request_token`.
    ///
    /// The token gives free deduplication inside the provider's ten-minute
    /// window. It is **transport** deduplication only; the durable idempotency
    /// receipt remains the product authority beyond that window.
    #[must_use]
    pub fn new(client_request_token: impl Into<String>) -> Self {
        Self {
            client_request_token: client_request_token.into(),
            participants: Vec::new(),
            actions: Vec::new(),
        }
    }

    /// Adds a read-only guard.
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] when the builder is incomplete or carries no
    /// condition expression.
    pub fn condition_check(
        &mut self,
        participant: Participant,
        builder: ConditionCheckBuilder,
    ) -> Result<&mut Self, StoreError> {
        let action: ConditionCheck = builder
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .build()
            .map_err(|error| invalid(participant, &error))?;
        require_condition(participant, Some(action.condition_expression()))?;
        self.push(
            participant,
            TransactWriteItem::builder().condition_check(action).build(),
        );
        Ok(self)
    }

    /// Adds a conditional put.
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] when the builder is incomplete or unconditional,
    /// and [`StoreError::ItemTooLarge`] when the item exceeds the application
    /// ceiling.
    pub fn put(
        &mut self,
        participant: Participant,
        builder: PutBuilder,
    ) -> Result<&mut Self, StoreError> {
        let action: Put = builder
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .build()
            .map_err(|error| invalid(participant, &error))?;
        require_condition(participant, action.condition_expression())?;
        measure::check_item(action.item())?;
        self.push(
            participant,
            TransactWriteItem::builder().put(action).build(),
        );
        Ok(self)
    }

    /// Adds a conditional update.
    ///
    /// # Errors
    ///
    /// As [`TransactionPlan::put`], minus the size check, which `DynamoDB` applies
    /// to the resulting item and which the caller preflights on the read side.
    pub fn update(
        &mut self,
        participant: Participant,
        builder: UpdateBuilder,
    ) -> Result<&mut Self, StoreError> {
        let action: Update = builder
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .build()
            .map_err(|error| invalid(participant, &error))?;
        require_condition(participant, action.condition_expression())?;
        self.push(
            participant,
            TransactWriteItem::builder().update(action).build(),
        );
        Ok(self)
    }

    /// Adds a conditional delete.
    ///
    /// # Errors
    ///
    /// As [`TransactionPlan::update`].
    pub fn delete(
        &mut self,
        participant: Participant,
        builder: DeleteBuilder,
    ) -> Result<&mut Self, StoreError> {
        let action: Delete = builder
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .build()
            .map_err(|error| invalid(participant, &error))?;
        require_condition(participant, action.condition_expression())?;
        self.push(
            participant,
            TransactWriteItem::builder().delete(action).build(),
        );
        Ok(self)
    }

    fn push(&mut self, participant: Participant, action: TransactWriteItem) {
        self.participants.push(participant);
        self.actions.push(action);
    }

    /// The participants, in plan order. Index *i* here is reason *i* there.
    #[must_use]
    pub fn participants(&self) -> &[Participant] {
        &self.participants
    }

    /// How many actions the plan holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Whether the plan holds no action at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// The transport deduplication identity.
    #[must_use]
    pub fn client_request_token(&self) -> &str {
        &self.client_request_token
    }

    /// Compiles the plan into a request builder.
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] for an empty plan or one above [`MAX_ACTIONS`].
    /// An empty transaction is always a planning bug: a command that decided to
    /// do nothing should not have produced a plan.
    pub fn compile(&self, client: &Client) -> Result<TransactWriteItemsFluentBuilder, StoreError> {
        if self.actions.is_empty() {
            return Err(StoreError::Invalid {
                detail: "a transaction plan with no action is always a planning bug".to_owned(),
            });
        }
        if self.actions.len() > MAX_ACTIONS {
            return Err(StoreError::Invalid {
                detail: format!(
                    "a transaction plan holds {} actions; the provider ceiling is {MAX_ACTIONS}",
                    self.actions.len()
                ),
            });
        }
        Ok(client
            .transact_write_items()
            .client_request_token(self.client_request_token.clone())
            .set_transact_items(Some(self.actions.clone())))
    }

    /// The compiled actions, for a test that asserts plan shape without a
    /// client.
    #[must_use]
    pub fn actions(&self) -> &[TransactWriteItem] {
        &self.actions
    }
}

fn require_condition(participant: Participant, condition: Option<&str>) -> Result<(), StoreError> {
    if condition.is_some_and(|expression| !expression.trim().is_empty()) {
        return Ok(());
    }
    Err(StoreError::Invalid {
        detail: format!(
            "participant `{participant}` carries no condition expression; an unconditional \
             authority write is a bug"
        ),
    })
}

fn invalid(participant: Participant, error: &impl fmt::Display) -> StoreError {
    StoreError::Invalid {
        detail: format!("participant `{participant}` could not be built: {error}"),
    }
}

/// A key, in the one attribute-name convention every regional table but the
/// provider-mandated keystore uses.
#[must_use]
pub fn key(pk: &str, sk: &str) -> HashMap<String, AttributeValue> {
    HashMap::from([
        (crate::attr::PK.to_owned(), crate::attr::s(pk)),
        (crate::attr::SK.to_owned(), crate::attr::s(sk)),
    ])
}

/// The condition every immutable row is written under.
pub const IMMUTABLE: &str = "attribute_not_exists(pk)";

/// Adds the `pk`/`sk` attributes to an item so a `Put` and its key agree.
#[must_use]
pub fn keyed(item: Item, pk: &str, sk: &str) -> Item {
    let mut item = item;
    item.insert(crate::attr::PK.to_owned(), crate::attr::s(pk));
    item.insert(crate::attr::SK.to_owned(), crate::attr::s(sk));
    item
}

#[cfg(test)]
mod tests {
    use aws_sdk_dynamodb::types::builders::{PutBuilder, UpdateBuilder};

    use super::{IMMUTABLE, Participant, RegionalTables, TransactionPlan, key, keyed};
    use crate::attr::{ItemBuilder, s};
    use crate::error::StoreError;

    fn conditional_put() -> PutBuilder {
        aws_sdk_dynamodb::types::Put::builder()
            .table_name("dev-eu-west-1-session-authority")
            .set_item(Some(keyed(
                ItemBuilder::new("run").set("runId", s("run_1")).build(),
                "SESSION#s",
                "RUN#r",
            )))
            .condition_expression(IMMUTABLE)
    }

    #[test]
    fn an_unconditional_write_is_refused_by_the_compiler() {
        let mut plan = TransactionPlan::new("token");
        let error = plan
            .put(
                Participant::SESSION_RUN,
                aws_sdk_dynamodb::types::Put::builder()
                    .table_name("t")
                    .set_item(Some(keyed(ItemBuilder::new("run").build(), "a", "b"))),
            )
            .expect_err("no condition expression");
        assert!(matches!(error, StoreError::Invalid { .. }), "{error}");
    }

    #[test]
    fn an_update_without_a_condition_is_refused_too() {
        let mut plan = TransactionPlan::new("token");
        let builder: UpdateBuilder = aws_sdk_dynamodb::types::Update::builder()
            .table_name("t")
            .set_key(Some(key("a", "b")))
            .update_expression("SET #x = :x");
        assert!(plan.update(Participant::SESSION_HEAD, builder).is_err());
    }

    #[test]
    fn participants_keep_plan_order() {
        let mut plan = TransactionPlan::new("token");
        plan.put(Participant::SESSION_RUN, conditional_put())
            .expect("put");
        plan.put(Participant::SESSION_MESSAGE, conditional_put())
            .expect("put");
        assert_eq!(
            plan.participants(),
            [Participant::SESSION_RUN, Participant::SESSION_MESSAGE]
        );
        assert_eq!(plan.len(), 2);
    }

    #[test]
    fn table_names_are_composed_from_the_plane_and_region() {
        let tables = RegionalTables::composed("dev", "eu-west-1");
        assert_eq!(tables.session_authority, "dev-eu-west-1-session-authority");
        assert_eq!(
            tables.regional_secret_keystore,
            "dev-eu-west-1-regional-secret-keystore"
        );
    }
}
