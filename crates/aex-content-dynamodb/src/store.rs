//! The `regional-content` port implementation.
//!
//! Two reads carry the weight. `reachability` is the strongly consistent query
//! a sweeper runs immediately before it decides to delete: if **any** pin or any
//! unexpired grant survives, the candidate is dropped. And `redeem_grant`
//! validates `expiresAt` against the request clock rather than trusting the TTL
//! attribute beside it, because AWS deletes a TTL'd row within 48 hours rather
//! than at the instant, and a fence that trusted the timer would authorise a
//! read after the grant had expired.

use aex_session_dynamodb::attr::{Item, Row, n, s};
use aex_session_dynamodb::error::{Idempotence, Resolution, StoreError, classify};
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::{Participant, key};
use aex_wire::ids::{ContentHash, WorkspaceId};
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure;
use futures::StreamExt as _;

use crate::codec::{
    self, ContentDescriptor, DownloadGrant, GcEpoch, GrantExpiryCursor, GrantExpiryPosition,
    TreePage, decode_descriptor, decode_gc_epoch, decode_grant, decode_grant_expiry_cursor,
    decode_inline_body, decode_tree_page, encode_grant_expiry_cursor,
    validate_grant_expiry_position,
};
use crate::expressions;
use crate::keys;
use crate::wire_pending::{Blake3Digest, GcSweepPlan, InlineBody, body_hex};

/// One row of the slim garbage-collection scan projection.
///
/// The projection is deliberately narrow: a mark scan enumerates identities and
/// sizes, and there is nowhere in this type for a ciphertext to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcScanEntry {
    /// The scanned digest, in whichever family the row belongs to.
    pub digest: String,
    /// The placement, when the row is a descriptor.
    pub placement: Option<String>,
    /// The size, when the row declares one.
    pub size_bytes: Option<u64>,
    /// The object key, when the body lives in the object store.
    pub object_key: Option<String>,
    /// The `ETag` a fenced delete would condition on.
    pub object_etag: Option<String>,
    /// The epoch the row was last marked in.
    pub gc_epoch: Option<u64>,
    /// The hold a candidate is under.
    pub not_before: Option<Timestamp>,
}

/// One page of the garbage-collection scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcScanPage {
    /// The rows this page names.
    pub entries: Vec<GcScanEntry>,
    /// Whether the bucket holds more.
    pub more: bool,
}

/// The narrow evidence projected by the grant-expiry due index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantExpiry {
    /// The grant token digest; the bearer token itself is never stored.
    pub token_sha256: String,
    /// The workspace that owns the pin.
    pub workspace: WorkspaceId,
    /// The pinned content body.
    pub digest: ContentHash,
    /// The explicit authority fence, independent of TTL timing.
    pub expires_at: Timestamp,
}

impl From<&DownloadGrant> for GrantExpiry {
    fn from(grant: &DownloadGrant) -> Self {
        Self {
            token_sha256: grant.token_sha256.clone(),
            workspace: grant.workspace,
            digest: grant.digest,
            expires_at: grant.expires_at,
        }
    }
}

/// One bounded page of expired grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantExpiryPage {
    /// Due grants named by the slim index projection.
    pub grants: Vec<GrantExpiry>,
    /// The exact complete GSI key after which the next page starts. Absence
    /// means this pass reached the end and the durable cursor wraps.
    pub next: Option<GrantExpiryPosition>,
}

/// What a strongly consistent reachability read found in one content partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reachability {
    /// How many pins survive.
    pub pins: usize,
    /// How many grants survive that have not provably expired.
    pub unexpired_grants: usize,
}

impl Reachability {
    /// Whether the body may be considered for deletion at all.
    #[must_use]
    pub const fn is_collectable(&self) -> bool {
        self.pins == 0 && self.unexpired_grants == 0
    }
}

/// A redeemed grant, with the body it authorises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedeemedGrant {
    /// What the grant authorises.
    pub grant: DownloadGrant,
    /// The sealed body, when the placement is inline.
    pub inline: Option<InlineBody>,
    /// The descriptor, so the caller can presign an object read.
    pub descriptor: ContentDescriptor,
}

