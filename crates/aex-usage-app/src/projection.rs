//! The pure fold from an admitted fact into the query projection.
//!
//! Nothing here talks to a table. The fold produces a neutral
//! [`ProjectionTransaction`] — a key, an `ADD` clause, a `SET` clause and a
//! condition per row — and each authority adapter renders that into one
//! `TransactWriteItems`. Keeping the fold pure is what lets the replay,
//! duplicate, disorder and correction properties be asserted exactly, with no
//! engine and no credentials.
//!
//! Three rules the shape enforces rather than documents:
//!
//! - **The coverage condition is the exactly-once fence.** Every transaction
//!   carries one coverage write conditioned on the previous projected sequence,
//!   so a redelivered fact either applies the whole transaction or none of it.
//! - **A correction never rewrites its target.** A `Void` subtracts the
//!   target's quantity and deactivates the target's detail row; a `Replace`
//!   does that and adds its own. The authority row is untouched either way.
//! - **A zero-dollar observability fact folds into no priced aggregate.** It has
//!   no [`PublicCategory`], so there is no aggregate partition it could land in;
//!   it advances the frontier and nothing else.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use aex_usage_domain::fact::{FactKind, UsageFact};
use aex_usage_domain::frontier::{AcceptedSequence, Frontier, FrontierState};
use aex_usage_domain::identity::FactId;
use aex_usage_domain::intent::{CanonicalError, IntentHash};
use aex_usage_domain::keys::ItemValue;
use aex_usage_domain::measurement::Measurement;
use aex_usage_domain::meter::PublicCategory;
use aex_usage_domain::projection::{Generation, ProjectionKey, ProjectionKeyError, ProjectionKeys};
use aex_usage_domain::quantity::Quantity;
use aex_usage_domain::wire_pending::TimestampError;
use serde::Serialize;

/// How long a detail row survives.
///
/// Ninety days of per-fact convenience over facts that stay in the authority
/// forever, so expiring one loses nothing recoverable.
pub const DETAIL_RETENTION_DAYS: u64 = 90;

/// The number of hex characters a dimension hash keeps.
///
/// Sixteen hex characters is 64 bits over a declared tuple that is itself stored
/// on the row, so a collision costs a merged rollup rather than a wrong answer,
/// and the reader never has to invert the hash.
pub const DIMENSION_HASH_HEX: usize = 16;

/// Why a fact could not be folded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FoldError {
    /// A correction was folded without its target.
    ///
    /// A correction's aggregate effect is expressed in the target's dimensions,
    /// not its own, so folding one without the target would subtract from the
    /// wrong rollup.
    #[error("correction `{correction}` cannot be folded without its target `{target}`")]
    TargetRequired {
        /// The correction fact.
        correction: String,
        /// The fact it corrects.
        target: String,
    },
    /// The supplied target is not the one the correction names.
    #[error("correction `{correction}` targets `{target}` but `{supplied}` was supplied")]
    TargetMismatch {
        /// The correction fact.
        correction: String,
        /// The fact the correction names.
        target: String,
        /// The fact that was supplied.
        supplied: String,
    },
    /// The correction and its target belong to different workspaces or regions.
    #[error("correction `{correction}` and target `{target}` are not in the same {scope}")]
    TargetForeign {
        /// The correction fact.
        correction: String,
        /// The fact it corrects.
        target: String,
        /// Which scope disagreed.
        scope: &'static str,
    },
    /// The target carries no measurement to withdraw.
    #[error("target `{target}` carries no measurement, so there is nothing to withdraw")]
    TargetNotMeasured {
        /// The fact that was supplied as a target.
        target: String,
    },
    /// The frontier does not describe this fact's position.
    #[error(
        "frontier reports projected {projected} for a fact at sequence {sequence}; \
         the coverage fence would be written against the wrong position"
    )]
    FrontierMisaligned {
        /// The sequence the frontier says was last projected.
        projected: u64,
        /// The sequence being folded.
        sequence: u64,
    },
    /// The frontier belongs to a different workspace or category.
    #[error("frontier for `{frontier}` cannot fence a fact in `{fact}`")]
    FrontierForeign {
        /// What the frontier describes.
        frontier: String,
        /// What the fact describes.
        fact: String,
    },
    /// A key could not be built.
    #[error(transparent)]
    Key(#[from] ProjectionKeyError),
    /// The dimension tuple could not be canonicalized.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    /// A derived instant was not representable.
    #[error(transparent)]
    Timestamp(#[from] TimestampError),
}

/// Which direction a quantity moves a rollup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// A new measurement.
    Add,
    /// A withdrawn measurement.
    Subtract,
}

