//! Failure-path cases for `runtime-activity`.

mod support;

use aex_hands_protocol::rpc::Fence;
use aex_runtime_activity_dynamodb::codec::{decode_generation, encode_generation};
use aex_runtime_activity_dynamodb::expressions;
use aex_runtime_control::generation::{GenerationState, Revision};
use aex_session_dynamodb::attr::CodecError;

use support::{TABLE, head, now, other_workspace, session, workspace};

fn condition(builder: aws_sdk_dynamodb::types::builders::UpdateBuilder) -> String {
    builder
        .build()
        .expect("a complete update")
        .condition_expression()
        .expect("conditional")
        .to_owned()
}

#[test]
fn a_superseded_lifecycle_reply_can_never_be_expressed_as_a_winning_write() {
    // Every transition names the fence and the revision it observed, so a reply
    // from a call the system has moved past simply loses its condition.
    let expression = condition(
        expressions::transition(
            TABLE,
            &head(GenerationState::Running),
            GenerationState::Launching,
            Fence(2),
            Revision::new(4),
            now(),
        )
        .expect("builds"),
    );
    assert!(expression.contains("fence = :fromFence"));
    assert!(expression.contains("revision = :fromRevision"));
}

#[test]
fn a_reschedule_can_never_advance_the_fence_out_from_under_an_in_flight_call() {
    let built = expressions::reschedule(
        TABLE,
        session(),
        support::generation(4),
        Fence(3),
        Revision::new(5),
        now(),
        now(),
    )
    .expect("builds")
    .build()
    .expect("a complete update");
    assert!(
        !built.update_expression().contains("fence ="),
        "{}",
        built.update_expression()
    );
}

#[test]
fn a_state_outside_the_vocabulary_is_corrupt_rather_than_a_new_variant() {
    let mut encoded = encode_generation(&head(GenerationState::Running)).expect("encodes");
    encoded.insert(
        "state".to_owned(),
        aex_session_dynamodb::attr::s("teleporting"),
    );
    let error = decode_generation(&encoded, workspace()).expect_err("an invented state");
    assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
}

#[test]
fn a_compute_shape_outside_the_five_public_tokens_is_refused() {
    let mut encoded = encode_generation(&head(GenerationState::Running)).expect("encodes");
    encoded.insert("size".to_owned(), aex_session_dynamodb::attr::s("16gb"));
    let error = decode_generation(&encoded, workspace()).expect_err("a sixth shape");
    assert!(
        error.to_string().contains("five public compute shapes"),
        "{error}"
    );
}

#[test]
fn a_generation_from_another_tenant_is_refused_after_read() {
    let encoded = encode_generation(&head(GenerationState::Running)).expect("encodes");
    let error = decode_generation(&encoded, other_workspace()).expect_err("another tenant");
    assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
}

#[test]
fn an_immutable_definition_that_disagrees_with_the_head_is_never_written() {
    let mut drifted = head(GenerationState::Requested);
    drifted.definition.workspace = other_workspace();
    let error = encode_generation(&drifted).expect_err("the launch authority must stay coherent");
    assert!(error.to_string().contains("workspace"), "{error}");
}

#[test]
fn an_intent_identity_that_could_forge_a_key_stops_the_builder() {
    let mut forged = support::intent();
    forged.intent_id = "int#evil".to_owned();
    assert!(expressions::record_intent(TABLE, &forged).is_err());
}
