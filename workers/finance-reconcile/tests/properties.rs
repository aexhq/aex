//! Generated properties over unknown-effect recovery.
//!
//! The claim that matters: inside the replay window an unresolved effect is
//! recovered by presenting the *same* idempotency key, and only after the
//! window does it become an operator's problem. A boundary that drifts by one
//! millisecond in the wrong direction is a second charge.

use aex_finance_domain::effect::{
    EffectKind, EffectOutcome, IndeterminateReason, ProviderEffect, RecoveryAction,
    recovery_action, transition,
};
use proptest::prelude::*;
use time::{Duration, OffsetDateTime};

/// The window F-15 pins: half of what the provider guarantees.
const WINDOW_HOURS: i64 = 12;

/// An effect whose outcome the provider never confirmed.
fn unresolved(at: OffsetDateTime) -> ProviderEffect {
    let dispatched = ProviderEffect::prepare(EffectKind::CheckoutSessionCreate, at)
        .dispatch(at)
        .expect("a prepared effect may dispatch");
    transition(
        &dispatched,
        EffectOutcome::Indeterminate(IndeterminateReason::Timeout),
        at,
    )
    .expect("an indeterminate observation is a legal edge")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Inside the window the recovery is always an exact-key replay.
    #[test]
    fn inside_the_window_recovery_replays_the_same_key(elapsed_ms in 0i64..(WINDOW_HOURS * 3_600_000 - 1)) {
        let dispatched_at = OffsetDateTime::UNIX_EPOCH;
        let effect = unresolved(dispatched_at);
        let now = dispatched_at + Duration::milliseconds(elapsed_ms);
        prop_assert_eq!(recovery_action(&effect, now), RecoveryAction::RetryExactKey);
    }

    /// Outside the window automatic replay stops; the effect is looked up.
    #[test]
    fn outside_the_window_recovery_is_never_a_blind_replay(
        elapsed_ms in (WINDOW_HOURS * 3_600_000)..(WINDOW_HOURS * 3_600_000 * 8),
    ) {
        let dispatched_at = OffsetDateTime::UNIX_EPOCH;
        let effect = unresolved(dispatched_at);
        let now = dispatched_at + Duration::milliseconds(elapsed_ms);
        prop_assert_ne!(recovery_action(&effect, now), RecoveryAction::RetryExactKey);
    }
}

#[test]
fn the_window_boundary_is_exact_to_the_millisecond() {
    let dispatched_at = OffsetDateTime::UNIX_EPOCH;
    let effect = unresolved(dispatched_at);
    let window = Duration::hours(WINDOW_HOURS);
    assert_eq!(
        recovery_action(&effect, dispatched_at + window - Duration::milliseconds(1)),
        RecoveryAction::RetryExactKey,
        "one millisecond before the boundary the key is still replayable"
    );
    assert_ne!(
        recovery_action(&effect, dispatched_at + window + Duration::milliseconds(1)),
        RecoveryAction::RetryExactKey,
        "one millisecond after the boundary a blind replay is unsafe"
    );
}
