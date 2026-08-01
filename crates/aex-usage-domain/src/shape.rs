//! Integer compute shapes.
//!
//! Every allocated-shape meter multiplies an integer capacity by an integer
//! running interval. A vCPU count is therefore carried as millicpu and a memory
//! size as bytes; there is no fractional vCPU anywhere.
//!
//! `TODO(cross-stream)`: [`ComputeShape`] and [`HandsShape`] are replaced by
//! `aex_runtime_control::{ComputeShape, ComputeSize}` at merge. The golden
//! table below is derived from `references/limits-and-ceilings-decision-2026-07-30.md`
//! row `microvm.shape`, and the evidence class that pins it
//! (`hands-compute-shape-golden-table`) is owned by `aex-runtime-control`, not
//! by this crate.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::meter::UnknownMeter;

/// One gibibyte in bytes.
const GIB: u64 = 1024 * 1024 * 1024;

/// One millicpu-and-bytes capacity pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ComputeShape {
    /// Compute capacity in millicpu; 1000 millicpu is one vCPU.
    pub millicpu: u32,
    /// Memory capacity in bytes.
    pub memory_bytes: u64,
}

impl ComputeShape {
    /// Builds a shape from millicpu and bytes.
    #[must_use]
    pub const fn new(millicpu: u32, memory_bytes: u64) -> Self {
        Self {
            millicpu,
            memory_bytes,
        }
    }
}

/// Which capacity of a [`ComputeShape`] an allocated-shape fact bills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeUnit {
    /// Bill the millicpu capacity against `compute.millicpu_ms.v1`.
    Millicpu,
    /// Bill the memory capacity against `memory.byte_ms.v1`.
    MemoryBytes,
}

/// The five public Hands baseline tokens.
///
/// The arity is five, not six. `references/limits-and-ceilings-decision-2026-07-30.md`
/// line 126 pins these tokens and their provider-derived baseline, peak and
/// disk triples, and line 312 calls them "the five public baseline tokens".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandsShape {
    /// `512mb`: baseline 0.5 GiB / 0.25 vCPU, peak 2 GiB / 1 vCPU, 8 GiB disk.
    Mb512,
    /// `1gb`: baseline 1 GiB / 0.5 vCPU, peak 4 GiB / 2 vCPU, 8 GiB disk.
    Gb1,
    /// `2gb`: baseline 2 GiB / 1 vCPU, peak 8 GiB / 4 vCPU, 8 GiB disk.
    Gb2,
    /// `4gb`: baseline 4 GiB / 2 vCPU, peak 16 GiB / 8 vCPU, 16 GiB disk.
    Gb4,
    /// `8gb`: baseline 8 GiB / 4 vCPU, peak 32 GiB / 16 vCPU, 32 GiB disk.
    Gb8,
}

impl HandsShape {
    /// Every public baseline token, in ascending size order. Exactly five.
    pub const ALL: [Self; 5] = [Self::Mb512, Self::Gb1, Self::Gb2, Self::Gb4, Self::Gb8];

    /// The public token string.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Mb512 => "512mb",
            Self::Gb1 => "1gb",
            Self::Gb2 => "2gb",
            Self::Gb4 => "4gb",
            Self::Gb8 => "8gb",
        }
    }

    /// The baseline capacity: what the customer holds for the whole running
    /// interval, and therefore what an allocated-shape fact bills.
    #[must_use]
    pub const fn baseline(self) -> ComputeShape {
        match self {
            Self::Mb512 => ComputeShape::new(250, GIB / 2),
            Self::Gb1 => ComputeShape::new(500, GIB),
            Self::Gb2 => ComputeShape::new(1_000, 2 * GIB),
            Self::Gb4 => ComputeShape::new(2_000, 4 * GIB),
            Self::Gb8 => ComputeShape::new(4_000, 8 * GIB),
        }
    }

    /// The provider-coupled peak capacity, four times the baseline.
    ///
    /// Never billed: it is a burst ceiling the provider grants, not an
    /// allocation the customer holds.
    #[must_use]
    pub const fn peak(self) -> ComputeShape {
        match self {
            Self::Mb512 => ComputeShape::new(1_000, 2 * GIB),
            Self::Gb1 => ComputeShape::new(2_000, 4 * GIB),
            Self::Gb2 => ComputeShape::new(4_000, 8 * GIB),
            Self::Gb4 => ComputeShape::new(8_000, 16 * GIB),
            Self::Gb8 => ComputeShape::new(16_000, 32 * GIB),
        }
    }

    /// The provider-coupled maximum disk, in bytes.
    #[must_use]
    pub const fn disk_bytes(self) -> u64 {
        match self {
            Self::Mb512 | Self::Gb1 | Self::Gb2 => 8 * GIB,
            Self::Gb4 => 16 * GIB,
            Self::Gb8 => 32 * GIB,
        }
    }
}

impl fmt::Display for HandsShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for HandsShape {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "Hands compute shape",
                value: value.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{GIB, HandsShape};
    use std::str::FromStr;

    #[test]
    fn the_public_shape_arity_is_five() {
        assert_eq!(HandsShape::ALL.len(), 5);
        let tokens: Vec<&str> = HandsShape::ALL.iter().map(|shape| shape.id()).collect();
        assert_eq!(tokens, vec!["512mb", "1gb", "2gb", "4gb", "8gb"]);
        assert!(HandsShape::from_str("16gb").is_err());
        assert!(HandsShape::from_str("256mb").is_err());
    }

    #[test]
    fn peak_is_exactly_four_times_baseline_for_every_token() {
        for shape in HandsShape::ALL {
            let baseline = shape.baseline();
            let peak = shape.peak();
            assert_eq!(peak.millicpu, baseline.millicpu * 4, "{shape}");
            assert_eq!(peak.memory_bytes, baseline.memory_bytes * 4, "{shape}");
        }
    }

    #[test]
    fn the_provider_derived_triples_match_the_limits_registry() {
        let expected = [
            (HandsShape::Mb512, 250, GIB / 2, 8 * GIB),
            (HandsShape::Gb1, 500, GIB, 8 * GIB),
            (HandsShape::Gb2, 1_000, 2 * GIB, 8 * GIB),
            (HandsShape::Gb4, 2_000, 4 * GIB, 16 * GIB),
            (HandsShape::Gb8, 4_000, 8 * GIB, 32 * GIB),
        ];
        for (shape, millicpu, memory_bytes, disk_bytes) in expected {
            assert_eq!(shape.baseline().millicpu, millicpu, "{shape}");
            assert_eq!(shape.baseline().memory_bytes, memory_bytes, "{shape}");
            assert_eq!(shape.disk_bytes(), disk_bytes, "{shape}");
        }
    }
}
