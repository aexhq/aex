//! The allowlist of provider effects.
//!
//! An effect is prepared and committed by finance *before* the edge is asked to
//! execute it, so the edge is a transport with no discretion. That is what makes
//! "the edge did something finance did not authorise" unrepresentable rather
//! than merely unlikely.

use aex_internal_contracts::SchemaVersion;
use aex_wire::Uuid7;
use aex_wire::ids::{IdText, OrganizationId};
use aex_wire::types::{Cents, HttpsUrl, Timestamp};
use serde::{Deserialize, Serialize};

use crate::result::TaxMode;
use crate::{
    ProviderChargeRef, ProviderCustomerRef, ProviderMethodRef, ProviderObjectRef, RedactedEmail,
};

/// An AEX-admitted provider effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectId(pub Uuid7);

/// Which command an effect carries. Used in the derived idempotency key, so the
/// spelling is part of the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    /// Ensure the provider customer record exists.
    EnsureCustomer,
    /// Create a hosted top-up checkout.
    CreateTopUpCheckout,
    /// Create a hosted billing portal session.
    CreatePortalSession,
    /// Charge a saved payment method.
    ChargeSavedMethod,
    /// Look up the outcome of an effect that ended indeterminate.
    LookupEffectOutcome,
    /// Refund a charge.
    RefundCharge,
}

impl CommandKind {
    /// Every command kind.
    pub const ALL: [Self; 6] = [
        Self::EnsureCustomer,
        Self::CreateTopUpCheckout,
        Self::CreatePortalSession,
        Self::ChargeSavedMethod,
        Self::LookupEffectOutcome,
        Self::RefundCharge,
    ];

    /// The stable spelling used in the derived idempotency key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EnsureCustomer => "ensure_customer",
            Self::CreateTopUpCheckout => "create_top_up_checkout",
            Self::CreatePortalSession => "create_portal_session",
            Self::ChargeSavedMethod => "charge_saved_method",
            Self::LookupEffectOutcome => "lookup_effect_outcome",
            Self::RefundCharge => "refund_charge",
        }
    }
}

/// The provider idempotency key.
///
/// Derived as `sha256("aex:" ‖ kind ‖ ":" ‖ organization ‖ ":" ‖ effect)`. There
/// is no constructor that takes a caller-supplied string, so a second attempt at
/// one effect always presents the same key and the provider deduplicates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderIdempotencyKey([u8; 32]);

impl ProviderIdempotencyKey {
    /// Derives the key for one effect.
    #[must_use]
    pub fn derive(kind: CommandKind, organization: OrganizationId, effect: EffectId) -> Self {
        use aex_wire::PrefixedId as _;
        use sha2::Digest as _;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"aex:");
        hasher.update(kind.as_str().as_bytes());
        hasher.update(b":");
        hasher.update(organization.encode().as_str().as_bytes());
        hasher.update(b":");
        hasher.update(effect.0.encode_suffix());
        Self(hasher.finalize().into())
    }

    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The header value, rendered without allocating.
    ///
    /// # Panics
    ///
    /// Never: the rendering is 32 ASCII characters, well inside the buffer.
    #[must_use]
    pub fn as_header_value(&self) -> IdText {
        let mut suffix = [0u8; 26];
        for (index, slot) in suffix.iter_mut().enumerate() {
            const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
            *slot = ALPHABET[usize::from(self.0[index] & 0x1f)];
        }
        IdText::new("aexk", &suffix)
    }
}

/// One command the edge may execute. There is no seventh option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PaymentCommand {
    /// Ensure the provider customer record exists.
    EnsureCustomer {
        /// The admitted effect.
        effect: EffectId,
        /// The account it belongs to.
        organization: OrganizationId,
        /// The address to attach.
        email: RedactedEmail,
    },
    /// Create a hosted top-up checkout.
    CreateTopUpCheckout {
        /// The admitted effect.
        effect: EffectId,
        /// The account it belongs to.
        organization: OrganizationId,
        /// How much credit the checkout buys.
        amount: Cents,
        /// Where to return after success.
        success_url: HttpsUrl,
        /// Where to return after cancellation.
        cancel_url: HttpsUrl,
        /// The provider customer.
        customer: ProviderCustomerRef,
        /// How tax is handled.
        tax: TaxMode,
    },
    /// Create a hosted billing portal session.
    CreatePortalSession {
        /// The admitted effect.
        effect: EffectId,
        /// The account it belongs to.
        organization: OrganizationId,
        /// Where to return when the user is done.
        return_url: HttpsUrl,
        /// The provider customer.
        customer: ProviderCustomerRef,
    },
    /// Charge a saved payment method.
    ChargeSavedMethod {
        /// The admitted effect.
        effect: EffectId,
        /// The account it belongs to.
        organization: OrganizationId,
        /// How much to charge.
        amount: Cents,
        /// The provider customer.
        customer: ProviderCustomerRef,
        /// The saved method.
        method: ProviderMethodRef,
    },
    /// Look up the outcome of an effect that ended indeterminate.
    LookupEffectOutcome {
        /// The effect to resolve.
        effect: EffectId,
        /// Which command it was.
        expect: CommandKind,
        /// Provider object already recorded for the effect, when one is known.
        provider: Option<ProviderObjectRef>,
    },
    /// Refund a charge.
    RefundCharge {
        /// The admitted effect.
        effect: EffectId,
        /// The charge to refund.
        original: ProviderChargeRef,
        /// How much to refund.
        amount: Cents,
    },
}

impl PaymentCommand {
    /// The effect this command executes.
    #[must_use]
    pub const fn effect(&self) -> EffectId {
        match self {
            Self::EnsureCustomer { effect, .. }
            | Self::CreateTopUpCheckout { effect, .. }
            | Self::CreatePortalSession { effect, .. }
            | Self::ChargeSavedMethod { effect, .. }
            | Self::LookupEffectOutcome { effect, .. }
            | Self::RefundCharge { effect, .. } => *effect,
        }
    }

    /// Which command kind this is.
    #[must_use]
    pub const fn kind(&self) -> CommandKind {
        match self {
            Self::EnsureCustomer { .. } => CommandKind::EnsureCustomer,
            Self::CreateTopUpCheckout { .. } => CommandKind::CreateTopUpCheckout,
            Self::CreatePortalSession { .. } => CommandKind::CreatePortalSession,
            Self::ChargeSavedMethod { .. } => CommandKind::ChargeSavedMethod,
            Self::LookupEffectOutcome { .. } => CommandKind::LookupEffectOutcome,
            Self::RefundCharge { .. } => CommandKind::RefundCharge,
        }
    }
}

/// The one metadata vocabulary written onto every provider object.
///
/// Every object carries the same four members, so an object is attributable from
/// its metadata alone. Two different writers using two different shapes is
/// exactly how a webhook ends up unable to derive the credit it should apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectMetadata {
    /// The admitted effect.
    pub effect: EffectId,
    /// The account it belongs to.
    pub organization: OrganizationId,
    /// Which command produced the object.
    pub kind: CommandKind,
    /// How much credit the effect grants on success.
    pub credit: Cents,
}

/// One command, ready to hand to the edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PaymentCommandEnvelope {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// What to do.
    pub command: PaymentCommand,
    /// The derived key the edge must present.
    pub provider_idempotency_key: String,
    /// After this instant the edge must not start the call.
    pub deadline: Timestamp,
    /// The metadata to write onto every object the call creates.
    pub metadata: EffectMetadata,
}
