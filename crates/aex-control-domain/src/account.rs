//! The one projection of an account's operational state.
//!
//! [`AccountProfile`] is the lossless public subset of `finance.account_state_v1`
//! — the projection finance's own pause and resume trigger writes — and it is the
//! **sole** input to every published [`AccountOperationalState`], on every plane
//! and every route. Nothing derives that state a second time from a balance, a
//! `BillingAccountState` or a placement row. Two producers of one fact is how the
//! same account came to be reported active by `dashboard_bootstrap_get` and
//! paused by `billing_balance_get` in the same second, and how finance came to
//! answer "top up" to an account under a dispute hold.
//!
//! The consequence is deliberate and is the correct semantics: a balance read can
//! show its own state lagging the number printed beside it by one trigger. The
//! pause is the trigger's fact; the number is live. Making the state balance-derived
//! instead would be unprojectable — a region cannot see finance's balance, so the
//! central and regional answers would diverge permanently by construction.

use aex_wire::models::{
    AccountActiveState, AccountOperationalState, AccountPauseReason, AccountPausedState,
};
use aex_wire::types::Timestamp;
use time::OffsetDateTime;

use crate::authz::AccountState;

/// The published account fact, exactly as `finance.account_state_v1` carries it.
///
/// The revocation epoch is deliberately **not** here. It is a
/// `control.authorization_epoch` row, it is projected onto the placement item
/// rather than published, and no route renders it — so it travels beside this
/// value in whatever record a reader needs both from, and never inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountProfile {
    /// Active or paused. An absent profile is unavailable and is not a value here.
    pub state: AccountState,
    /// Finance's stable state reason, present exactly when the account is paused.
    pub reason: Option<String>,
    /// Monotonic finance revision.
    pub revision: u64,
    /// When finance last changed the state.
    pub changed_at: OffsetDateTime,
}

/// Why an account is paused.
///
/// The spelling is the durable one: `finance.billing_account.state_reason` stores
/// exactly these four words and `finance.account_state_v1` republishes the column
/// as `reason`. It is the customer-facing **remedy**, which is a different fact
/// from the internal hold in `finance.billing_account.state` — two holds can share
/// one remedy, and the view collapses the four holds to one status on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccountPauseCause {
    /// The prepaid balance is exhausted; a top-up restores service.
    TopUpRequired,
    /// A charge is held pending payment; a top-up does not clear it.
    PaymentHold,
    /// A chargeback is open; nothing the customer pays restores service.
    DisputeHold,
    /// The account is closed. There is no remedy.
    AccountClosed,
}

impl AccountPauseCause {
    /// Every cause.
    pub const ALL: [Self; 4] = [
        Self::TopUpRequired,
        Self::PaymentHold,
        Self::DisputeHold,
        Self::AccountClosed,
    ];

    /// The durable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TopUpRequired => "top_up_required",
            Self::PaymentHold => "payment_hold",
            Self::DisputeHold => "dispute_hold",
            Self::AccountClosed => "account_closed",
        }
    }

    /// Resolves a durable spelling. An unrecognised reason is never a guess.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cause| cause.as_str() == text)
    }

    /// The published reason for this cause.
    ///
    /// Today `AccountPauseReason` carries one value, so all four collapse onto
    /// it. That collapse is the whole of the pending contract delta: when the
    /// published enum widens to `top_up_required | payment_hold | dispute_hold |
    /// account_closed` this becomes a four-arm identity and nothing else in the
    /// projection changes.
    #[must_use]
    pub const fn published(self) -> AccountPauseReason {
        match self {
            Self::TopUpRequired | Self::PaymentHold | Self::DisputeHold | Self::AccountClosed => {
                AccountPauseReason::TopUpRequired
            }
        }
    }
}

/// Why an account profile could not be projected onto the wire.
///
/// Every arm is a refusal. There is no arm that answers "active" for a fact the
/// projection could not establish, because an absence is never a permission.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountProjectionError {
    /// The state could not be established. Answers `503 account_state_unavailable`.
    #[error("the account state could not be established")]
    Unavailable,
    /// A paused account published no reason. The durable `CHECK` forbids it, so
    /// the row is corrupt rather than merely incomplete.
    #[error("a paused account published no reason")]
    MissingPauseCause,
    /// A paused account published a reason outside the durable vocabulary.
    #[error("`{0}` is not a published account pause reason")]
    UnknownPauseCause(String),
    /// The change instant does not fit the wire's millisecond timestamp.
    #[error("the account state instant is unrepresentable")]
    UnrepresentableInstant,
}

