//! Closed finance account taxonomy.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The only journal currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Currency {
    /// United States dollar, accounted in micro-USD.
    #[serde(rename = "USD")]
    Usd,
}

/// Presentation side for an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountSide {
    /// Debit-normal.
    Debit,
    /// Credit-normal.
    Credit,
}

/// The closed v1 chart of accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// Funds accepted by the provider.
    ProviderClearing,
    /// Provider processing fees.
    ProcessorFeeExpense,
    /// An organization's spendable prepaid liability.
    CustomerAvailable,
    /// An organization's reserved prepaid liability.
    CustomerReserved,
    /// Earned infrastructure revenue.
    UsageRevenue,
    /// Tax collected for remittance.
    TaxPayable,
    /// Refund funds in flight.
    RefundClearing,
    /// Disputed funds in flight.
    DisputeHolding,
    /// Dispute loss without customer credit.
    DisputeLossExpense,
    /// Operator-granted credit expense.
    GoodwillExpense,
}

impl AccountKind {
    /// Whether the account must be scoped to an organization.
    #[must_use]
    pub const fn is_customer(self) -> bool {
        matches!(self, Self::CustomerAvailable | Self::CustomerReserved)
    }

    /// Normal presentation side.
    #[must_use]
    pub const fn normal_side(self) -> AccountSide {
        match self {
            Self::CustomerAvailable
            | Self::CustomerReserved
            | Self::UsageRevenue
            | Self::TaxPayable => AccountSide::Credit,
            Self::ProviderClearing
            | Self::ProcessorFeeExpense
            | Self::RefundClearing
            | Self::DisputeHolding
            | Self::DisputeLossExpense
            | Self::GoodwillExpense => AccountSide::Debit,
        }
    }
}

/// Stable reference to one account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AccountRef {
    id: Uuid,
    kind: AccountKind,
}

impl AccountRef {
    /// Constructs an account reference.
    #[must_use]
    pub const fn new(id: Uuid, kind: AccountKind) -> Self {
        Self { id, kind }
    }

    /// Account identity.
    #[must_use]
    pub const fn id(self) -> Uuid {
        self.id
    }

    /// Account taxonomy entry.
    #[must_use]
    pub const fn kind(self) -> AccountKind {
        self.kind
    }
}