/// A signed integer delta, rendered exactly as `DynamoDB`'s `ADD` clause reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignedDelta {
    /// Whether the delta adds or subtracts.
    pub sign: Sign,
    /// The unsigned magnitude.
    pub magnitude: u128,
}

impl SignedDelta {
    /// An addition.
    #[must_use]
    pub const fn add(magnitude: u128) -> Self {
        Self {
            sign: Sign::Add,
            magnitude,
        }
    }

    /// A subtraction.
    #[must_use]
    pub const fn subtract(magnitude: u128) -> Self {
        Self {
            sign: Sign::Subtract,
            magnitude,
        }
    }

    /// The decimal literal an `ADD` clause carries.
    #[must_use]
    pub fn render(self) -> String {
        match self.sign {
            Sign::Add => self.magnitude.to_string(),
            Sign::Subtract => format!("-{}", self.magnitude),
        }
    }
}

/// The declared dimension tuple one rollup is grouped by.
///
/// The tuple is stored on the row as well as hashed into its sort key, so a
/// reader never needs to invert the hash and grouping can never be unbounded:
/// there is no dimension outside this closed set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DimensionTuple {
    /// The deployable that produced the measurement.
    pub service: String,
    /// What kind of physical resource was measured.
    pub resource_kind: String,
    /// Which receipt family the evidence came from.
    pub source: String,
    /// Whether the quantity was consumed or reserved.
    pub basis: String,
    /// The session the work belongs to, when it belongs to one.
    pub session: Option<String>,
    /// The run the work belongs to, when it belongs to one.
    pub run: Option<String>,
    /// The operation the work belongs to, when it belongs to one.
    pub operation: Option<String>,
}

impl DimensionTuple {
    /// The tuple a measured fact is grouped under.
    ///
    /// Returns `None` for a fact carrying no measurement — a `Void` is folded
    /// under its target's tuple, and an observability fact folds into no
    /// aggregate at all.
    #[must_use]
    pub fn of(fact: &UsageFact) -> Option<Self> {
        let measurement: &Measurement = fact.kind.measurement()?;
        Some(Self {
            service: fact.service.to_string(),
            resource_kind: fact.resource.kind.id().to_owned(),
            source: measurement.source_receipt().kind.id().to_owned(),
            basis: measurement.basis().id().to_owned(),
            session: fact.attribution.session.as_ref().map(ToString::to_string),
            run: fact.attribution.run.as_ref().map(ToString::to_string),
            operation: fact.attribution.operation.as_ref().map(ToString::to_string),
        })
    }

    /// The stable sixteen-hex-character group identifier.
    ///
    /// Canonicalization goes through the one workspace rule, so this hash and
    /// the intent hash can never disagree about what "the same tuple" means.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalError`] when the tuple cannot be canonicalized, which
    /// a tuple of strings never is.
    pub fn hash(&self) -> Result<String, CanonicalError> {
        let full = IntentHash::of(self)?.to_string();
        Ok(full.chars().take(DIMENSION_HASH_HEX).collect())
    }

    /// The tuple as a nested attribute map.
    #[must_use]
    pub fn to_item(&self) -> ItemValue {
        let mut map = BTreeMap::from([
            ("service".to_owned(), ItemValue::text(self.service.clone())),
            (
                "resourceKind".to_owned(),
                ItemValue::text(self.resource_kind.clone()),
            ),
            ("source".to_owned(), ItemValue::text(self.source.clone())),
            ("basis".to_owned(), ItemValue::text(self.basis.clone())),
        ]);
        for (name, value) in [
            ("sessionId", self.session.as_ref()),
            ("runId", self.run.as_ref()),
            ("operationId", self.operation.as_ref()),
        ] {
            if let Some(value) = value {
                map.insert(name.to_owned(), ItemValue::text(value.clone()));
            }
        }
        ItemValue::M(map)
    }
}

