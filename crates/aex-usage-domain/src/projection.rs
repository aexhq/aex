//! The generation-keyed key grammar for `usage-query-projection`.
//!
//! The projection is generation-keyed, so a rebuild is a normal operation rather
//! than an outage: generation `n+1` is written under its own prefix and the
//! pointer flips only once the rebuild has been verified. A cursor binds the
//! generation it was issued against, so a stale cursor expires instead of
//! silently mixing two generations.
//!
//! The grammar lives in the domain because projection writers and readers need
//! the same keys. A second copy would let them disagree about where a row lives.
use std::fmt;

use crate::identity::FactId;
use crate::meter::{Category, PublicCategory};
use crate::wire_pending::WorkspaceId;

/// A projection generation.
///
/// Rendered `G{gen:04}` so the prefix is fixed width and a range query cannot
/// straddle two generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u16);

/// The largest generation the `G{gen:04}` prefix can render at fixed width.
///
/// Above this the prefix would grow a fifth digit and a range query could
/// straddle two generations, so the ceiling is enforced at construction rather
/// than discovered by a mis-scoped read.
pub const MAX_GENERATION: u16 = 9_999;

impl Generation {
    /// The generation a fresh projection starts at.
    pub const FIRST: Self = Self(0);

    /// Builds a generation.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionKeyError::GenerationExhausted`] above [`MAX_GENERATION`].
    pub const fn new(value: u16) -> Result<Self, ProjectionKeyError> {
        if value > MAX_GENERATION {
            return Err(ProjectionKeyError::GenerationExhausted);
        }
        Ok(Self(value))
    }

    /// The underlying number.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }

    /// The next generation a rebuild writes into.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionKeyError::GenerationExhausted`] at `u16::MAX`.
    pub const fn next(self) -> Result<Self, ProjectionKeyError> {
        match self.0.checked_add(1) {
            Some(value) => Self::new(value),
            None => Err(ProjectionKeyError::GenerationExhausted),
        }
    }

    /// The fixed-width key prefix.
    #[must_use]
    pub fn prefix(self) -> String {
        format!("G{:04}", self.0)
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.prefix())
    }
}

/// Why a read could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionKeyError {
    /// The generation counter reached its ceiling.
    #[error("the projection generation counter is exhausted at {MAX_GENERATION}")]
    GenerationExhausted,
    /// A key component could forge the separator.
    #[error("`{component}` contains the `#` key separator")]
    Separator {
        /// The offending component.
        component: String,
    },
    /// A bucket string was not the expected fixed width.
    #[error("`{value}` is not a `{expected}` bucket")]
    MalformedBucket {
        /// The value that was refused.
        value: String,
        /// The shape that was expected.
        expected: &'static str,
    },
}

/// Rejects a component that could forge the separator.
fn component(value: &str) -> Result<&str, ProjectionKeyError> {
    if value.contains('#') {
        return Err(ProjectionKeyError::Separator {
            component: value.to_owned(),
        });
    }
    Ok(value)
}

/// The time grain a rollup row is bucketed at.
///
/// The grain lives in the domain rather than in either adapter because the
/// coarse face's sort key encodes it, so the fold and the reader would otherwise
/// each own half of one spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Grain {
    /// One bucket per UTC hour, `YYYY-MM-DDTHH`.
    Hourly,
    /// One bucket per UTC day, `YYYY-MM-DD`.
    Daily,
}

impl Grain {
    /// Both grains, in bucket-width order.
    pub const ALL: [Self; 2] = [Self::Hourly, Self::Daily];

    /// The single character the sort key carries.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Hourly => "H",
            Self::Daily => "D",
        }
    }

    /// The width a bucket string must have at this grain.
    #[must_use]
    pub const fn bucket_width(self) -> usize {
        match self {
            // `YYYY-MM-DDTHH`
            Self::Hourly => 13,
            // `YYYY-MM-DD`
            Self::Daily => 10,
        }
    }

    /// The shape a bucket string must have at this grain.
    #[must_use]
    pub const fn bucket_shape(self) -> &'static str {
        match self {
            Self::Hourly => "YYYY-MM-DDTHH",
            Self::Daily => "YYYY-MM-DD",
        }
    }

    /// The sort-key prefix a coarse range read is bounded by.
    ///
    /// `T#` sorts strictly after both `D#` and `H#`, so a coarse range can never
    /// pick up a per-tuple rollup and double-count.
    #[must_use]
    pub const fn coarse_prefix(self) -> &'static str {
        match self {
            Self::Hourly => "T#H#",
            Self::Daily => "T#D#",
        }
    }
}

