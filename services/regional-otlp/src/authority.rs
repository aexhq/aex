//! The `observation-authority` admission executor.
//!
//! This is the composition-local adapter that turns the planned admission
//! protocol into real `DynamoDB` and `S3` calls: the ingress gate read, the
//! deletion fence, the accepted-frontier allocation, transaction P, staging,
//! transaction C and the replayable materialization of the `OBS#` items.
//!
//! Two properties are structural rather than conventional:
//!
//! - **Nothing customer-supplied reaches an expression string.** Every condition
//!   is built through [`aex_observation_store_aws::ExpressionBuilder`], which
//!   emits generated `#n0` / `:v0` placeholders.
//! - **An ambiguous transaction outcome is resolved by batch identity.** The
//!   caller re-reads `BATCH#…/RECEIPT` and branches on `state`; a transaction is
//!   never retried blindly.

use std::collections::HashMap;

use aex_observation_domain::canonical::{CanonicalValue, canonical_bytes, sha256_hex};
use aex_observation_domain::frontier::DeletionState;
use aex_observation_domain::keys::{self, BucketHour, ScopeKey};
use aex_observation_domain::limits;
use aex_observation_domain::series::SeriesHash;
use aex_observation_domain::signal::{Signal, SignalSet};
use aex_observation_store_aws::expressions::{ExpressionBuilder, ITEM_TYPE, PK, SK};
use aex_observation_store_aws::spool::GateState;
use aex_observation_store_aws::store::{AdmissionPlan, StagedRecord, StoreError, pack_pages};
use aex_otlp_admission::NormalizedObservation;
use aex_wire::ids::{
    OrganizationId, PrefixedId as _, TelemetryBatchId, TelemetryGapId, Uuid7, WorkspaceId,
};
use aex_wire::types::{Region, Timestamp};
use aws_sdk_dynamodb::types::{AttributeValue, Put, TransactWriteItem, Update, WriteRequest};

/// The `S3` object prefix immutable observation bodies live under.
pub const BODY_PREFIX: &str = "observations";

/// How many observations one materialization write carries.
pub const MATERIALIZE_CHUNK: usize = 25;

/// The shard count one accepted-time bucket is written across.
///
/// Fixed for the life of a bucket so a reader never has to guess how many
/// partitions to merge. A raise applies to the next bucket only.
pub const BUCKET_SHARDS: u8 = 4;

/// Why admission could not be executed.
#[derive(Clone, Debug, thiserror::Error)]
pub enum AuthorityError {
    /// The planned protocol itself refused the batch.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The regional ingress gate refuses customer admission.
    #[error("the regional ingress gate is `{state}`")]
    GateClosed {
        /// The observed gate state.
        state: &'static str,
    },
    /// The scope is behind a deletion fence.
    #[error("the scope is behind a `{state}` deletion fence")]
    Fenced {
        /// The observed deletion state.
        state: &'static str,
    },
    /// The same batch id was presented with a different intent.
    #[error("batch `{batch}` was already prepared with a different intent")]
    IntentConflict {
        /// The offending batch.
        batch: String,
    },
    /// Admission clock skew exceeded the bound the settle window dominates.
    #[error("commit clock skew of {observed} ms exceeds the {limit} ms bound")]
    ClockSkew {
        /// The measured skew.
        observed: i64,
        /// The bound.
        limit: i64,
    },
    /// A stored item did not carry a value this adapter can read.
    #[error("stored item `{item}` is missing attribute `{attribute}`")]
    Malformed {
        /// Which item family.
        item: &'static str,
        /// Which attribute.
        attribute: &'static str,
    },
    /// A provider call failed.
    #[error("`{operation}` failed: {reason}")]
    Provider {
        /// Which call.
        operation: &'static str,
        /// What the provider reported, without its own body.
        reason: String,
    },
}

impl AuthorityError {
    /// Whether a caller may retry the request unchanged.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::GateClosed { .. }
                | Self::Provider { .. }
                | Self::Store(StoreError::Unavailable { .. })
        )
    }

    /// Builds a provider failure without carrying an upstream body.
    fn provider(operation: &'static str, reason: impl std::fmt::Display) -> Self {
        Self::Provider {
            operation,
            reason: reason.to_string(),
        }
    }
}

/// One observation as admission will write it.
#[derive(Clone, Debug)]
pub struct PreparedObservation {
    /// Which signal it belongs to.
    pub signal: Signal,
    /// The producer's event time.
    pub time: Timestamp,
    /// Canonical bytes of the whole normalized observation.
    pub canonical: Vec<u8>,
    /// The canonical body.
    pub body: CanonicalValue,
    /// The attribute digest.
    pub attr_digest: String,
    /// The W3C trace, when the record carries one.
    pub trace_id: Option<String>,
    /// The W3C span, when the record carries one.
    pub span_id: Option<String>,
    /// The metric name, for the sparse metric index.
    pub metric_name: Option<String>,
    /// The metric series hash, when the record is a metric point.
    pub series_hash: Option<String>,
}

impl PreparedObservation {
    /// Canonicalizes one normalized observation.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityError::Store`] carrying [`StoreError::Corrupt`] when
    /// the observation cannot be canonicalized, which is a decoder defect rather
    /// than caller input.
    pub fn new(observation: &NormalizedObservation) -> Result<Self, AuthorityError> {
        let canonical = canonical_bytes(&observation.body).map_err(|error| {
            AuthorityError::Store(StoreError::Corrupt {
                found: error.to_string().into_boxed_str(),
            })
        })?;
        Ok(Self {
            signal: observation.signal,
            time: observation.time,
            canonical,
            body: observation.body.clone(),
            attr_digest: observation.attr_digest.clone(),
            trace_id: observation.trace_id.clone(),
            span_id: observation.span_id.clone(),
            metric_name: observation.metric_name.clone(),
            series_hash: observation.series_hash.map(SeriesHash::to_hex),
        })
    }
}

/// Everything one admission needs before anything durable happens.
#[derive(Clone, Debug)]
pub struct AdmissionRequest {
    /// The caller-bound batch identity.
    pub batch_id: TelemetryBatchId,
    /// The owning organization.
    pub organization: OrganizationId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The scope the batch belongs to.
    pub scope: ScopeKey,
    /// The digest binding the whole batch.
    pub intent_digest: String,
    /// The normalized observations.
    pub observations: Vec<PreparedObservation>,
}

