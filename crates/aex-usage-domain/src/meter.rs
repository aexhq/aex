//! Meters, base units, authority categories and the disjoint zero-dollar
//! observability set.
//!
//! Four meters are priced and nothing else ever is. The observability meters are
//! a separate type with no [`Meter`] conversion in either direction, so a rate
//! lookup for a model token is not a policy decision that can be reversed by
//! configuration — it is a value that cannot be constructed.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Why a meter or category string was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{value}` is not a known {kind}")]
pub struct UnknownMeter {
    /// The vocabulary that refused the value.
    pub kind: &'static str,
    /// The value that was refused.
    pub value: String,
}

/// The base unit a meter's quantity counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseUnit {
    /// Millicpu-milliseconds. One CPU microsecond is exactly one millicpu·ms.
    MillicpuMs,
    /// Byte-milliseconds of held allocation.
    ByteMs,
    /// Byte-minutes of retained storage.
    ByteMin,
    /// Bytes crossing one billed public boundary.
    Byte,
}

impl BaseUnit {
    /// The stable identifier written to a row and a rating request.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::MillicpuMs => "millicpu_ms",
            Self::ByteMs => "byte_ms",
            Self::ByteMin => "byte_min",
            Self::Byte => "byte",
        }
    }
}

/// The three authority tables. Memory is a discriminated fact inside
/// [`Category::Compute`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// `usage-storage-authority`.
    Storage,
    /// `usage-compute-authority`; compute, memory and every observability fact.
    Compute,
    /// `usage-transfer-authority`.
    Transfer,
}

impl Category {
    /// Every authority category, in a stable order.
    pub const ALL: [Self; 3] = [Self::Storage, Self::Compute, Self::Transfer];

    /// The stable identifier written to a row, a queue message and a table name.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Compute => "compute",
            Self::Transfer => "transfer",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for Category {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "usage category",
                value: value.to_owned(),
            })
    }
}

/// The four-value public category set the wire contract exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicCategory {
    /// Retained storage.
    Storage,
    /// CPU time.
    Compute,
    /// Held memory.
    Memory,
    /// Egress across a billed public boundary.
    DataTransfer,
}

impl PublicCategory {
    /// Every public category, in a stable order.
    pub const ALL: [Self; 4] = [
        Self::Storage,
        Self::Compute,
        Self::Memory,
        Self::DataTransfer,
    ];

    /// The stable identifier written to a projection row and the public wire.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Compute => "compute",
            Self::Memory => "memory",
            Self::DataTransfer => "data_transfer",
        }
    }

    /// The authority table this public category is folded from.
    #[must_use]
    pub const fn category(self) -> Category {
        match self {
            Self::Storage => Category::Storage,
            Self::Compute | Self::Memory => Category::Compute,
            Self::DataTransfer => Category::Transfer,
        }
    }
}

impl fmt::Display for PublicCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for PublicCategory {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "public usage category",
                value: value.to_owned(),
            })
    }
}

/// The four priced meters. Nothing else is ever priced.
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
    /// Every priced meter, in a stable order.
    pub const ALL: [Self; 4] = [
        Self::ComputeMillicpuMs,
        Self::MemoryByteMs,
        Self::StorageByteMin,
        Self::DataTransferEgressByte,
    ];

    /// The versioned public meter identifier.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::ComputeMillicpuMs => "compute.millicpu_ms.v1",
            Self::MemoryByteMs => "memory.byte_ms.v1",
            Self::StorageByteMin => "storage.byte_min.v1",
            Self::DataTransferEgressByte => "data_transfer.egress_byte.v1",
        }
    }

    /// The base unit this meter's quantity counts.
    #[must_use]
    pub const fn base_unit(self) -> BaseUnit {
        match self {
            Self::ComputeMillicpuMs => BaseUnit::MillicpuMs,
            Self::MemoryByteMs => BaseUnit::ByteMs,
            Self::StorageByteMin => BaseUnit::ByteMin,
            Self::DataTransferEgressByte => BaseUnit::Byte,
        }
    }

    /// The authority table that owns facts for this meter.
    #[must_use]
    pub const fn category(self) -> Category {
        match self {
            Self::ComputeMillicpuMs | Self::MemoryByteMs => Category::Compute,
            Self::StorageByteMin => Category::Storage,
            Self::DataTransferEgressByte => Category::Transfer,
        }
    }

    /// The public category this meter is reported under.
    #[must_use]
    pub const fn public(self) -> PublicCategory {
        match self {
            Self::ComputeMillicpuMs => PublicCategory::Compute,
            Self::MemoryByteMs => PublicCategory::Memory,
            Self::StorageByteMin => PublicCategory::Storage,
            Self::DataTransferEgressByte => PublicCategory::DataTransfer,
        }
    }

    /// Whether this meter's service time must be a half-open interval.
    ///
    /// Egress is the one meter whose crossing can be a single instant.
    #[must_use]
    pub const fn requires_interval(self) -> bool {
        !matches!(self, Self::DataTransferEgressByte)
    }
}

