//! `aex-payment-contracts` is the command, result and event boundary between the
//! two minimal `TypeScript` Stripe edges and the Rust finance path.
//!
//! It is the one process boundary that is not HTTP-contract-generated where Rust
//! and `TypeScript` must agree byte for byte.
//!
//! # Invariants
//!
//! - the edge can perform no provider call outside [`PaymentCommand`];
//! - the provider idempotency key is **derived**, never free-form, so "no caller
//!   invents a key" is a type-level fact rather than a convention;
//! - a Stripe 5xx is outcome-indeterminate and becomes
//!   [`PaymentResult::Unknown`], never [`PaymentResult::Failed`]. Collapsing the
//!   two is exactly how an account gets charged twice;
//! - no PAN, CVC, provider secret, billing address, hosted payment URL or raw
//!   provider message is representable in a loggable field, asserted by
//!   `tests/redaction.rs`;
//! - money is `Cents` only; a source-scanning test proves the crate contains no
//!   `f32` and no `f64`.
//!
//! # Not this crate's job
//!
//! - talking to Stripe: the pinned provider protocol lives in the edge;
//! - money arithmetic, balances or the finance state machine;
//! - webhook signature verification.

pub mod command;
pub mod event;
pub mod result;

pub use command::{
    CommandKind, EffectId, EffectMetadata, PaymentCommand, PaymentCommandEnvelope,
    ProviderIdempotencyKey,
};
pub use event::{
    PinnedApiVersion, ProviderEventEnvelope, ProviderEventFacts, ProviderEventId,
    ProviderEventKind, ProviderInboxKey,
};
pub use result::{
    EffectState, HostedSession, PaymentFailure, PaymentFailureClass, PaymentResult, TaxEvidence,
    TaxMode, UnknownEvidence,
};

use serde::{Deserialize, Serialize};

/// An opaque provider object reference, such as a Stripe object id.
///
/// Carried verbatim so reconciliation can look it up, and never parsed, because
/// parsing it would make the shape of a provider id part of the AEX contract.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderObjectRef(pub String);

/// A reference to the provider customer record for one organization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderCustomerRef(pub String);

/// A reference to a saved payment method.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderMethodRef(pub String);

/// A reference to a provider charge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderChargeRef(pub String);

/// An email address that never renders in full.
///
/// The provider needs one to create a customer; nothing downstream needs to read
/// it back, so `Debug` shows only the domain.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RedactedEmail(String);

impl RedactedEmail {
    /// Wraps an address.
    #[must_use]
    pub fn new(address: impl Into<String>) -> Self {
        Self(address.into())
    }

    /// The address, for the one call that has to send it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RedactedEmail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let domain = self.0.split_once('@').map_or("", |(_, domain)| domain);
        write!(formatter, "RedactedEmail(<redacted>@{domain})")
    }
}