impl fmt::Display for Grain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

/// A composite key into the projection.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectionKey {
    /// The partition key.
    pub pk: String,
    /// The sort key, absent for a prefix query.
    pub sk: Option<String>,
}

/// Read-only key expressions.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectionKeys;

impl ProjectionKeys {
    /// The aggregate partition for one workspace, category and month.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionKeyError::Separator`] for a forging component and
    /// [`ProjectionKeyError::MalformedBucket`] when `month` is not `YYYY-MM`.
    pub fn aggregate_partition(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
        month: &str,
    ) -> Result<String, ProjectionKeyError> {
        if month.len() != 7 || month.as_bytes()[4] != b'-' {
            return Err(ProjectionKeyError::MalformedBucket {
                value: month.to_owned(),
                expected: "YYYY-MM",
            });
        }
        Ok(format!(
            "{}#{}#{}#{}",
            generation.prefix(),
            component(workspace.as_str())?,
            category.id(),
            component(month)?
        ))
    }

    /// The hourly aggregate key.
    ///
    /// # Errors
    ///
    /// As [`ProjectionKeys::aggregate_partition`].
    pub fn hourly(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
        month: &str,
        hour: &str,
        dimension_hash: &str,
    ) -> Result<ProjectionKey, ProjectionKeyError> {
        Ok(ProjectionKey {
            pk: self.aggregate_partition(generation, workspace, category, month)?,
            sk: Some(format!(
                "H#{}#{}",
                component(hour)?,
                component(dimension_hash)?
            )),
        })
    }

    /// The daily aggregate key.
    ///
    /// # Errors
    ///
    /// As [`ProjectionKeys::aggregate_partition`].
    pub fn daily(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
        month: &str,
        day: &str,
        dimension_hash: &str,
    ) -> Result<ProjectionKey, ProjectionKeyError> {
        Ok(ProjectionKey {
            pk: self.aggregate_partition(generation, workspace, category, month)?,
            sk: Some(format!(
                "D#{}#{}",
                component(day)?,
                component(dimension_hash)?
            )),
        })
    }

    /// The coarse rollup key: one row per generation, workspace, public
    /// category, month and bucket, carrying no dimension identity at all.
    ///
    /// This is the face `usage_query` reads. The per-tuple `H#`/`D#` rows are
    /// grouped by a seven-member dimension hash that includes the session, so a
    /// workspace with ten thousand sessions has at least ten thousand rows per
    /// category per day and a monthly total would read hundreds of thousands of
    /// them. The coarse face has exactly one row per `(category, bucket)`, which
    /// is what makes the exact upper bound on rows a query will read pure
    /// arithmetic — known before the first read, so an over-budget query is a
    /// pre-flight refusal rather than a mid-read truncation.
    ///
    /// It shares the aggregate partition with the per-tuple rows and is
    /// separated from them by the `T#` sort-key prefix, which sorts strictly
    /// after both `D#` and `H#`.
    ///
    /// # Errors
    ///
    /// As [`ProjectionKeys::aggregate_partition`], plus
    /// [`ProjectionKeyError::MalformedBucket`] when `bucket` is not the shape
    /// `grain` requires.
    pub fn coarse(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
        month: &str,
        grain: Grain,
        bucket: &str,
    ) -> Result<ProjectionKey, ProjectionKeyError> {
        if bucket.len() != grain.bucket_width() {
            return Err(ProjectionKeyError::MalformedBucket {
                value: bucket.to_owned(),
                expected: grain.bucket_shape(),
            });
        }
        Ok(ProjectionKey {
            pk: self.aggregate_partition(generation, workspace, category, month)?,
            sk: Some(format!("{}{}", grain.coarse_prefix(), component(bucket)?)),
        })
    }