/// The condition one projection write is guarded by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionCondition {
    /// A rollup may only ever accumulate its own public category.
    ///
    /// The projection is a shared table, so this is the guard that keeps a
    /// mis-keyed write from silently merging two categories' quantities.
    CategoryMatches {
        /// The category this row belongs to.
        category: PublicCategory,
    },
    /// The row must not already exist.
    WriteOnce,
    /// The row must already exist.
    Exists,
    /// The coverage row must still be at the position this fold expects.
    ///
    /// This is the exactly-once fence for the whole transaction: a redelivered
    /// fact whose sequence is already covered fails the condition and applies
    /// nothing.
    ProjectedSequenceIs {
        /// Where the coverage row must currently be.
        previous: AcceptedSequence,
    },
}

impl ProjectionCondition {
    /// The condition expression.
    #[must_use]
    pub const fn expression(&self) -> &'static str {
        match self {
            Self::CategoryMatches { .. } => {
                "attribute_not_exists(publicCategory) OR publicCategory = :conditionCategory"
            }
            Self::WriteOnce => "attribute_not_exists(pk)",
            Self::Exists => "attribute_exists(pk)",
            Self::ProjectedSequenceIs { .. } => {
                "attribute_not_exists(projectedSequence) OR \
                 projectedSequence = :conditionProjectedSequence"
            }
        }
    }

    /// The values the condition expression binds.
    #[must_use]
    pub fn values(&self) -> BTreeMap<String, ItemValue> {
        match self {
            Self::CategoryMatches { category } => BTreeMap::from([(
                ":conditionCategory".to_owned(),
                ItemValue::text(category.id()),
            )]),
            Self::ProjectedSequenceIs { previous } => BTreeMap::from([(
                ":conditionProjectedSequence".to_owned(),
                ItemValue::number(previous.get()),
            )]),
            Self::WriteOnce | Self::Exists => BTreeMap::new(),
        }
    }
}

/// Which projection row a write addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProjectionRow {
    /// An hourly rollup.
    HourlyAggregate,
    /// A daily rollup.
    DailyAggregate,
    /// One fact's detail row.
    Detail,
    /// One fact's detail row being withdrawn by a correction.
    DetailWithdrawal,
    /// The copied frontier.
    Coverage,
}

impl ProjectionRow {
    /// The `itemType` this row declares.
    #[must_use]
    pub const fn item_type(self) -> &'static str {
        match self {
            Self::HourlyAggregate | Self::DailyAggregate => "usage_aggregate",
            Self::Detail | Self::DetailWithdrawal => "usage_detail",
            Self::Coverage => "usage_coverage",
        }
    }
}

/// One conditional write against `usage-query-projection`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionWrite {
    /// Which row this addresses.
    pub row: ProjectionRow,
    /// The composite key.
    pub key: ProjectionKey,
    /// The `ADD` clause: accumulating counters only.
    pub add: BTreeMap<String, SignedDelta>,
    /// The `SET` clause: last-writer values.
    pub set: BTreeMap<String, ItemValue>,
    /// The guard.
    pub condition: ProjectionCondition,
}

/// The whole fold of one fact, as one atomic transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionTransaction {
    /// The generation this fold writes into.
    pub generation: Generation,
    /// The sequence being projected.
    pub sequence: AcceptedSequence,
    /// Every write, aggregates first, coverage last.
    pub writes: Vec<ProjectionWrite>,
}

impl ProjectionTransaction {
    /// The coverage write that fences the whole transaction.
    ///
    /// There is always exactly one, which is why this returns a reference rather
    /// than an option: a transaction without the fence would not be exactly
    /// once, and the constructor never builds one.
    ///
    /// # Panics
    ///
    /// Never: [`fold_fact`] always appends exactly one coverage write.
    #[must_use]
    pub fn fence(&self) -> &ProjectionWrite {
        self.writes
            .iter()
            .find(|write| write.row == ProjectionRow::Coverage)
            .expect("every folded transaction carries exactly one coverage fence")
    }
}

