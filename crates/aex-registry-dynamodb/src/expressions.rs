//! The verbatim `regional-registry` condition and update expressions.
//!
//! Every pointer write is fenced on the revision the caller observed, which is
//! what makes `If-Match` on the public route mean something: the client's `ETag`
//! is a pure function of `(kind, revision, digest)`, so conditioning on the
//! revision *is* conditioning on the tag.

use std::fmt::Write as _;

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_session_dynamodb::attr::{n, s};
use aex_session_dynamodb::component::Component;
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{IMMUTABLE, key};
use aex_session_dynamodb::replay::{Receipt, encode_receipt_row};
use aex_wire::ids::{UploadId, WorkspaceId};
use aex_wire::types::ETag;
use aex_workspace_domain::registry::RegistryPointer;
use aex_workspace_domain::upload::{Upload, UploadState};
use aws_sdk_dynamodb::types::builders::{DeleteBuilder, PutBuilder, UpdateBuilder};
use aws_sdk_dynamodb::types::{Delete, Put, Update};

use crate::codec::{self, upload_state_str};
use crate::keys;

/// Creates a pointer that must not already exist.
///
/// # Errors
///
/// [`StoreError`] when the name could not enter a key.
pub fn create_pointer(table: &str, pointer: &RegistryPointer) -> Result<PutBuilder, StoreError> {
    let item = codec::encode_pointer(pointer).map_err(|error| invalid(&error))?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE))
}

/// Replaces a pointer under the revision the caller observed.
///
/// # Errors
///
/// As [`create_pointer`].
pub fn replace_pointer(
    table: &str,
    pointer: &RegistryPointer,
    from_revision: Revision,
) -> Result<PutBuilder, StoreError> {
    let item = codec::encode_pointer(pointer).map_err(|error| invalid(&error))?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression("attribute_exists(pk) AND revision = :fromRevision")
        .expression_attribute_values(":fromRevision", n(from_revision.0)))
}

/// Removes a pointer, honouring `If-Match` exactly and reading nothing first.
///
/// The tag is stored on the row, so `If-Match` is expressible as a condition and
/// the happy path performs no read at all (D-9). The `attribute_exists` half is
/// what makes the returned `ALL_OLD` item decisive: a condition failure with no
/// observed item means the name was already absent, which is an idempotent
/// success, and one *with* an observed item means the tag did not match, which
/// is `412` carrying that row's current tag.
///
/// # Errors
///
/// As [`create_pointer`].
pub fn delete_pointer(
    table: &str,
    workspace: WorkspaceId,
    kind: RegistryKind,
    name: &str,
    if_match: Option<&ETag>,
) -> Result<DeleteBuilder, StoreError> {
    let pointer = keys::pointer(workspace, kind, name)?;
    let builder = Delete::builder()
        .table_name(table)
        .set_key(Some(key(&pointer.pk, &pointer.sk)));
    Ok(match if_match {
        None => builder.condition_expression("attribute_exists(pk)"),
        Some(expected) => builder
            .condition_expression("attribute_exists(pk) AND etag = :ifMatch")
            .expression_attribute_values(":ifMatch", s(expected.as_str().to_owned())),
    })
}

/// Writes the durable idempotency receipt of one registry mutation.
///
/// `IMMUTABLE` is what makes the receipt the fence: the winner writes it inside
/// its own transaction, and a loser learns the winner's answer from the item
/// this condition failure returns rather than from a second read (D-8).
///
/// # Errors
///
/// [`StoreError`] when the rendered scope or key digest could not enter a key.
pub fn put_receipt(
    table: &str,
    workspace: WorkspaceId,
    receipt: &Receipt,
) -> Result<PutBuilder, StoreError> {
    let item = encode_receipt_row(workspace, receipt).map_err(|error| StoreError::Invalid {
        detail: error.to_string(),
    })?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE))
}

/// Claims one entry against the `registry.entries` cap.
///
/// Only a **create** claims: a replace occupies a name it already holds. The cap
/// is the condition, not a preceding `Select=COUNT`, so two concurrent creates
/// at the boundary cannot both win.
#[must_use]
pub fn claim_entry(table: &str, workspace: WorkspaceId, kind: RegistryKind, cap: u64) -> UpdateBuilder {
    let target = keys::count(workspace, kind);
    Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression("attribute_not_exists(#count) OR #count < :cap")
        .update_expression(
            "SET #count = if_not_exists(#count, :zero) + :one, itemType = :type, \
             workspaceId = :workspace, kind = :kind",
        )
        .expression_attribute_names("#count", codec::COUNT)
        .expression_attribute_values(":cap", n(cap))
        .expression_attribute_values(":zero", n(0))
        .expression_attribute_values(":one", n(1))
        .expression_attribute_values(":type", s(codec::REGISTRY_COUNT))
        .expression_attribute_values(":workspace", s(workspace.to_string()))
        .expression_attribute_values(":kind", s(kind.as_str()))
}

