//! Public integer-only row-codec conformance evidence.

use aex_finance_aurora::row::{RowDecodeError, bigint_money, numeric_u128};
use aex_finance_aurora::wire_pending::RdsField;

#[test]
fn public_row_codec_rejects_every_floating_money_shape() {
    assert_eq!(
        bigint_money(&RdsField::DoublePresent),
        Err(RowDecodeError::FloatInMoneyColumn)
    );
    assert_eq!(
        numeric_u128(&RdsField::DoublePresent),
        Err(RowDecodeError::FloatInMoneyColumn)
    );
}
