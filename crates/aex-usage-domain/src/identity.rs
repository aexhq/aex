//! Deterministic fact identity.
//!
//! A fact's identifier is a pure function of the physical thing it measures.
//! That is what makes a producer retry, an SQS redelivery and a crashed-then-
//! replayed transaction converge on exactly one row without any extra state.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::meter::{Category, UnknownMeter};
use crate::wire_pending::{IdError, RegionId, validate_id};

/// The kind of physical thing an authority key names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityKind {
    /// One customer run.
    Run,
    /// One durable operation.
    Operation,
    /// One session.
    Session,
    /// One agent within a session.
    Agent,
    /// One mux activation of one agent.
    Activation,
    /// One Hands MicroVM generation.
    HandsGeneration,
    /// One storage owner root (content, observation batch, snapshot).
    ContentOwner,
    /// One admitted observation batch.
    ObservationBatch,
    /// One observation export operation.
    ExportOperation,
    /// One content download grant.
    DownloadGrant,
    /// One physical crossing of one billed public boundary.
    EgressCrossing,
    /// One Lambda invocation.
    LambdaInvocation,
    /// One closed CPU reconciliation interval.
    CpuInterval,
    /// One held memory reservation.
    MemoryReservation,
    /// One storage residence of one owner generation.
    StorageResidence,
    /// One correction case.
    CorrectionCase,
}

impl AuthorityKind {
    /// Every authority kind, in a stable order.
    pub const ALL: [Self; 16] = [
        Self::Run,
        Self::Operation,
        Self::Session,
        Self::Agent,
        Self::Activation,
        Self::HandsGeneration,
        Self::ContentOwner,
        Self::ObservationBatch,
        Self::ExportOperation,
        Self::DownloadGrant,
        Self::EgressCrossing,
        Self::LambdaInvocation,
        Self::CpuInterval,
        Self::MemoryReservation,
        Self::StorageResidence,
        Self::CorrectionCase,
    ];

    /// The stable identifier used in the canonical key and on the row.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Operation => "operation",
            Self::Session => "session",
            Self::Agent => "agent",
            Self::Activation => "activation",
            Self::HandsGeneration => "hands_generation",
            Self::ContentOwner => "content_owner",
            Self::ObservationBatch => "observation_batch",
            Self::ExportOperation => "export_operation",
            Self::DownloadGrant => "download_grant",
            Self::EgressCrossing => "egress_crossing",
            Self::LambdaInvocation => "lambda_invocation",
            Self::CpuInterval => "cpu_interval",
            Self::MemoryReservation => "memory_reservation",
            Self::StorageResidence => "storage_residence",
            Self::CorrectionCase => "correction_case",
        }
    }
}

impl fmt::Display for AuthorityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for AuthorityKind {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "authority kind",
                value: value.to_owned(),
            })
    }
}

/// The identifier of the physical thing an authority key names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AuthorityId(Box<str>);

impl AuthorityId {
    /// Parses an authority identifier under the shared bounded grammar.
    ///
    /// # Errors
    ///
    /// Returns [`IdError`] when the value is empty, over the byte ceiling, or
    /// contains a reserved separator.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        validate_id("AuthorityId", value)?;
        Ok(Self(Box::from(value)))
    }

    /// Borrows the identifier body.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AuthorityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for AuthorityId {
    type Error = IdError;

    fn try_from(value: String) -> Result<Self, IdError> {
        Self::parse(&value)
    }
}

impl From<AuthorityId> for String {
    fn from(value: AuthorityId) -> Self {
        value.0.into_string()
    }
}

/// A zero-based, monotone segment index within one authority.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SegmentOrdinal(u64);

impl SegmentOrdinal {
    /// The first segment of any authority.
    pub const FIRST: Self = Self(0);

    /// Builds a segment ordinal.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying index.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The next segment of the same authority.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::OrdinalOverflow`] at `u64::MAX`.
    pub const fn next(self) -> Result<Self, IdentityError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(IdentityError::OrdinalOverflow),
        }
    }
}

impl fmt::Display for SegmentOrdinal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why an identity could not be produced or parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// The segment ordinal reached `u64::MAX`.
    #[error("segment ordinal overflowed")]
    OrdinalOverflow,
    /// A fact identifier did not have the `usage_` prefix.
    #[error("`{value}` is not a `usage_`-prefixed fact identifier")]
    Prefix {
        /// The value that was refused.
        value: String,
    },
    /// A fact identifier's digest was not 64 lowercase hex characters.
    #[error("`{value}` does not carry a 64-character lowercase hex digest")]
    Digest {
        /// The value that was refused.
        value: String,
    },
}

/// The exact physical thing one fact measures.
///
/// Its canonical string is
/// `"{region}/{category}/{authority_kind}/{authority_id}/{segment_ordinal}"`
/// with every component percent-encoded, so no component value can forge a
/// separator and collide with a different key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorityKey {
    /// The region the measurement was taken in.
    pub region: RegionId,
    /// The authority table the fact belongs to.
    pub category: Category,
    /// The kind of thing measured.
    pub kind: AuthorityKind,
    /// The identifier of the thing measured.
    pub authority_id: AuthorityId,
    /// Which segment of that thing this fact closes.
    pub segment_ordinal: SegmentOrdinal,
}

