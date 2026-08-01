//! The five public Hands compute shapes and every provider-derived capacity that
//! follows from one of them.
//!
//! The arity is **five**, not six. `references/limits-and-ceilings-decision-2026-07-30.md`
//! line 126 pins the tokens and their baseline/peak/disk triples; AWS couples
//! baseline, four-times peak and maximum disk at image creation, so a public token
//! selects an image and never an independent compute field.
//!
//! The token itself is `aex_wire::types::ComputeSize`, which the contract already
//! publishes. This module adds the capacities as an extension trait rather than a
//! second enum, so there is exactly one shape vocabulary in the workspace and no
//! mapping table to drift.

use aex_wire::types::ComputeSize;

/// A public compute token that is not one of the five.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{token}` is not a Hands compute size; the five public tokens are 512mb, 1gb, 2gb, 4gb, 8gb"
)]
pub struct UnknownComputeSize {
    /// The rejected token, echoed verbatim.
    pub token: String,
}

/// One gibibyte in bytes.
const GIB: u64 = 1_073_741_824;
/// One mebibyte in bytes.
const MIB: u64 = 1_048_576;

/// Micro-USD charged per vCPU-hour in the Area 11 rate derivation.
pub const VCPU_MICRO_USD_PER_HOUR: u64 = 250_000;

/// Micro-USD charged per GiB-hour in the Area 11 rate derivation.
pub const MEMORY_GIB_MICRO_USD_PER_HOUR: u64 = 30_000;

/// Parses a public compute token, refusing anything outside the five.
///
/// # Errors
///
/// Returns [`UnknownComputeSize`] for any token that is not one of the five,
/// including the retired Fargate-era `<cpu>-<memory>` vocabulary. There is no
/// nearest-match fallback: silently promoting an unknown token to a shape would
/// bill a customer for capacity they did not ask for.
pub fn parse_compute_size(text: &str) -> Result<ComputeSize, UnknownComputeSize> {
    ComputeSize::ALL
        .into_iter()
        .find(|size| size.as_str() == text)
        .ok_or_else(|| UnknownComputeSize {
            token: text.to_owned(),
        })
}

/// Every provider-derived capacity a public shape token selects.
///
/// Metering uses the **baseline** allocation, never the peak: the accepted
/// $0.155/hour for `1gb` is exactly `0.5 vCPU x $0.25 + 1 GiB x $0.03`.
pub trait ShapeCapacity: Copy {
    /// Baseline CPU in thousandths of a vCPU. This is the compute meter's rate.
    fn baseline_millicpu(self) -> u32;

    /// Baseline memory in bytes. This is the memory meter's rate.
    fn baseline_memory_bytes(self) -> u64;

    /// Peak CPU in thousandths of a vCPU. Provider-coupled at exactly four times
    /// baseline; reported to the customer, never metered.
    fn peak_millicpu(self) -> u32 {
        self.baseline_millicpu() * 4
    }

    /// Peak memory in bytes. Provider-coupled at exactly four times baseline.
    fn peak_memory_bytes(self) -> u64 {
        self.baseline_memory_bytes() * 4
    }

    /// Maximum disk in bytes.
    fn disk_bytes(self) -> u64;

    /// `minimumMemoryInMiB` for `CreateMicrovmImage`.
    fn minimum_memory_mib(self) -> u32;

    /// Provider-hard endpoint bandwidth in bytes per second.
    fn network_bytes_per_second(self) -> u64;

    /// Provider-hard concurrent connection ceiling.
    fn max_connections(self) -> u32;

    /// Shared-safety ceiling on simultaneously open operations,
    /// `min(32, max_connections * 2)`.
    fn max_concurrent_operations(self) -> u32 {
        let doubled = self.max_connections() * 2;
        if doubled < 32 { doubled } else { 32 }
    }