// TODO(cross-stream): `aex-content-domain` has no `ports` module and publishes no
// traits at all — it is a pure decision crate. This port has no peer to be replaced
// by; whoever owns it must first decide where the content ports live.
/// The `regional-content` authority.
#[async_trait]
pub trait ContentMetadataStore: Send + Sync + 'static {
    /// Reads one body descriptor.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn load_descriptor(
        &self,
        workspace: WorkspaceId,
        digest: &ContentHash,
    ) -> Result<Option<ContentDescriptor>, StoreError>;

    /// Writes one committed body descriptor.
    ///
    /// A descriptor is immutable and content-addressed, so an already-present row
    /// is an **idempotent success**: it can only hold the same bytes. This is what
    /// makes E's D-18 ordering safe — `upload_complete` writes the descriptor
    /// first and the upload row second, and a client retry after a partial
    /// failure re-resolves rather than conflicting.
    ///
    /// # Errors
    ///
    /// As [`ContentMetadataStore::load_descriptor`].
    async fn put_descriptor(&self, descriptor: &ContentDescriptor) -> Result<(), StoreError>;

    /// Reads one inline ciphertext body.
    ///
    /// # Errors
    ///
    /// As [`ContentMetadataStore::load_descriptor`].
    async fn read_inline_body(
        &self,
        workspace: WorkspaceId,
        digest: &ContentHash,
    ) -> Result<Option<InlineBody>, StoreError>;

    /// Reads one Merkle tree page.
    ///
    /// # Errors
    ///
    /// As [`ContentMetadataStore::load_descriptor`].
    async fn read_tree_page(
        &self,
        workspace: WorkspaceId,
        page: Blake3Digest,
    ) -> Result<Option<TreePage>, StoreError>;

    /// Writes tree pages, each immutably.
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] for an over-large page, otherwise as above. An
    /// already-present page is an idempotent success, because a page is
    /// content-addressed and therefore identical by construction.
    async fn put_tree_pages(&self, pages: &[TreePage]) -> Result<(), StoreError>;

    /// Admits one body: stage the descriptor, commit it, then pin it.
    ///
    /// The three steps are separate writes rather than one transaction, and each
    /// is individually idempotent, because a body is content-addressed: a second
    /// admission of the same bytes under a second name must succeed, not
    /// collide. The order is what makes a crash safe — a pin can outlive the
    /// caller that took it and is reclaimed by garbage collection, while a name
    /// referencing an unpinned body could not be.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming `content.commit` when a
    /// descriptor exists under this digest that the commit condition rejects;
    /// otherwise any transport failure.
    async fn admit_body(
        &self,
        descriptor: &crate::codec::ContentDescriptor,
        pin: &crate::codec::ContentPin,
        now: Timestamp,
    ) -> Result<(), StoreError>;

    /// Reads the workspace's garbage-collection epoch.
    ///
    /// # Errors
    ///
    /// As [`ContentMetadataStore::load_descriptor`].
    async fn load_gc_epoch(&self, workspace: WorkspaceId) -> Result<Option<GcEpoch>, StoreError>;

    /// Scans one garbage-collection bucket.
    ///
    /// # Errors
    ///
    /// As [`ContentMetadataStore::load_descriptor`].
    async fn scan_gc_bucket(
        &self,
        workspace: WorkspaceId,
        bucket: u16,
        budget: PageBudget,
    ) -> Result<GcScanPage, StoreError>;

    /// Reads every pin and grant that survives in one content partition.
    ///
    /// # Errors
    ///
    /// As [`ContentMetadataStore::load_descriptor`].
    async fn reachability(
        &self,
        workspace: WorkspaceId,
        digest: &ContentHash,
        now: Timestamp,
    ) -> Result<Reachability, StoreError>;

    /// Commits one fenced sweep.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming the participant that lost,
    /// which always means the body survives.
    async fn sweep_candidate(&self, plan: &GcSweepPlan) -> Result<(), StoreError>;

    /// Mints a download grant and its pin in one transaction.
    ///
    /// # Errors
    ///
    /// [`StoreError`] as above.
    async fn mint_grant(&self, grant: &DownloadGrant, now: Timestamp) -> Result<(), StoreError>;

    /// Queries one sharded due-index page. This is never a table scan.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for transport or projected-row decode failure.
    async fn scan_expired_grants(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
        after: Option<&GrantExpiryPosition>,
    ) -> Result<GrantExpiryPage, StoreError>;

    /// Strongly reads one shard's durable expiry position.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for transport or corrupt cursor state.
    async fn load_grant_expiry_cursor(
        &self,
        shard: u16,
    ) -> Result<Option<GrantExpiryCursor>, StoreError>;

    /// Optimistically advances one shard to `position`, resolving a
    /// conditional or ambiguous result by one exact strong reread.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when another cursor winner differs.
    async fn advance_grant_expiry_cursor(
        &self,
        current: Option<&GrantExpiryCursor>,
        shard: u16,
        position: Option<&GrantExpiryPosition>,
        now: Timestamp,
    ) -> Result<(), StoreError>;

    /// Atomically removes an expired grant and the exact grant pin it owns.
    ///
    /// The transaction is idempotent when either row has already gone. A row
    /// that is not explicitly past `expiresAt` always survives.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for transport, contention, or a lost expiry condition.
    async fn expire_grant(&self, grant: &GrantExpiry, now: Timestamp) -> Result<(), StoreError>;

    /// Redeems a grant against the request clock.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming `content.grant` when the grant
    /// is absent or expired — the two are reported identically so a prober
    /// cannot tell a forged token from a stale one.
    async fn redeem_grant(
        &self,
        token_sha256_hex: &str,
        now: Timestamp,
    ) -> Result<RedeemedGrant, StoreError>;
}

