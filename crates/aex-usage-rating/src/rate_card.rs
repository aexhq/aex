//! Immutable signed rate-book loading and fail-closed validation.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_internal_contracts::PricingVersion;
use aex_internal_contracts::usage::Meter;
use aex_wire::canonical::to_jcs_bytes;
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::Zero as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest as _;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::exact::RateBookId;

/// Plane-level money mode relevant to rate-book admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneBilling {
    /// Ratings are observed but cannot post charges.
    Shadow,
    /// Ratings can post customer charges.
    Active,
}

/// Context-declared rounding policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundingRule {
    /// Round ties to the even integer.
    HalfEven,
    /// Ceiling the segment total.
    CeilTotal,
}

/// Reduced exact micro-USD rate per base unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rate(BigRational);

impl Rate {
    /// Exact rational value.
    #[must_use]
    pub const fn exact(&self) -> &BigRational {
        &self.0
    }
}

/// Verifies the signature and supplies the caller-owned clock.
pub trait BookVerifier {
    /// Verifies the canonical payload without its `signature` member.
    fn verify(&self, canonical_payload: &[u8], signature: &str) -> bool;
    /// Current time for validity checks.
    fn now(&self) -> OffsetDateTime;
}

/// Parsed, validated immutable rate book.
#[derive(Debug, Clone)]
pub struct RateBook {
    /// Content identity.
    pub id: RateBookId,
    /// Business revision.
    pub pricing_version: PricingVersion,
    /// Declared rounding policy.
    pub rounding: RoundingRule,
    /// Whether the artifact claims it is active.
    pub billing_active: bool,
    rates: BTreeMap<Meter, Rate>,
}

/// Verified rating context.
#[derive(Debug, Clone)]
pub struct RateContext {
    book: Arc<RateBook>,
}

impl RateContext {
    /// Parses and verifies an immutable rate book before use.
    ///
    /// # Errors
    /// Fails closed on malformed content, signature, hash, validity, schema,
    /// currency, meter or active-zero-book errors.
    pub fn open(
        raw: &[u8],
        verifier: &dyn BookVerifier,
        plane: PlaneBilling,
    ) -> Result<Self, RatingError> {
        let value: Value =
            serde_json::from_slice(raw).map_err(|_| RatingError::MalformedRateBook)?;
        let document: RawBook =
            serde_json::from_value(value.clone()).map_err(|_| RatingError::MalformedRateBook)?;
        if document.schema_version != 1 {
            return Err(RatingError::SchemaVersionUnsupported(
                document.schema_version,
            ));
        }
        if document.currency != "USD" {
            return Err(RatingError::CurrencyMismatch);
        }

        let mut payload = value;
        payload
            .as_object_mut()
            .ok_or(RatingError::MalformedRateBook)?
            .remove("signature");
        let canonical_payload =
            to_jcs_bytes(&payload).map_err(|_| RatingError::MalformedRateBook)?;
        if !verifier.verify(&canonical_payload, &document.signature) {
            return Err(RatingError::UnsignedRateBook);
        }

        let rates_value = payload.get("rates").ok_or(RatingError::MalformedRateBook)?;
        let actual_hash = format!(
            "sha256:{}",
            hex::encode(sha2::Sha256::digest(
                to_jcs_bytes(rates_value).map_err(|_| RatingError::MalformedRateBook)?
            ))
        );
        if document.rate_card_hash != actual_hash {
            return Err(RatingError::HashMismatch);
        }

        let effective_from = OffsetDateTime::parse(&document.effective_from, &Rfc3339)
            .map_err(|_| RatingError::MalformedRateBook)?;
        let effective_to = document
            .effective_to
            .as_deref()
            .map(|text| OffsetDateTime::parse(text, &Rfc3339))
            .transpose()
            .map_err(|_| RatingError::MalformedRateBook)?;
        let now = verifier.now();
        if now < effective_from || effective_to.is_some_and(|end| now >= end) {
            return Err(RatingError::ContextExpired);
        }

        let mut rates = BTreeMap::new();
        for (name, raw_rate) in document.rates {
            let meter =
                parse_meter(&name).ok_or_else(|| RatingError::UnknownMeter(name.clone()))?;
            let numerator = parse_nonnegative_bigint(&raw_rate.numerator_microusd)?;
            let denominator = parse_nonnegative_bigint(&raw_rate.denominator_units)?;
            if denominator.is_zero() {
                return Err(RatingError::ZeroDenominator);
            }
            rates.insert(meter, Rate(BigRational::new(numerator, denominator)));
        }
        for meter in Meter::ALL {
            if !rates.contains_key(&meter) {
                return Err(RatingError::MeterNotPriced(meter));
            }
        }
        if rates.len() != Meter::ALL.len() {
            return Err(RatingError::MalformedRateBook);
        }
        let zero_book = rates.values().all(|rate| rate.0.is_zero());
        if plane == PlaneBilling::Active && (zero_book || !document.billing_active) {
            return Err(RatingError::ZeroBookOnActivePlane);
        }

        Ok(Self {
            book: Arc::new(RateBook {
                id: RateBookId::new(*blake3::hash(&canonical_payload).as_bytes()),
                pricing_version: PricingVersion(document.pricing_version),
                rounding: document.rounding_rule,
                billing_active: document.billing_active,
                rates,
            }),
        })
    }

