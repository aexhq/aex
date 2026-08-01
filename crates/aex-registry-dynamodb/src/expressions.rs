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
use aex_wire::ids::{UploadId, WorkspaceId};
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

/// Removes a pointer under the revision the caller observed.
///
/// # Errors
///
/// As [`create_pointer`].
pub fn delete_pointer(
    table: &str,
    workspace: WorkspaceId,
    kind: RegistryKind,
    name: &str,
    from_revision: Revision,
) -> Result<DeleteBuilder, StoreError> {
    let pointer = keys::pointer(workspace, kind, name)?;
    Ok(Delete::builder()
        .table_name(table)
        .set_key(Some(key(&pointer.pk, &pointer.sk)))
        .condition_expression("attribute_exists(pk) AND revision = :fromRevision")
        .expression_attribute_values(":fromRevision", n(from_revision.0)))
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
    use aex_content_domain::identity::{RegistryKind, Revision};
    use aex_wire::ids::{PrefixedId, UploadId, Uuid7, WorkspaceId};
    use aex_workspace_domain::upload::UploadState;

    use super::{
        begin_completion, consume_upload, delete_pointer, finish_completion, transition_upload,
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

    #[test]
    fn every_pointer_write_is_fenced_on_the_revision_the_caller_observed() {
        let built = delete_pointer(
            TABLE,
            workspace(),
            RegistryKind::File,
            "notes.md",
            Revision(4),
        )
        .expect("builds")
        .build()
        .expect("a complete delete");
        assert!(
            built
                .condition_expression()
                .expect("conditional")
                .contains("revision = :fromRevision")
        );
    }

    #[test]
    fn a_name_that_could_forge_a_key_stops_the_builder() {
        assert!(
            delete_pointer(
                TABLE,
                workspace(),
                RegistryKind::File,
                "a#b",
                Revision::FIRST
            )
            .is_err()
        );
        assert!(consume_upload(TABLE, upload(), RegistryKind::Tool, "a#b").is_err());
    }
}
