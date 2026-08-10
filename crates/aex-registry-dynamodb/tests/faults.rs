//! Failure-path cases for `regional-registry`.

mod support;

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_registry_dynamodb::codec::{decode_pointer, decode_upload, encode_pointer, encode_upload};
use aex_registry_dynamodb::expressions;
use aex_session_dynamodb::attr::CodecError;
use aex_workspace_domain::registry::etag_of;
use aex_workspace_domain::upload::UploadState;

use support::{TABLE, pointer, upload, upload_id, workspace};

const TABLE_NAME: &str = TABLE;

#[test]
fn a_stored_etag_that_no_longer_describes_its_row_is_refused_on_read() {
    let mut encoded = encode_pointer(&pointer()).expect("encodes");
    encoded.insert(
        "etag".to_owned(),
        aex_session_dynamodb::attr::s(
            etag_of(RegistryKind::Skill, Revision::FIRST, &pointer().row.sha256)
                .as_str()
                .to_owned(),
        ),
    );
    let error = decode_pointer(&encoded, workspace()).expect_err("a tag from another kind");
    assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
}

#[test]
fn a_revision_that_is_not_a_whole_number_is_refused_rather_than_truncated() {
    let mut encoded = encode_pointer(&pointer()).expect("encodes");
    encoded.insert(
        "revision".to_owned(),
        aws_sdk_dynamodb::types::AttributeValue::N("1.5".to_owned()),
    );
    assert!(decode_pointer(&encoded, workspace()).is_err());
}

#[test]
fn a_part_plan_that_is_not_the_declared_grammar_is_refused() {
    let mut encoded = encode_upload(&upload()).head;
    encoded.insert(
        "parts".to_owned(),
        aex_session_dynamodb::attr::string_list(["not-a-part".to_owned()]),
    );
    let error = decode_upload(&encoded, workspace()).expect_err("a malformed plan");
    assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
}

#[test]
fn a_completion_can_never_reopen_a_consumed_upload() {
    let expression = expressions::begin_completion(TABLE_NAME, &upload(), &"a".repeat(64))
        .build()
        .expect("a complete update")
        .condition_expression()
        .expect("conditional")
        .to_owned();
    assert!(
        expression.contains("attribute_not_exists(consumedByName)"),
        "an upload a registry entry already points at must never be completed \
         again: {expression}"
    );
}

#[test]
fn a_finish_can_never_run_without_the_manifest_the_begin_pinned() {
    let expression = expressions::finish_completion(TABLE_NAME, upload_id(), &"b".repeat(64))
        .build()
        .expect("a complete update")
        .condition_expression()
        .expect("conditional")
        .to_owned();
    assert!(expression.contains("#state = :completing"));
    assert!(expression.contains("completionIntentHash = :hash"));
}

#[test]
fn a_transition_from_a_terminal_state_is_expressible_only_as_a_failing_condition() {
    // The adapter never inspects the state itself: it names the source state in
    // the condition, so an upload that has moved on simply loses.
    for terminal in [
        UploadState::Consumed,
        UploadState::Aborted,
        UploadState::Expired,
    ] {
        assert!(terminal.is_terminal());
        let expression =
            expressions::transition_upload(TABLE_NAME, upload_id(), terminal, UploadState::Ready)
                .build()
                .expect("a complete update")
                .condition_expression()
                .expect("conditional")
                .to_owned();
        assert!(expression.contains("#state = :from"));
    }
}

#[test]
fn a_name_that_could_forge_a_key_stops_every_builder() {
    assert!(
        expressions::delete_pointer(
            TABLE_NAME,
            workspace(),
            RegistryKind::File,
            "notes#evil",
            None
        )
        .is_err()
    );
    assert!(
        aex_registry_dynamodb::keys::pointer(workspace(), RegistryKind::File, "notes#evil")
            .is_err()
    );
}