/// Which consistency one content point read requires.
///
/// Content-addressed rows — descriptors, inline bodies, tree pages — are
/// immutable: a present row already holds the only bytes its digest can name,
/// so the sole question replica lag can change is *presence* moments after a
/// write, and those reads take the eventual path for half the read cost. The
/// garbage-collection epoch and the durable expiry cursor are fences and stay
/// strong, as does the grant read behind `redeem_grant`, whose expiry check is
/// an authority decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Consistency {
    /// A fence: the read must observe every acknowledged write.
    Strong,
    /// An immutable content-addressed row: replica lag is accepted.
    Eventual,
}

/// How many conditional tree-page puts one commit holds open at once.
///
/// The bound converts the transport's own concurrency appetite (the shared
/// HTTP pool) rather than a per-table quota — an on-demand table has none.
/// Eight keeps a large tree write inside one connection pool without queueing
/// behind itself.
const MAX_CONCURRENT_TREE_PAGE_PUTS: usize = 8;

/// The adapter.
#[derive(Debug, Clone)]
pub struct ContentStore {
    client: Client,
    table: String,
}

impl ContentStore {
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

    /// Issues a conditional put whose lost condition is an idempotent success.
    ///
    /// Only for content-addressed rows: an existing row under a digest-derived
    /// key holds the bytes that digest names, so the write it would have made is
    /// the write already there.
    async fn admitting_put(
        &self,
        builder: aws_sdk_dynamodb::types::builders::PutBuilder,
        participant: Participant,
    ) -> Result<(), StoreError> {
        let built = builder.build().map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
        let outcome = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(built.item().clone()))
            .set_condition_expression(built.condition_expression().map(str::to_owned))
            .set_expression_attribute_names(built.expression_attribute_names().cloned())
            .set_expression_attribute_values(built.expression_attribute_values().cloned())
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => {
                if matches!(
                    error.as_service_error(),
                    Some(
                        aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(
                            _
                        )
                    )
                ) {
                    return Ok(());
                }
                let _ = participant;
                Err(classify(&error, Idempotence::Write(Resolution::TargetItem)))
            }
        }
    }

    /// Issues a conditional update, reporting a lost condition as a typed
    /// precondition failure carrying the row the condition saw.
    async fn conditional_update(
        &self,
        builder: aws_sdk_dynamodb::types::builders::UpdateBuilder,
        participant: Participant,
    ) -> Result<(), StoreError> {
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
            .return_values_on_condition_check_failure(
                aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure::AllOld,
            )
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
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

    async fn get(
        &self,
        pk: &str,
        sk: &str,
        consistency: Consistency,
    ) -> Result<Option<Item>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(pk, sk)))
            .consistent_read(matches!(consistency, Consistency::Strong))
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        Ok(output.item)
    }

    /// Writes one immutable tree page, treating a lost condition as replay.
    async fn put_tree_page(&self, page: &TreePage) -> Result<(), StoreError> {
        let builder = expressions::put_tree_page(&self.table, page)?;
        let built = builder.build().map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
        let outcome = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(built.item().clone()))
            .set_condition_expression(built.condition_expression().map(str::to_owned))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => {
                // A page is content addressed, so an existing row holds the
                // identical bytes and the write is an idempotent success.
                if matches!(
                    error.as_service_error(),
                    Some(
                        aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(
                            _
                        )
                    )
                ) {
                    return Ok(());
                }
                Err(classify(&error, Idempotence::Write(Resolution::TargetItem)))
            }
        }
    }
}