/// What one fold needs beyond the fact itself.
#[derive(Debug, Clone, Copy)]
pub struct FoldContext<'a> {
    /// The generation being written.
    pub generation: Generation,
    /// The frontier as it stands *before* this fact is projected.
    pub frontier: &'a Frontier,
    /// The fact a correction withdraws, when this fact is a correction.
    pub target: Option<&'a UsageFact>,
}

/// Folds one admitted fact into the projection.
///
/// # Errors
///
/// Returns [`FoldError::TargetRequired`], [`FoldError::TargetMismatch`],
/// [`FoldError::TargetForeign`] or [`FoldError::TargetNotMeasured`] when a
/// correction's target is absent or wrong, [`FoldError::FrontierMisaligned`] or
/// [`FoldError::FrontierForeign`] when the supplied frontier does not describe
/// this fact's position, and [`FoldError::Key`] when a key cannot be built.
pub fn fold_fact(
    fact: &UsageFact,
    context: FoldContext<'_>,
) -> Result<ProjectionTransaction, FoldError> {
    check_frontier(fact, context.frontier)?;
    let keys = ProjectionKeys;
    let mut writes = Vec::new();

    // The fact's own contribution, when it has one. An observability fact has no
    // public category, so it contributes nothing but a frontier advance.
    if let (Some(measurement), Some(category)) = (fact.kind.measurement(), fact.public_category()) {
        let tuple = DimensionTuple::of(fact).ok_or_else(|| FoldError::TargetNotMeasured {
            target: fact.fact_id.to_string(),
        })?;
        writes.extend(aggregate_writes(
            fact,
            category,
            &tuple,
            measurement,
            SignedDelta::add(measurement.quantity().get()),
            1,
            context.generation,
            keys,
        )?);
        writes.push(detail_write(
            fact,
            category,
            measurement,
            context.generation,
            keys,
        )?);
    }

    // A correction's withdrawal, expressed in the target's dimensions.
    if let Some(correction) = fact.kind.correction() {
        let target = context.target.ok_or_else(|| FoldError::TargetRequired {
            correction: fact.fact_id.to_string(),
            target: correction.target.to_string(),
        })?;
        check_target(fact, &correction.target, target)?;
        let measurement =
            target
                .kind
                .measurement()
                .ok_or_else(|| FoldError::TargetNotMeasured {
                    target: target.fact_id.to_string(),
                })?;
        let category = target
            .public_category()
            .ok_or_else(|| FoldError::TargetNotMeasured {
                target: target.fact_id.to_string(),
            })?;
        let tuple = DimensionTuple::of(target).ok_or_else(|| FoldError::TargetNotMeasured {
            target: target.fact_id.to_string(),
        })?;
        writes.extend(aggregate_writes(
            target,
            category,
            &tuple,
            measurement,
            SignedDelta::subtract(measurement.quantity().get()),
            -1,
            context.generation,
            keys,
        )?);
        writes.push(withdrawal_write(
            target,
            category,
            measurement,
            fact,
            context.generation,
            keys,
        )?);
    }

    writes.push(coverage_write(fact, context, keys)?);
    Ok(ProjectionTransaction {
        generation: context.generation,
        sequence: fact.accepted_sequence,
        writes,
    })
}

/// Refuses a frontier that does not describe this fact's position.
fn check_frontier(fact: &UsageFact, frontier: &Frontier) -> Result<(), FoldError> {
    if frontier.workspace != fact.workspace
        || frontier.category != fact.category()
        || frontier.region != fact.region
    {
        return Err(FoldError::FrontierForeign {
            frontier: format!(
                "{}/{}/{}",
                frontier.region, frontier.workspace, frontier.category
            ),
            fact: format!("{}/{}/{}", fact.region, fact.workspace, fact.category()),
        });
    }
    // The fold writes the coverage fence against `projected + 1`, so anything
    // else means the caller is folding out of order and the fence would be
    // written against a position this fact does not occupy.
    if frontier.projected.get() + 1 != fact.accepted_sequence.get() {
        return Err(FoldError::FrontierMisaligned {
            projected: frontier.projected.get(),
            sequence: fact.accepted_sequence.get(),
        });
    }
    Ok(())
}

