//! The verbatim `regional-content` condition and update expressions.
//!
//! The garbage collector is the reason this module reads the way it does. Every
//! sweep decision is fenced three ways — the epoch, the mark, and a strongly
//! consistent re-read of the partition for surviving pins — and **any** lost
//! condition drops the candidate rather than deleting the body. Uncertainty
//! always keeps data.

use aex_session_dynamodb::attr::{n, s, stamp};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{IMMUTABLE, Participant, TransactionPlan, key, keyed};
use aex_wire::ids::{ContentHash, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::builders::{DeleteBuilder, PutBuilder, UpdateBuilder};
use aws_sdk_dynamodb::types::{ConditionCheck, Delete, Put, Update};

use crate::codec::{
    self, ContentPin, DownloadGrant, GcCandidate, GcEpoch, TreePage, grant_pin_attributes,
    pin_attributes,
};
use crate::keys;
use crate::wire_pending::{Blake3Digest, GcSweepPlan, PinOwner, SealedBytes, body_hex};

/// A transport deduplication identity that fits the provider's 36-character
/// ceiling whatever the inputs are.
///
/// A token is a fixed-width digest of the operation's identity rather than a
/// concatenation of identifiers, because two thirty-character identifiers
/// already overflow the field and the service rejects the request.
#[must_use]
pub fn token(tag: &str, parts: &[&str]) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0x1f]);
    }
    let digest = hex::encode(hasher.finalize());
    let tag: String = tag.chars().take(3).collect();
    format!("{tag}-{}", &digest[..32])
}

/// Commits a staged body, as participant 2 of the admission transaction.
///
/// The condition admits an already-committed descriptor so an idempotent replay
/// of the same admission is not turned into a failure, and pins the digest so a
/// descriptor swapped underneath the caller cannot be committed by mistake.
///
/// # Errors
///
/// [`StoreError`] when the update could not be built.
pub fn commit_staged(
    table: &str,
    workspace: WorkspaceId,
    digest: &ContentHash,
    now: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let descriptor = keys::descriptor(workspace, digest);
    Ok(Update::builder()
        .table_name(table)
        .set_key(Some(key(&descriptor.pk, &descriptor.sk)))
        .condition_expression("#state IN (:staged, :committed) AND digestSha256 = :digest")
        .update_expression("SET #state = :committed, verifiedAt = :now")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":staged", s("staged"))
        .expression_attribute_values(":committed", s("committed"))
        .expression_attribute_values(":digest", s(digest.to_wire()))
        .expression_attribute_values(":now", stamp(now)))
}

/// Stages a descriptor, which is immutable once written.
///
/// # Errors
///
/// [`StoreError::Invalid`] when the row could not be encoded.
pub fn stage_descriptor(
    table: &str,
    descriptor: &codec::ContentDescriptor,
) -> Result<PutBuilder, StoreError> {
    let item = codec::encode_descriptor(descriptor).map_err(|error| invalid(&error))?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE))
}

/// Writes the inline ciphertext body beside its descriptor.
///
/// # Errors
///
/// [`StoreError::Invalid`] when the sealed body is over the item ceiling.
pub fn put_inline_body(
    table: &str,
    workspace: WorkspaceId,
    digest: &ContentHash,
    sealed: &SealedBytes,
) -> Result<PutBuilder, StoreError> {
    let item =
        codec::encode_inline_body(workspace, digest, sealed).map_err(|error| invalid(&error))?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE))
}

/// Pins one loose body.
///
/// # Errors
///
/// [`StoreError::Key`] when the pin identity could not enter a key.
pub fn pin_body(
    table: &str,
    digest: &ContentHash,
    pin: &ContentPin,
) -> Result<PutBuilder, StoreError> {
    let pin_key = keys::pin(pin.workspace, digest, &pin.owner)?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(keyed(pin_attributes(pin), &pin_key.pk, &pin_key.sk)))
        .condition_expression(IMMUTABLE))
}

/// Pins one root, which is where a binding's pin belongs.
///
/// # Errors
///
/// As [`pin_body`].
pub fn pin_root(
    table: &str,
    root: Blake3Digest,
    pin: &ContentPin,
) -> Result<PutBuilder, StoreError> {
    let pin_key = keys::root_pin(pin.workspace, root, &pin.owner)?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(keyed(pin_attributes(pin), &pin_key.pk, &pin_key.sk)))
        .condition_expression(IMMUTABLE))
}

