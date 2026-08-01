//! Failure-path cases for `regional-secret-custody`.

mod support;

use aex_secret_custody_dynamodb::codec::{
    decode_manifest, decode_provider_credential, decode_secret, encode_manifest,
    encode_provider_credential, encode_secret,
};
use aex_secret_custody_dynamodb::expressions::{self, AUTHORIZE_ORDER};
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{SecretRevision, SourceGeneration};
use aex_session_dynamodb::attr::CodecError;
use aex_session_dynamodb::error::{StoreError, decode_cancellation};
use aex_session_dynamodb::plan::Participant;
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_sdk_dynamodb::types::CancellationReason;
use aws_sdk_dynamodb::types::error::TransactionCanceledException;

use support::{
    TABLE, authorization, manifest, metadata, now, provider_credential, secret_name, workspace,
};

fn cancelled(codes: &[&str]) -> TransactWriteItemsError {
    TransactWriteItemsError::TransactionCanceledException(
        TransactionCanceledException::builder()
            .set_cancellation_reasons(Some(
                codes
                    .iter()
                    .map(|code| CancellationReason::builder().code(*code).build())
                    .collect(),
            ))
            .build(),
    )
}

#[test]
fn a_revoked_record_names_the_metadata_participant_so_the_caller_never_decrypts() {
    let plan = expressions::authorize_managed_call(TABLE, &authorization(), SecretRevision::FIRST)
        .expect("compiles");
    let error = decode_cancellation(
        &cancelled(&["ConditionalCheckFailed", "None", "None"]),
        plan.participants(),
    );
    assert!(
        matches!(
            error,
            StoreError::PreconditionFailed {
                participant: Participant::SECRET_METADATA,
                ..
            }
        ),
        "{error}"
    );
    assert!(
        !error.retryable(),
        "`secret_revoked` is never retried into a decrypt"
    );
}

#[test]
fn a_concurrent_rebind_names_the_custody_head_participant() {
    let plan = expressions::authorize_managed_call(TABLE, &authorization(), SecretRevision::FIRST)
        .expect("compiles");
    assert_eq!(plan.participants(), AUTHORIZE_ORDER);
    let error = decode_cancellation(
        &cancelled(&["None", "ConditionalCheckFailed", "None"]),
        plan.participants(),
    );
    match error {
        StoreError::PreconditionFailed { participant, .. } => {
            assert_eq!(participant, Participant::CUSTODY_HEAD);
        }
        other => panic!("{other}"),
    }
}

#[test]
fn an_existing_authorization_is_an_idempotent_replay_rather_than_a_new_grant() {
    let plan = expressions::authorize_managed_call(TABLE, &authorization(), SecretRevision::FIRST)
        .expect("compiles");
    let error = decode_cancellation(
        &cancelled(&["None", "None", "ConditionalCheckFailed"]),
        plan.participants(),
    );
    match error {
        StoreError::PreconditionFailed { participant, .. } => {
            assert_eq!(participant, Participant::CUSTODY_AUTHORIZATION);
        }
        other => panic!("{other}"),
    }
}

#[test]
fn a_metadata_row_that_grew_sealed_bytes_is_refused_rather_than_read() {
    let mut encoded = encode_secret(&metadata()).expect("encodes");
    encoded.insert(
        "ciphertext".to_owned(),
        aex_session_dynamodb::attr::b(vec![1, 2, 3]),
    );
    let error = decode_secret(&encoded, workspace()).expect_err("sealed bytes on metadata");
    assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
}

#[test]
fn a_manifest_that_grew_sealed_bytes_is_refused_rather_than_read() {
    let mut encoded = encode_manifest(&manifest());
    encoded.insert(
        "wrappedKey".to_owned(),
        aex_session_dynamodb::attr::b(vec![9; 32]),
    );
    let error = decode_manifest(&encoded, workspace()).expect_err("sealed bytes on a manifest");
    assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
}

#[test]
fn a_manifest_entry_that_is_not_a_digest_is_refused() {
    let mut encoded = encode_manifest(&manifest());
    encoded.insert(
        "digests".to_owned(),
        aex_session_dynamodb::attr::string_list(["a-real-looking-value".to_owned()]),
    );
    let error = decode_manifest(&encoded, workspace()).expect_err("not a digest");
    assert!(
        matches!(error, CodecError::Malformed { .. }),
        "a manifest that can hold a raw value is a manifest that can leak one: {error}"
    );
}

#[test]
fn a_name_that_could_forge_a_key_stops_every_builder() {
    assert!(
        expressions::revoke(TABLE, workspace(), "a#b", RevocationEpoch::INITIAL, now()).is_err()
    );
    assert!(aex_secret_custody_dynamodb::keys::secret(workspace(), "a#b").is_err());
    let _ = secret_name();
}

#[test]
fn a_set_plan_refuses_a_revision_jump_and_mismatched_generation_identity() {
    let mut jumped = metadata();
    jumped.revision = SecretRevision(9);
    let error = expressions::set(TABLE, &support::generation(), &jumped, None)
        .expect_err("the first set writes revision one");
    assert!(matches!(error, StoreError::Invalid { .. }), "{error}");

    let mut wrong_generation = support::generation();
    wrong_generation.generation = SourceGeneration(2);
    let error = expressions::set(TABLE, &wrong_generation, &metadata(), None)
        .expect_err("metadata cannot point at a different generation identity");
    assert!(matches!(error, StoreError::Invalid { .. }), "{error}");
}

/// A stored provider or state outside the closed set is corruption, and must be
/// a decode failure rather than a value a projection has to guess at.
#[test]
fn a_credential_row_outside_the_closed_vocabularies_is_refused() {
    for (attribute, corrupt) in [("provider", "openrouter"), ("state", "active")] {
        let mut row = encode_provider_credential(&provider_credential()).expect("encodes");
        row.insert(
            attribute.to_owned(),
            aex_session_dynamodb::attr::s(corrupt.to_owned()),
        );
        let refusal = decode_provider_credential(&row, workspace()).expect_err("refused");
        assert!(
            matches!(refusal, CodecError::Malformed { attribute: found, .. } if found == attribute),
            "`{corrupt}` on `{attribute}` decoded instead of failing: {refusal:?}"
        );
    }
}

/// The fingerprint is persisted, so a row without one cannot be answered with a
/// synthesised value: the read path has no plaintext to recompute it from.
#[test]
fn a_credential_row_without_a_fingerprint_is_refused_rather_than_defaulted() {
    let mut row = encode_provider_credential(&provider_credential()).expect("encodes");
    row.remove("fingerprint");
    let refusal = decode_provider_credential(&row, workspace()).expect_err("refused");
    assert!(
        matches!(
            refusal,
            CodecError::Missing {
                attribute: "fingerprint",
                ..
            }
        ),
        "{refusal:?}"
    );
}