/// Refuses a target that is not the one the correction names.
fn check_target(fact: &UsageFact, named: &FactId, supplied: &UsageFact) -> Result<(), FoldError> {
    if named != &supplied.fact_id {
        return Err(FoldError::TargetMismatch {
            correction: fact.fact_id.to_string(),
            target: named.to_string(),
            supplied: supplied.fact_id.to_string(),
        });
    }
    for (scope, same) in [
        ("workspace", fact.workspace == supplied.workspace),
        ("region", fact.region == supplied.region),
        ("organization", fact.organization == supplied.organization),
    ] {
        if !same {
            return Err(FoldError::TargetForeign {
                correction: fact.fact_id.to_string(),
                target: supplied.fact_id.to_string(),
                scope,
            });
        }
    }
    Ok(())
}

/// The hourly and daily rollup writes for one contribution.
#[allow(clippy::too_many_arguments)]
fn aggregate_writes(
    fact: &UsageFact,
    category: PublicCategory,
    tuple: &DimensionTuple,
    measurement: &Measurement,
    delta: SignedDelta,
    fact_count: i64,
    generation: Generation,
    keys: ProjectionKeys,
) -> Result<Vec<ProjectionWrite>, FoldError> {
    let start = measurement.service_time().start();
    let hash = tuple.hash()?;
    let month = start.month_bucket();
    let day = start.day_bucket();
    let hour = start.hour_bucket();

    let count = SignedDelta {
        sign: if fact_count < 0 {
            Sign::Subtract
        } else {
            Sign::Add
        },
        magnitude: fact_count.unsigned_abs().into(),
    };
    let add = BTreeMap::from([
        ("quantity".to_owned(), delta),
        ("factCount".to_owned(), count),
    ]);
    let condition = ProjectionCondition::CategoryMatches { category };

    let mut writes = Vec::with_capacity(2);
    for (row, key, bucket_start, bucket_end) in [
        (
            ProjectionRow::HourlyAggregate,
            keys.hourly(generation, &fact.workspace, category, &month, &hour, &hash)?,
            hour.clone(),
            hour.clone(),
        ),
        (
            ProjectionRow::DailyAggregate,
            keys.daily(generation, &fact.workspace, category, &month, &day, &hash)?,
            day.clone(),
            day.clone(),
        ),
    ] {
        let mut set = BTreeMap::from([
            (
                "itemType".to_owned(),
                ItemValue::text(ProjectionRow::HourlyAggregate.item_type()),
            ),
            ("generation".to_owned(), ItemValue::number(generation.get())),
            (
                "workspaceId".to_owned(),
                ItemValue::text(fact.workspace.to_string()),
            ),
            (
                "organizationId".to_owned(),
                ItemValue::text(fact.organization.to_string()),
            ),
            ("publicCategory".to_owned(), ItemValue::text(category.id())),
            (
                "meter".to_owned(),
                ItemValue::text(measurement.meter().id()),
            ),
            ("bucketStart".to_owned(), ItemValue::text(bucket_start)),
            ("bucketEnd".to_owned(), ItemValue::text(bucket_end)),
            ("dimensions".to_owned(), tuple.to_item()),
            (
                "highestSequence".to_owned(),
                ItemValue::number(fact.accepted_sequence.get()),
            ),
            ("updatedAt".to_owned(), ItemValue::instant(fact.accepted_at)),
        ]);
        set.insert(
            "itemType".to_owned(),
            ItemValue::text(row.item_type().to_owned()),
        );
        writes.push(ProjectionWrite {
            row,
            key,
            add: add.clone(),
            set,
            condition: condition.clone(),
        });
    }
    Ok(writes)
}