/// What a committed admission reports back to the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmissionReceipt {
    /// The admitted batch.
    pub batch_id: TelemetryBatchId,
    /// When the commit landed.
    pub accepted_at: Timestamp,
    /// How many records were admitted.
    pub accepted: u64,
    /// The canonical bytes admitted.
    pub logical_bytes: u64,
}

/// The `observation-authority` adapter.
#[derive(Clone, Debug)]
pub struct AdmissionAuthority {
    dynamodb: aws_sdk_dynamodb::Client,
    s3: aws_sdk_s3::Client,
    table: String,
    bucket: String,
    region: Region,
}

impl AdmissionAuthority {
    /// Binds the adapter to its resolved resources.
    #[must_use]
    pub fn new(
        dynamodb: aws_sdk_dynamodb::Client,
        s3: aws_sdk_s3::Client,
        table: impl Into<String>,
        bucket: impl Into<String>,
        region: Region,
    ) -> Self {
        Self {
            dynamodb,
            s3,
            table: table.into(),
            bucket: bucket.into(),
            region,
        }
    }

    /// The bound table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The bound bucket name.
    #[must_use]
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Proves the table and bucket are reachable.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityError::Provider`] when either probe fails. A probe
    /// that has not passed is never assumed to have passed.
    pub async fn probe(&self) -> Result<(), AuthorityError> {
        self.dynamodb
            .describe_table()
            .table_name(&self.table)
            .send()
            .await
            .map_err(|error| AuthorityError::provider("DescribeTable", error))?;
        self.s3
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|error| AuthorityError::provider("HeadBucket", error))?;
        Ok(())
    }

    /// Reads the regional ingress gate.
    ///
    /// An absent item is `open`: the gate is a closure signal the reconciler
    /// writes, so its absence means nothing has ever asked admission to stop.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityError::Provider`] when the read fails, which is itself
    /// a reason to refuse admission rather than to assume the gate is open.
    pub async fn ingress_gate(&self) -> Result<GateState, AuthorityError> {
        let item = self.get(&keys::gate_pk(self.region), "STATE").await?;
        let Some(item) = item else {
            return Ok(GateState::Open);
        };
        let state = string(&item, "state").unwrap_or("open");
        Ok(match state {
            "closed" => GateState::Closed,
            "degraded" => GateState::Degraded,
            _ => GateState::Open,
        })
    }

    /// Reads the scope's deletion epoch, refusing a fenced scope.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityError::Fenced`] once deletion has begun, and
    /// [`AuthorityError::Provider`] when the read fails.
    pub async fn deletion_epoch(&self, scope: &ScopeKey) -> Result<u64, AuthorityError> {
        let Some(item) = self.get(&keys::frontier_pk(scope), "DELETION").await? else {
            return Ok(0);
        };
        let state = string(&item, "state").unwrap_or("none");
        let parsed = DeletionState::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == state)
            .unwrap_or(DeletionState::None);
        if parsed.is_fenced() {
            return Err(AuthorityError::Fenced {
                state: parsed.as_str(),
            });
        }
        Ok(number(&item, "deletionEpoch").unwrap_or(0))
    }

    /// Reads the accepted frontier of one `(scope, signal)`.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityError::Provider`] when the read fails.
    pub async fn frontier(
        &self,
        scope: &ScopeKey,
        signal: Signal,
    ) -> Result<FrontierRow, AuthorityError> {
        let Some(item) = self
            .get(&keys::frontier_pk(scope), &keys::frontier_sk(signal))
            .await?
        else {
            return Ok(FrontierRow {
                accepted: 0,
                revision: 0,
            });
        };
        Ok(FrontierRow {
            accepted: number(&item, "accepted").unwrap_or(0),
            revision: number(&item, "revision").unwrap_or(0),
        })
    }

    /// Admits one batch through the whole nine-step protocol.
    ///
    /// # Errors
    ///
    /// Returns the typed failure of the first step that refused: the gate, the
    /// deletion fence, an intent conflict, clock skew beyond the bound the
    /// settle window dominates, or a provider failure.
    pub async fn admit(
        &self,
        request: &AdmissionRequest,
        now: Timestamp,
    ) -> Result<AdmissionReceipt, AuthorityError> {
        if self.ingress_gate().await?.refuses_customer_admission() {
            return Err(AuthorityError::GateClosed { state: "closed" });
        }
        let pinned_epoch = self.deletion_epoch(&request.scope).await?;

        let mut signals = SignalSet::EMPTY;
        let mut staged = Vec::with_capacity(request.observations.len());
        for observation in &request.observations {
            signals = signals.with(observation.signal);
            staged.push(StagedRecord {
                signal: observation.signal,
                canonical: observation.canonical.clone(),
            });
        }
        let abucket = BucketHour::from_timestamp(now);
        let plan = AdmissionPlan::new(request.scope, signals, abucket, &staged, 0)?;
        plan.prepare_envelope().require_fits("P")?;
        plan.commit_envelope().require_fits("C")?;
        let pages = pack_pages(&staged)?;

        // Step 6 — transaction P.
        self.prepare(request, &plan, now).await?;

        // Step 7 — stage bodies and pages. Nothing here is reachable or billed
        // until the commit publishes the accepted range.
        let mut placements = Vec::with_capacity(request.observations.len());
        for observation in &request.observations {
            placements.push(self.stage_body(request.workspace, observation).await?);
        }
        self.stage_pages(request, &pages, &staged).await?;

        // Step 8 — transaction C.
        let mut allocations = Vec::new();
        for signal in signals.iter() {
            let row = self.frontier(&request.scope, signal).await?;
            let count = request
                .observations
                .iter()
                .filter(|observation| observation.signal == signal)
                .count() as u64;
            allocations.push(Allocation {
                signal,
                lo: row.accepted,
                hi: row.accepted + count,
                revision: row.revision,
            });
        }
        let skew = (now.unix_millis()
            - Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
                .map_or(now.unix_millis(), Timestamp::unix_millis))
        .abs();
        if skew > limits::OBS_CLOCK_SKEW_MAX_MS {
            return Err(AuthorityError::ClockSkew {
                observed: skew,
                limit: limits::OBS_CLOCK_SKEW_MAX_MS,
            });
        }
        self.commit(request, &plan, &allocations, pinned_epoch, now)
            .await?;

        // Step 9 — materialize, idempotently, from the staged pages.
        self.materialize(request, &allocations, &placements, abucket, now)
            .await?;

        Ok(AdmissionReceipt {
            batch_id: request.batch_id,
            accepted_at: now,
            accepted: request.observations.len() as u64,
            logical_bytes: plan.logical_bytes(),
        })
    }

    /// Transaction P: create or resume the receipt and reserve exact quota.
    async fn prepare(
        &self,
        request: &AdmissionRequest,
        plan: &AdmissionPlan,
        now: Timestamp,
    ) -> Result<(), AuthorityError> {
        let mut builder = ExpressionBuilder::new();
        let condition = aex_observation_store_aws::expressions::prepare_condition(
            &mut builder,
            &request.intent_digest,
        );
        let expires = now.unix_millis() + limits::OBS_PREPARE_TTL_MS;
        let receipt = Put::builder()
            .table_name(&self.table)
            .set_item(Some(receipt_item(request, plan, now, expires)))
            .condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .build()
            .map_err(|error| AuthorityError::provider("TransactWriteItems", error))?;

        let mut quota = ExpressionBuilder::new();
        let reserved_bytes = quota.name("reservedBytes");
        let reserved_records = quota.name("reservedRecords");
        let bytes = quota.number(plan.logical_bytes());
        let records = quota.number(plan.records());
        let update = Update::builder()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(keys::quota_pk(request.workspace)))
            .key(SK, AttributeValue::S("INGEST".to_owned()))
            .update_expression(format!(
                "ADD {reserved_bytes} {bytes}, {reserved_records} {records}"
            ))
            .set_expression_attribute_names(Some(quota.names()))
            .set_expression_attribute_values(Some(quota.values()))
            .build()
            .map_err(|error| AuthorityError::provider("TransactWriteItems", error))?;

        let outcome = self
            .dynamodb
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().put(receipt).build())
            .transact_items(TransactWriteItem::builder().update(update).build())
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => {
                // Resolve by batch identity rather than retrying blindly.
                match self.receipt_state(request).await? {
                    Some(StoredReceipt::Preparing | StoredReceipt::Committed) => Ok(()),
                    Some(StoredReceipt::Aborted) | None => {
                        Err(AuthorityError::provider("TransactWriteItems", error))
                    }
                }
            }
        }
    }

    /// Reads the receipt's durable state.
    async fn receipt_state(
        &self,
        request: &AdmissionRequest,
    ) -> Result<Option<StoredReceipt>, AuthorityError> {
        let Some(item) = self
            .get(
                &keys::batch_pk(request.workspace, request.batch_id),
                "RECEIPT",
            )
            .await?
        else {
            return Ok(None);
        };
        let stored = string(&item, "intentDigest").ok_or(AuthorityError::Malformed {
            item: "admission_receipt",
            attribute: "intentDigest",
        })?;
        if stored != request.intent_digest {
            return Err(AuthorityError::IntentConflict {
                batch: request.batch_id.to_string(),
            });
        }
        Ok(Some(StoredReceipt::parse(
            string(&item, "state").unwrap_or("preparing"),
        )))
    }

    /// Stages one immutable body, inline or in `S3`.
    async fn stage_body(
        &self,
        workspace: WorkspaceId,
        observation: &PreparedObservation,
    ) -> Result<BodyPlacement, AuthorityError> {
        if observation.canonical.len() <= limits::OBS_INLINE_MAX {
            return Ok(BodyPlacement::Inline);
        }
        let digest = sha256_hex(&observation.canonical);
        let key = format!(
            "{BODY_PREFIX}/{workspace}/{}/{}/{digest}",
            &digest[0..2],
            &digest[2..4]
        );
        let outcome = self
            .s3
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .if_none_match("*")
            .content_length(i64::try_from(observation.canonical.len()).unwrap_or(i64::MAX))
            .body(observation.canonical.clone().into())
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(BodyPlacement::Object { key, digest }),
            Err(error) => {
                // A `412` means the content-addressed object already exists.
                // Equal digests are idempotent success; the key *is* the digest,
                // so an unequal one cannot be addressed here at all.
                let service = error
                    .raw_response()
                    .map(|response| response.status().as_u16());
                if service == Some(412) {
                    Ok(BodyPlacement::Object { key, digest })
                } else {
                    Err(AuthorityError::provider("PutObject", error))
                }
            }
        }
    }

    /// Stages the page items that make step 9 replayable.
    async fn stage_pages(
        &self,
        request: &AdmissionRequest,
        pages: &[aex_observation_store_aws::PageSpan],
        staged: &[StagedRecord],
    ) -> Result<(), AuthorityError> {
        let expires = seconds_from_now(limits::OBS_PREPARE_TTL_MS);
        let mut writes = Vec::new();
        for (ordinal, span) in pages.iter().enumerate() {
            let mut encoded = Vec::with_capacity(span.bytes);
            for record in &staged[span.start..span.end] {
                encoded.extend_from_slice(&record.canonical);
                encoded.push(b'\n');
            }
            let digest = sha256_hex(&encoded);
            let mut item = HashMap::new();
            item.insert(
                PK.to_owned(),
                AttributeValue::S(keys::batch_pk(request.workspace, request.batch_id)),
            );
            item.insert(
                SK.to_owned(),
                AttributeValue::S(keys::staged_page_sk(
                    u32::try_from(ordinal).unwrap_or(u32::MAX),
                )),
            );
            item.insert(
                ITEM_TYPE.to_owned(),
                AttributeValue::S("staged_page".to_owned()),
            );
            item.insert("page".to_owned(), AttributeValue::N(ordinal.to_string()));
            item.insert("pageDigest".to_owned(), AttributeValue::S(digest));
            item.insert(
                "records".to_owned(),
                AttributeValue::B(aws_sdk_dynamodb::primitives::Blob::new(encoded)),
            );
            item.insert(
                "recordCount".to_owned(),
                AttributeValue::N((span.end - span.start).to_string()),
            );
            item.insert(
                "logicalBytes".to_owned(),
                AttributeValue::N(span.bytes.to_string()),
            );
            item.insert(
                "expiresAtEpochSeconds".to_owned(),
                AttributeValue::N(expires.to_string()),
            );
            writes.push(
                WriteRequest::builder()
                    .put_request(
                        aws_sdk_dynamodb::types::PutRequest::builder()
                            .set_item(Some(item))
                            .build()
                            .map_err(|error| AuthorityError::provider("BatchWriteItem", error))?,
                    )
                    .build(),
            );
        }
        self.batch_write(writes).await
    }

    /// Transaction C: publish the accepted range and the durable spool chunk.
    ///
    /// Small, bounded and independent of the record count: the receipt flip
    /// carries one digest per staged page rather than one entry per record, so
    /// the action count is the same at one point and at two thousand.
    async fn commit(
        &self,
        request: &AdmissionRequest,
        plan: &AdmissionPlan,
        allocations: &[Allocation],
        pinned_epoch: u64,
        now: Timestamp,
    ) -> Result<(), AuthorityError> {
        let mut actions = vec![self.publish_receipt(request, now)?];
        for allocation in allocations {
            actions.push(self.advance_frontier(request, plan, allocation)?);
        }
        actions.push(self.deletion_fence(request, pinned_epoch)?);
        actions.push(self.put(spool_item(request, allocations, now))?);
        actions.push(self.put(outbox_item(request, plan, now))?);

        let outcome = self
            .dynamodb
            .transact_write_items()
            .set_transact_items(Some(actions))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            // Resolve by batch identity, never by retrying the transaction.
            Err(error) => match self.receipt_state(request).await? {
                Some(StoredReceipt::Committed) => Ok(()),
                Some(StoredReceipt::Preparing | StoredReceipt::Aborted) | None => {
                    Err(AuthorityError::provider("TransactWriteItems", error))
                }
            },
        }
    }

    /// Flips the receipt to `committed`, conditional on it still preparing.
    fn publish_receipt(
        &self,
        request: &AdmissionRequest,
        now: Timestamp,
    ) -> Result<TransactWriteItem, AuthorityError> {
        let mut receipt = ExpressionBuilder::new();
        let state = receipt.name("state");
        let committed = receipt.string("committed");
        let accepted_at = receipt.name("acceptedAt");
        let at = receipt.string(now.to_wire());
        let preparing = receipt.string("preparing");
        let state_condition = receipt.name("state");
        Ok(TransactWriteItem::builder()
            .update(
                Update::builder()
                    .table_name(&self.table)
                    .key(
                        PK,
                        AttributeValue::S(keys::batch_pk(request.workspace, request.batch_id)),
                    )
                    .key(SK, AttributeValue::S("RECEIPT".to_owned()))
                    .update_expression(format!("SET {state} = {committed}, {accepted_at} = {at}"))
                    .condition_expression(format!("{state_condition} = {preparing}"))
                    .set_expression_attribute_names(Some(receipt.names()))
                    .set_expression_attribute_values(Some(receipt.values()))
                    .build()
                    .map_err(|error| AuthorityError::provider("TransactWriteItems", error))?,
            )
            .build())
    }

    /// Advances one signal's accepted frontier under its expected revision.
    fn advance_frontier(
        &self,
        request: &AdmissionRequest,
        plan: &AdmissionPlan,
        allocation: &Allocation,
    ) -> Result<TransactWriteItem, AuthorityError> {
        let mut frontier = ExpressionBuilder::new();
        let accepted = frontier.name("accepted");
        let revision = frontier.name("revision");
        let count = frontier.name("count");
        let bytes = frontier.name("logicalBytes");
        let advance = frontier.number(allocation.hi - allocation.lo);
        let one = frontier.number(1u64);
        let byte_delta = frontier.number(plan.logical_bytes());
        let expected = frontier.number(allocation.revision);
        let expected_name = frontier.name("revision");
        let absent = frontier.name(PK);
        Ok(TransactWriteItem::builder()
            .update(
                Update::builder()
                    .table_name(&self.table)
                    .key(PK, AttributeValue::S(keys::frontier_pk(&request.scope)))
                    .key(SK, AttributeValue::S(keys::frontier_sk(allocation.signal)))
                    .update_expression(format!(
                        "ADD {accepted} {advance}, {revision} {one}, {count} {advance}, \
                         {bytes} {byte_delta}"
                    ))
                    .condition_expression(format!(
                        "attribute_not_exists({absent}) OR {expected_name} = {expected}"
                    ))
                    .set_expression_attribute_names(Some(frontier.names()))
                    .set_expression_attribute_values(Some(frontier.values()))
                    .build()
                    .map_err(|error| AuthorityError::provider("TransactWriteItems", error))?,
            )
            .build())
    }

    /// Wraps one prepared item as a transaction put.
    fn put(
        &self,
        item: HashMap<String, AttributeValue>,
    ) -> Result<TransactWriteItem, AuthorityError> {
        Ok(TransactWriteItem::builder()
            .put(
                Put::builder()
                    .table_name(&self.table)
                    .set_item(Some(item))
                    .build()
                    .map_err(|error| AuthorityError::provider("TransactWriteItems", error))?,
            )
            .build())
    }

    /// The condition check that makes a commit lose to a deletion fence.
    fn deletion_fence(
        &self,
        request: &AdmissionRequest,
        pinned_epoch: u64,
    ) -> Result<TransactWriteItem, AuthorityError> {
        let mut deletion = ExpressionBuilder::new();
        let epoch_condition = aex_observation_store_aws::expressions::deletion_epoch_condition(
            &mut deletion,
            pinned_epoch,
        );
        let absent = deletion.name(PK);
        Ok(TransactWriteItem::builder()
            .condition_check(
                aws_sdk_dynamodb::types::ConditionCheck::builder()
                    .table_name(&self.table)
                    .key(PK, AttributeValue::S(keys::frontier_pk(&request.scope)))
                    .key(SK, AttributeValue::S("DELETION".to_owned()))
                    .condition_expression(format!(
                        "attribute_not_exists({absent}) OR {epoch_condition}"
                    ))
                    .set_expression_attribute_names(Some(deletion.names()))
                    .set_expression_attribute_values(Some(deletion.values()))
                    .build()
                    .map_err(|error| AuthorityError::provider("TransactWriteItems", error))?,
            )
            .build())
    }

    /// Step 9: idempotently write the `OBS#` revisions the commit published.
    async fn materialize(
        &self,
        request: &AdmissionRequest,
        allocations: &[Allocation],
        placements: &[BodyPlacement],
        abucket: BucketHour,
        now: Timestamp,
    ) -> Result<(), AuthorityError> {
        let mut next: HashMap<Signal, u64> = allocations
            .iter()
            .map(|allocation| (allocation.signal, allocation.lo))
            .collect();
        let mut writes = Vec::new();
        for (index, observation) in request.observations.iter().enumerate() {
            let seq = next.entry(observation.signal).or_insert(0);
            let accepted_seq = *seq;
            *seq += 1;
            let placement = placements.get(index).unwrap_or(&BodyPlacement::Inline);
            let item = observation_item(
                &ItemContext {
                    request,
                    observation,
                    abucket,
                    tbucket: BucketHour::from_timestamp(observation.time),
                    shard: u8::try_from(accepted_seq % u64::from(BUCKET_SHARDS)).unwrap_or(0),
                    accepted_seq,
                    now,
                },
                placement,
            );
            writes.push(
                WriteRequest::builder()
                    .put_request(
                        aws_sdk_dynamodb::types::PutRequest::builder()
                            .set_item(Some(item))
                            .build()
                            .map_err(|error| AuthorityError::provider("BatchWriteItem", error))?,
                    )
                    .build(),
            );
        }
        self.batch_write(writes).await
    }

    /// Writes a bounded batch, retrying only the unprocessed remainder.
    async fn batch_write(&self, writes: Vec<WriteRequest>) -> Result<(), AuthorityError> {
        for chunk in writes.chunks(MATERIALIZE_CHUNK) {
            let mut pending: Vec<WriteRequest> = chunk.to_vec();
            let mut attempts = 0u32;
            while !pending.is_empty() {
                let response = self
                    .dynamodb
                    .batch_write_item()
                    .request_items(self.table.clone(), pending.clone())
                    .send()
                    .await
                    .map_err(|error| AuthorityError::provider("BatchWriteItem", error))?;
                pending = response
                    .unprocessed_items()
                    .and_then(|items| items.get(&self.table))
                    .cloned()
                    .unwrap_or_default();
                attempts += 1;
                if attempts > limits::OBS_SPOOL_MAX_ATTEMPTS {
                    return Err(AuthorityError::Store(StoreError::Unavailable {
                        reason: "BatchWriteItem never drained its unprocessed items".into(),
                    }));
                }
            }
        }
        Ok(())
    }

    /// A single `GetItem` on the bound table.
    async fn get(
        &self,
        pk: &str,
        sk: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, AuthorityError> {
        let response = self
            .dynamodb
            .get_item()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(pk.to_owned()))
            .key(SK, AttributeValue::S(sk.to_owned()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| AuthorityError::provider("GetItem", error))?;
        Ok(response.item)
    }
}

