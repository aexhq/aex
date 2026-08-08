use aex_finance_domain::{Microusd, MicrousdDelta};
use aex_rds_data::{CommitFailure, DataApiError, DecodeError, Record, TransactionId};
use aws_sdk_rdsdata::types::Field;

use crate::row::{
    DATA_API_MAX_ROW_BYTES, RowDecodeError, RowPage, bigint_delta, bigint_money, numeric_u128,
};
use crate::tx::{
    CommitDisposition, CommitProbe, CommitResolution, PostedReceipt, UnknownCommit,
    UnknownCommitError, resolve_unknown_commit,
};

fn text(value: &str) -> Field {
    Field::StringValue(value.to_owned())
}

#[test]
fn money_decodes_only_from_the_bigint_transport_shape() {
    let long = [Field::LongValue(42)];
    assert_eq!(
        bigint_money(&Record::new(&long), 0),
        Ok(Microusd::new(42).expect("bound"))
    );
    let negative = [Field::LongValue(-42)];
    assert_eq!(
        bigint_delta(&Record::new(&negative), 0),
        Ok(MicrousdDelta::new(-42).expect("bound"))
    );
    // `amount_microusd` and `balance_microusd` are `bigint`, so the Data API
    // renders them as `longValue`. A string in a money column is a schema
    // change nobody reviewed, not a spelling to be re-parsed.
    let stringy = [text("42")];
    assert_eq!(
        bigint_money(&Record::new(&stringy), 0),
        Err(RowDecodeError::Transport(DecodeError::TypeMismatch {
            index: 0,
            expected: "bigint"
        }))
    );
}

#[test]
fn no_money_column_has_a_floating_or_null_decode_path() {
    let floating = [Field::DoubleValue(1.5)];
    assert_eq!(
        bigint_money(&Record::new(&floating), 0),
        Err(RowDecodeError::FloatInMoneyColumn)
    );
    assert_eq!(
        bigint_delta(&Record::new(&floating), 0),
        Err(RowDecodeError::FloatInMoneyColumn)
    );
    assert_eq!(
        numeric_u128(&Record::new(&floating), 0),
        Err(RowDecodeError::FloatInMoneyColumn)
    );
    let null = [Field::IsNull(true)];
    assert_eq!(
        bigint_money(&Record::new(&null), 0),
        Err(RowDecodeError::Null)
    );
    assert_eq!(
        bigint_delta(&Record::new(&null), 0),
        Err(RowDecodeError::Null)
    );
    assert_eq!(
        numeric_u128(&Record::new(&null), 0),
        Err(RowDecodeError::Null)
    );
}

#[test]
fn a_numeric_quantity_decodes_only_from_canonical_digits() {
    let long = [Field::LongValue(42)];
    assert_eq!(
        numeric_u128(&Record::new(&long), 0),
        Err(RowDecodeError::QuantityNotString)
    );
    let wide = [text("9223372036854775808")];
    assert_eq!(
        numeric_u128(&Record::new(&wide), 0),
        Ok(9_223_372_036_854_775_808)
    );
    for spelling in ["+1", "01", "-0", "1.0", "1e3", ""] {
        let fields = [text(spelling)];
        assert!(
            numeric_u128(&Record::new(&fields), 0).is_err(),
            "accepted {spelling}"
        );
    }
}

#[test]
fn a_money_column_the_row_does_not_project_is_an_arity_failure() {
    let one = [Field::LongValue(42)];
    assert_eq!(
        bigint_money(&Record::new(&one), 3),
        Err(RowDecodeError::Transport(DecodeError::ArityMismatch {
            expected: 4,
            actual: 1
        }))
    );
}

#[test]
fn data_api_row_and_result_limits_refuse_instead_of_clamp() {
    assert!(RowPage::new(vec![vec![0; DATA_API_MAX_ROW_BYTES]], None).is_ok());
    assert_eq!(
        RowPage::new(vec![vec![0; DATA_API_MAX_ROW_BYTES + 1]], None),
        Err(RowDecodeError::RowTooLarge {
            observed: DATA_API_MAX_ROW_BYTES + 1
        })
    );
    assert!(RowPage::new(vec![vec![0; DATA_API_MAX_ROW_BYTES]; 16], None).is_ok());
    assert_eq!(
        RowPage::new(vec![vec![0; DATA_API_MAX_ROW_BYTES]; 17], None),
        Err(RowDecodeError::ResultTooLarge {
            observed: DATA_API_MAX_ROW_BYTES * 17
        })
    );
}

