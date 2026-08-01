//! Public integer-only row-codec conformance evidence.

use aex_finance_aurora::row::{RowDecodeError, bigint_delta, bigint_money, numeric_u128};
use aex_rds_data::Record;
use aws_sdk_rdsdata::types::Field;

#[test]
fn public_row_codec_rejects_every_floating_money_shape() {
    let fields = [Field::DoubleValue(1.5)];
    let record = Record::new(&fields);
    assert_eq!(
        bigint_money(&record, 0),
        Err(RowDecodeError::FloatInMoneyColumn)
    );
    assert_eq!(
        bigint_delta(&record, 0),
        Err(RowDecodeError::FloatInMoneyColumn)
    );
    assert_eq!(
        numeric_u128(&record, 0),
        Err(RowDecodeError::FloatInMoneyColumn)
    );
}