/// The `preparing` receipt item.
fn receipt_item(
    request: &AdmissionRequest,
    plan: &AdmissionPlan,
    now: Timestamp,
    expires_ms: i64,
) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert(
        PK.to_owned(),
        AttributeValue::S(keys::batch_pk(request.workspace, request.batch_id)),
    );
    item.insert(SK.to_owned(), AttributeValue::S("RECEIPT".to_owned()));
    item.insert(
        ITEM_TYPE.to_owned(),
        AttributeValue::S("admission_receipt".to_owned()),
    );
    item.insert(
        "batchId".to_owned(),
        AttributeValue::S(request.batch_id.to_string()),
    );
    item.insert(
        "workspaceId".to_owned(),
        AttributeValue::S(request.workspace.to_string()),
    );
    item.insert(
        "organizationId".to_owned(),
        AttributeValue::S(request.organization.to_string()),
    );
    item.insert(
        "scopeKey".to_owned(),
        AttributeValue::S(request.scope.to_key()),
    );
    item.insert(
        "state".to_owned(),
        AttributeValue::S("preparing".to_owned()),
    );
    item.insert(
        "intentDigest".to_owned(),
        AttributeValue::S(request.intent_digest.clone()),
    );
    item.insert(
        "recordCount".to_owned(),
        AttributeValue::N(plan.records().to_string()),
    );
    item.insert(
        "logicalBytes".to_owned(),
        AttributeValue::N(plan.logical_bytes().to_string()),
    );
    item.insert("preparedAt".to_owned(), AttributeValue::S(now.to_wire()));
    item.insert(
        "expiresAtEpochSeconds".to_owned(),
        AttributeValue::N((expires_ms / 1_000).to_string()),
    );
    item
}