/// Removes one root pin.
///
/// There is no reverse owner index and no reference count: the owner knows its
/// own root digest, so releasing a pin is a `DeleteItem` on a key the caller
/// already has (R-DELETE).
///
/// # Errors
///
/// As [`pin_body`].
pub fn unpin_root(
    table: &str,
    workspace: WorkspaceId,
    root: Blake3Digest,
    owner: &PinOwner,
) -> Result<DeleteBuilder, StoreError> {
    let pin_key = keys::root_pin(workspace, root, owner)?;
    Ok(Delete::builder()
        .table_name(table)
        .set_key(Some(key(&pin_key.pk, &pin_key.sk)))
        .condition_expression("attribute_exists(pk)"))
}

/// Writes one Merkle tree page.
///
/// # Errors
///
/// [`StoreError::Invalid`] when the page is over the 192 KiB target.
pub fn put_tree_page(table: &str, page: &TreePage) -> Result<PutBuilder, StoreError> {
    let item = codec::encode_tree_page(page).map_err(|error| invalid(&error))?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE))
}

/// Mints a download grant together with the pin that keeps its body alive.
///
/// The two rows are one transaction on purpose: a grant that outlived its pin
/// would authorise a read of a body the collector is free to delete.
///
/// # Errors
///
/// [`StoreError`] when a key component is unusable or a row cannot be encoded.
pub fn mint_grant(
    table: &str,
    grant: &DownloadGrant,
    now: Timestamp,
) -> Result<TransactionPlan, StoreError> {
    let mut plan = TransactionPlan::new(token("grt", &[&grant.token_sha256]));
    let item = codec::encode_grant(grant).map_err(|error| invalid(&error))?;
    plan.put(
        Participant::CONTENT_GRANT,
        Put::builder()
            .table_name(table)
            .set_item(Some(item))
            .condition_expression(IMMUTABLE),
    )?;
    let pin_key = keys::grant_pin(grant.workspace, &grant.digest, &grant.token_sha256)?;
    plan.put(
        Participant::CONTENT_GRANT_PIN,
        Put::builder()
            .table_name(table)
            .set_item(Some(keyed(
                grant_pin_attributes(grant.workspace, &grant.token_sha256, now, grant.expires_at),
                &pin_key.pk,
                &pin_key.sk,
            )))
            .condition_expression(IMMUTABLE),
    )?;
    Ok(plan)
}

/// Opens a new mark.
///
/// # Errors
///
/// [`StoreError`] when the update could not be built.
pub fn begin_mark(
    table: &str,
    epoch: &GcEpoch,
    now: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let epoch_key = keys::gc_epoch(epoch.workspace);
    Ok(Update::builder()
        .table_name(table)
        .set_key(Some(key(&epoch_key.pk, &epoch_key.sk)))
        .condition_expression("#state = :idle AND revision = :revision")
        .update_expression(
            "SET epoch = epoch + :one, #state = :marking, markStartedAt = :now, \
             markBucketCursor = :zero, revision = :nextRevision",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":idle", s("idle"))
        .expression_attribute_values(":marking", s("marking"))
        .expression_attribute_values(":revision", n(epoch.revision))
        .expression_attribute_values(":nextRevision", n(epoch.revision.saturating_add(1)))
        .expression_attribute_values(":one", n(1))
        .expression_attribute_values(":zero", n(0))
        .expression_attribute_values(":now", stamp(now)))
}

/// Advances the mark's bucket cursor.
///
/// # Errors
///
/// [`StoreError`] when the update could not be built.
pub fn advance_mark(
    table: &str,
    epoch: &GcEpoch,
    through_bucket: u64,
) -> Result<UpdateBuilder, StoreError> {
    let epoch_key = keys::gc_epoch(epoch.workspace);
    Ok(Update::builder()
        .table_name(table)
        .set_key(Some(key(&epoch_key.pk, &epoch_key.sk)))
        .condition_expression("#state = :marking AND epoch = :epoch AND revision = :revision")
        .update_expression("SET markBucketCursor = :bucket, revision = :nextRevision")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":marking", s("marking"))
        .expression_attribute_values(":epoch", n(epoch.epoch))
        .expression_attribute_values(":revision", n(epoch.revision))
        .expression_attribute_values(":nextRevision", n(epoch.revision.saturating_add(1)))
        .expression_attribute_values(":bucket", n(through_bucket)))
}