/// One fact's detail row.
fn detail_write(
    fact: &UsageFact,
    category: PublicCategory,
    measurement: &Measurement,
    generation: Generation,
    keys: ProjectionKeys,
) -> Result<ProjectionWrite, FoldError> {
    let start = measurement.service_time().start();
    let key = keys.detail(
        generation,
        &fact.workspace,
        category,
        &start.day_bucket(),
        &start.to_canonical(),
        &fact.fact_id,
    )?;
    let expires = fact
        .accepted_at
        .plus_millis(DETAIL_RETENTION_DAYS * 24 * 60 * 60 * 1_000)?;
    let mut set = BTreeMap::from([
        (
            "itemType".to_owned(),
            ItemValue::text(ProjectionRow::Detail.item_type()),
        ),
        ("generation".to_owned(), ItemValue::number(generation.get())),
        (
            "workspaceId".to_owned(),
            ItemValue::text(fact.workspace.to_string()),
        ),
        (
            "organizationId".to_owned(),
            ItemValue::text(fact.organization.to_string()),
        ),
        ("publicCategory".to_owned(), ItemValue::text(category.id())),
        (
            "meter".to_owned(),
            ItemValue::text(measurement.meter().id()),
        ),
        (
            "factId".to_owned(),
            ItemValue::text(fact.fact_id.to_string()),
        ),
        (
            "quantity".to_owned(),
            ItemValue::N(measurement.quantity().to_string()),
        ),
        (
            "serviceTimeStart".to_owned(),
            ItemValue::instant(measurement.service_time().start()),
        ),
        (
            "serviceTimeEnd".to_owned(),
            ItemValue::instant(measurement.service_time().end()),
        ),
        (
            "basis".to_owned(),
            ItemValue::text(measurement.basis().id()),
        ),
        (
            "sourceReceiptKind".to_owned(),
            ItemValue::text(measurement.source_receipt().kind.id()),
        ),
        (
            "acceptedSequence".to_owned(),
            ItemValue::number(fact.accepted_sequence.get()),
        ),
        ("active".to_owned(), ItemValue::Bool(true)),
        // The epoch attribute is what DynamoDB reclaims on; the string is what
        // the reader checks. TTL is never the fence, so both are written.
        (
            "expiresAtEpochSeconds".to_owned(),
            ItemValue::number(u64::try_from(expires.unix_millis().max(0) / 1_000).unwrap_or(0)),
        ),
        ("expiresAt".to_owned(), ItemValue::instant(expires)),
    ]);
    if let FactKind::Replace { correction, .. } = &fact.kind {
        set.insert(
            "correctionOf".to_owned(),
            ItemValue::text(correction.target.to_string()),
        );
    }
    Ok(ProjectionWrite {
        row: ProjectionRow::Detail,
        key,
        add: BTreeMap::new(),
        set,
        condition: ProjectionCondition::WriteOnce,
    })
}

/// The withdrawal of a corrected fact's detail row.
///
/// The row is deactivated, never deleted: public usage detail may show the
/// correction lineage, and the original measurement stays visible as withdrawn
/// rather than vanishing.
fn withdrawal_write(
    target: &UsageFact,
    category: PublicCategory,
    measurement: &Measurement,
    correction: &UsageFact,
    generation: Generation,
    keys: ProjectionKeys,
) -> Result<ProjectionWrite, FoldError> {
    let start = measurement.service_time().start();
    let key = keys.detail(
        generation,
        &target.workspace,
        category,
        &start.day_bucket(),
        &start.to_canonical(),
        &target.fact_id,
    )?;
    Ok(ProjectionWrite {
        row: ProjectionRow::DetailWithdrawal,
        key,
        add: BTreeMap::new(),
        set: BTreeMap::from([
            ("active".to_owned(), ItemValue::Bool(false)),
            (
                "withdrawnBy".to_owned(),
                ItemValue::text(correction.fact_id.to_string()),
            ),
            (
                "withdrawnAt".to_owned(),
                ItemValue::instant(correction.accepted_at),
            ),
        ]),
        condition: ProjectionCondition::Exists,
    })
}

