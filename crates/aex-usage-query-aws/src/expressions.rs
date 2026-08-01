//! Read-only key expressions for `usage-query-projection`.
//!
//! The projection is generation-keyed, so a rebuild is a normal operation rather
//! than an outage: generation `n+1` is written under its own prefix and the
//! pointer flips only once the rebuild has been verified. A cursor binds the
//! generation it was issued against, so a stale cursor expires instead of
//! silently mixing two generations.
//!
//! Nothing here writes. That is proved rather than asserted — see
//! `tests/write_incapability.rs` for the source-conformance and link-graph
//! halves of `U-20`.

use std::fmt;

use aex_usage_domain::meter::PublicCategory;
use aex_usage_domain::wire_pending::WorkspaceId;

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
    /// Returns [`QueryError::GenerationExhausted`] above [`MAX_GENERATION`].
    pub const fn new(value: u16) -> Result<Self, QueryError> {
        if value > MAX_GENERATION {
            return Err(QueryError::GenerationExhausted);
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
    /// Returns [`QueryError::GenerationExhausted`] at `u16::MAX`.
    pub const fn next(self) -> Result<Self, QueryError> {
        match self.0.checked_add(1) {
            Some(value) => Self::new(value),
            None => Err(QueryError::GenerationExhausted),
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
pub enum QueryError {
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
fn component(value: &str) -> Result<&str, QueryError> {
    if value.contains('#') {
        return Err(QueryError::Separator {
            component: value.to_owned(),
        });
    }
    Ok(value)
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
    /// Returns [`QueryError::Separator`] for a forging component and
    /// [`QueryError::MalformedBucket`] when `month` is not `YYYY-MM`.
    pub fn aggregate_partition(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
        month: &str,
    ) -> Result<String, QueryError> {
        if month.len() != 7 || month.as_bytes()[4] != b'-' {
            return Err(QueryError::MalformedBucket {
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
    ) -> Result<ProjectionKey, QueryError> {
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
    ) -> Result<ProjectionKey, QueryError> {
        Ok(ProjectionKey {
            pk: self.aggregate_partition(generation, workspace, category, month)?,
            sk: Some(format!(
                "D#{}#{}",
                component(day)?,
                component(dimension_hash)?
            )),
        })
    }

    /// The coverage vector key.
    ///
    /// This is the row that lets a customer read distinguish "you used nothing"
    /// from "we have not folded your facts yet". A page that omitted it would be
    /// confidently empty rather than honestly incomplete.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::Separator`] for a forging component.
    pub fn coverage(
        self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
    ) -> Result<ProjectionKey, QueryError> {
        Ok(ProjectionKey {
            pk: format!(
                "{}#{}#{}",
                generation.prefix(),
                component(workspace.as_str())?,
                category.id()
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

#[cfg(test)]
mod tests {
    use super::{Generation, MAX_GENERATION, ProjectionKeys, QueryError};
    use aex_usage_domain::meter::PublicCategory;
    use aex_usage_domain::wire_pending::WorkspaceId;

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
            Err(QueryError::GenerationExhausted)
        ));
        assert!(matches!(
            Generation::new(MAX_GENERATION)
                .expect("the ceiling is in range")
                .next(),
            Err(QueryError::GenerationExhausted)
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
            Err(QueryError::Separator { .. })
        ));
    }

    #[test]
    fn the_coverage_vector_has_one_row_per_workspace_and_category() {
        let keys = ProjectionKeys;
        let coverage = keys
            .coverage(
                Generation::new(1).expect("in range"),
                &workspace(),
                PublicCategory::Memory,
            )
            .expect("builds");
        assert_eq!(coverage.pk, "G0001#ws-1#memory");
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
