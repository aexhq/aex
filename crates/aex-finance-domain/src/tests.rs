use std::collections::BTreeMap;

use proptest::prelude::*;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::account::{AccountKind, AccountRef};
use crate::billing_account::{
    AdmissionDecision, AutoTopUpDecision, AutoTopUpHistory, BillingAccount, BillingAccountState,
    BlockReason, admit, auto_topup_decision,
};
use crate::effect::{
    EffectKind, EffectOutcome, EffectState, EffectTransitionError, IndeterminateReason,
    ProviderEffect, RecoveryAction, recovery_action, transition,
};
use crate::journal::{
    BalancedTransaction, BusinessKey, ConservationError, IntentHash, Posting, TransactionId,
    TransactionKind,
};
use crate::money::{Cents, Microusd, MicrousdDelta, MoneyError};
use crate::reservation::{Closure, Reservation, ReservationError, ReservationState};

#[test]
fn money_rejects_noncanonical_and_out_of_domain_values() {
    for invalid in [
        "1.0000001",
        "1e3",
        "+1",
        "01",
        "-0",
        "−0",
        "NaN",
        "1,000",
        "USD 1",
        "1 EUR",
        "",
    ] {
        assert!(
            Cents::parse_decimal(invalid).is_err(),
            "accepted {invalid:?}"
        );
        assert!(
            Microusd::parse_canonical(invalid).is_err(),
            "accepted {invalid:?}"
        );
    }
    assert_eq!(Cents::new(-1), Err(MoneyError::Negative));
    assert_eq!(Microusd::new(-1), Err(MoneyError::Negative));
    assert!(Cents::new(Cents::MAX_RAW + 1).is_err());
    assert!(Microusd::new(Microusd::MAX_RAW + 1).is_err());
    assert!(MicrousdDelta::new(MicrousdDelta::MAX_ABS + 1).is_err());
}

proptest! {
    #[test]
    fn cent_microusd_round_trip_is_exact(value in 0_i64..=Cents::MAX_RAW) {
        let cents = Cents::new(value).expect("generated bound");
        prop_assert_eq!(cents.to_microusd().to_cents_exact(), Ok(cents));
    }

    #[test]
    fn journal_accepts_exactly_balanced_vectors(
        amounts in prop::collection::vec(-1_000_000_i64..=1_000_000, 0..16)
    ) {
        let postings: Vec<_> = amounts.iter().enumerate().map(|(index, amount)| Posting {
            account: account(index as u128 + 1, AccountKind::UsageRevenue),
            amount: MicrousdDelta::new(*amount).expect("generated bound"),
        }).collect();
        let result = transaction(postings);
        let sum = amounts.iter().try_fold(0_i64, |sum, amount| sum.checked_add(*amount));
        let should_succeed = amounts.len() >= 2
            && amounts.iter().all(|amount| *amount != 0)
            && sum == Some(0);
        prop_assert_eq!(result.is_ok(), should_succeed);
    }

    #[test]
    fn double_reversal_restores_every_account(amount in 1_i64..1_000_000) {
        let original = transaction(vec![
            Posting { account: account(1, AccountKind::ProviderClearing), amount: delta(amount) },
            Posting { account: account(2, AccountKind::UsageRevenue), amount: delta(-amount) },
        ]).expect("balanced");
        let reversed = original.reverse(TransactionId::from_uuid(Uuid::from_u128(10)), at()).expect("reverse");
        let restored = reversed.reverse(TransactionId::from_uuid(Uuid::from_u128(11)), at()).expect("reverse twice");
        for posting in original.postings() {
            prop_assert_eq!(original.net_for(&posting.account), restored.net_for(&posting.account));
        }
    }
}

#[test]
fn every_conservation_error_is_reachable() {
    assert!(matches!(
        transaction(vec![]),
        Err(ConservationError::TooFewPostings(0))
    ));
    assert!(matches!(
        transaction(vec![posting(1, 1), posting(2, -2)]),
        Err(ConservationError::Unbalanced {
            imbalance_microusd: -1
        })
    ));
    assert!(matches!(
        transaction(vec![posting(1, 0), posting(2, 0)]),
        Err(ConservationError::ZeroAmount { seq: 0 })
    ));
    assert!(matches!(
        transaction(vec![posting(1, 1), posting(1, -1)]),
        Err(ConservationError::DuplicateAccount(_))
    ));
}

