//! Public balanced-journal conservation evidence.

use aex_finance_domain::{
    AccountKind, AccountRef, BalancedTransaction, BusinessKey, IntentHash, MicrousdDelta, Posting,
    TransactionId, TransactionKind,
};
use time::OffsetDateTime;
use uuid::Uuid;

#[test]
fn public_constructor_and_reversal_conserve_every_sample_amount() {
    for amount in [1, 10_000, 1_000_000, MicrousdDelta::MAX_ABS] {
        let left = AccountRef::new(Uuid::from_u128(1), AccountKind::ProviderClearing);
        let right = AccountRef::new(Uuid::from_u128(2), AccountKind::CustomerAvailable);
        let transaction = BalancedTransaction::try_new(
            TransactionId::from_uuid(Uuid::from_u128(10)),
            TransactionKind::TopUpSettled,
            None,
            BusinessKey::parse("topup:fixture").expect("business key"),
            IntentHash::new([7; 32]),
            OffsetDateTime::UNIX_EPOCH,
            vec![
                Posting {
                    account: left,
                    amount: MicrousdDelta::new(amount).expect("positive amount"),
                },
                Posting {
                    account: right,
                    amount: MicrousdDelta::new(-amount).expect("negative amount"),
                },
            ],
        )
        .expect("balanced transaction");
        let reversal = transaction
            .reverse(
                TransactionId::from_uuid(Uuid::from_u128(11)),
                OffsetDateTime::UNIX_EPOCH,
            )
            .expect("exact reversal");
        assert_eq!(
            transaction.net_for(&left).get() + reversal.net_for(&left).get(),
            0
        );
        assert_eq!(
            transaction.net_for(&right).get() + reversal.net_for(&right).get(),
            0
        );
    }
}
