//! Usage facts and their settlement receipts.
//!
//! Area 11 pins exactly four priced meters. Model tokens are a separate
//! zero-dollar observability fact and never enter [`Meter`]: putting them here
//! would make a BYOK token count look like something the platform can charge for.
//!
//! `fact_id` is deterministic, derived from the authority key. A random id
//! cannot make a producer retry idempotent, and it would contradict the already
//! pinned `MessageDeduplicationId` and `business_key` grammar.

use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{OperationId, OrganizationId, RunId, SessionId, WorkspaceId};
use aex_wire::types::{DecimalU128, Region, Timestamp};
use serde::{Deserialize, Serialize};

use crate::money::Microusd;
use crate::{PricingVersion, SchemaVersion};

/// The four priced meters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Meter {
    /// `compute.millicpu_ms.v1`.
    ComputeMillicpuMs,
    /// `memory.byte_ms.v1`.
    MemoryByteMs,
    /// `storage.byte_min.v1`.
    StorageByteMin,
    /// `data_transfer.egress_byte.v1`.
    DataTransferEgressByte,
}

impl Meter {
    /// Every meter, in rate-book order.
    pub const ALL: [Self; 4] = [
        Self::ComputeMillicpuMs,
        Self::MemoryByteMs,
        Self::StorageByteMin,
        Self::DataTransferEgressByte,
    ];

    /// The stable meter identifier used in the rate book and the business key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ComputeMillicpuMs => "compute.millicpu_ms.v1",
            Self::MemoryByteMs => "memory.byte_ms.v1",
            Self::StorageByteMin => "storage.byte_min.v1",
            Self::DataTransferEgressByte => "data_transfer.egress_byte.v1",
        }
    }

    /// The rating category this meter settles under.
    ///
    /// Declared once, here, because the same spelling appears in the central
    /// inbox primary key, the FIFO deduplication grammar and the `business_key`
    /// — a second copy of this mapping is a second settlement authority.
    #[must_use]
    pub const fn category(self) -> &'static str {
        match self {
            Self::ComputeMillicpuMs | Self::MemoryByteMs => "compute",
            Self::StorageByteMin => "storage",
            Self::DataTransferEgressByte => "transfer",
        }
    }
}

/// Whether a fact records consumption or a reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactBasis {
    /// Work that actually happened.
    Consumed,
    /// Capacity held against admitted work.
    Reserved,
}

/// When a fact's service happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ServiceTime {
    /// A point event.
    Instant {
        /// When it happened.
        at: Timestamp,
    },
    /// A half-open interval.
    Interval {
        /// Inclusive start.
        start: Timestamp,
        /// Exclusive end.
        end: Timestamp,
    },
}

/// Which authority minted a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityKind {
    /// The storage authority.
    Storage,
    /// The compute authority; memory is a discriminated fact inside it.
    Compute,
    /// The transfer authority.
    Transfer,
}

/// The authority key a fact identity is derived from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FactAuthority {
    /// Which authority.
    pub kind: AuthorityKind,
    /// The authority's own identity for the measured thing.
    pub authority_id: Box<str>,
    /// Which segment of that thing, for an interval that was split.
    pub segment_ordinal: DecimalU128,
}

/// The deterministic identity of one usage fact.
///
/// Derived, never random: `region/meter/authority-kind/authority-id/ordinal`
/// hashed once. Two producers that measured the same thing therefore mint the
/// same id, which is what makes a retry idempotent instead of double-billing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FactId([u8; 32]);