/// The coverage row, which is also the transaction's exactly-once fence.
fn coverage_write(
    fact: &UsageFact,
    context: FoldContext<'_>,
    keys: ProjectionKeys,
) -> Result<ProjectionWrite, FoldError> {
    // Every fact in one authority sequence lands on one coverage row, whichever
    // public category it reports under. A fact without a public category at all
    // still advances the frontier, so it still writes the fence.
    let category = canonical_public(fact.category());
    let key = keys.coverage(context.generation, &fact.workspace, category)?;
    let frontier = context.frontier;
    let mut set = BTreeMap::from([
        (
            "itemType".to_owned(),
            ItemValue::text(ProjectionRow::Coverage.item_type()),
        ),
        (
            "generation".to_owned(),
            ItemValue::number(context.generation.get()),
        ),
        (
            "workspaceId".to_owned(),
            ItemValue::text(fact.workspace.to_string()),
        ),
        (
            "organizationId".to_owned(),
            ItemValue::text(fact.organization.to_string()),
        ),
        ("publicCategory".to_owned(), ItemValue::text(category.id())),
        (
            "acceptedSequence".to_owned(),
            ItemValue::number(frontier.accepted.get().max(fact.accepted_sequence.get())),
        ),
        (
            "projectedSequence".to_owned(),
            ItemValue::number(fact.accepted_sequence.get()),
        ),
        (
            "publishedSequence".to_owned(),
            ItemValue::number(frontier.published.get()),
        ),
        (
            "settledSequence".to_owned(),
            ItemValue::number(frontier.settled.get()),
        ),
        ("updatedAt".to_owned(), ItemValue::instant(fact.accepted_at)),
    ]);
    if let Some(through) = frontier.service_through {
        set.insert("serviceThrough".to_owned(), ItemValue::instant(through));
    }
    match frontier.state {
        FrontierState::Advancing => {
            set.insert("state".to_owned(), ItemValue::text("advancing"));
        }
        FrontierState::Quarantined { at, reason } => {
            set.insert("state".to_owned(), ItemValue::text("quarantined"));
            set.insert("quarantineAt".to_owned(), ItemValue::number(at.get()));
            set.insert("quarantineReason".to_owned(), ItemValue::text(reason.id()));
        }
    }
    Ok(ProjectionWrite {
        row: ProjectionRow::Coverage,
        key,
        add: BTreeMap::new(),
        set,
        condition: ProjectionCondition::ProjectedSequenceIs {
            previous: frontier.projected,
        },
    })
}

/// The public category an authority's coverage row is keyed by.
const fn canonical_public(category: aex_usage_domain::meter::Category) -> PublicCategory {
    match category {
        aex_usage_domain::meter::Category::Storage => PublicCategory::Storage,
        aex_usage_domain::meter::Category::Compute => PublicCategory::Compute,
        aex_usage_domain::meter::Category::Transfer => PublicCategory::DataTransfer,
    }
}

/// The total a set of transactions moves one meter by, as a signed integer.
///
/// Used by the replay and parity harnesses: two runs that fold the same corpus
/// must move every rollup by the same total whatever order or batching they saw.
#[must_use]
pub fn quantity_total(transactions: &[ProjectionTransaction]) -> BTreeMap<String, i128> {
    let mut totals: BTreeMap<String, i128> = BTreeMap::new();
    for transaction in transactions {
        for write in &transaction.writes {
            if write.row != ProjectionRow::HourlyAggregate {
                continue;
            }
            let Some(delta) = write.add.get("quantity") else {
                continue;
            };
            let Some(ItemValue::S(meter)) = write.set.get("meter") else {
                continue;
            };
            let magnitude = i128::try_from(delta.magnitude).unwrap_or(i128::MAX);
            let signed = match delta.sign {
                Sign::Add => magnitude,
                Sign::Subtract => -magnitude,
            };
            *totals.entry(meter.clone()).or_default() += signed;
        }
    }
    totals
}

/// A quantity as a signed magnitude, for a caller folding by hand.
#[must_use]
pub const fn signed(quantity: Quantity, sign: Sign) -> SignedDelta {
    SignedDelta {
        sign,
        magnitude: quantity.get(),
    }
}