    /// Pool size in [`crate::generation::TransportMode::PerRequest`]: every
    /// connection but the two reserved for probe and lifecycle.
    fn per_request_pool_size(self) -> u32 {
        self.max_connections() - 2
    }

    /// Whether a browser image variant is offered for this shape. Chromium is not
    /// offered below a 2 GiB baseline.
    fn offers_browser(self) -> bool;

    /// Baseline cost in micro-USD per hour, by the Area 11 derivation.
    ///
    /// Integer arithmetic only: money never travels as a floating-point value.
    fn baseline_micro_usd_per_hour(self) -> u64 {
        let cpu = u64::from(self.baseline_millicpu()) * VCPU_MICRO_USD_PER_HOUR / 1_000;
        let memory = self.baseline_memory_bytes() * MEMORY_GIB_MICRO_USD_PER_HOUR / GIB;
        cpu + memory
    }
}

impl ShapeCapacity for ComputeSize {
    fn baseline_millicpu(self) -> u32 {
        match self {
            Self::Mb512 => 250,
            Self::Gb1 => 500,
            Self::Gb2 => 1_000,
            Self::Gb4 => 2_000,
            Self::Gb8 => 4_000,
        }
    }

    fn baseline_memory_bytes(self) -> u64 {
        match self {
            Self::Mb512 => GIB / 2,
            Self::Gb1 => GIB,
            Self::Gb2 => 2 * GIB,
            Self::Gb4 => 4 * GIB,
            Self::Gb8 => 8 * GIB,
        }
    }

    fn disk_bytes(self) -> u64 {
        match self {
            Self::Mb512 | Self::Gb1 | Self::Gb2 => 8 * GIB,
            Self::Gb4 => 16 * GIB,
            Self::Gb8 => 32 * GIB,
        }
    }

    fn minimum_memory_mib(self) -> u32 {
        u32::try_from(self.baseline_memory_bytes() / MIB)
            .expect("every offered baseline is under 4 TiB")
    }

    fn network_bytes_per_second(self) -> u64 {
        match self {
            Self::Mb512 => 1_000_000,
            Self::Gb1 => 2_000_000,
            Self::Gb2 => 4_000_000,
            Self::Gb4 => 8_000_000,
            Self::Gb8 => 16_000_000,
        }
    }

    fn max_connections(self) -> u32 {
        match self {
            Self::Mb512 => 8,
            Self::Gb1 => 16,
            Self::Gb2 => 32,
            Self::Gb4 => 64,
            Self::Gb8 => 128,
        }
    }

    fn offers_browser(self) -> bool {
        matches!(self, Self::Gb2 | Self::Gb4 | Self::Gb8)
    }
}

#[cfg(test)]
mod tests {
    use super::{ShapeCapacity, parse_compute_size};
    use aex_wire::types::ComputeSize;

    #[test]
    fn the_arity_is_five_and_the_default_is_one_gigabyte() {
        assert_eq!(ComputeSize::ALL.len(), 5);
        assert_eq!(ComputeSize::DEFAULT, ComputeSize::Gb1);
        assert_eq!(
            ComputeSize::ALL.map(ComputeSize::as_str),
            ["512mb", "1gb", "2gb", "4gb", "8gb"]
        );
    }