/// Releases one entry when a name is removed.
///
/// D-13 specifies the claim and is silent on the release. A counter that only
/// ever rose would make a workspace that created and deleted `cap` names
/// permanently unable to create another, so the release rides the delete
/// transaction and is exact whenever the counter exists. An **absent** counter
/// resolves to zero rather than refusing: `registry_files_delete` declares no
/// error a missing counter could be reported as, and a delete that cannot
/// succeed is worse than a count that heals.
#[must_use]
pub fn release_entry(table: &str, workspace: WorkspaceId, kind: RegistryKind) -> UpdateBuilder {
    let target = keys::count(workspace, kind);
    Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression("attribute_not_exists(#count) OR #count > :zero")
        .update_expression(
            "SET #count = if_not_exists(#count, :one) - :one, itemType = :type, \
             workspaceId = :workspace, kind = :kind",
        )
        .expression_attribute_names("#count", codec::COUNT)
        .expression_attribute_values(":zero", n(0))
        .expression_attribute_values(":one", n(1))
        .expression_attribute_values(":type", s(codec::REGISTRY_COUNT))
        .expression_attribute_values(":workspace", s(workspace.to_string()))
        .expression_attribute_values(":kind", s(kind.as_str()))
}

/// Stages an upload's head row, which must not already exist.
#[must_use]
pub fn create_upload(table: &str, upload: &Upload) -> PutBuilder {
    Put::builder()
        .table_name(table)
        .set_item(Some(codec::encode_upload(upload).head))
        .condition_expression(IMMUTABLE)
}

/// Stages every spilled part block of an upload. Empty below the spill bound.
#[must_use]
pub fn create_upload_part_blocks(table: &str, upload: &Upload) -> Vec<PutBuilder> {
    codec::encode_upload(upload)
        .part_blocks
        .into_iter()
        .map(|item| {
            Put::builder()
                .table_name(table)
                .set_item(Some(item))
                .condition_expression(IMMUTABLE)
        })
        .collect()
}

/// Moves an upload from one state to another, and only from that one.
///
/// A state machine expressed as a condition rather than as a read followed by a
/// write is the difference between "two callers cannot both complete" and "two
/// callers usually do not both complete".
#[must_use]
pub fn transition_upload(
    table: &str,
    upload: UploadId,
    from: UploadState,
    to: UploadState,
) -> UpdateBuilder {
    let target = keys::upload(upload);
    Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression("attribute_exists(pk) AND #state = :from")
        .update_expression("SET #state = :to")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":from", s(upload_state_str(from)))
        .expression_attribute_values(":to", s(upload_state_str(to)))
}

/// Moves an upload after the provider effect that the transition records.
///
/// The irreversible provider effect goes first and this write is conditional on
/// **the exact state the decision was made under and the exact handle it was made
/// against** (E D-6). `providerUploadId = :handle` is load-bearing and must not be
/// weakened: without it a stale worker could terminalise an upload that was
/// re-created under the same identity.
#[must_use]
pub fn transition_upload_fenced(
    table: &str,
    upload: UploadId,
    from: UploadState,
    to: UploadState,
    provider_upload_id: &str,
) -> UpdateBuilder {
    let target = keys::upload(upload);
    Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression(
            "attribute_exists(pk) AND #state = :from AND providerUploadId = :handle",
        )
        .update_expression("SET #state = :to")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":from", s(upload_state_str(from)))
        .expression_attribute_values(":to", s(upload_state_str(to)))
        .expression_attribute_values(":handle", s(provider_upload_id.to_owned()))
}

/// Records the settled per-part digests a grant call declared.
#[must_use]
pub fn record_part_declarations(table: &str, upload: &Upload) -> UpdateBuilder {
    let target = keys::upload(upload.id);
    let rows = codec::encode_upload(upload);
    let parts = rows
        .head
        .get("parts")
        .cloned()
        .unwrap_or_else(|| aex_session_dynamodb::attr::string_list(Vec::new()));
    Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression(
            "attribute_exists(pk) AND #state IN (:created, :granted) AND partBlockCount = :inline",
        )
        .update_expression("SET #state = :granted, parts = :parts")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":created", s(upload_state_str(UploadState::Created)))
        .expression_attribute_values(":granted", s(upload_state_str(UploadState::PartsGranted)))
        .expression_attribute_values(":inline", aex_session_dynamodb::attr::n(0))
        .expression_attribute_values(":parts", parts)
}