/// Percent-encodes one canonical-key component.
///
/// Everything outside the RFC 3986 unreserved set is escaped, so the `/`
/// separator is unforgeable even if a future identifier grammar admits one.
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

impl AuthorityKey {
    /// The canonical string that the fact identifier hashes.
    #[must_use]
    pub fn canonical(&self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            percent_encode(self.region.as_str()),
            percent_encode(self.category.id()),
            percent_encode(self.kind.id()),
            percent_encode(self.authority_id.as_str()),
            self.segment_ordinal.get()
        )
    }

    /// The deterministic identifier of the fact this key admits.
    ///
    /// `"usage_" + lowercase_hex(sha256(canonical()))` — exactly 70 ASCII
    /// characters. SHA-256 matches the identity finance already pinned in its
    /// `MessageDeduplicationId` and `business_key` grammar.
    #[must_use]
    pub fn fact_id(&self) -> FactId {
        let digest = Sha256::digest(self.canonical().as_bytes());
        FactId(format!("{FACT_ID_PREFIX}{}", hex::encode(digest)).into_boxed_str())
    }
}

/// The prefix every fact identifier carries.
pub const FACT_ID_PREFIX: &str = "usage_";

/// The exact length of a fact identifier: prefix plus a 64-character digest.
pub const FACT_ID_LEN: usize = FACT_ID_PREFIX.len() + 64;

/// A deterministic fact identifier derived from an [`AuthorityKey`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FactId(Box<str>);

impl FactId {
    /// Parses a fact identifier, checking prefix, length and digest alphabet.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::Prefix`] or [`IdentityError::Digest`] for
    /// anything that could not have come from [`AuthorityKey::fact_id`].
    pub fn parse(value: &str) -> Result<Self, IdentityError> {
        let Some(digest) = value.strip_prefix(FACT_ID_PREFIX) else {
            return Err(IdentityError::Prefix {
                value: value.to_owned(),
            });
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(IdentityError::Digest {
                value: value.to_owned(),
            });
        }
        Ok(Self(Box::from(value)))
    }

    /// Borrows the identifier body.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for FactId {
    type Error = IdentityError;

    fn try_from(value: String) -> Result<Self, IdentityError> {
        Self::parse(&value)
    }
}

impl From<FactId> for String {
    fn from(value: FactId) -> Self {
        value.0.into_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorityId, AuthorityKey, AuthorityKind, FACT_ID_LEN, FactId, IdentityError,
        SegmentOrdinal, percent_encode,
    };
    use crate::meter::Category;
    use crate::wire_pending::RegionId;

    fn key(authority_id: &str, ordinal: u64) -> AuthorityKey {
        AuthorityKey {
            region: RegionId::parse("eu-west-1").expect("region"),
            category: Category::Compute,
            kind: AuthorityKind::Activation,
            authority_id: AuthorityId::parse(authority_id).expect("authority id"),
            segment_ordinal: SegmentOrdinal::new(ordinal),
        }
    }

    #[test]
    fn the_canonical_form_is_stable_and_percent_encoded() {
        let canonical = key("act 1", 7).canonical();
        assert_eq!(canonical, "eu-west-1/compute/activation/act%201/7");
    }

    #[test]
    fn a_fact_identifier_is_seventy_characters() {
        let id = key("act1", 0).fact_id();
        assert_eq!(id.as_str().len(), FACT_ID_LEN);
        assert_eq!(FACT_ID_LEN, 70);
        assert_eq!(FactId::parse(id.as_str()).expect("round trip"), id);
    }

    #[test]
    fn identity_is_a_pure_function_of_the_key() {
        assert_eq!(key("act1", 0).fact_id(), key("act1", 0).fact_id());
        assert_ne!(key("act1", 0).fact_id(), key("act1", 1).fact_id());
        assert_ne!(key("act1", 0).fact_id(), key("act2", 0).fact_id());
    }

    #[test]
    fn a_separator_cannot_be_forged_from_a_component() {
        assert_eq!(percent_encode("a/b"), "a%2Fb");
        assert_eq!(percent_encode("a#b"), "a%23b");
        assert!(AuthorityId::parse("a/b").is_err());
    }

    #[test]
    fn hostile_fact_identifiers_are_refused() {
        assert!(matches!(
            FactId::parse("fact_0000"),
            Err(IdentityError::Prefix { .. })
        ));
        assert!(matches!(
            FactId::parse("usage_short"),
            Err(IdentityError::Digest { .. })
        ));
        let uppercase = format!("usage_{}", "A".repeat(64));
        assert!(matches!(
            FactId::parse(&uppercase),
            Err(IdentityError::Digest { .. })
        ));
    }

    #[test]
    fn ordinals_advance_and_refuse_overflow() {
        assert_eq!(
            SegmentOrdinal::FIRST.next().expect("advances"),
            SegmentOrdinal::new(1)
        );
        assert_eq!(
            SegmentOrdinal::new(u64::MAX).next(),
            Err(IdentityError::OrdinalOverflow)
        );
    }
}
