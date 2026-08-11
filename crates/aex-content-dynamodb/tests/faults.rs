//! Failure-path cases for `regional-content`.
//!
//! Every one of them asserts the same rule from a different angle: when the
//! collector is unsure, the body survives.

mod support;

use aex_content_dynamodb::codec::ContentDescriptor;
use aex_content_dynamodb::expressions::{SWEEP_ORDER, sweep};
use aex_content_dynamodb::keys;
use aex_content_dynamodb::store::Reachability;
use aex_content_dynamodb::wire_pending::{GcSweepPlan, PinOwner};
use aex_session_dynamodb::error::{StoreError, decode_cancellation};
use aex_session_dynamodb::measure;
use aex_session_dynamodb::plan::Participant;
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_sdk_dynamodb::types::CancellationReason;
use aws_sdk_dynamodb::types::error::TransactionCanceledException;

use support::{TABLE, descriptor, digest, organization, workspace};

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

fn plan() -> GcSweepPlan {
    GcSweepPlan {
        workspace: workspace(),
        organization: organization(),
        digest: digest(0xab),
        epoch: 4,
        marked_epoch: 3,
    }
}

#[test]
fn a_new_epoch_between_the_mark_and_the_sweep_names_the_epoch_participant() {
    let transaction = sweep(TABLE, &plan()).expect("compiles");
    let error = decode_cancellation(
        &cancelled(&["ConditionalCheckFailed", "None", "None"]),
        transaction.participants(),
    );
    assert!(
        matches!(
            error,
            StoreError::PreconditionFailed {
                participant: Participant::CONTENT_GC_EPOCH,
                ..
            }
        ),
        "{error}"
    );
    assert!(
        !error.retryable(),
        "a lost fence is never retried into a delete"
    );
}

#[test]
fn a_body_remarked_since_the_mark_names_the_descriptor_participant() {
    let transaction = sweep(TABLE, &plan()).expect("compiles");
    let error = decode_cancellation(
        &cancelled(&["None", "ConditionalCheckFailed", "None"]),
        transaction.participants(),
    );
    match error {
        StoreError::PreconditionFailed { participant, .. } => {
            assert_eq!(participant, Participant::CONTENT_DESCRIPTOR);
        }
        other => panic!("{other}"),
    }
}

#[test]
fn a_candidate_restaged_under_a_newer_epoch_names_the_candidate_participant() {
    let transaction = sweep(TABLE, &plan()).expect("compiles");
    let error = decode_cancellation(
        &cancelled(&["None", "None", "ConditionalCheckFailed"]),
        transaction.participants(),
    );
    match error {
        StoreError::PreconditionFailed { participant, .. } => {
            assert_eq!(participant, Participant::CONTENT_GC_CANDIDATE);
        }
        other => panic!("{other}"),
    }
}

#[test]
fn a_reason_vector_that_does_not_match_the_sweep_is_a_bug_and_never_a_guess() {
    let transaction = sweep(TABLE, &plan()).expect("compiles");
    assert_eq!(transaction.participants(), SWEEP_ORDER);
    let error = decode_cancellation(
        &cancelled(&["None", "ConditionalCheckFailed"]),
        transaction.participants(),
    );
    assert!(matches!(error, StoreError::Invalid { .. }), "{error}");
}

#[test]
fn a_surviving_pin_or_an_unexpired_grant_makes_a_body_uncollectable() {
    assert!(
        Reachability {
            pins: 0,
            unexpired_grants: 0
        }
        .is_collectable()
    );
    assert!(
        !Reachability {
            pins: 1,
            unexpired_grants: 0
        }
        .is_collectable()
    );
    assert!(
        !Reachability {
            pins: 0,
            unexpired_grants: 1
        }
        .is_collectable(),
        "an unexpired grant authorises a read that a delete would break"
    );
}

#[test]
fn an_over_large_descriptor_is_refused_before_it_reaches_the_service() {
    let mut over: ContentDescriptor = descriptor();
    over.media_type = "x".repeat(measure::APPLICATION_ITEM_CEILING + 1);
    assert!(aex_content_dynamodb::codec::encode_descriptor(&over).is_err());
}

#[test]
fn a_pin_identity_carrying_a_separator_stops_every_expression_builder() {
    let owner = PinOwner::Registry {
        kind: "tool".to_owned(),
        name: "cur#evil".to_owned(),
    };
    assert!(keys::pin(workspace(), &digest(1), &owner).is_err());
}