/// Settles an upload as `Ready`, writing the completion evidence that proves it.
///
/// Reachable from any pre-`Ready` state, because E's D-3 oracle is total: a
/// `HeadObject` that finds the declared bytes at the content-addressed key
/// establishes that they are committed regardless of which state the row was left
/// in. Fenced on the handle, like every write that follows a provider effect.
///
/// # Errors
///
/// [`StoreError::Invalid`] when the upload carries no completion evidence, which
/// would make `Ready` an assertion rather than a proof.
pub fn settle_ready(table: &str, upload: &Upload) -> Result<UpdateBuilder, StoreError> {
    let evidence = upload
        .completion
        .as_ref()
        .ok_or_else(|| StoreError::Invalid {
            detail: "a Ready upload must carry the evidence that proves its object exists"
                .to_owned(),
        })?;
    let target = keys::upload(upload.id);
    let mut assignments = vec!["#state = :ready".to_owned(), "objectEtag = :etag".to_owned()];
    let mut builder = Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression(
            "attribute_exists(pk) AND #state IN (:created, :granted, :completing) \
             AND providerUploadId = :handle",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":created", s(upload_state_str(UploadState::Created)))
        .expression_attribute_values(":granted", s(upload_state_str(UploadState::PartsGranted)))
        .expression_attribute_values(":completing", s(upload_state_str(UploadState::Completing)))
        .expression_attribute_values(":ready", s(upload_state_str(UploadState::Ready)))
        .expression_attribute_values(":handle", s(upload.provider_upload_id.clone()))
        .expression_attribute_values(":etag", s(evidence.etag.clone()));
    let mut removals = Vec::new();
    match &evidence.checksum_sha256 {
        Some(value) => {
            assignments.push("objectChecksumSha256 = :sha".to_owned());
            builder = builder.expression_attribute_values(":sha", s(value.clone()));
        }
        None => removals.push("objectChecksumSha256"),
    }
    match &evidence.checksum_crc64_nvme {
        Some(value) => {
            assignments.push("objectChecksumCrc64Nvme = :crc".to_owned());
            builder = builder.expression_attribute_values(":crc", s(value.clone()));
        }
        None => removals.push("objectChecksumCrc64Nvme"),
    }
    match evidence.part_count {
        Some(value) => {
            assignments.push("objectPartCount = :parts".to_owned());
            builder = builder.expression_attribute_values(":parts", n(value));
        }
        None => removals.push("objectPartCount"),
    }
    let mut expression = format!("SET {}", assignments.join(", "));
    if !removals.is_empty() {
        let _ = write!(expression, " REMOVE {}", removals.join(", "));
    }
    Ok(builder.update_expression(expression))
}

/// Removes an upload row that has no remaining consumer (E D-5).
///
/// Fenced on the terminal state the sweep observed, so a row that moved on since
/// the decision survives. An already-absent row is an idempotent success at the
/// caller, never a condition this expression relaxes.
#[must_use]
pub fn delete_upload(table: &str, upload: UploadId, from: UploadState) -> DeleteBuilder {
    let target = keys::upload(upload);
    Delete::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression("attribute_exists(pk) AND #state = :from")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":from", s(upload_state_str(from)))
}

/// Removes one spilled part block of an upload that is being reclaimed.
#[must_use]
pub fn delete_upload_part_block(table: &str, upload: UploadId, block: usize) -> DeleteBuilder {
    let target = keys::upload_parts(upload, block);
    Delete::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
}