impl fmt::Display for Meter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for Meter {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "usage meter",
                value: value.to_owned(),
            })
    }
}

/// Zero-dollar BYOK observability counters.
///
/// Structurally disjoint from [`Meter`]: there is no conversion, no shared
/// trait and no rate path, so no rate can ever be looked up for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservabilityMeter {
    /// Model tokens by class, under a customer-held provider credential.
    ModelTokens,
    /// Provider request counts.
    ProviderCalls,
    /// Tool invocation counts.
    ToolInvocations,
}

impl ObservabilityMeter {
    /// Every observability meter, in a stable order.
    pub const ALL: [Self; 3] = [Self::ModelTokens, Self::ProviderCalls, Self::ToolInvocations];

    /// The versioned identifier written to an observability row.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::ModelTokens => "observability.model_tokens.v1",
            Self::ProviderCalls => "observability.provider_calls.v1",
            Self::ToolInvocations => "observability.tool_invocations.v1",
        }
    }

    /// Observability facts live in the compute authority and nowhere else.
    #[must_use]
    pub const fn category(self) -> Category {
        Category::Compute
    }
}

impl fmt::Display for ObservabilityMeter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for ObservabilityMeter {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "observability meter",
                value: value.to_owned(),
            })
    }
}

/// A counted token class on a [`ObservabilityMeter::ModelTokens`] fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenClass {
    /// Prompt tokens sent to the provider.
    Input,
    /// Completion tokens returned by the provider.
    Output,
    /// Tokens served from the provider's prompt cache.
    CacheRead,
    /// Tokens written into the provider's prompt cache.
    CacheWrite,
    /// Reasoning tokens billed separately by the provider.
    Reasoning,
}

impl TokenClass {
    /// Every token class, in a stable order.
    pub const ALL: [Self; 5] = [
        Self::Input,
        Self::Output,
        Self::CacheRead,
        Self::CacheWrite,
        Self::Reasoning,
    ];

    /// The stable identifier written to an observability row.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::CacheRead => "cache_read",
            Self::CacheWrite => "cache_write",
            Self::Reasoning => "reasoning",
        }
    }
}

impl fmt::Display for TokenClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for TokenClass {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "token class",
                value: value.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Category, Meter, ObservabilityMeter, PublicCategory};
    use std::str::FromStr;

    #[test]
    fn the_priced_meter_set_is_exactly_four() {
        assert_eq!(Meter::ALL.len(), 4);
        let ids: Vec<&str> = Meter::ALL.iter().map(|meter| meter.id()).collect();
        assert_eq!(
            ids,
            vec![
                "compute.millicpu_ms.v1",
                "memory.byte_ms.v1",
                "storage.byte_min.v1",
                "data_transfer.egress_byte.v1",
            ]
        );
    }

    #[test]
    fn every_meter_round_trips_through_its_identifier() {
        for meter in Meter::ALL {
            assert_eq!(Meter::from_str(meter.id()).expect("known meter"), meter);
        }
        for meter in ObservabilityMeter::ALL {
            assert_eq!(
                ObservabilityMeter::from_str(meter.id()).expect("known meter"),
                meter
            );
        }
    }

    #[test]
    fn an_observability_identifier_is_never_a_priced_meter() {
        for meter in ObservabilityMeter::ALL {
            assert!(Meter::from_str(meter.id()).is_err());
        }
        for meter in Meter::ALL {
            assert!(ObservabilityMeter::from_str(meter.id()).is_err());
        }
    }

    #[test]
    fn memory_is_a_compute_authority_fact_with_its_own_public_category() {
        assert_eq!(Meter::MemoryByteMs.category(), Category::Compute);
        assert_eq!(Meter::MemoryByteMs.public(), PublicCategory::Memory);
        assert_eq!(PublicCategory::Memory.category(), Category::Compute);
    }

    #[test]
    fn every_public_category_maps_back_to_an_authority() {
        for public in PublicCategory::ALL {
            assert!(Category::ALL.contains(&public.category()));
        }
    }
}