impl FactId {
    /// Wraps a digest the regional authority already derived.
    ///
    /// The regional usage domain hashes the canonical authority key at
    /// admission; the outbox carries those exact bytes onto the wire rather
    /// than deriving a second identity for one fact. The regional spelling
    /// carries a `usage_` prefix over the same 64 hex characters; this contract
    /// serializes the bare digest, which is the spelling the central inbox
    /// keys on.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Derives the identity from the authority key.
    #[must_use]
    pub fn derive(region: Region, meter: Meter, authority: &FactAuthority) -> Self {
        use sha2::Digest as _;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"aex:usage-fact:v1");
        hasher.update([0x1f]);
        hasher.update(region.as_str().as_bytes());
        hasher.update([0x1f]);
        hasher.update(meter.as_str().as_bytes());
        hasher.update([0x1f]);
        hasher.update(authority_kind_key(authority.kind).as_bytes());
        hasher.update([0x1f]);
        hasher.update(authority.authority_id.as_bytes());
        hasher.update([0x1f]);
        hasher.update(authority.segment_ordinal.to_string().as_bytes());
        Self(hasher.finalize().into())
    }

    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The lowercase hexadecimal rendering used in the business key.
    #[must_use]
    pub fn to_hex(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

impl std::fmt::Display for FactId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for FactId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FactId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let text = <std::borrow::Cow<'de, str> as Deserialize<'de>>::deserialize(deserializer)?;
        if text.len() != 64 {
            return Err(D::Error::custom("a FactId is 64 lowercase hex characters"));
        }
        let mut bytes = [0u8; 32];
        for (index, slot) in bytes.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
                .map_err(|_| D::Error::custom("a FactId is 64 lowercase hex characters"))?;
        }
        Ok(Self(bytes))
    }
}

/// The stable key for an authority kind.
const fn authority_kind_key(kind: AuthorityKind) -> &'static str {
    match kind {
        AuthorityKind::Storage => "storage",
        AuthorityKind::Compute => "compute",
        AuthorityKind::Transfer => "transfer",
    }
}

/// What a fact is attributed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Attribution {
    /// The session, when attributable.
    pub session: Option<SessionId>,
    /// The run, when attributable.
    pub run: Option<RunId>,
    /// The durable operation, when attributable.
    pub operation: Option<OperationId>,
}

/// The producer's own receipt for a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceReceipt {
    /// Which producer measured it.
    pub source: Box<str>,
    /// The producer's own identity for the measurement.
    pub receipt_id: Box<str>,
    /// When the producer observed it.
    pub observed_at: Timestamp,
}

/// The replay identity a producer carries with a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FactIdempotency {
    /// The SQS deduplication identity.
    pub deduplication_id: Box<str>,
    /// The business key the central inbox folds on.
    pub business_key: Box<str>,
}

/// One usage fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageFact {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The deterministic identity derived from the authority key.
    pub fact_id: FactId,
    /// Which priced meter.
    pub meter: Meter,
    /// The account the money belongs to.
    pub organization: OrganizationId,
    /// The workspace the usage happened in.
    pub workspace: WorkspaceId,
    /// The region the usage happened in.
    pub region: Region,
    /// What the usage is attributed to.
    pub attribution: Attribution,
    /// The authority key the identity was derived from.
    pub authority: FactAuthority,
    /// Consumption or reservation.
    pub basis: FactBasis,
    /// How much, in the meter's own unit.
    pub quantity: DecimalU128,
    /// When the service happened.
    pub service_time: ServiceTime,
    /// The producer's receipt.
    pub source_receipt: SourceReceipt,
    /// Which rate book it will be priced against.
    pub pricing_version: PricingVersion,
    /// The replay identity.
    pub idempotency: FactIdempotency,
}

/// The body of one message on the regional-to-central rating queue.
///
/// This is the one wire shape: the regional outbox serializes it and the
/// settlement worker decodes it, both through this declaration. Before it
/// existed each side declared its own `RatingRequest` — the producer's nested
/// domain fact against this crate's flat fact — and the first published message
/// would have been undecodable by its consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RatingRequest {
    /// The whole admitted fact. Central re-validates it rather than trusting it.
    pub fact: UsageFact,
    /// The admission-time intent digest, which decides replay from conflict.
    pub intent_hash: IntentDigest,
}

/// The central inbox identity of a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FactInboxKey {
    /// Where the fact came from.
    pub region: Region,
    /// Which meter.
    pub meter: Meter,
    /// The deterministic fact identity.
    pub fact_id: FactId,
    /// The canonical intent digest the producer recorded.
    pub intent: IntentDigest,
}

/// What settlement wrote for one fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SettlementReceipt {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// Which fact was settled.
    pub fact_inbox: FactInboxKey,
    /// What it was rated at.
    pub rated: Microusd,
    /// Which rate book priced it.
    pub pricing_version: PricingVersion,
    /// Its position in the settled sequence.
    pub settled_sequence: DecimalU128,
    /// When settlement committed.
    pub committed_at: Timestamp,
}