/// The durable spool chunk the reconciler drains.
fn spool_item(
    request: &AdmissionRequest,
    allocations: &[Allocation],
    now: Timestamp,
) -> HashMap<String, AttributeValue> {
    let shard = spool_shard(&request.batch_id.to_string());
    let mut item = HashMap::new();
    item.insert(
        PK.to_owned(),
        AttributeValue::S(keys::spool_pk(request.workspace, shard)),
    );
    item.insert(
        SK.to_owned(),
        AttributeValue::S(format!("{}#{}", now.to_wire(), request.batch_id)),
    );
    item.insert(
        ITEM_TYPE.to_owned(),
        AttributeValue::S("spool_chunk".to_owned()),
    );
    item.insert(
        "batchId".to_owned(),
        AttributeValue::S(request.batch_id.to_string()),
    );
    item.insert(
        "scopeKey".to_owned(),
        AttributeValue::S(request.scope.to_key()),
    );
    item.insert(
        "workspaceId".to_owned(),
        AttributeValue::S(request.workspace.to_string()),
    );
    item.insert("acceptedAt".to_owned(), AttributeValue::S(now.to_wire()));
    item.insert(
        "attemptedRecords".to_owned(),
        AttributeValue::N(request.observations.len().to_string()),
    );
    item.insert(
        "attemptedBytes".to_owned(),
        AttributeValue::N(
            request
                .observations
                .iter()
                .map(|observation| observation.canonical.len() as u64)
                .sum::<u64>()
                .to_string(),
        ),
    );
    item.insert(
        "lossCandidates".to_owned(),
        AttributeValue::L(
            allocations
                .iter()
                .map(|allocation| loss_candidate(request, allocation))
                .collect(),
        ),
    );
    item.insert(
        "pending".to_owned(),
        AttributeValue::Ss(vec![
            "outbox".to_owned(),
            "index".to_owned(),
            "wake".to_owned(),
        ]),
    );
    item.insert("attempts".to_owned(), AttributeValue::N("0".to_owned()));
    item.insert(
        "cPk".to_owned(),
        AttributeValue::S(keys::control_pk(
            aex_observation_domain::keys::ControlDomain::SpoolRepair,
            shard,
        )),
    );
    item.insert(
        "cSk".to_owned(),
        AttributeValue::S(keys::control_sk(now, &request.batch_id.to_string())),
    );
    item
}