#[async_trait]
impl ContentMetadataStore for ContentStore {
    async fn load_descriptor(
        &self,
        workspace: WorkspaceId,
        digest: &ContentHash,
    ) -> Result<Option<ContentDescriptor>, StoreError> {
        let target = keys::descriptor(workspace, digest);
        match self
            .get(&target.pk, &target.sk, Consistency::Eventual)
            .await?
        {
            None => Ok(None),
            Some(item) => Ok(Some(decode_descriptor(&item, workspace)?)),
        }
    }

    async fn put_descriptor(&self, descriptor: &ContentDescriptor) -> Result<(), StoreError> {
        let builder = crate::expressions::stage_descriptor(&self.table, descriptor)?;
        let built = builder.build().map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
        let outcome = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(built.item().clone()))
            .set_condition_expression(built.condition_expression().map(str::to_owned))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(
                    aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_),
                ) = error.as_service_error()
                {
                    // The row already exists. A descriptor is content-addressed
                    // and immutable, so it names the same bytes by construction.
                    return Ok(());
                }
                Err(classify(
                    &error,
                    Idempotence::Write(Resolution::TargetItem),
                ))
            }
        }
    }

    async fn read_inline_body(
        &self,
        workspace: WorkspaceId,
        digest: &ContentHash,
    ) -> Result<Option<InlineBody>, StoreError> {
        let target = keys::inline_body(workspace, digest);
        match self
            .get(&target.pk, &target.sk, Consistency::Eventual)
            .await?
        {
            None => Ok(None),
            Some(item) => Ok(Some(decode_inline_body(&item, workspace)?)),
        }
    }

    async fn read_tree_page(
        &self,
        workspace: WorkspaceId,
        page: Blake3Digest,
    ) -> Result<Option<TreePage>, StoreError> {
        let target = keys::tree_page(workspace, page);
        match self
            .get(&target.pk, &target.sk, Consistency::Eventual)
            .await?
        {
            None => Ok(None),
            Some(item) => Ok(Some(decode_tree_page(&item, workspace)?)),
        }
    }

    async fn put_tree_pages(&self, pages: &[TreePage]) -> Result<(), StoreError> {
        // Every put is built before the wave starts (an unpolled future has
        // issued nothing), then driven at a fixed width instead of serially -
        // a large tree commit was paying one round trip per page. `buffered`
        // settles in input order, so the error a caller sees is the earliest
        // failing page, and dropping the stream on that error cancels the rest;
        // cancelling a content-addressed conditional put is safe because a
        // replay writes the identical bytes or loses its condition, which is
        // treated as success either way.
        let puts: Vec<_> = pages.iter().map(|page| self.put_tree_page(page)).collect();
        let mut open = futures::stream::iter(puts).buffered(MAX_CONCURRENT_TREE_PAGE_PUTS);
        while let Some(outcome) = open.next().await {
            outcome?;
        }
        Ok(())
    }

    async fn admit_body(
        &self,
        descriptor: &crate::codec::ContentDescriptor,
        pin: &crate::codec::ContentPin,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        // An existing descriptor under this digest already describes these exact
        // bytes, so a lost `IMMUTABLE` is the second admission of one body and is
        // a success. `commit_staged` still runs, because the descriptor that
        // exists may be `staged` from an interrupted admission.
        self.admitting_put(
            expressions::stage_descriptor(&self.table, descriptor)?,
            Participant::CONTENT_DESCRIPTOR,
        )
        .await?;
        self.conditional_update(
            expressions::commit_staged(&self.table, descriptor.workspace, &descriptor.digest, now)?,
            Participant::CONTENT_COMMIT,
        )
        .await?;
        // A pin is keyed by `(workspace, digest, owner)`, so an existing pin is
        // this same pin and taking it again is a success.
        self.admitting_put(
            expressions::pin_body(&self.table, &descriptor.digest, pin)?,
            Participant::CONTENT_DESCRIPTOR,
        )
        .await
    }

    async fn load_gc_epoch(&self, workspace: WorkspaceId) -> Result<Option<GcEpoch>, StoreError> {
        // The sweeper's fence: an epoch served from a lagging replica could
        // let a sweep proceed under an epoch that has already advanced.
        let target = keys::gc_epoch(workspace);
        match self
            .get(&target.pk, &target.sk, Consistency::Strong)
            .await?
        {
            None => Ok(None),
            Some(item) => Ok(Some(decode_gc_epoch(&item, workspace)?)),
        }
    }

    async fn scan_gc_bucket(
        &self,
        workspace: WorkspaceId,
        bucket: u16,
        budget: PageBudget,
    ) -> Result<GcScanPage, StoreError> {
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .index_name(keys::GC_INDEX)
            .key_condition_expression("#pk = :pk")
            .expression_attribute_names("#pk", keys::GC_PK)
            .expression_attribute_values(":pk", s(keys::gc_scan_partition(workspace, bucket)))
            .limit(budget.limit())
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;

        let mut entries = Vec::new();
        for item in output.items.unwrap_or_default() {
            // The projection carries `digestSha256` for a body or a candidate
            // and `pageDigest` for a tree page; exactly one of them is present.
            let digest = item
                .get("digestSha256")
                .or_else(|| item.get("pageDigest"))
                .and_then(|value| value.as_s().ok())
                .cloned()
                .ok_or(StoreError::Invalid {
                    detail: "a garbage-collection scan row names no digest".to_owned(),
                })?;
            let read = |name: &str| item.get(name).and_then(|value| value.as_s().ok()).cloned();
            let number = |name: &str| {
                item.get(name)
                    .and_then(|value| value.as_n().ok())
                    .and_then(|text| text.parse::<u64>().ok())
            };
            entries.push(GcScanEntry {
                digest,
                placement: read("placement"),
                size_bytes: number("sizeBytes"),
                object_key: read("objectKey"),
                object_etag: read("objectEtag"),
                gc_epoch: number("gcEpoch"),
                not_before: read("notBefore").and_then(|text| Timestamp::parse(&text).ok()),
            });
        }
        Ok(GcScanPage {
            more: output.last_evaluated_key.is_some(),
            entries,
        })
    }

    async fn reachability(
        &self,
        workspace: WorkspaceId,
        digest: &ContentHash,
        now: Timestamp,
    ) -> Result<Reachability, StoreError> {
        let partition = keys::content_partition(workspace, digest);
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk AND #sk BETWEEN :low AND :high")
            .expression_attribute_names("#pk", aex_session_dynamodb::attr::PK)
            .expression_attribute_names("#sk", aex_session_dynamodb::attr::SK)
            .expression_attribute_values(":pk", s(partition))
            // `GRANT#` sorts before `PIN#`, so one range covers both families
            // and the sweeper cannot miss one by reading only the other.
            .expression_attribute_values(":low", s(keys::grant_pin_prefix()))
            .expression_attribute_values(":high", s(format!("{}\u{fffd}", keys::pin_prefix())))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;

        let mut pins = 0;
        let mut unexpired_grants = 0;
        for item in output.items.unwrap_or_default() {
            let row = Row::bind(&item, codec::CONTENT_PIN)?;
            let kind = row.enumerated("pinKind", codec::STORED_PIN_KINDS)?;
            if kind == codec::GRANT_PIN_KIND {
                // Expiry is read from the row, never inferred from the TTL
                // attribute having fired.
                let expires_at = row.timestamp("expiresAt")?;
                if expires_at > now {
                    unexpired_grants += 1;
                }
            } else {
                pins += 1;
            }
        }
        Ok(Reachability {
            pins,
            unexpired_grants,
        })
    }

    async fn sweep_candidate(&self, plan: &GcSweepPlan) -> Result<(), StoreError> {
        let transaction = expressions::sweep(&self.table, plan)?;
        let request = transaction.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => Err(match error.as_service_error() {
                Some(service) => aex_session_dynamodb::error::decode_cancellation(
                    service,
                    transaction.participants(),
                ),
                None => classify(&error, Idempotence::Write(Resolution::TargetItem)),
            }),
        }
    }

    async fn mint_grant(&self, grant: &DownloadGrant, now: Timestamp) -> Result<(), StoreError> {
        let transaction = expressions::mint_grant(&self.table, grant, now)?;
        let request = transaction.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => Err(match error.as_service_error() {
                Some(service) => aex_session_dynamodb::error::decode_cancellation(
                    service,
                    transaction.participants(),
                ),
                None => classify(&error, Idempotence::Write(Resolution::TargetItem)),
            }),
        }
    }

    async fn scan_expired_grants(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
        after: Option<&GrantExpiryPosition>,
    ) -> Result<GrantExpiryPage, StoreError> {
        if u64::from(shard) >= keys::EXPIRY_SHARDS {
            return Err(StoreError::Invalid {
                detail: format!("expiry shard {shard} is outside 0..{}", keys::EXPIRY_SHARDS),
            });
        }
        if let Some(position) = after {
            validate_grant_expiry_position(position, shard)?;
        }
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .index_name(keys::EXPIRY_INDEX)
            .key_condition_expression("#pk = :pk AND #sk <= :now")
            .expression_attribute_names("#pk", keys::EXPIRY_PK)
            .expression_attribute_names("#sk", keys::EXPIRY_SK)
            .expression_attribute_values(":pk", s(keys::expiry_partition(shard)))
            // A high suffix includes every token due in this millisecond; the
            // bare `timestamp#` prefix would sort before all of them.
            .expression_attribute_values(":now", s(format!("{}#\u{fffd}", now.to_wire())))
            .limit(budget.limit())
            .set_exclusive_start_key(after.map(expiry_exclusive_start))
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;

        let mut grants = Vec::new();
        for item in output.items.unwrap_or_default() {
            let row = Row::bind_projected(&item, codec::DOWNLOAD_GRANT);
            let pk = row.string(aex_session_dynamodb::attr::PK)?;
            let token_sha256 = pk
                .strip_prefix("GRANT#")
                .ok_or_else(|| StoreError::Invalid {
                    detail: "an expiry index row is not a grant key".to_owned(),
                })?
                .to_owned();
            let expected_key = keys::grant(&token_sha256)?;
            let expected_sort = keys::expiry_sort(row.timestamp("expiresAt")?, &token_sha256)?;
            if pk != expected_key.pk
                || row.string(aex_session_dynamodb::attr::SK)? != expected_key.sk
                || row.string(keys::EXPIRY_PK)? != keys::expiry_partition(shard)
                || row.string(keys::EXPIRY_SK)? != expected_sort
                || keys::expiry_shard(&token_sha256) != shard
            {
                return Err(StoreError::Invalid {
                    detail: format!(
                        "grant `{token_sha256}` carries forged base or expiry-index keys"
                    ),
                });
            }
            let digest_text = row.string("contentDigest")?;
            let digest = ContentHash::parse(digest_text).map_err(|error| StoreError::Invalid {
                detail: format!("an expiry index row has an invalid content digest: {error}"),
            })?;
            grants.push(GrantExpiry {
                token_sha256,
                workspace: row.id::<WorkspaceId>("workspaceId")?,
                digest,
                expires_at: row.timestamp("expiresAt")?,
            });
        }
        Ok(GrantExpiryPage {
            grants,
            next: output
                .last_evaluated_key
                .map(|key| expiry_position(key, shard))
                .transpose()?,
        })
    }

    async fn load_grant_expiry_cursor(
        &self,
        shard: u16,
    ) -> Result<Option<GrantExpiryCursor>, StoreError> {
        if u64::from(shard) >= keys::EXPIRY_SHARDS {
            return Err(StoreError::Invalid {
                detail: format!("expiry shard {shard} is outside 0..{}", keys::EXPIRY_SHARDS),
            });
        }
        let target = keys::expiry_cursor(shard);
        let item = self
            .get(&target.pk, &target.sk, Consistency::Strong)
            .await?;
        let Some(item) = item else {
            return Ok(None);
        };
        let cursor = decode_grant_expiry_cursor(&item)?;
        if cursor.shard != shard {
            return Err(StoreError::Invalid {
                detail: format!(
                    "grant-expiry cursor for shard {shard} decoded as shard {}",
                    cursor.shard
                ),
            });
        }
        Ok(Some(cursor))
    }

    async fn advance_grant_expiry_cursor(
        &self,
        current: Option<&GrantExpiryCursor>,
        shard: u16,
        position: Option<&GrantExpiryPosition>,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        if current.is_some_and(|cursor| cursor.shard != shard) {
            return Err(StoreError::Invalid {
                detail: "a grant-expiry cursor belongs to another shard".to_owned(),
            });
        }
        if let Some(position) = position {
            validate_grant_expiry_position(position, shard)?;
        }
        let revision = current
            .map_or(Some(1), |cursor| cursor.revision.checked_add(1))
            .ok_or(StoreError::Invalid {
                detail: "a grant-expiry cursor revision is exhausted".to_owned(),
            })?;
        let target = GrantExpiryCursor {
            shard,
            position: position.cloned(),
            revision,
            updated_at: now,
        };
        let mut request = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(encode_grant_expiry_cursor(&target)))
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld);
        request = match current {
            None => request.condition_expression("attribute_not_exists(pk)"),
            Some(cursor) => request
                .condition_expression(
                    "itemType = :cursor AND shard = :shard AND revision = :previous",
                )
                .expression_attribute_values(":cursor", s(codec::GRANT_EXPIRY_CURSOR))
                .expression_attribute_values(":shard", n(u64::from(shard)))
                .expression_attribute_values(":previous", n(cursor.revision)),
        };
        let outcome = request.send().await;
        let observed = match outcome {
            Ok(_) => return Ok(()),
            Err(error) => {
                if let Some(
                    aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(failed),
                ) = error.as_service_error()
                {
                    failed.item.clone().map(Box::new)
                } else {
                    let classified = classify(
                        &error,
                        Idempotence::Write(Resolution::TargetItem),
                    );
                    if !matches!(classified, StoreError::CommitAmbiguous { .. }) {
                        return Err(classified);
                    }
                    None
                }
            }
        };
        let winner = self.load_grant_expiry_cursor(shard).await?;
        resolve_expiry_cursor_winner(&target, winner.as_ref(), observed)
    }

    async fn expire_grant(&self, grant: &GrantExpiry, now: Timestamp) -> Result<(), StoreError> {
        let transaction = expressions::expire_grant(&self.table, grant, now)?;
        let request = transaction.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => Err(match error.as_service_error() {
                Some(service) => aex_session_dynamodb::error::decode_cancellation(
                    service,
                    transaction.participants(),
                ),
                None => classify(&error, Idempotence::Write(Resolution::TargetItem)),
            }),
        }
    }

    async fn redeem_grant(
        &self,
        token_sha256_hex: &str,
        now: Timestamp,
    ) -> Result<RedeemedGrant, StoreError> {
        let target = keys::grant(token_sha256_hex)?;
        let refused = || StoreError::PreconditionFailed {
            participant: aex_session_dynamodb::plan::Participant::CONTENT_GRANT,
            observed: None,
        };
        let item = self
            .get(&target.pk, &target.sk, Consistency::Strong)
            .await?
            .ok_or_else(refused)?;
        let grant = decode_grant(&item)?;
        if grant.expires_at <= now {
            return Err(refused());
        }
        let descriptor = self
            .load_descriptor(grant.workspace, &grant.digest)
            .await?
            .ok_or(StoreError::Invalid {
                detail: format!(
                    "grant for `{}` names a body with no descriptor",
                    body_hex(&grant.digest)
                ),
            })?;
        let inline = if descriptor.placement == aex_session_dynamodb::measure::Placement::Inline {
            self.read_inline_body(grant.workspace, &grant.digest)
                .await?
        } else {
            None
        };
        Ok(RedeemedGrant {
            grant,
            inline,
            descriptor,
        })
    }
}

