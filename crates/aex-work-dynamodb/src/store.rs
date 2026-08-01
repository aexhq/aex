//! The `regional-work` port implementation.
//!
//! The due scan reads the slim index projection and never the base row, so a
//! reconciler sweeping for overdue work cannot observe a payload it has no
//! business seeing.

use aex_session_dynamodb::attr::{Item, Row, n, s};
use aex_session_dynamodb::error::{Idempotence, Resolution, StoreError, classify};
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::{Participant, key};
use aex_wire::ids::WorkspaceId;
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure;
use aws_sdk_dynamodb::types::builders::UpdateBuilder;

use crate::claim::{WorkClaim, claim, complete, poison, renew};
use crate::codec::{self, ReconciliationCursor, WorkRecord};
use crate::keys;

/// One page of the due index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuePage {
    /// The outstanding work this page names, in due order.
    pub items: Vec<DueEntry>,
    /// The instant the scan covered, so the reconciler can persist its position
    /// without re-reading the base table.
    pub scanned_through: Option<Timestamp>,
}

/// One row of the slim due-index projection.
///
/// The projection carries no payload, so a due scan can never read one even
/// though the base row has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueEntry {
    /// Which record.
    pub work_id: String,
    /// Its kind.
    pub kind: String,
    /// Its state.
    pub state: String,
    /// Its attempt count.
    pub attempt: u64,
    /// Its attempt budget.
    pub max_attempts: u64,
    /// Its fence.
    pub fence: u64,
    /// When the current lease expires, when it is claimed.
    pub lease_expires_at: Option<Timestamp>,
}

// TODO(cross-stream): `aex-session-app` publishes no work authority. Its ports are
// readers plus `aex_session_app::ports::AuthorityCommitter`, and the runnable-work
// vocabulary lives in `aex_operation_domain::lease` (`WorkItem`, `Lease`,
// `ClaimOutcome`, `WorkCommit`) as data rather than as a trait.
/// The runnable-work authority.
#[async_trait]
pub trait WorkAuthority: Send + Sync + 'static {
    /// Claims one record for `owner` until `lease_until`.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when another worker holds a live lease
    /// or the attempt budget is spent, which the caller acks without work.
    async fn claim_work(
        &self,
        work_id: &str,
        owner: &str,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<WorkClaim, StoreError>;

    /// Extends a live lease without advancing the fence.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when the lease was already stolen.
    async fn renew_claim(
        &self,
        hold: &WorkClaim,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<WorkClaim, StoreError>;

    /// Reads one record.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn load(
        &self,
        workspace: WorkspaceId,
        work_id: &str,
    ) -> Result<Option<WorkRecord>, StoreError>;

    /// Scans one due shard for work due at or before `now`.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn scan_due(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
    ) -> Result<DuePage, StoreError>;

    /// Retires a record under its fence.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when the lease was stolen mid step.
    /// The caller must then discard its prepared effect and **not** retry.
    async fn complete_work(&self, hold: &WorkClaim, now: Timestamp) -> Result<(), StoreError>;

    /// Poisons a record whose attempt budget is spent.
    ///
    /// # Errors
    ///
    /// As [`WorkAuthority::complete_work`].
    async fn poison_work(
        &self,
        hold: &WorkClaim,
        failure_code: &str,
        now: Timestamp,
    ) -> Result<(), StoreError>;

    /// Advances one shard's reconciliation cursor.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when another reconciler advanced it.
    async fn advance_cursor(&self, cursor: &ReconciliationCursor) -> Result<(), StoreError>;
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct WorkStore {
    client: Client,
    table: String,
}

impl WorkStore {
    /// Binds a store to a client and a physical table name.
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

    /// Issues one conditional update and maps a lost condition onto
    /// `participant`.
    async fn conditional_update(
        &self,
        builder: UpdateBuilder,
        participant: Participant,
    ) -> Result<Option<Item>, StoreError> {
        let built = builder.build().map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
        let outcome = self
            .client
            .update_item()
            .table_name(&self.table)
            .set_key(Some(built.key().clone()))
            .set_condition_expression(built.condition_expression().map(str::to_owned))
            .update_expression(built.update_expression())
            .set_expression_attribute_names(built.expression_attribute_names().cloned())
            .set_expression_attribute_values(built.expression_attribute_values().cloned())
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew)
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .send()
            .await;
        match outcome {
            Ok(response) => Ok(response.attributes),
            Err(error) => {
                if let Some(
                    aws_sdk_dynamodb::operation::update_item::UpdateItemError::ConditionalCheckFailedException(failed),
                ) = error.as_service_error()
                {
                    return Err(StoreError::PreconditionFailed {
                        participant,
                        observed: failed.item.clone().map(Box::new),
                    });
                }
                Err(classify(&error, Idempotence::Write(Resolution::TargetItem)))
            }
        }
    }
}