    /// One golden row: token, baseline millicpu, baseline bytes, peak millicpu,
    /// peak bytes, disk bytes, bandwidth bytes/s, connections, browser.
    type GoldenRow = (&'static str, u32, u64, u32, u64, u64, u64, u32, bool);

    /// The `hands-compute-shape-golden-table` evidence class.
    #[test]
    fn the_golden_shape_table_is_exact() {
        let expected: [GoldenRow; 5] = [
            (
                "512mb",
                250,
                536_870_912,
                1_000,
                2_147_483_648,
                8_589_934_592,
                1_000_000,
                8,
                false,
            ),
            (
                "1gb",
                500,
                1_073_741_824,
                2_000,
                4_294_967_296,
                8_589_934_592,
                2_000_000,
                16,
                false,
            ),
            (
                "2gb",
                1_000,
                2_147_483_648,
                4_000,
                8_589_934_592,
                8_589_934_592,
                4_000_000,
                32,
                true,
            ),
            (
                "4gb",
                2_000,
                4_294_967_296,
                8_000,
                17_179_869_184,
                17_179_869_184,
                8_000_000,
                64,
                true,
            ),
            (
                "8gb",
                4_000,
                8_589_934_592,
                16_000,
                34_359_738_368,
                34_359_738_368,
                16_000_000,
                128,
                true,
            ),
        ];
        for (size, row) in ComputeSize::ALL.into_iter().zip(expected) {
            assert_eq!(size.as_str(), row.0);
            assert_eq!(size.baseline_millicpu(), row.1, "{}", row.0);
            assert_eq!(size.baseline_memory_bytes(), row.2, "{}", row.0);
            assert_eq!(size.peak_millicpu(), row.3, "{}", row.0);
            assert_eq!(size.peak_memory_bytes(), row.4, "{}", row.0);
            assert_eq!(size.disk_bytes(), row.5, "{}", row.0);
            assert_eq!(size.network_bytes_per_second(), row.6, "{}", row.0);
            assert_eq!(size.max_connections(), row.7, "{}", row.0);
            assert_eq!(size.offers_browser(), row.8, "{}", row.0);
        }
    }

    #[test]
    fn the_one_gigabyte_baseline_derives_the_accepted_hourly_rate() {
        // 0.5 vCPU x $0.25 + 1 GiB x $0.03 = $0.155.
        assert_eq!(ComputeSize::Gb1.baseline_micro_usd_per_hour(), 155_000);
        assert_eq!(ComputeSize::Mb512.baseline_micro_usd_per_hour(), 77_500);
        assert_eq!(ComputeSize::Gb8.baseline_micro_usd_per_hour(), 1_240_000);
    }

    #[test]
    fn concurrency_and_pool_sizes_follow_the_connection_ceiling() {
        assert_eq!(ComputeSize::Mb512.max_concurrent_operations(), 16);
        assert_eq!(ComputeSize::Gb1.max_concurrent_operations(), 32);
        assert_eq!(ComputeSize::Gb8.max_concurrent_operations(), 32);
        assert_eq!(ComputeSize::Mb512.per_request_pool_size(), 6);
        assert_eq!(ComputeSize::Gb8.per_request_pool_size(), 126);
    }

    #[test]
    fn minimum_memory_matches_the_image_build_input() {
        assert_eq!(
            ComputeSize::ALL.map(ShapeCapacity::minimum_memory_mib),
            [512, 1_024, 2_048, 4_096, 8_192]
        );
    }

    #[test]
    fn every_token_round_trips_through_text_and_json() {
        for size in ComputeSize::ALL {
            assert_eq!(parse_compute_size(size.as_str()), Ok(size));
            let json = serde_json::to_string(&size).expect("a token serializes");
            assert_eq!(json, format!("\"{}\"", size.as_str()));
            assert_eq!(
                serde_json::from_str::<ComputeSize>(&json).expect("a token deserializes"),
                size
            );
        }
    }

    #[test]
    fn a_sixth_token_is_rejected_rather_than_guessed() {
        let error = parse_compute_size("16gb").expect_err("a sixth shape does not exist");
        assert_eq!(error.token, "16gb");
        assert!(serde_json::from_str::<ComputeSize>("\"16gb\"").is_err());
        for stale in [
            "0.25cpu-1gb",
            "0.5cpu-4gb",
            "1cpu-6gb",
            "2cpu-8gb",
            "4cpu-12gb",
        ] {
            assert!(
                parse_compute_size(stale).is_err(),
                "the Fargate-era vocabulary is deleted, not mapped: {stale}"
            );
        }
    }
}