fn expiry_exclusive_start(position: &GrantExpiryPosition) -> Item {
    [
        (
            keys::EXPIRY_PK.to_owned(),
            s(position.expiry_partition.clone()),
        ),
        (keys::EXPIRY_SK.to_owned(), s(position.expiry_sort.clone())),
        (
            aex_session_dynamodb::attr::PK.to_owned(),
            s(position.grant_pk.clone()),
        ),
        (
            aex_session_dynamodb::attr::SK.to_owned(),
            s(position.grant_sk.clone()),
        ),
    ]
    .into_iter()
    .collect()
}

fn expiry_position(mut key: Item, shard: u16) -> Result<GrantExpiryPosition, StoreError> {
    if key.len() != 4 {
        return Err(StoreError::Invalid {
            detail: "an expiry last-evaluated key does not contain exactly four members".to_owned(),
        });
    }
    let read = |key: &mut Item, name: &'static str| -> Result<String, StoreError> {
        key.remove(name)
            .ok_or(StoreError::Invalid {
                detail: format!("an expiry last-evaluated key is missing `{name}`"),
            })?
            .as_s()
            .cloned()
            .map_err(|_| StoreError::Invalid {
                detail: format!("an expiry last-evaluated key `{name}` is not a string"),
            })
    };
    let position = GrantExpiryPosition {
        expiry_partition: read(&mut key, keys::EXPIRY_PK)?,
        expiry_sort: read(&mut key, keys::EXPIRY_SK)?,
        grant_pk: read(&mut key, aex_session_dynamodb::attr::PK)?,
        grant_sk: read(&mut key, aex_session_dynamodb::attr::SK)?,
    };
    validate_grant_expiry_position(&position, shard)?;
    Ok(position)
}