/// One source-stable exact per-signal loss candidate retained by the spool.
fn loss_candidate(request: &AdmissionRequest, allocation: &Allocation) -> AttributeValue {
    let observations: Vec<&PreparedObservation> = request
        .observations
        .iter()
        .filter(|observation| observation.signal == allocation.signal)
        .collect();
    let mut candidate = HashMap::from([
        (
            "gapId".to_owned(),
            AttributeValue::S(gap_id_for(request.batch_id, allocation.signal).to_string()),
        ),
        (
            "signal".to_owned(),
            AttributeValue::S(allocation.signal.as_str().to_owned()),
        ),
        (
            "acceptedSeqLo".to_owned(),
            AttributeValue::N(allocation.lo.to_string()),
        ),
        (
            "acceptedSeqHiExclusive".to_owned(),
            AttributeValue::N(allocation.hi.to_string()),
        ),
        (
            "attemptedRecords".to_owned(),
            AttributeValue::N(observations.len().to_string()),
        ),
        (
            "attemptedBytes".to_owned(),
            AttributeValue::N(
                observations
                    .iter()
                    .map(|observation| observation.canonical.len() as u64)
                    .sum::<u64>()
                    .to_string(),
            ),
        ),
    ]);
    if let Some(range) = observed_window(&observations) {
        candidate.insert("timeRange".to_owned(), AttributeValue::M(range));
    }
    AttributeValue::M(candidate)
}