/// Begins a completion, pinning the manifest the caller asked for.
///
/// A retry carrying a different manifest fails the `completionIntentHash`
/// condition and is `idempotency_conflict`; a retry carrying the same manifest
/// re-enters `completing` and is safe.
#[must_use]
pub fn begin_completion(
    table: &str,
    upload: &Upload,
    completion_intent_hash: &str,
) -> UpdateBuilder {
    let target = keys::upload(upload.id);
    // The submitted manifest is persisted with the intent, so a retried
    // completion is deterministic rather than dependent on the client resending
    // identical input. A spilled upload keeps its manifest in the blocks, which
    // `record_completion_manifest` writes first.
    let manifest = codec::encode_upload(upload)
        .head
        .get("completionManifest")
        .cloned()
        .unwrap_or_else(|| aex_session_dynamodb::attr::string_list(Vec::new()));
    Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression(
            "attribute_exists(pk) AND #state IN (:created, :granted, :completing) \
             AND attribute_not_exists(consumedByName) \
             AND providerUploadId = :handle \
             AND (attribute_not_exists(completionIntentHash) OR completionIntentHash = :hash)",
        )
        .update_expression(
            "SET #state = :completing, completionIntentHash = :hash, \
             completionManifest = :manifest",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":created", s(upload_state_str(UploadState::Created)))
        .expression_attribute_values(":granted", s(upload_state_str(UploadState::PartsGranted)))
        .expression_attribute_values(":completing", s(upload_state_str(UploadState::Completing)))
        .expression_attribute_values(":handle", s(upload.provider_upload_id.clone()))
        .expression_attribute_values(":manifest", manifest)
        .expression_attribute_values(":hash", s(completion_intent_hash.to_owned()))
}

/// Writes one spilled block's share of the submitted completion manifest.
#[must_use]
pub fn record_completion_manifest(table: &str, upload: &Upload, block: usize) -> UpdateBuilder {
    let target = keys::upload_parts(upload.id, block);
    let manifest = codec::encode_upload(upload)
        .part_blocks
        .get(block)
        .and_then(|item| item.get("completionManifest").cloned())
        .unwrap_or_else(|| aex_session_dynamodb::attr::string_list(Vec::new()));
    Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression("attribute_exists(pk)")
        .update_expression("SET completionManifest = :manifest")
        .expression_attribute_values(":manifest", manifest)
}

/// Finishes a completion under the manifest it began with.
#[must_use]
pub fn finish_completion(
    table: &str,
    upload: UploadId,
    completion_intent_hash: &str,
) -> UpdateBuilder {
    let target = keys::upload(upload);
    Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression("#state = :completing AND completionIntentHash = :hash")
        .update_expression("SET #state = :ready")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":completing", s(upload_state_str(UploadState::Completing)))
        .expression_attribute_values(":ready", s(upload_state_str(UploadState::Ready)))
        .expression_attribute_values(":hash", s(completion_intent_hash.to_owned()))
}

/// Consumes a ready upload into one registry entry, exactly once.
///
/// # Errors
///
/// [`StoreError`] when the name could not enter a key.
pub fn consume_upload(
    table: &str,
    upload: UploadId,
    kind: RegistryKind,
    name: &str,
) -> Result<UpdateBuilder, StoreError> {
    let target = keys::upload(upload);
    let name = Component::parse(name)?;
    Ok(Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression("#state = :ready AND attribute_not_exists(consumedByName)")
        .update_expression("SET #state = :consumed, consumedByKind = :kind, consumedByName = :name")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":ready", s(upload_state_str(UploadState::Ready)))
        .expression_attribute_values(":consumed", s(upload_state_str(UploadState::Consumed)))
        .expression_attribute_values(":kind", s(kind.as_str()))
        .expression_attribute_values(":name", s(name.as_str().to_owned())))
}