fn same_expiry_cursor_target(left: &GrantExpiryCursor, right: &GrantExpiryCursor) -> bool {
    left.shard == right.shard && left.revision == right.revision && left.position == right.position
}

fn resolve_expiry_cursor_winner(
    expected: &GrantExpiryCursor,
    winner: Option<&GrantExpiryCursor>,
    observed: Option<Box<Item>>,
) -> Result<(), StoreError> {
    if winner.is_some_and(|winner| same_expiry_cursor_target(winner, expected)) {
        return Ok(());
    }
    Err(StoreError::PreconditionFailed {
        participant: Participant::CONTENT_GRANT_EXPIRY_CURSOR,
        observed: observed.or_else(|| winner.map(encode_grant_expiry_cursor).map(Box::new)),
    })
}

#[cfg(test)]
mod tests {
    use aex_wire::types::Timestamp;

    use aex_session_dynamodb::error::StoreError;

    use super::{
        GrantExpiryCursor, GrantExpiryPosition, resolve_expiry_cursor_winner,
        same_expiry_cursor_target,
    };

    fn cursor(revision: u64, position: Option<&str>, updated_at: &str) -> GrantExpiryCursor {
        GrantExpiryCursor {
            shard: 7,
            position: position.map(|suffix| GrantExpiryPosition {
                expiry_partition: "EXPIRY#0007".to_owned(),
                expiry_sort: format!("2026-08-02T12:34:56.789Z#{suffix}"),
                grant_pk: format!("GRANT#{suffix}"),
                grant_sk: "STATE".to_owned(),
            }),
            revision,
            updated_at: Timestamp::parse(updated_at).expect("a timestamp"),
        }
    }