/// Moves a completed mark into the sweep.
///
/// # Errors
///
/// [`StoreError`] when the update could not be built.
pub fn begin_sweep(table: &str, epoch: &GcEpoch) -> Result<UpdateBuilder, StoreError> {
    let epoch_key = keys::gc_epoch(epoch.workspace);
    Ok(Update::builder()
        .table_name(table)
        .set_key(Some(key(&epoch_key.pk, &epoch_key.sk)))
        .condition_expression("#state = :marking AND epoch = :epoch AND revision = :revision")
        .update_expression("SET #state = :sweeping, revision = :nextRevision")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":marking", s("marking"))
        .expression_attribute_values(":sweeping", s("sweeping"))
        .expression_attribute_values(":epoch", n(epoch.epoch))
        .expression_attribute_values(":revision", n(epoch.revision))
        .expression_attribute_values(":nextRevision", n(epoch.revision.saturating_add(1))))
}

/// Closes the epoch.
///
/// # Errors
///
/// [`StoreError`] when the update could not be built.
pub fn finish_epoch(table: &str, epoch: &GcEpoch) -> Result<UpdateBuilder, StoreError> {
    let epoch_key = keys::gc_epoch(epoch.workspace);
    Ok(Update::builder()
        .table_name(table)
        .set_key(Some(key(&epoch_key.pk, &epoch_key.sk)))
        .condition_expression("#state = :sweeping AND epoch = :epoch AND revision = :revision")
        .update_expression("SET #state = :idle, revision = :nextRevision REMOVE sweepCursor")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":sweeping", s("sweeping"))
        .expression_attribute_values(":idle", s("idle"))
        .expression_attribute_values(":epoch", n(epoch.epoch))
        .expression_attribute_values(":revision", n(epoch.revision))
        .expression_attribute_values(":nextRevision", n(epoch.revision.saturating_add(1))))
}

/// Stages one candidate for a body the current mark did not reach.
///
/// # Errors
///
/// [`StoreError::Invalid`] when the candidate could not be encoded.
pub fn stage_candidate(table: &str, candidate: &GcCandidate) -> Result<PutBuilder, StoreError> {
    let item = codec::encode_gc_candidate(candidate).map_err(|error| invalid(&error))?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE))
}

/// The participants a fenced sweep names, in plan order.
pub const SWEEP_ORDER: [Participant; 4] = [
    Participant::CONTENT_GC_EPOCH,
    Participant::CONTENT_DESCRIPTOR,
    Participant::CONTENT_DESCRIPTOR,
    Participant::CONTENT_GC_CANDIDATE,
];

/// Compiles the fenced sweep of exactly one candidate.
///
/// The caller has already proved, with a strongly consistent query of the
/// content partition, that no `PIN#` and no unexpired `GRANT#` item survives,
/// and has already deleted the object with `If-Match`. Any condition that loses
/// here means a pin, a grant or a new epoch appeared in between, and the body
/// survives.
///
/// # Errors
///
/// [`StoreError`] when an action could not be built.
pub fn sweep(table: &str, plan: &GcSweepPlan) -> Result<TransactionPlan, StoreError> {
    let epoch_key = keys::gc_epoch(plan.workspace);
    let descriptor = keys::descriptor(plan.workspace, &plan.digest);
    let candidate = keys::gc_candidate(plan.workspace, &plan.digest);

    let mut transaction = TransactionPlan::new(token(
        "gcs",
        &[
            &plan.workspace.to_string(),
            &body_hex(&plan.digest),
            &plan.epoch.to_string(),
        ],
    ));
    transaction.condition_check(
        Participant::CONTENT_GC_EPOCH,
        ConditionCheck::builder()
            .table_name(table)
            .set_key(Some(key(&epoch_key.pk, &epoch_key.sk)))
            .condition_expression("epoch = :epoch AND #state = :sweeping")
            .expression_attribute_names("#state", "state")
            .expression_attribute_values(":epoch", n(plan.epoch))
            .expression_attribute_values(":sweeping", s("sweeping")),
    )?;
    transaction.condition_check(
        Participant::CONTENT_DESCRIPTOR,
        ConditionCheck::builder()
            .table_name(table)
            .set_key(Some(key(&descriptor.pk, &descriptor.sk)))
            .condition_expression("gcEpoch = :markedEpoch")
            .expression_attribute_values(":markedEpoch", n(plan.marked_epoch)),
    )?;
    transaction.delete(
        Participant::CONTENT_DESCRIPTOR,
        Delete::builder()
            .table_name(table)
            .set_key(Some(key(&descriptor.pk, &descriptor.sk)))
            .condition_expression("gcEpoch = :markedEpoch")
            .expression_attribute_values(":markedEpoch", n(plan.marked_epoch)),
    )?;
    transaction.delete(
        Participant::CONTENT_GC_CANDIDATE,
        Delete::builder()
            .table_name(table)
            .set_key(Some(key(&candidate.pk, &candidate.sk)))
            .condition_expression("epoch = :epoch")
            .expression_attribute_values(":epoch", n(plan.epoch)),
    )?;
    Ok(transaction)
}