/// Projects one account profile onto the published operational state.
///
/// This is the only implementation. `central-identity-api` feeds it from Aurora
/// and the regional plane feeds it from the projected profile row; both get the
/// same value from the same mapping, and may differ only in staleness — never in
/// vocabulary, discriminator or derivation.
///
/// # Errors
///
/// Returns [`AccountProjectionError`] rather than an assumed state for every
/// input this projection cannot resolve.
pub fn account_operational_state(
    profile: &AccountProfile,
) -> Result<AccountOperationalState, AccountProjectionError> {
    let changed_at = Timestamp::from_datetime_trunc_ms(profile.changed_at)
        .map_err(|_| AccountProjectionError::UnrepresentableInstant)?;
    match profile.state {
        AccountState::Active => Ok(AccountOperationalState::Active(AccountActiveState {
            changed_at,
            revision: profile.revision,
        })),
        AccountState::PausedTopUpRequired => {
            let raw = profile
                .reason
                .as_deref()
                .ok_or(AccountProjectionError::MissingPauseCause)?;
            let cause = AccountPauseCause::parse(raw)
                .ok_or_else(|| AccountProjectionError::UnknownPauseCause(raw.to_owned()))?;
            Ok(AccountOperationalState::Paused(AccountPausedState {
                changed_at,
                // Deletion scheduling and retention funding are the regional
                // content lifecycle's facts. Both are published and permanently
                // empty by owner decision; nothing on either plane may guess one.
                deletion_scheduled_at: None,
                // The smallest restoring top-up is a money fact, and the profile
                // is the sole input to this projection by decision. The field
                // therefore stays empty until the profile carries the amount —
                // see the contract delta that makes it required for
                // `top_up_required`.
                minimum_restore_cents: None,
                reason: cause.published(),
                retention_funded_until: None,
                revision: profile.revision,
            }))
        }
        AccountState::Unavailable => Err(AccountProjectionError::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountPauseCause, AccountProfile, AccountProjectionError, AccountState};
    use time::OffsetDateTime;

    fn profile(state: AccountState, reason: Option<&str>) -> AccountProfile {
        AccountProfile {
            state,
            reason: reason.map(str::to_owned),
            revision: 7,
            changed_at: OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("an instant"),
        }
    }

    #[test]
    fn the_durable_vocabulary_round_trips_and_admits_nothing_else() {
        for cause in AccountPauseCause::ALL {
            assert_eq!(AccountPauseCause::parse(cause.as_str()), Some(cause));
        }
        assert_eq!(AccountPauseCause::parse("credit_exhausted"), None);
        assert_eq!(AccountPauseCause::parse(""), None);
    }

    #[test]
    fn a_paused_account_with_an_unknown_reason_is_refused_not_guessed() {
        let error = super::account_operational_state(&profile(
            AccountState::PausedTopUpRequired,
            Some("fixture"),
        ))
        .expect_err("an unpublished reason is a refusal");
        assert!(matches!(
            error,
            AccountProjectionError::UnknownPauseCause(reason) if reason == "fixture"
        ));
        assert_eq!(
            super::account_operational_state(&profile(AccountState::PausedTopUpRequired, None)),
            Err(AccountProjectionError::MissingPauseCause)
        );
        assert_eq!(
            super::account_operational_state(&profile(AccountState::Unavailable, None)),
            Err(AccountProjectionError::Unavailable)
        );
    }

    #[test]
    fn every_hold_projects_and_none_of_them_is_active() {
        use aex_wire::models::AccountOperationalState;
        for cause in AccountPauseCause::ALL {
            let state = super::account_operational_state(&profile(
                AccountState::PausedTopUpRequired,
                Some(cause.as_str()),
            ))
            .expect("a declared cause projects");
            assert!(
                matches!(state, AccountOperationalState::Paused(_)),
                "{cause:?} must never read as active"
            );
        }
        assert!(matches!(
            super::account_operational_state(&profile(AccountState::Active, None))
                .expect("an active account projects"),
            AccountOperationalState::Active(_)
        ));
    }
}