    /// The detail partition for one workspace, category and day.
    ///
    /// Detail rows partition by day rather than by month: a month partition of
    /// per-fact rows is the one place this table could grow a hot key, and the
    /// reader always knows which day it is asking about.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionKeyError::Separator`] for a forging component and
    /// [`ProjectionKeyError::MalformedBucket`] when `day` is not `YYYY-MM-DD`.
    pub fn detail_partition(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
        day: &str,
    ) -> Result<String, ProjectionKeyError> {
        if day.len() != 10 || day.as_bytes()[4] != b'-' || day.as_bytes()[7] != b'-' {
            return Err(ProjectionKeyError::MalformedBucket {
                value: day.to_owned(),
                expected: "YYYY-MM-DD",
            });
        }
        Ok(format!(
            "{}#{}#{}#{}",
            generation.prefix(),
            component(workspace.as_str())?,
            category.id(),
            component(day)?
        ))
    }

    /// One fact's detail key, ordered by service-time start then fact identity.
    ///
    /// # Errors
    ///
    /// As [`ProjectionKeys::detail_partition`].
    pub fn detail(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
        day: &str,
        service_time_start: &str,
        fact: &FactId,
    ) -> Result<ProjectionKey, ProjectionKeyError> {
        Ok(ProjectionKey {
            pk: self.detail_partition(generation, workspace, category, day)?,
            sk: Some(format!(
                "X#{}#{}",
                component(service_time_start)?,
                component(fact.as_str())?
            )),
        })
    }

    /// The coverage vector key.
    ///
    /// This is the row that lets a customer read distinguish "you used nothing"
    /// from "we have not folded your facts yet". A page that omitted it would be
    /// confidently empty rather than honestly incomplete.
    ///
    /// The public category is normalised to its authority's face by
    /// [`coverage_face`], so memory and compute — two public categories in one
    /// authority — share one coverage row. Two rows would each see only a subset
    /// of one contiguous sequence, and the projection fence would stop meaning
    /// anything.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionKeyError::Separator`] for a forging component.
    pub fn coverage(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
    ) -> Result<ProjectionKey, ProjectionKeyError> {
        Ok(ProjectionKey {
            pk: format!(
                "{}#{}#{}",
                generation.prefix(),
                component(workspace.as_str())?,
                coverage_face(category).id()
            ),
            sk: Some("COVERAGE".to_owned()),
        })
    }

    /// The generation pointer key.
    #[must_use]
    pub fn generation_pointer(self) -> ProjectionKey {
        ProjectionKey {
            pk: "GENERATION".to_owned(),
            sk: Some("CURRENT".to_owned()),
        }
    }
}

/// The public category one authority's coverage row is keyed by.
///
/// A frontier is one contiguous sequence per `(workspace, category)`, and the
/// coverage row is that frontier copied. Memory therefore reports its coverage
/// under compute: they are one sequence, so they are one row.
#[must_use]
pub const fn coverage_face(category: PublicCategory) -> PublicCategory {
    match category.category() {
        Category::Storage => PublicCategory::Storage,
        Category::Compute => PublicCategory::Compute,
        Category::Transfer => PublicCategory::DataTransfer,
    }
}

#[cfg(test)]
mod tests {
    use super::{Generation, Grain, MAX_GENERATION, ProjectionKeyError, ProjectionKeys};
    use crate::meter::PublicCategory;
    use crate::wire_pending::WorkspaceId;

    fn workspace() -> WorkspaceId {
        WorkspaceId::parse("ws-1").expect("workspace")
    }

