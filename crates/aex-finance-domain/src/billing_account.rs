//! Billing admission and automatic top-up runaway guards.

use time::{Duration, OffsetDateTime};

use crate::money::Microusd;

/// Billing account operational state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingAccountState {
    /// Normal operation.
    Active,
    /// Payment problem.
    PaymentHold,
    /// Open dispute.
    DisputeHold,
    /// Permanently closed.
    Closed,
}

/// Billing policy needed for admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillingAccount {
    /// Operational state.
    pub state: BillingAccountState,
    /// Whether a reusable payment method exists.
    pub default_payment_method: bool,
    /// Automatic recharge enabled.
    pub auto_topup_enabled: bool,
    /// Balance threshold that permits recharge.
    pub auto_topup_threshold: Microusd,
    /// Recharge amount, with a $10 minimum enforced by decision logic and SQL.
    pub auto_topup_amount: Microusd,
    /// Optional spend cap for the current period.
    pub spend_cap: Option<Microusd>,
}

impl BillingAccount {
    /// Active account with top-up disabled.
    #[must_use]
    pub const fn active() -> Self {
        Self {
            state: BillingAccountState::Active,
            default_payment_method: false,
            auto_topup_enabled: false,
            auto_topup_threshold: Microusd::ZERO,
            auto_topup_amount: Microusd::ZERO,
            spend_cap: None,
        }
    }
}

/// Admission result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Existing prepaid credit covers admission.
    Admit,
    /// Admission is fenced.
    Block(BlockReason),
}

/// Admission fence reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Account not active.
    AccountState,
    /// No spendable credit remains.
    InsufficientCredit,
}

/// Decides admission solely from durable state.
#[must_use]
pub fn admit(
    account: &BillingAccount,
    available: Microusd,
    reserved: Microusd,
) -> AdmissionDecision {
    if account.state != BillingAccountState::Active {
        AdmissionDecision::Block(BlockReason::AccountState)
    } else if available.get() <= reserved.get() {
        AdmissionDecision::Block(BlockReason::InsufficientCredit)
    } else {
        AdmissionDecision::Admit
    }
}

/// Durable charge history used by the four runaway guards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoTopUpHistory {
    /// Succeeded charges inside the rolling 24-hour window.
    pub succeeded_last_24h: u8,
    /// Last succeeded automatic charge.
    pub last_succeeded_at: Option<OffsetDateTime>,
    /// Already spent in the bounded period.
    pub spent_this_period: Microusd,
    /// Whether another unresolved charge exists.
    pub has_open_charge: bool,
}

/// Automatic recharge result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoTopUpDecision {
    /// Dispatch this exact amount.
    Charge(Microusd),
    /// No charge is allowed.
    DoNotCharge(AutoTopUpBlock),
}

/// Runaway guard that blocked recharge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoTopUpBlock {
    /// Account or policy disabled.
    Disabled,
    /// No reusable payment method.
    MissingPaymentMethod,
    /// Amount is below $10.
    BelowMinimum,
    /// One unresolved charge already exists.
    OpenCharge,
    /// Four charges succeeded in the rolling day.
    DailyCount,
    /// Last success was less than 60 seconds ago.
    Spacing,
    /// Spend cap would be exceeded.
    SpendCap,
    /// Available amount is still above threshold.
    AboveThreshold,
}

/// Applies every automatic top-up guard without fallback.
#[must_use]
pub fn auto_topup_decision(
    account: &BillingAccount,
    available: Microusd,
    history: &AutoTopUpHistory,
    now: OffsetDateTime,
) -> AutoTopUpDecision {
    if account.state != BillingAccountState::Active || !account.auto_topup_enabled {
        return AutoTopUpDecision::DoNotCharge(AutoTopUpBlock::Disabled);
    }
    if !account.default_payment_method {
        return AutoTopUpDecision::DoNotCharge(AutoTopUpBlock::MissingPaymentMethod);
    }
    if available.get() > account.auto_topup_threshold.get() {
        return AutoTopUpDecision::DoNotCharge(AutoTopUpBlock::AboveThreshold);
    }
    if account.auto_topup_amount.get() < 10_000_000 {
        return AutoTopUpDecision::DoNotCharge(AutoTopUpBlock::BelowMinimum);
    }
    if history.has_open_charge {
        return AutoTopUpDecision::DoNotCharge(AutoTopUpBlock::OpenCharge);
    }
    if history.succeeded_last_24h >= 4 {
        return AutoTopUpDecision::DoNotCharge(AutoTopUpBlock::DailyCount);
    }
    if history
        .last_succeeded_at
        .is_some_and(|last| now - last < Duration::seconds(60))
    {
        return AutoTopUpDecision::DoNotCharge(AutoTopUpBlock::Spacing);
    }
    if account.spend_cap.is_some_and(|cap| {
        history.spent_this_period.get() + account.auto_topup_amount.get() > cap.get()
    }) {
        return AutoTopUpDecision::DoNotCharge(AutoTopUpBlock::SpendCap);
    }
    AutoTopUpDecision::Charge(account.auto_topup_amount)
}