fn invalid(error: &codec::EncodeError) -> StoreError {
    StoreError::Invalid {
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use aex_content_domain::identity::RegistryKind;
    use aex_wire::ids::{PrefixedId, UploadId, Uuid7, WorkspaceId};
    use aex_wire::types::ETag;
    use aex_workspace_domain::upload::UploadState;

    use super::{
        begin_completion, claim_entry, consume_upload, delete_pointer, delete_upload,
        finish_completion, release_entry, transition_upload, transition_upload_fenced,
    };

    const TABLE: &str = "dev-eu-west-1-regional-registry";

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
    }

    fn upload() -> UploadId {
        UploadId::from_uuid7(Uuid7::compose(1_754_051_696_789, [4; 10]))
    }

    fn upload_row() -> aex_workspace_domain::upload::Upload {
        aex_workspace_domain::upload::Upload {
            id: upload(),
            workspace: workspace(),
            state: UploadState::PartsGranted,
            provider_upload_id: "provider-mpu-1".to_owned(),
            object_key: "wks/ab/cd/abcd".to_owned(),
            declared_size: 1_024,
            declared_sha256: aex_wire::ids::ContentHash::from_bytes([2; 32]),
            content_type: None,
            parts: aex_workspace_domain::upload::PartPlan {
                parts: vec![aex_workspace_domain::upload::PlannedPart {
                    number: 1,
                    bytes: 1_024,
                    sha256: None,
                }],
            },
            completion_manifest: Vec::new(),
            completion: None,
            consumed_by: None,
            created_at: aex_wire::types::Timestamp::from_unix_millis(0).expect("in range"),
            expires_at: aex_wire::types::Timestamp::from_unix_millis(1).expect("in range"),
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
    fn a_transition_names_exactly_one_source_state() {
        let expression = condition(transition_upload(
            TABLE,
            upload(),
            UploadState::Created,
            UploadState::Aborted,
        ));
        assert!(expression.contains("#state = :from"), "{expression}");
    }

    #[test]
    fn an_abort_is_fenced_on_the_exact_provider_handle_it_was_decided_against() {
        let expression = condition(transition_upload_fenced(
            TABLE,
            upload(),
            UploadState::PartsGranted,
            UploadState::Aborted,
            "provider-mpu-1",
        ));
        assert!(expression.contains("#state = :from"), "{expression}");
        assert!(
            expression.contains("providerUploadId = :handle"),
            "an abort that is not fenced on the handle could terminalise a \
             re-created upload: {expression}"
        );
    }

    #[test]
    fn a_reclaimed_row_is_deleted_under_the_terminal_state_the_sweep_observed() {
        let built = delete_upload(TABLE, upload(), UploadState::Ready)
            .build()
            .expect("a complete delete");
        assert!(
            built
                .condition_expression()
                .expect("conditional")
                .contains("#state = :from")
        );
    }

    #[test]
    fn a_completion_pins_its_manifest_and_refuses_a_consumed_upload() {
        let expression = condition(begin_completion(TABLE, &upload_row(), &"a".repeat(64)));
        assert!(expression.contains("attribute_not_exists(consumedByName)"));
        assert!(expression.contains("completionIntentHash = :hash"));
        assert!(
            condition(finish_completion(TABLE, upload(), &"a".repeat(64)))
                .contains("completionIntentHash = :hash"),
            "a completion must finish under the manifest it began with"
        );
    }

    #[test]
    fn an_upload_can_be_consumed_by_exactly_one_registry_entry() {
        let expression = condition(
            consume_upload(TABLE, upload(), RegistryKind::Tool, "search").expect("builds"),
        );
        assert!(expression.contains("#state = :ready"));
        assert!(expression.contains("attribute_not_exists(consumedByName)"));
    }

    /// A delete honours `If-Match` as a condition and reads nothing first.
    ///
    /// The tag is stored on the row, so the precondition is expressible without
    /// a pre-read, and the `attribute_exists` half is what makes the returned
    /// `ALL_OLD` item decisive between "already absent" and "stale tag" (D-9).
    #[test]
    fn a_delete_conditions_on_the_stored_tag_without_reading_it_first() {
        let tag = ETag::parse("registry-4").expect("a strong validator");
        let built = delete_pointer(
            TABLE,
            workspace(),
            RegistryKind::File,
            "notes.md",
            Some(&tag),
        )
        .expect("builds")
        .build()
        .expect("a complete delete");
        let condition = built.condition_expression().expect("conditional");
        assert!(condition.contains("attribute_exists(pk)"));
        assert!(condition.contains("etag = :ifMatch"));

        let unconditional = delete_pointer(TABLE, workspace(), RegistryKind::File, "notes.md", None)
            .expect("builds")
            .build()
            .expect("a complete delete");
        assert_eq!(
            unconditional.condition_expression(),
            Some("attribute_exists(pk)"),
            "without `If-Match` the only condition is existence"
        );
    }

    /// The entry cap is the write's own condition, never a preceding count.
    #[test]
    fn the_entry_claim_is_fenced_by_the_cap_and_the_release_never_underflows() {
        let claim = claim_entry(TABLE, workspace(), RegistryKind::File, 1_000)
            .build()
            .expect("a complete update");
        assert_eq!(
            claim.condition_expression(),
            Some("attribute_not_exists(#count) OR #count < :cap")
        );
        let release = release_entry(TABLE, workspace(), RegistryKind::File)
            .build()
            .expect("a complete update");
        assert_eq!(
            release.condition_expression(),
            Some("attribute_not_exists(#count) OR #count > :zero")
        );
    }

    #[test]
    fn a_name_that_could_forge_a_key_stops_the_builder() {
        assert!(delete_pointer(TABLE, workspace(), RegistryKind::File, "a#b", None).is_err());
        assert!(consume_upload(TABLE, upload(), RegistryKind::Tool, "a#b").is_err());
    }
}
