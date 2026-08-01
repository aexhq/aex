use aex_finance_domain::{Microusd, MicrousdDelta};

use crate::row::{
    DATA_API_MAX_ROW_BYTES, RowDecodeError, RowPage, bigint_delta, bigint_money, numeric_u128,
};
use crate::tx::{CommitProbe, CommitResolution, PostedReceipt, resolve_unknown_commit};
use crate::wire_pending::{CommitOutcomeUnknown, RdsField};

#[test]
fn money_decodes_only_from_integer_transport_shapes() {
    assert_eq!(
        bigint_money(&RdsField::Long(42)),
        Ok(Microusd::new(42).expect("bound"))
    );
    assert_eq!(
        bigint_money(&RdsField::String("42".into())),
        Ok(Microusd::new(42).expect("bound"))
    );
    assert_eq!(
        bigint_delta(&RdsField::String("-42".into())),
        Ok(MicrousdDelta::new(-42).expect("bound"))
    );
    assert_eq!(
        bigint_money(&RdsField::DoublePresent),
        Err(RowDecodeError::FloatInMoneyColumn)
    );
    assert_eq!(
        numeric_u128(&RdsField::Long(42)),
        Err(RowDecodeError::QuantityNotString)
    );
    assert_eq!(
        numeric_u128(&RdsField::String("9223372036854775808".into())),
        Ok(9_223_372_036_854_775_808)
    );
    for text in ["+1", "01", "-0", "1.0", "1e3", ""] {
        assert!(
            numeric_u128(&RdsField::String(text.into())).is_err(),
            "accepted {text}"
        );
    }
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

#[test]
fn unknown_commit_requeries_the_same_business_key_and_never_mints_another() {
    let unknown = CommitOutcomeUnknown {
        transaction_id: "rds-tx-fixture".into(),
    };
    let receipt = PostedReceipt {
        transaction_id: "txn_fixture".into(),
        business_key: "usage:euw1:compute:fact".into(),
        intent_hash: [7; 32],
    };
    assert_eq!(
        resolve_unknown_commit(&unknown, CommitProbe::Committed(receipt.clone()), [7; 32]),
        Ok(CommitResolution::Original(receipt))
    );
    assert_eq!(
        resolve_unknown_commit(&unknown, CommitProbe::Absent, [7; 32]),
        Ok(CommitResolution::CleanRetry)
    );
    assert_eq!(
        resolve_unknown_commit(&unknown, CommitProbe::Pending, [7; 32]),
        Ok(CommitResolution::Retryable)
    );
    let conflict = PostedReceipt {
        transaction_id: "txn_fixture".into(),
        business_key: "usage:euw1:compute:fact".into(),
        intent_hash: [8; 32],
    };
    assert!(resolve_unknown_commit(&unknown, CommitProbe::Committed(conflict), [7; 32]).is_err());
}

#[test]
fn adapter_sql_never_casts_money_to_float() {
    let source = include_str!("store.rs");
    assert!(!source.contains("::float8"));
    assert!(!source.contains("doubleValue"));
    assert!(source.contains("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"));
}