fn invalid(error: &codec::EncodeError) -> StoreError {
    StoreError::Invalid {
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{ContentHash, OrganizationId, PrefixedId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{SWEEP_ORDER, begin_mark, commit_staged, finish_epoch, sweep, token};
    use crate::codec::GcEpoch;
    use crate::wire_pending::GcSweepPlan;

    const TABLE: &str = "dev-eu-west-1-regional-content";

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
    }

    fn now() -> Timestamp {
        Timestamp::parse("2026-08-01T12:34:56.789Z").expect("the pinned spelling")
    }

    fn epoch() -> GcEpoch {
        GcEpoch {
            workspace: workspace(),
            epoch: 4,
            state: "idle".to_owned(),
            mark_started_at: None,
            mark_bucket_cursor: None,
            sweep_cursor: None,
            revision: 9,
        }
    }

    fn condition(builder: aws_sdk_dynamodb::types::builders::UpdateBuilder) -> String {
        builder
            .build()
            .expect("a complete update")
            .condition_expression()
            .expect("conditional")
            .to_owned()
    }

    #[test]
    fn a_transport_token_always_fits_the_provider_ceiling() {
        let long = "a".repeat(512);
        let value = token("gcs", &[&long, &long, &long]);
        assert!(value.len() <= 36, "{} characters", value.len());
        assert_eq!(value, token("gcs", &[&long, &long, &long]));
        assert_ne!(value, token("gcs", &[&long, &long]));
    }

    #[test]
    fn beginning_a_mark_requires_an_idle_epoch_at_the_observed_revision() {
        let expression = condition(begin_mark(TABLE, &epoch(), now()).expect("builds"));
        assert!(expression.contains("#state = :idle"));
        assert!(expression.contains("revision = :revision"));
    }

    #[test]
    fn closing_an_epoch_requires_the_sweep_it_opened() {
        let expression = condition(finish_epoch(TABLE, &epoch()).expect("builds"));
        assert!(expression.contains("#state = :sweeping"));
        assert!(expression.contains("epoch = :epoch"));
    }

    #[test]
    fn committing_a_staged_body_pins_the_digest_and_admits_an_idempotent_replay() {
        let expression = condition(
            commit_staged(TABLE, workspace(), &ContentHash::from_bytes([1; 32]), now())
                .expect("builds"),
        );
        assert!(expression.contains("#state IN (:staged, :committed)"));
        assert!(expression.contains("digestSha256 = :digest"));
    }

    #[test]
    fn a_sweep_names_the_epoch_the_descriptor_and_the_candidate_in_that_order() {
        let plan = sweep(
            TABLE,
            &GcSweepPlan {
                workspace: workspace(),
                organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
                digest: ContentHash::from_bytes([3; 32]),
                epoch: 4,
                marked_epoch: 3,
            },
        )
        .expect("compiles");
        assert_eq!(plan.participants(), SWEEP_ORDER);
        assert_eq!(plan.len(), 4);
    }
}