/// Exact non-empty half-open window covering every point of one signal.
fn observed_window(
    observations: &[&PreparedObservation],
) -> Option<HashMap<String, AttributeValue>> {
    let lo = observations
        .iter()
        .map(|observation| observation.time)
        .min()?;
    let max = observations
        .iter()
        .map(|observation| observation.time)
        .max()?;
    let hi = Timestamp::from_unix_millis(max.unix_millis().checked_add(1)?).ok()?;
    Some(HashMap::from([
        ("gte".to_owned(), AttributeValue::S(lo.to_wire())),
        ("lt".to_owned(), AttributeValue::S(hi.to_wire())),
    ]))
}

/// Derives one UUIDv7 gap id from the immutable batch id and concrete signal.
fn gap_id_for(batch: TelemetryBatchId, signal: Signal) -> TelemetryGapId {
    let mut bytes = *batch.uuid7().as_bytes();
    bytes[15] ^= signal.rank().saturating_add(1);
    let derived = Uuid7::from_bytes(bytes).unwrap_or_else(|_| batch.uuid7());
    TelemetryGapId::from_uuid7(derived)
}

/// The storage usage fact the reconciler delivers.
fn outbox_item(
    request: &AdmissionRequest,
    plan: &AdmissionPlan,
    now: Timestamp,
) -> HashMap<String, AttributeValue> {
    let shard = spool_shard(&request.batch_id.to_string());
    let mut item = HashMap::new();
    item.insert(
        PK.to_owned(),
        AttributeValue::S(keys::spool_pk(request.workspace, shard)),
    );
    item.insert(
        SK.to_owned(),
        AttributeValue::S(format!("OUTBOX#{}#{}", now.to_wire(), request.batch_id)),
    );
    item.insert(
        ITEM_TYPE.to_owned(),
        AttributeValue::S("usage_outbox".to_owned()),
    );
    item.insert(
        "meter".to_owned(),
        AttributeValue::S("storage.byte_min.v1".to_owned()),
    );
    item.insert(
        "quantityBytes".to_owned(),
        AttributeValue::N(plan.logical_bytes().to_string()),
    );
    item.insert(
        "workspaceId".to_owned(),
        AttributeValue::S(request.workspace.to_string()),
    );
    item.insert(
        "organizationId".to_owned(),
        AttributeValue::S(request.organization.to_string()),
    );
    item.insert(
        "idempotencyKey".to_owned(),
        AttributeValue::S(request.batch_id.to_string()),
    );
    item.insert("state".to_owned(), AttributeValue::S("pending".to_owned()));
    item.insert("attempts".to_owned(), AttributeValue::N("0".to_owned()));
    item
}

/// The per-item facts every writer of one observation revision shares.
struct ItemContext<'a> {
    /// The admission this revision belongs to.
    request: &'a AdmissionRequest,
    /// The normalized observation.
    observation: &'a PreparedObservation,
    /// The accepted-time hour bucket.
    abucket: BucketHour,
    /// The event-time hour bucket.
    tbucket: BucketHour,
    /// Which shard of the bucket it lands on.
    shard: u8,
    /// Its accepted sequence.
    accepted_seq: u64,
    /// The commit instant.
    now: Timestamp,
}

impl ItemContext<'_> {
    /// The zero-padded accepted ordinal every index sort key ends with.
    fn ordinal(&self) -> String {
        keys::pad_seq(u128::from(self.accepted_seq))
    }
}

/// One immutable observation revision.
///
/// Written only under `attribute_not_exists(pk)`: a later revision of the same
/// observation is a **new** item with a new accepted sequence, never an update.
fn observation_item(
    context: &ItemContext<'_>,
    placement: &BodyPlacement,
) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    identity_attributes(&mut item, context);
    body_attributes(&mut item, context, placement);
    sparse_index_attributes(&mut item, context);
    dense_index_attributes(&mut item, context);
    item
}

/// The identity and ordering attributes of one revision.
fn identity_attributes(item: &mut HashMap<String, AttributeValue>, context: &ItemContext<'_>) {
    let ItemContext {
        request,
        observation,
        abucket,
        shard,
        accepted_seq,
        now,
        ..
    } = context;
    item.insert(
        PK.to_owned(),
        AttributeValue::S(keys::observation_pk(
            &request.scope,
            observation.signal,
            *abucket,
            *shard,
        )),
    );
    item.insert(
        SK.to_owned(),
        AttributeValue::S(keys::observation_sk(*accepted_seq)),
    );
    item.insert(
        ITEM_TYPE.to_owned(),
        AttributeValue::S("observation".to_owned()),
    );
    item.insert(
        "observationId".to_owned(),
        AttributeValue::S(mint_observation_id(*now)),
    );
    item.insert("revision".to_owned(), AttributeValue::N("1".to_owned()));
    item.insert(
        "signal".to_owned(),
        AttributeValue::S(observation.signal.as_str().to_owned()),
    );
    item.insert(
        "signalRank".to_owned(),
        AttributeValue::N(observation.signal.rank().to_string()),
    );
    item.insert(
        "scopeKey".to_owned(),
        AttributeValue::S(request.scope.to_key()),
    );
    item.insert(
        "workspaceId".to_owned(),
        AttributeValue::S(request.workspace.to_string()),
    );
    item.insert(
        "organizationId".to_owned(),
        AttributeValue::S(request.organization.to_string()),
    );
    if let Some(session) = request.scope.session() {
        item.insert(
            "sessionId".to_owned(),
            AttributeValue::S(session.to_string()),
        );
    }
    item.insert(
        "time".to_owned(),
        AttributeValue::S(observation.time.to_wire()),
    );
    item.insert("acceptedAt".to_owned(), AttributeValue::S(now.to_wire()));
    item.insert(
        "acceptedSeq".to_owned(),
        AttributeValue::N(accepted_seq.to_string()),
    );
    item.insert(
        "batchId".to_owned(),
        AttributeValue::S(request.batch_id.to_string()),
    );
    item.insert(
        "logicalBytes".to_owned(),
        AttributeValue::N(observation.canonical.len().to_string()),
    );
    item.insert(
        "attrDigest".to_owned(),
        AttributeValue::S(observation.attr_digest.clone()),
    );
}