    #[test]
    fn the_generation_prefix_is_fixed_width_so_a_range_cannot_straddle_two() {
        assert_eq!(Generation::FIRST.prefix(), "G0000");
        assert_eq!(Generation::new(7).expect("in range").prefix(), "G0007");
        assert_eq!(Generation::new(1_234).expect("in range").prefix(), "G1234");

        // Every prefix is the same length, so lexical ordering never mixes them.
        let widths: Vec<usize> = [0u16, 1, 99, 1_000, MAX_GENERATION]
            .map(|value| Generation::new(value).expect("in range").prefix().len())
            .to_vec();
        assert!(widths.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn a_rebuild_writes_into_the_next_generation() {
        assert_eq!(
            Generation::FIRST.next().expect("advances"),
            Generation::new(1).expect("in range")
        );
        assert!(matches!(
            Generation::new(u16::MAX),
            Err(ProjectionKeyError::GenerationExhausted)
        ));
        assert!(matches!(
            Generation::new(MAX_GENERATION)
                .expect("the ceiling is in range")
                .next(),
            Err(ProjectionKeyError::GenerationExhausted)
        ));
    }

    #[test]
    fn aggregates_partition_by_generation_workspace_category_and_month() {
        let keys = ProjectionKeys;
        let hourly = keys
            .hourly(
                Generation::new(2).expect("in range"),
                &workspace(),
                PublicCategory::Compute,
                "2026-08",
                "2026-08-01T12",
                "abcd1234",
            )
            .expect("builds");
        assert_eq!(hourly.pk, "G0002#ws-1#compute#2026-08");
        assert_eq!(hourly.sk.as_deref(), Some("H#2026-08-01T12#abcd1234"));

        let daily = keys
            .daily(
                Generation::new(2).expect("in range"),
                &workspace(),
                PublicCategory::Compute,
                "2026-08",
                "2026-08-01",
                "abcd1234",
            )
            .expect("builds");
        assert_eq!(
            daily.pk, hourly.pk,
            "hourly and daily rollups share one partition, so one query reads both"
        );
        assert_eq!(daily.sk.as_deref(), Some("D#2026-08-01#abcd1234"));
    }

    #[test]
    fn the_coarse_face_shares_the_partition_and_carries_no_dimension_identity() {
        let keys = ProjectionKeys;
        let generation = Generation::new(2).expect("in range");
        let hourly = keys
            .coarse(
                generation,
                &workspace(),
                PublicCategory::Compute,
                "2026-08",
                Grain::Hourly,
                "2026-08-01T12",
            )
            .expect("builds");
        assert_eq!(hourly.pk, "G0002#ws-1#compute#2026-08");
        assert_eq!(hourly.sk.as_deref(), Some("T#H#2026-08-01T12"));

        let daily = keys
            .coarse(
                generation,
                &workspace(),
                PublicCategory::Compute,
                "2026-08",
                Grain::Daily,
                "2026-08-01",
            )
            .expect("builds");
        assert_eq!(daily.pk, hourly.pk);
        assert_eq!(daily.sk.as_deref(), Some("T#D#2026-08-01"));

        // One row per (category, bucket): the sort key names no dimension hash,
        // so the row count a range read touches is arithmetic, not data.
        let again = keys
            .coarse(
                generation,
                &workspace(),
                PublicCategory::Compute,
                "2026-08",
                Grain::Hourly,
                "2026-08-01T12",
            )
            .expect("builds");
        assert_eq!(again, hourly);
    }

    #[test]
    fn the_coarse_prefix_sorts_after_every_per_tuple_rollup() {
        let keys = ProjectionKeys;
        let per_tuple = keys
            .hourly(
                Generation::FIRST,
                &workspace(),
                PublicCategory::Storage,
                "2026-08",
                "2026-08-01T12",
                "abcd1234abcd1234",
            )
            .expect("builds")
            .sk
            .expect("a sort key");
        let daily = keys
            .daily(
                Generation::FIRST,
                &workspace(),
                PublicCategory::Storage,
                "2026-08",
                "2026-08-01",
                "abcd1234abcd1234",
            )
            .expect("builds")
            .sk
            .expect("a sort key");
        let coarse = keys
            .coarse(
                Generation::FIRST,
                &workspace(),
                PublicCategory::Storage,
                "2026-08",
                Grain::Hourly,
                "2026-08-01T12",
            )
            .expect("builds")
            .sk
            .expect("a sort key");
        assert!(coarse > per_tuple, "a `T#` row sorts after every `H#` row");
        assert!(coarse > daily, "a `T#` row sorts after every `D#` row");
        for prefix in [Grain::Hourly.coarse_prefix(), Grain::Daily.coarse_prefix()] {
            assert!(
                !per_tuple.starts_with(prefix) && !daily.starts_with(prefix),
                "a coarse range must never pick up a per-tuple rollup and double-count"
            );
        }
        assert!(
            !Grain::Hourly
                .coarse_prefix()
                .starts_with(Grain::Daily.coarse_prefix())
                && !Grain::Daily
                    .coarse_prefix()
                    .starts_with(Grain::Hourly.coarse_prefix()),
            "the two coarse grains occupy disjoint sort-key space"
        );
    }

    #[test]
    fn a_coarse_bucket_of_the_wrong_shape_is_refused_rather_than_padded() {
        let keys = ProjectionKeys;
        for (grain, bad) in [
            (Grain::Hourly, "2026-08-01"),
            (Grain::Hourly, "2026-08-01T12:00"),
            (Grain::Daily, "2026-08-01T12"),
            (Grain::Daily, "2026-08"),
        ] {
            assert!(
                matches!(
                    keys.coarse(
                        Generation::FIRST,
                        &workspace(),
                        PublicCategory::Compute,
                        "2026-08",
                        grain,
                        bad,
                    ),
                    Err(ProjectionKeyError::MalformedBucket { .. })
                ),
                "`{bad}` is not a {} bucket",
                grain.bucket_shape()
            );
        }
    }

    #[test]
    fn the_four_public_categories_each_get_their_own_partition() {
        let keys = ProjectionKeys;
        let mut partitions: Vec<String> = PublicCategory::ALL
            .into_iter()
            .map(|category| {
                keys.aggregate_partition(Generation::FIRST, &workspace(), category, "2026-08")
                    .expect("builds")
            })
            .collect();
        let count = partitions.len();
        partitions.sort();
        partitions.dedup();
        assert_eq!(
            count,
            partitions.len(),
            "no two categories share a partition"
        );
    }

    #[test]
    fn a_malformed_month_bucket_is_refused() {
        let keys = ProjectionKeys;
        for bad in ["2026", "2026-8", "2026-08-01", "26-08"] {
            assert!(
                keys.aggregate_partition(
                    Generation::FIRST,
                    &workspace(),
                    PublicCategory::Storage,
                    bad,
                )
                .is_err(),
                "`{bad}` is not a YYYY-MM bucket"
            );
        }
    }

    #[test]
    fn a_component_that_could_forge_a_separator_is_refused() {
        let keys = ProjectionKeys;
        assert!(matches!(
            keys.hourly(
                Generation::FIRST,
                &workspace(),
                PublicCategory::Storage,
                "2026-08",
                "2026-08-01T12",
                "ab#cd",
            ),
            Err(ProjectionKeyError::Separator { .. })
        ));
    }

    #[test]
    fn the_coverage_vector_has_one_row_per_workspace_and_authority() {
        let keys = ProjectionKeys;
        let coverage = keys
            .coverage(
                Generation::new(1).expect("in range"),
                &workspace(),
                PublicCategory::Memory,
            )
            .expect("builds");
        // Memory reports under its authority's face: one sequence, one row.
        assert_eq!(coverage.pk, "G0001#ws-1#compute");
        assert_eq!(coverage.sk.as_deref(), Some("COVERAGE"));

        // The coverage row sits outside every month partition, so it is readable
        // without knowing which months a workspace has data in.
        let aggregate = keys
            .aggregate_partition(
                Generation::new(1).expect("in range"),
                &workspace(),
                PublicCategory::Memory,
                "2026-08",
            )
            .expect("builds");
        assert_ne!(coverage.pk, aggregate);
    }

    #[test]
    fn memory_and_compute_share_one_coverage_row() {
        let keys = ProjectionKeys;
        let compute = keys
            .coverage(Generation::FIRST, &workspace(), PublicCategory::Compute)
            .expect("builds");
        let memory = keys
            .coverage(Generation::FIRST, &workspace(), PublicCategory::Memory)
            .expect("builds");
        assert_eq!(
            compute, memory,
            "one authority is one contiguous sequence, so it is one coverage row"
        );
        assert!(compute.pk.ends_with("#compute"));

        // The other two authorities keep their own rows.
        let storage = keys
            .coverage(Generation::FIRST, &workspace(), PublicCategory::Storage)
            .expect("builds");
        let transfer = keys
            .coverage(
                Generation::FIRST,
                &workspace(),
                PublicCategory::DataTransfer,
            )
            .expect("builds");
        assert_ne!(compute, storage);
        assert_ne!(compute, transfer);
        assert_ne!(storage, transfer);
    }

    #[test]
    fn the_generation_pointer_is_a_single_fixed_row() {
        let pointer = ProjectionKeys.generation_pointer();
        assert_eq!(pointer.pk, "GENERATION");
        assert_eq!(pointer.sk.as_deref(), Some("CURRENT"));
    }

    #[test]
    fn two_generations_never_share_a_key() {
        let keys = ProjectionKeys;
        let first = keys
            .aggregate_partition(
                Generation::new(1).expect("in range"),
                &workspace(),
                PublicCategory::Storage,
                "2026-08",
            )
            .expect("builds");
        let second = keys
            .aggregate_partition(
                Generation::new(2).expect("in range"),
                &workspace(),
                PublicCategory::Storage,
                "2026-08",
            )
            .expect("builds");
        assert_ne!(
            first, second,
            "a rebuild must write generation n+1 without touching n"
        );
    }
}
