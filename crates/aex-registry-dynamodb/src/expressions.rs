//! The verbatim `regional-registry` condition and update expressions.
//!
//! Every pointer write is fenced on the revision the caller observed, which is
//! what makes `If-Match` on the public route mean something: the client's `ETag`
//! is a pure function of `(kind, revision, digest)`, so conditioning on the
//! revision *is* conditioning on the tag.

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

/// Stages an upload, which must not already exist.
#[must_use]
pub fn create_upload(table: &str, upload: &Upload) -> PutBuilder {
    Put::builder()
        .table_name(table)
        .set_item(Some(codec::encode_upload(upload)))
        .condition_expression(IMMUTABLE)
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

/// Begins a completion, pinning the manifest the caller asked for.
///
/// A retry carrying a different manifest fails the `completionIntentHash`
/// condition and is `idempotency_conflict`; a retry carrying the same manifest
/// re-enters `completing` and is safe.
#[must_use]
pub fn begin_completion(
    table: &str,
    upload: UploadId,
    completion_intent_hash: &str,
) -> UpdateBuilder {
    let target = keys::upload(upload);
    Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression(
            "attribute_exists(pk) AND #state IN (:created, :granted, :completing) \
             AND attribute_not_exists(consumedByName) \
             AND (attribute_not_exists(completionIntentHash) OR completionIntentHash = :hash)",
        )
        .update_expression("SET #state = :completing, completionIntentHash = :hash")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":created", s(upload_state_str(UploadState::Created)))
        .expression_attribute_values(":granted", s(upload_state_str(UploadState::PartsGranted)))
        .expression_attribute_values(":completing", s(upload_state_str(UploadState::Completing)))
        .expression_attribute_values(":hash", s(completion_intent_hash.to_owned()))
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
        begin_completion, claim_entry, consume_upload, delete_pointer, finish_completion,
        release_entry, transition_upload,
    };

    const TABLE: &str = "dev-eu-west-1-regional-registry";

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
    }

    fn upload() -> UploadId {
        UploadId::from_uuid7(Uuid7::compose(1_754_051_696_789, [4; 10]))
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
    fn a_completion_pins_its_manifest_and_refuses_a_consumed_upload() {
        let expression = condition(begin_completion(TABLE, upload(), &"a".repeat(64)));
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