fn receipt(intent: u8) -> PostedReceipt {
    PostedReceipt {
        transaction_id: "txn_fixture".into(),
        business_key: "usage:euw1:compute:fact".into(),
        intent_hash: [intent; 32],
    }
}

fn unresolved() -> UnknownCommit {
    match CommitDisposition::classify(
        CommitFailure::Unknown(DataApiError::Timeout),
        TransactionId::new("rds-tx-fixture"),
    )
    .expect("the transport named its transaction")
    {
        CommitDisposition::Unresolved(unknown) => unknown,
        CommitDisposition::NotApplied(cause) => {
            panic!("a lost commit response must not be classified as not applied: {cause}")
        }
    }
}

#[test]
fn a_lost_commit_response_becomes_an_explicit_unknown_outcome_carrying_its_transaction() {
    let unknown = unresolved();
    assert_eq!(unknown.transaction(), &TransactionId::new("rds-tx-fixture"));
    assert_eq!(unknown.cause(), &DataApiError::Timeout);
}

#[test]
fn every_lost_commit_response_is_unresolved_whatever_lost_it() {
    for cause in [
        DataApiError::Timeout,
        DataApiError::DeadlineExceeded,
        DataApiError::Throttled,
        DataApiError::TransactionNotFound,
        DataApiError::Unavailable {
            message: "connection reset".to_owned(),
        },
        DataApiError::Fatal {
            code: None,
            message: "unclassified".to_owned(),
        },
    ] {
        let disposition = CommitDisposition::classify(
            CommitFailure::Unknown(cause.clone()),
            TransactionId::new("rds-tx-fixture"),
        )
        .expect("the transport named its transaction");
        let CommitDisposition::Unresolved(unknown) = disposition else {
            panic!("`{cause}` lost the commit response and must stay unresolved");
        };
        assert_eq!(unknown.cause(), &cause);
    }
}

#[test]
fn a_rolled_back_commit_is_not_applied_and_never_becomes_an_unknown_outcome() {
    let disposition = CommitDisposition::classify(
        CommitFailure::RolledBack(DataApiError::Serialization),
        TransactionId::new("rds-tx-fixture"),
    )
    .expect("the transport named its transaction");
    // `resolve_unknown_commit` takes an `UnknownCommit`, and the only mint site
    // is the `Unknown` arm of `classify`. A rolled-back commit therefore cannot
    // reach the ambiguity path at all: it is a clean replay of the exact
    // transaction under the same business key.
    assert_eq!(
        disposition,
        CommitDisposition::NotApplied(DataApiError::Serialization)
    );
}

#[test]
fn an_unknown_commit_that_already_landed_returns_the_original_receipt_and_never_charges_again() {
    let landed = receipt(7);
    assert_eq!(
        resolve_unknown_commit(
            &unresolved(),
            CommitProbe::Committed(landed.clone()),
            [7; 32]
        ),
        Ok(CommitResolution::Original(landed))
    );
}

#[test]
fn an_unknown_commit_with_no_durable_row_replays_the_same_business_key() {
    assert_eq!(
        resolve_unknown_commit(&unresolved(), CommitProbe::Absent, [7; 32]),
        Ok(CommitResolution::CleanRetry)
    );
}

#[test]
fn an_unknown_commit_still_pending_is_retryable_without_minting_another_identity() {
    assert_eq!(
        resolve_unknown_commit(&unresolved(), CommitProbe::Pending, [7; 32]),
        Ok(CommitResolution::Retryable)
    );
}

#[test]
fn an_unknown_commit_under_a_different_intent_is_a_conflict_not_a_second_posting() {
    assert_eq!(
        resolve_unknown_commit(&unresolved(), CommitProbe::Committed(receipt(8)), [7; 32]),
        Err(UnknownCommitError::IntentConflict)
    );
}

#[test]
fn a_commit_failure_the_transport_did_not_name_cannot_be_reconciled() {
    for failure in [
        CommitFailure::Unknown(DataApiError::Timeout),
        CommitFailure::RolledBack(DataApiError::Serialization),
    ] {
        assert_eq!(
            CommitDisposition::classify(failure, TransactionId::new(String::new())),
            Err(UnknownCommitError::MissingTransportIdentity)
        );
    }
}