#[async_trait]
impl WorkAuthority for WorkStore {
    async fn claim_work(
        &self,
        work_id: &str,
        owner: &str,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<WorkClaim, StoreError> {
        let builder = claim(&self.table, work_id, owner, now, lease_until)?;
        let attributes = self
            .conditional_update(builder, Participant::WORK_ROOT_WAKE)
            .await?
            .ok_or(StoreError::Invalid {
                detail: "a claim must return the claimed row".to_owned(),
            })?;
        let row = Row::bind(&attributes, codec::WORK)?;
        Ok(WorkClaim {
            work_id: row.string("workId")?.to_owned(),
            fence: row.u64("fence")?,
            owner: owner.to_owned(),
            attempt: row.u64("attempt")?,
            lease_expires_at: row.timestamp("leaseExpiresAt")?,
        })
    }

    async fn renew_claim(
        &self,
        hold: &WorkClaim,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<WorkClaim, StoreError> {
        let builder = renew(&self.table, hold, now, lease_until)?;
        self.conditional_update(builder, Participant::WORK_ROOT_WAKE)
            .await?;
        Ok(WorkClaim {
            lease_expires_at: lease_until,
            ..hold.clone()
        })
    }

    async fn load(
        &self,
        workspace: WorkspaceId,
        work_id: &str,
    ) -> Result<Option<WorkRecord>, StoreError> {
        let work_key = keys::work(work_id)?;
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(&work_key.pk, &work_key.sk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        match output.item {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_work(&item, workspace)?)),
        }
    }

    async fn scan_due(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
    ) -> Result<DuePage, StoreError> {
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .index_name(keys::DUE_INDEX)
            .key_condition_expression("#pk = :pk AND #sk <= :now")
            .expression_attribute_names("#pk", keys::DUE_PK)
            .expression_attribute_names("#sk", keys::DUE_SK)
            .expression_attribute_values(":pk", s(keys::due_partition_for_shard(shard)))
            // The upper bound carries the separator so the whole of that
            // millisecond is included whatever work identity follows it.
            .expression_attribute_values(":now", s(format!("{}#", now.to_wire())))
            .limit(budget.limit())
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;

        let mut items = Vec::new();
        for item in output.items.unwrap_or_default() {
            // The due index is an `INCLUDE` projection and does not carry the
            // discriminator, so a checked bind can never succeed here. The
            // index is sparse on `duePartition`, which only a work row writes,
            // so the family is already established by the key.
            let row = Row::bind_projected(&item, codec::WORK);
            items.push(DueEntry {
                work_id: row.string("workId")?.to_owned(),
                kind: row.enumerated("kind", keys::KINDS)?.to_owned(),
                state: row.enumerated("state", keys::STATES)?.to_owned(),
                attempt: row.u64("attempt")?,
                max_attempts: row.u64("maxAttempts")?,
                fence: row.u64("fence")?,
                lease_expires_at: row.opt_timestamp("leaseExpiresAt")?,
            });
        }
        let scanned_through = (!items.is_empty()).then_some(now);
        Ok(DuePage {
            items,
            scanned_through,
        })
    }

    async fn complete_work(&self, hold: &WorkClaim, now: Timestamp) -> Result<(), StoreError> {
        let builder = complete(&self.table, hold, now)?;
        self.conditional_update(builder, Participant::WORK_WAKE_DONE)
            .await?;
        Ok(())
    }

    async fn poison_work(
        &self,
        hold: &WorkClaim,
        failure_code: &str,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let builder = poison(&self.table, hold, failure_code, now)?;
        self.conditional_update(builder, Participant::WORK_WAKE_DONE)
            .await?;
        Ok(())
    }

    async fn advance_cursor(&self, cursor: &ReconciliationCursor) -> Result<(), StoreError> {
        let previous = cursor.revision.checked_sub(1).ok_or(StoreError::Invalid {
            detail: "a cursor advance always follows a revision".to_owned(),
        })?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(codec::encode_cursor(cursor)))
            .condition_expression("attribute_not_exists(pk) OR revision = :previous")
            .expression_attribute_values(":previous", n(previous))
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Write(Resolution::TargetItem)))?;
        Ok(())
    }
}