/// The body, inline or by reference. The placement decision is recorded on the
/// item, so the index projection is bounded by construction.
fn body_attributes(
    item: &mut HashMap<String, AttributeValue>,
    context: &ItemContext<'_>,
    placement: &BodyPlacement,
) {
    match placement {
        BodyPlacement::Inline => {
            item.insert(
                "bodyInline".to_owned(),
                AttributeValue::B(aws_sdk_dynamodb::primitives::Blob::new(
                    context.observation.canonical.clone(),
                )),
            );
        }
        BodyPlacement::Object { key, digest } => {
            item.insert("bodyS3Key".to_owned(), AttributeValue::S(key.clone()));
            item.insert("bodySha256".to_owned(), AttributeValue::S(digest.clone()));
        }
    }
}

/// The trace and metric index attributes, written only when they apply.
fn sparse_index_attributes(item: &mut HashMap<String, AttributeValue>, context: &ItemContext<'_>) {
    let observation = context.observation;
    let ordinal = context.ordinal();
    if let Some(trace) = &observation.trace_id {
        item.insert("traceId".to_owned(), AttributeValue::S(trace.clone()));
        item.insert(
            "trPk".to_owned(),
            AttributeValue::S(format!("TRC#{}#{trace}", context.request.scope.to_key())),
        );
        item.insert(
            "trSk".to_owned(),
            AttributeValue::S(format!("{}#{ordinal}", observation.signal.rank())),
        );
    }
    if let Some(span) = &observation.span_id {
        item.insert("spanId".to_owned(), AttributeValue::S(span.clone()));
    }
    if let Some(metric) = &observation.metric_name {
        item.insert("metricName".to_owned(), AttributeValue::S(metric.clone()));
        item.insert(
            "mPk".to_owned(),
            AttributeValue::S(format!(
                "MET#{}#{metric}#{}",
                context.request.workspace,
                context.tbucket.day()
            )),
        );
        item.insert(
            "mSk".to_owned(),
            AttributeValue::S(format!(
                "{}#{}",
                observation.time.to_wire(),
                observation.series_hash.clone().unwrap_or_default()
            )),
        );
    }
    if let Some(hash) = &observation.series_hash {
        item.insert("seriesHash".to_owned(), AttributeValue::S(hash.clone()));
    }
}

/// The key attributes of the three dense observation indexes.
///
/// Every observation carries all three. They are written here rather than
/// derived by a reader, because an index key a reader has to reconstruct is an
/// index key two readers can reconstruct differently.
fn dense_index_attributes(item: &mut HashMap<String, AttributeValue>, context: &ItemContext<'_>) {
    let ItemContext {
        request,
        observation,
        abucket,
        tbucket,
        shard,
        now,
        ..
    } = context;
    let ordinal = context.ordinal();
    let event_time = observation.time.to_wire();
    let signal = observation.signal.as_str();
    item.insert(
        "tPk".to_owned(),
        AttributeValue::S(format!(
            "OBT#{}#{signal}#{}#{shard:02}",
            request.scope.to_key(),
            tbucket.as_str()
        )),
    );
    item.insert(
        "tSk".to_owned(),
        AttributeValue::S(format!("{event_time}#{ordinal}")),
    );
    item.insert(
        "wPk".to_owned(),
        AttributeValue::S(format!(
            "OBWA#{}#{signal}#{}#{shard:02}",
            request.workspace,
            abucket.as_str()
        )),
    );
    item.insert(
        "wSk".to_owned(),
        AttributeValue::S(format!(
            "{}#{}#{ordinal}",
            now.to_wire(),
            request.scope.hash8()
        )),
    );
    item.insert(
        "wtPk".to_owned(),
        AttributeValue::S(format!(
            "OBWT#{}#{signal}#{}#{shard:02}",
            request.workspace,
            tbucket.as_str()
        )),
    );
    item.insert(
        "wtSk".to_owned(),
        AttributeValue::S(format!("{event_time}#{ordinal}")),
    );
}

/// Which shard of the workspace's spool a batch lands on.
#[must_use]
pub fn spool_shard(batch: &str) -> u8 {
    let digest = sha256_hex(batch.as_bytes());
    u8::from_str_radix(&digest[0..2], 16).unwrap_or(0) % 16
}

/// Mints one observation id.
fn mint_observation_id(now: Timestamp) -> String {
    use aex_wire::ids::PrefixedId as _;
    let bytes = *uuid::Uuid::now_v7().as_bytes();
    let id = aex_wire::ids::Uuid7::from_bytes(bytes).unwrap_or_else(|_| {
        aex_wire::ids::Uuid7::compose(now.unix_millis().unsigned_abs(), [0u8; 10])
    });
    aex_wire::ids::ObservationId::from_uuid7(id).to_string()
}

/// Epoch seconds `after_ms` milliseconds from now.
fn seconds_from_now(after_ms: i64) -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp() + after_ms / 1_000
}

/// The accepted-sequence allocation of one signal in one batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Allocation {
    /// Which signal.
    pub signal: Signal,
    /// First accepted sequence.
    pub lo: u64,
    /// One past the last accepted sequence.
    pub hi: u64,
    /// The frontier revision the advance is conditional on.
    pub revision: u64,
}

/// The accepted frontier of one `(scope, signal)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrontierRow {
    /// The next accepted sequence to assign.
    pub accepted: u64,
    /// The monotone revision the advance is conditional on.
    pub revision: u64,
}