    /// Looks up one of the four closed meters.
    ///
    /// # Errors
    /// Returns [`RatingError::MeterNotPriced`] when the immutable artifact lacks it.
    pub fn rate(&self, meter: Meter) -> Result<&Rate, RatingError> {
        self.book
            .rates
            .get(&meter)
            .ok_or(RatingError::MeterNotPriced(meter))
    }

    /// Declared rounding rule.
    #[must_use]
    pub fn rounding(&self) -> RoundingRule {
        self.book.rounding
    }

    /// Content-addressed rate-book id.
    #[must_use]
    pub fn book_id(&self) -> RateBookId {
        self.book.id
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawBook {
    pricing_version: String,
    schema_version: u32,
    currency: String,
    rounding_rule: RoundingRule,
    billing_active: bool,
    effective_from: String,
    effective_to: Option<String>,
    rate_card_hash: String,
    rates: BTreeMap<String, RawRate>,
    signature: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawRate {
    numerator_microusd: String,
    denominator_units: String,
}

fn parse_nonnegative_bigint(text: &str) -> Result<BigInt, RatingError> {
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(RatingError::MalformedRateBook);
    }
    text.parse().map_err(|_| RatingError::MalformedRateBook)
}

fn parse_meter(value: &str) -> Option<Meter> {
    Meter::ALL.into_iter().find(|meter| meter.as_str() == value)
}

/// Exact rating failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RatingError {
    /// Meter absent from the immutable book.
    #[error("meter is not priced: {0:?}")]
    MeterNotPriced(Meter),
    /// Rate-book signature is absent or invalid.
    #[error("rate book signature is invalid")]
    UnsignedRateBook,
    /// Rate-card content hash differs.
    #[error("rate book hash differs")]
    HashMismatch,
    /// A denominator was zero.
    #[error("rate denominator is zero")]
    ZeroDenominator,
    /// Schema revision is not v1.
    #[error("rate-book schema {0} is unsupported")]
    SchemaVersionUnsupported(u32),
    /// Context is not currently effective.
    #[error("rate book is outside its validity interval")]
    ContextExpired,
    /// A zero or shadow book was bound to an active plane.
    #[error("an active plane cannot use the zero rate book")]
    ZeroBookOnActivePlane,
    /// Rounded amount exceeds the finance business bound.
    #[error("rated amount exceeds the finance business bound")]
    AmountExceedsBusinessBound,
    /// Quantity arithmetic overflowed.
    #[error("quantity arithmetic overflowed")]
    QuantityOverflow,
    /// Currency is not USD.
    #[error("rate-book currency is not USD")]
    CurrencyMismatch,
    /// Rate book had unknown or invalid structure.
    #[error("rate book is malformed")]
    MalformedRateBook,
    /// Artifact named an unknown meter.
    #[error("rate book named unknown meter `{0}`")]
    UnknownMeter(String),
    /// A segment total was negative where an amount is required.
    #[error("rated segment cannot be negative")]
    NegativeAmount,
}