#[test]
fn reservation_histories_refuse_over_settlement_and_double_release() {
    let reservation = Reservation::open(Microusd::new(100).expect("bound"));
    assert_eq!(
        reservation.apply_settlement(Microusd::new(101).expect("bound")),
        Err(ReservationError::OverSettlement)
    );
    let closing = reservation.begin_close().expect("open closes");
    let closure = Closure::new(BTreeMap::from([("compute".into(), false)]));
    assert_eq!(
        closing.apply_release(&closure),
        Err(ReservationError::ClosureUnsatisfied)
    );
    let closure = closure.satisfy("compute", false).expect("declared");
    let (released, amount) = closing.apply_release(&closure).expect("closed release");
    assert_eq!(released.state(), ReservationState::Released);
    assert_eq!(amount, Microusd::new(100).expect("bound"));
    assert_eq!(
        released.apply_release(&closure),
        Err(ReservationError::InvalidState)
    );
}

#[test]
fn effect_5xx_is_unknown_and_recovery_stops_at_twelve_hours() {
    let now = at();
    let effect = ProviderEffect::prepare(EffectKind::OffSessionCharge, now);
    let dispatched = effect.dispatch(now).expect("prepared dispatches");
    let unknown = transition(
        &dispatched,
        EffectOutcome::Indeterminate(IndeterminateReason::Provider5xx),
        now,
    )
    .expect("5xx is represented");
    assert_eq!(unknown.state(), EffectState::OutcomeUnknown);
    assert_eq!(
        recovery_action(
            &unknown,
            now + Duration::hours(12) - Duration::milliseconds(1)
        ),
        RecoveryAction::RetryExactKey
    );
    assert_eq!(
        recovery_action(
            &unknown,
            now + Duration::hours(12) + Duration::milliseconds(1)
        ),
        RecoveryAction::EscalateManualReview
    );
    assert_eq!(
        transition(
            &unknown,
            EffectOutcome::Indeterminate(IndeterminateReason::Timeout),
            now
        ),
        Err(EffectTransitionError::InvalidTransition)
    );
}

#[test]
fn admission_and_auto_topup_guards_fail_closed() {
    let mut account = BillingAccount::active();
    let zero = Microusd::ZERO;
    assert_eq!(
        admit(&account, zero, zero),
        AdmissionDecision::Block(BlockReason::InsufficientCredit)
    );
    account.default_payment_method = true;
    account.auto_topup_enabled = true;
    account.auto_topup_amount = Microusd::new(10_000_000).expect("minimum");
    account.spend_cap = Some(Microusd::new(20_000_000).expect("bound"));
    assert_eq!(
        auto_topup_decision(&account, zero, &AutoTopUpHistory::default(), at()),
        AutoTopUpDecision::Charge(account.auto_topup_amount)
    );
    account.state = BillingAccountState::DisputeHold;
    assert_eq!(
        admit(&account, Microusd::new(1).expect("bound"), zero),
        AdmissionDecision::Block(BlockReason::AccountState)
    );
}

fn at() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}

fn account(id: u128, kind: AccountKind) -> AccountRef {
    AccountRef::new(Uuid::from_u128(id), kind)
}

fn delta(value: i64) -> MicrousdDelta {
    MicrousdDelta::new(value).expect("fixture bound")
}

fn posting(id: u128, amount: i64) -> Posting {
    Posting {
        account: account(id, AccountKind::UsageRevenue),
        amount: delta(amount),
    }
}

fn transaction(postings: Vec<Posting>) -> Result<BalancedTransaction, ConservationError> {
    BalancedTransaction::try_new(
        TransactionId::from_uuid(Uuid::from_u128(9)),
        TransactionKind::UsageSettlement,
        None,
        BusinessKey::parse("usage:eu-west-1:compute:usage_0123456789abcdef").expect("valid key"),
        IntentHash::new([7; 32]),
        at(),
        postings,
    )
}