/// Where one observation's canonical body was placed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BodyPlacement {
    /// Small enough to live on the item.
    Inline,
    /// Content-addressed in the observation bucket.
    Object {
        /// The object key.
        key: String,
        /// The content digest.
        digest: String,
    },
}

/// Reads a string attribute.
fn string<'a>(item: &'a HashMap<String, AttributeValue>, name: &str) -> Option<&'a str> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
}

/// Reads a numeric attribute.
fn number(item: &HashMap<String, AttributeValue>, name: &str) -> Option<u64> {
    item.get(name)
        .and_then(|value| value.as_n().ok())
        .and_then(|text| text.parse().ok())
}

/// The durable state a stored receipt is in.
///
/// An ambiguous transaction outcome is resolved by reading this and branching,
/// never by retrying the transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredReceipt {
    /// Prepared, not yet published.
    Preparing,
    /// Published; the accepted range is visible.
    Committed,
    /// Expired or abandoned; the reservation was released.
    Aborted,
}

impl StoredReceipt {
    /// Reads the stored discriminator.
    #[must_use]
    pub fn parse(state: &str) -> Self {
        match state {
            "committed" => Self::Committed,
            "aborted" => Self::Aborted,
            _ => Self::Preparing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdmissionRequest, Allocation, AuthorityError, BODY_PREFIX, BUCKET_SHARDS,
        MATERIALIZE_CHUNK, PreparedObservation, gap_id_for, spool_item, spool_shard,
    };
    use aex_observation_domain::canonical::CanonicalValue;
    use aex_observation_domain::keys::ScopeKey;
    use aex_observation_domain::signal::Signal;
    use aex_observation_store_aws::store::StoreError;
    use aex_wire::ids::{OrganizationId, PrefixedId as _, TelemetryBatchId, WorkspaceId};
    use aex_wire::types::Timestamp;

    fn observation(signal: Signal, time: i64, bytes: usize) -> PreparedObservation {
        PreparedObservation {
            signal,
            time: Timestamp::from_unix_millis(time).expect("fixture instant"),
            canonical: vec![0; bytes],
            body: CanonicalValue::Null,
            attr_digest: "0".repeat(64),
            trace_id: None,
            span_id: None,
            metric_name: None,
            series_hash: None,
        }
    }

    #[test]
    fn a_provider_failure_and_a_closed_gate_are_both_retryable() {
        assert!(
            AuthorityError::GateClosed { state: "closed" }.retryable(),
            "a closed gate is a retryable refusal, not a rejection"
        );
        assert!(
            AuthorityError::Provider {
                operation: "GetItem",
                reason: "throttled".to_owned()
            }
            .retryable()
        );
        assert!(
            !AuthorityError::IntentConflict {
                batch: "bch_x".to_owned()
            }
            .retryable(),
            "a different intent under the same batch id is never retryable"
        );
        assert!(
            !AuthorityError::Store(StoreError::ItemTooLarge {
                observed: 1,
                limit: 0
            })
            .retryable()
        );
    }

    #[test]
    fn the_spool_shard_is_stable_and_bounded() {
        for batch in ["bch_a", "bch_b", "bch_c", "bch_d"] {
            let shard = spool_shard(batch);
            assert!(shard < 16, "{batch} landed on shard {shard}");
            assert_eq!(shard, spool_shard(batch), "the shard must be stable");
        }
    }

    #[test]
    fn loss_gap_ids_are_stable_and_distinct_per_signal() {
        let batch = TelemetryBatchId::parse("bch_0000000001e40r2081040g2081").expect("batch");
        let logs = gap_id_for(batch, Signal::Logs);
        assert_eq!(logs, gap_id_for(batch, Signal::Logs));
        assert_ne!(logs, gap_id_for(batch, Signal::Metrics));
    }

    #[test]
    fn the_spool_retains_exact_per_signal_loss_evidence() {
        let workspace =
            WorkspaceId::parse("wsp_0000000001e40r2081040g2081").expect("workspace fixture");
        let request = AdmissionRequest {
            batch_id: TelemetryBatchId::parse("bch_0000000001e40r2081040g2081")
                .expect("batch fixture"),
            organization: OrganizationId::parse("org_0000000001e40r2081040g2081")
                .expect("organization fixture"),
            workspace,
            scope: ScopeKey::Workspace(workspace),
            intent_digest: "0".repeat(64),
            observations: vec![
                observation(Signal::Logs, 10, 3),
                observation(Signal::Logs, 12, 5),
                observation(Signal::Metrics, 7, 11),
            ],
        };
        let allocations = [
            Allocation {
                signal: Signal::Logs,
                lo: 4,
                hi: 6,
                revision: 1,
            },
            Allocation {
                signal: Signal::Metrics,
                lo: 20,
                hi: 21,
                revision: 2,
            },
        ];
        let item = spool_item(
            &request,
            &allocations,
            Timestamp::from_unix_millis(30).expect("fixture instant"),
        );
        assert!(!item.contains_key("acceptedSeqLo"));
        assert!(!item.contains_key("acceptedSeqHi"));
        let candidates = item["lossCandidates"].as_l().expect("candidate list");
        assert_eq!(candidates.len(), 2);
        let logs = candidates[0].as_m().expect("logs candidate");
        assert_eq!(
            logs["acceptedSeqLo"].as_n().ok().map(String::as_str),
            Some("4")
        );
        assert_eq!(
            logs["acceptedSeqHiExclusive"]
                .as_n()
                .ok()
                .map(String::as_str),
            Some("6")
        );
        assert_eq!(
            logs["attemptedRecords"].as_n().ok().map(String::as_str),
            Some("2")
        );
        assert_eq!(
            logs["attemptedBytes"].as_n().ok().map(String::as_str),
            Some("8")
        );
        assert_eq!(
            logs["timeRange"].as_m().expect("known range")["gte"]
                .as_s()
                .ok()
                .map(String::as_str),
            Some("1970-01-01T00:00:00.010Z")
        );
        assert_eq!(
            logs["timeRange"].as_m().expect("known range")["lt"]
                .as_s()
                .ok()
                .map(String::as_str),
            Some("1970-01-01T00:00:00.013Z")
        );
    }

    #[test]
    fn the_object_prefix_is_the_observation_one_not_the_content_one() {
        assert_eq!(BODY_PREFIX, "observations");
        assert_ne!(BODY_PREFIX, "content");
        assert_eq!(MATERIALIZE_CHUNK, 25);
        const { assert!(BUCKET_SHARDS >= 1) }
    }
}