    #[test]
    fn an_identical_concurrent_cursor_target_is_an_exact_replay() {
        let expected = cursor(10, Some("a"), "2026-08-02T12:34:56.789Z");
        let concurrent = cursor(10, Some("a"), "2026-08-02T12:35:00.000Z");
        assert!(
            same_expiry_cursor_target(&concurrent, &expected),
            "the winner instant is diagnostic; revision and position are authority"
        );
        assert_eq!(
            resolve_expiry_cursor_winner(&expected, Some(&concurrent), None),
            Ok(())
        );
    }

    #[test]
    fn a_later_or_different_cursor_winner_is_not_an_ambiguous_success() {
        let expected = cursor(10, None, "2026-08-02T12:34:56.789Z");
        assert!(!same_expiry_cursor_target(
            &cursor(11, None, "2026-08-02T12:35:00.000Z"),
            &expected
        ));
        assert!(!same_expiry_cursor_target(
            &cursor(10, Some("b"), "2026-08-02T12:35:00.000Z"),
            &expected
        ));
        assert!(matches!(
            resolve_expiry_cursor_winner(
                &expected,
                Some(&cursor(11, None, "2026-08-02T12:35:00.000Z")),
                None,
            ),
            Err(StoreError::PreconditionFailed { .. })
        ));
    }
}
