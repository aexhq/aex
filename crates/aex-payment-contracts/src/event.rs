//! The verified webhook handoff.
//!
//! Exactly seven pieces of information cross this boundary, and the facts union
//! is closed per event kind. That is what makes the redaction test decidable: it
//! is not "the Stripe object minus some fields", it is a fixed shape that has no
//! room for a raw payload, a card detail, a customer secret or a payment URL.

use aex_internal_contracts::SchemaVersion;
use aex_wire::idempotency::{IdempotencyKey, IntentDigest};
use aex_wire::ids::ContentHash;
use aex_wire::ids::OrganizationId;
use aex_wire::types::{Cents, Timestamp};
use serde::{Deserialize, Serialize};

use crate::ProviderObjectRef;
use crate::command::EffectId;
use crate::result::PaymentFailure;

/// The provider identity of one delivered event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderEventId(pub String);

/// The pinned provider API version an event was rendered under.
///
/// Opaque here on purpose: the actual pin is release configuration, and putting
/// the literal in the contract would make a provider version bump a contract
/// change.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PinnedApiVersion(pub String);

/// The five provider events AEX acts on.
///
/// The enum is closed. An unlisted signed type is acknowledged with a bounded
/// metric at the edge and never forwarded, because forwarding an event nothing
/// downstream understands is how an inbox grows a silent backlog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEventKind {
    /// A payment intent succeeded; this is where credit is applied.
    PaymentIntentSucceeded,
    /// A payment intent failed.
    PaymentIntentPaymentFailed,
    /// A dispute was opened against a charge.
    ChargeDisputeCreated,
    /// A refund was created.
    RefundCreated,
    /// A refund failed.
    RefundFailed,
}

/// The exact money and status facts each event kind carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ProviderEventFacts {
    /// A payment intent succeeded.
    PaymentIntentSucceeded {
        /// The account.
        organization: OrganizationId,
        /// How much credit to apply.
        credit: Cents,
        /// How much was charged.
        charged: Cents,
        /// The intent object.
        intent: ProviderObjectRef,
    },
    /// A payment intent failed.
    PaymentIntentPaymentFailed {
        /// The account.
        organization: OrganizationId,
        /// The intent object.
        intent: ProviderObjectRef,
        /// Why it failed.
        failure: PaymentFailure,
    },
    /// A dispute was opened.
    ChargeDisputeCreated {
        /// The account.
        organization: OrganizationId,
        /// The disputed charge.
        charge: ProviderObjectRef,
        /// The disputed amount.
        amount: Cents,
    },
    /// A refund was created.
    RefundCreated {
        /// The account.
        organization: OrganizationId,
        /// The refunded charge.
        charge: ProviderObjectRef,
        /// The refund object.
        refund: ProviderObjectRef,
        /// The refunded amount.
        amount: Cents,
    },
    /// A refund failed.
    RefundFailed {
        /// The account.
        organization: OrganizationId,
        /// The charge.
        charge: ProviderObjectRef,
        /// The refund object.
        refund: ProviderObjectRef,
        /// The amount.
        amount: Cents,
    },
}

impl ProviderEventFacts {
    /// Which event kind these facts belong to.
    #[must_use]
    pub const fn kind(&self) -> ProviderEventKind {
        match self {
            Self::PaymentIntentSucceeded { .. } => ProviderEventKind::PaymentIntentSucceeded,
            Self::PaymentIntentPaymentFailed { .. } => {
                ProviderEventKind::PaymentIntentPaymentFailed
            }
            Self::ChargeDisputeCreated { .. } => ProviderEventKind::ChargeDisputeCreated,
            Self::RefundCreated { .. } => ProviderEventKind::RefundCreated,
            Self::RefundFailed { .. } => ProviderEventKind::RefundFailed,
        }
    }

    /// The account the event is about.
    #[must_use]
    pub const fn organization(&self) -> OrganizationId {
        match self {
            Self::PaymentIntentSucceeded { organization, .. }
            | Self::PaymentIntentPaymentFailed { organization, .. }
            | Self::ChargeDisputeCreated { organization, .. }
            | Self::RefundCreated { organization, .. }
            | Self::RefundFailed { organization, .. } => *organization,
        }
    }
}

/// One verified provider event, reduced to exactly what finance needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderEventEnvelope {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The provider identity, for inbox deduplication.
    pub provider_event_id: ProviderEventId,
    /// The object the event is about.
    pub object: ProviderObjectRef,
    /// Which event this is.
    pub kind: ProviderEventKind,
    /// When the provider says it happened.
    pub occurred_at: Timestamp,
    /// The exact money and status facts.
    pub facts: ProviderEventFacts,
    /// The digest of the verified raw body, never the body itself.
    pub raw_digest: ContentHash,
    /// Which provider API version rendered it.
    pub provider_api_version: PinnedApiVersion,
    /// The AEX effect it settles, when the object carried one.
    pub effect: Option<EffectId>,
    /// When the edge received it.
    pub received_at: Timestamp,
}

/// The application identity of one delivered event.
///
/// Event-id uniqueness alone is not enough: the same logical outcome can arrive
/// under two event ids, so the object identity travels with it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderInboxKey {
    /// The provider event identity.
    pub provider_event_id: ProviderEventId,
    /// The object the event is about.
    pub object: ProviderObjectRef,
}

/// A replay identity carried across the finance path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IdempotencyIntent {
    /// The caller-chosen key.
    pub key: IdempotencyKey,
    /// The canonical intent it was bound to.
    pub intent: IntentDigest,
}
