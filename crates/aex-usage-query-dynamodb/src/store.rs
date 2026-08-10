//! The `usage-query-projection` port implementation.
//!
//! [`crate::expressions`] says *where* a row lives; this module is the only thing
//! in the stream that holds a client and goes and gets it. Three properties are
//! structural rather than reviewed:
//!
//! - **The port is read-shaped.** Every method returns rows and none takes one,
//!   so there is no method a composition root could hand a value to. That is what
//!   keeps `tests/write_incapability.rs` a fact about this crate's own sources
//!   rather than a claim about its callers.
//! - **Every read is bounded and fully qualified.** A page comes from
//!   [`ProjectionReads::aggregate_page`], which pins the generation, the
//!   workspace, the category and the month into the partition, so no read this
//!   module can issue straddles two workspaces or two generations.
//! - **Decoding never coerces.** An absent attribute, a value of the wrong shape
//!   or a row of another item type is a typed [`QueryError`], never a default
//!   that later prices something.
//!
//! # Consistency
//!
//! The split here is load-bearing and is the one place this crate departs from
//! "every read is strongly consistent".
//!
//! - **The rollup reads are strongly consistent.** These are the billed numbers.
//!   A replica-stale rollup under-reports against the very watermark the same
//!   response states, and nothing on the row lets the reader notice. The price
//!   is two read units instead of one and leader routing instead of any
//!   replica, at single-digit-millisecond latency either way — a cost trade,
//!   not a speed trade, and correctness outranks cost.
//! - **[`UsageProjectionReads::coverage_eventual`] is not.** It exists to be
//!   read *first*, before the rollups. The fold applies facts in contiguous
//!   sequence order behind the coverage fence and the sequence is monotone, so
//!   an eventually-consistent coverage read yields a projected position no
//!   greater than the true one, and strongly consistent rollup reads issued
//!   afterwards reflect at least that much. The response can therefore claim
//!   "complete through this sequence" and the claim is always true. Staleness
//!   here can only understate completeness, never overstate it, which is the
//!   safe direction and free.
//!
//! Reading coverage *after* the rows is the hazard this ordering exists to
//! avoid: a coverage row observed later can name a frontier ahead of the rows
//! the same request returned. [`UsageProjectionReads::coverage`] keeps the
//! strong read for callers that need the frontier for its own sake rather than
//! as a lower bound on an answer.

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr as _;

use aex_usage_domain::frontier::{AcceptedSequence, FrontierState, PoisonReason};
use aex_usage_domain::meter::PublicCategory;
use aex_usage_domain::projection::{Generation, ProjectionKey};
use aex_usage_domain::quantity::Quantity;
use aex_usage_domain::wire_pending::{Timestamp, WorkspaceId};
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_dynamodb::types::AttributeValue;

use crate::expressions::{
    AggregateRequest, AggregateRow, CoarseRequest, CoarseRow, CoverageRow, Grain, ProjectionReads,
    QueryError,
};

/// One stored row, as the SDK hands it over.
pub type Row = HashMap<String, AttributeValue>;

/// The discriminator attribute every projection row carries.
pub const ITEM_TYPE: &str = "itemType";
/// The partition key attribute.
pub const PARTITION: &str = "pk";
/// The sort key attribute.
pub const SORT: &str = "sk";

/// The `itemType` of an hourly or daily per-tuple rollup.
pub const AGGREGATE: &str = "usage_aggregate";
/// The `itemType` of a coarse rollup: one row per category and bucket.
pub const TOTAL: &str = "usage_total";
/// The `itemType` of the copied frontier.
pub const COVERAGE: &str = "usage_coverage";
/// The `itemType` of the generation pointer.
pub const GENERATION_POINTER: &str = "usage_generation_pointer";

/// The attribute a projection row carries its generation under.
///
/// The fold writes `generation` on every aggregate, detail and coverage row, so
/// the pointer is read under that name rather than a second spelling invented
/// here.
pub const GENERATION: &str = "generation";

/// One bounded page of rollups plus its continuation.
///
/// `next` is `Some` exactly when the partition continued past the row budget, so
/// a caller can never mistake a full page for the end of a month.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregatePageRows {
    /// The rollups, in sort-key order.
    pub rows: Vec<AggregateRow>,
    /// The sort key the next page resumes after, when there is one.
    pub next: Option<String>,
}

/// One bounded page of coarse rollups plus its continuation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoarsePageRows {
    /// The rollups, in sort-key order.
    pub rows: Vec<CoarseRow>,
    /// The sort key the next page resumes after, when there is one.
    pub next: Option<String>,
}

/// The read-only `usage-query-projection` port.
///
/// A trait with no method that accepts a row is a trait no composition root can
/// give write authority to by mistake.
#[async_trait]
pub trait UsageProjectionReads: Send + Sync + 'static {
    /// Reads the generation the projection currently serves.
    ///
    /// `None` means the pointer has never been written, which is the honest
    /// state of a region whose projection has not been cut over. It is not a
    /// synonym for [`Generation::FIRST`] and this port deliberately does not
    /// substitute one: a read against the wrong generation is confidently empty
    /// rather than visibly absent.
    ///
    /// # Errors
    ///
    /// [`QueryError::Unavailable`], [`QueryError::Denied`] or
    /// [`QueryError::Misconfigured`] for a failed call, and the decode variants
    /// for a pointer row that does not name a usable generation.
    async fn current_generation(&self) -> Result<Option<Generation>, QueryError>;

    /// Reads one workspace-and-authority coverage row.
    ///
    /// # Errors
    ///
    /// As [`UsageProjectionReads::current_generation`], plus [`QueryError::Key`]
    /// when the key could not be built.
    async fn coverage(
        &self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
    ) -> Result<Option<CoverageRow>, QueryError>;

    /// Reads one workspace-and-authority coverage row from any replica.
    ///
    /// Read this **before** the rollups and never after. See the module's
    /// consistency note: staleness here can only understate completeness, and
    /// a coverage row observed after the rows can name a frontier ahead of
    /// them.
    ///
    /// # Errors
    ///
    /// As [`UsageProjectionReads::coverage`].
    async fn coverage_eventual(
        &self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
    ) -> Result<Option<CoverageRow>, QueryError>;

    /// Reads one bounded page of per-tuple rollups.
    ///
    /// # Errors
    ///
    /// As [`UsageProjectionReads::coverage`]. A page size outside the row budget
    /// and an inverted range are refused before the call rather than widened
    /// into a scan.
    async fn aggregates(
        &self,
        request: &AggregateRequest<'_>,
    ) -> Result<AggregatePageRows, QueryError>;

    /// Reads one bounded page of coarse rollups.
    ///
    /// # Errors
    ///
    /// As [`UsageProjectionReads::aggregates`].
    async fn coarse(&self, request: &CoarseRequest<'_>) -> Result<CoarsePageRows, QueryError>;
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct UsageQueryStore {
    client: Client,
    table: String,
}

impl UsageQueryStore {
    /// Binds a store to a client and a physical table name.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    /// The physical table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Reads one fully-qualified row.
    ///
    /// `consistent` is passed rather than fixed because the coverage row is the
    /// one read on this table whose staleness is safe *and useful*: see the
    /// module's consistency note.
    async fn read_one(
        &self,
        projection: &ProjectionKey,
        consistent: bool,
    ) -> Result<Option<Row>, QueryError> {
        let sort = projection
            .sk
            .as_deref()
            .ok_or(QueryError::MissingAttribute { attribute: SORT })?;
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .key(PARTITION, AttributeValue::S(projection.pk.clone()))
            .key(SORT, AttributeValue::S(sort.to_owned()))
            .consistent_read(consistent)
            .send()
            .await
            .map_err(|error| self.classify(&error))?;
        Ok(output.item)
    }

    /// Reads one workspace-and-authority coverage row at a chosen consistency.
    async fn coverage_at(
        &self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
        consistent: bool,
    ) -> Result<Option<CoverageRow>, QueryError> {
        let coverage = ProjectionReads::new().coverage(generation, workspace, category)?;
        let Some(item) = self.read_one(&coverage, consistent).await? else {
            return Ok(None);
        };
        Ok(Some(decode_coverage(&item)?))
    }

    /// Maps one SDK failure onto the query vocabulary.
    ///
    /// Every call this module makes is a read, so there is no ambiguous-commit
    /// arm to get wrong: a failed read is either denied, misconfigured, or worth
    /// trying again.
    fn classify<E, R>(&self, error: &SdkError<E, R>) -> QueryError
    where
        E: ProvideErrorMetadata,
    {
        let SdkError::ServiceError(inner) = error else {
            return QueryError::Unavailable {
                reason: "the request did not reach the projection".to_owned(),
            };
        };
        let service = inner.err();
        let code = service.code().unwrap_or("Unknown");
        let reason = service
            .message()
            .map_or_else(|| code.to_owned(), |message| format!("{code}: {message}"));
        match code {
            "AccessDeniedException" => QueryError::Denied,
            "ResourceNotFoundException" => QueryError::Misconfigured {
                table: self.table.clone(),
            },
            // Everything else the service can answer a read with — a throttle, an
            // internal error, a validation refusal — is reported with the
            // service's own rendering rather than sorted into a guess.
            _ => QueryError::Unavailable { reason },
        }
    }
}

#[async_trait]
impl UsageProjectionReads for UsageQueryStore {
    async fn current_generation(&self) -> Result<Option<Generation>, QueryError> {
        let pointer = ProjectionReads::new().generation_pointer();
        let Some(item) = self.read_one(&pointer, true).await? else {
            return Ok(None);
        };
        Ok(Some(decode_generation(&item)?))
    }

    async fn coverage(
        &self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
    ) -> Result<Option<CoverageRow>, QueryError> {
        self.coverage_at(generation, workspace, category, true)
            .await
    }

    async fn coverage_eventual(
        &self,
        generation: Generation,
        workspace: &WorkspaceId,
        category: PublicCategory,
    ) -> Result<Option<CoverageRow>, QueryError> {
        self.coverage_at(generation, workspace, category, false)
            .await
    }

    async fn aggregates(
        &self,
        request: &AggregateRequest<'_>,
    ) -> Result<AggregatePageRows, QueryError> {
        let page = ProjectionReads::new().aggregate_page(request)?;
        let limit = i32::try_from(page.limit).map_err(|_| QueryError::PageBudget {
            requested: page.limit,
        })?;
        // `BETWEEN` is inclusive at both ends and the builder's upper bound is the
        // bare bucket prefix. Every stored rollup sort key is
        // `<granularity>#<bucket>#<dimension hash>`, which sorts strictly after
        // the bare `<granularity>#<bucket>`, so an inclusive comparison
        // implements the half-open bucket range exactly. A separate `<` bound is
        // not available: a key condition carries at most one comparison on the
        // sort key.
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk AND #sk BETWEEN :from AND :until")
            .expression_attribute_names("#pk", PARTITION)
            .expression_attribute_names("#sk", SORT)
            .expression_attribute_values(":pk", AttributeValue::S(page.partition.clone()))
            .expression_attribute_values(":from", AttributeValue::S(page.from.clone()))
            .expression_attribute_values(":until", AttributeValue::S(page.until.clone()))
            .limit(limit)
            .consistent_read(true)
            .set_exclusive_start_key(page.after.as_ref().map(|after| {
                HashMap::from([
                    (
                        PARTITION.to_owned(),
                        AttributeValue::S(page.partition.clone()),
                    ),
                    (SORT.to_owned(), AttributeValue::S(after.clone())),
                ])
            }))
            .send()
            .await
            .map_err(|error| self.classify(&error))?;

        let mut rows = Vec::new();
        for item in output.items.unwrap_or_default() {
            rows.push(decode_aggregate(&item)?);
        }
        let next = output
            .last_evaluated_key
            .as_ref()
            .map(sort_key_of)
            .transpose()?;
        Ok(AggregatePageRows { rows, next })
    }

    async fn coarse(&self, request: &CoarseRequest<'_>) -> Result<CoarsePageRows, QueryError> {
        let page = ProjectionReads::new().coarse_page(request)?;
        let limit = i32::try_from(page.limit).map_err(|_| QueryError::PageBudget {
            requested: page.limit,
        })?;
        // Unlike the per-tuple face there is no dimension-hash suffix to make
        // an inclusive `BETWEEN` behave as a half-open range: a stored coarse
        // key is exactly `T#<grain>#<bucket>`, so the exclusive end bound is
        // itself a storable key. It is stepped back to its immediate lexical
        // predecessor instead.
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk AND #sk BETWEEN :from AND :until")
            .expression_attribute_names("#pk", PARTITION)
            .expression_attribute_names("#sk", SORT)
            .expression_attribute_values(":pk", AttributeValue::S(page.partition.clone()))
            .expression_attribute_values(":from", AttributeValue::S(page.from.clone()))
            .expression_attribute_values(":until", AttributeValue::S(exclusive_bound(&page.until)?))
            .limit(limit)
            .consistent_read(true)
            .set_exclusive_start_key(page.after.as_ref().map(|after| {
                HashMap::from([
                    (
                        PARTITION.to_owned(),
                        AttributeValue::S(page.partition.clone()),
                    ),
                    (SORT.to_owned(), AttributeValue::S(after.clone())),
                ])
            }))
            .send()
            .await
            .map_err(|error| self.classify(&error))?;

        let mut rows = Vec::new();
        for item in output.items.unwrap_or_default() {
            rows.push(decode_coarse(&item)?);
        }
        let next = output
            .last_evaluated_key
            .as_ref()
            .map(sort_key_of)
            .transpose()?;
        Ok(CoarsePageRows { rows, next })
    }
}

/// The immediate lexical predecessor of `until` among keys of the same length.
///
/// Every coarse sort key in one partition and grain has the same length, so
/// decrementing the final byte yields a bound that includes every key below
/// `until` and excludes `until` itself — an exact half-open range through an
/// inclusive `BETWEEN`. Bucket labels end in an ASCII digit, so the final byte
/// is never zero and the decrement never underflows.
///
/// # Errors
///
/// Returns [`QueryError::MalformedAttribute`] for an empty bound or one whose
/// final byte is zero, rather than issuing a read over a range nobody chose.
fn exclusive_bound(until: &str) -> Result<String, QueryError> {
    let mut bytes = until.as_bytes().to_vec();
    let last = bytes.last_mut().ok_or(QueryError::MalformedAttribute {
        attribute: "sk",
        reason: "an empty range bound".to_owned(),
    })?;
    if *last == 0 {
        return Err(QueryError::MalformedAttribute {
            attribute: "sk",
            reason: "a range bound with no predecessor".to_owned(),
        });
    }
    *last -= 1;
    String::from_utf8(bytes).map_err(|error| QueryError::MalformedAttribute {
        attribute: "sk",
        reason: error.to_string(),
    })
}

/// The sort key a continuation resumes after.
///
/// A last-evaluated key without the base table's own sort key would produce a
/// continuation nothing could resume, so it is refused rather than dropped.
fn sort_key_of(last: &Row) -> Result<String, QueryError> {
    last.get(SORT)
        .and_then(|value| value.as_s().ok())
        .cloned()
        .ok_or(QueryError::MissingAttribute { attribute: SORT })
}

/// Refuses a row that is not the item type the caller asked for.
fn expect_item_type(item: &Row, expected: &'static str) -> Result<(), QueryError> {
    let declared = item
        .get(ITEM_TYPE)
        .ok_or(QueryError::MissingAttribute {
            attribute: ITEM_TYPE,
        })?
        .as_s()
        .map_err(|_| QueryError::MalformedAttribute {
            attribute: ITEM_TYPE,
            reason: "the discriminator is not a string".to_owned(),
        })?;
    if declared != expected {
        return Err(QueryError::ItemTypeMismatch {
            expected,
            actual: declared.clone(),
        });
    }
    Ok(())
}

/// A required string attribute.
fn text<'a>(item: &'a Row, attribute: &'static str) -> Result<&'a str, QueryError> {
    item.get(attribute)
        .ok_or(QueryError::MissingAttribute { attribute })?
        .as_s()
        .map(String::as_str)
        .map_err(|_| QueryError::MalformedAttribute {
            attribute,
            reason: "expected a string".to_owned(),
        })
}

/// An optional string attribute.
fn opt_text<'a>(item: &'a Row, attribute: &'static str) -> Result<Option<&'a str>, QueryError> {
    match item.get(attribute) {
        None => Ok(None),
        Some(value) => value
            .as_s()
            .map(|stored| Some(stored.as_str()))
            .map_err(|_| QueryError::MalformedAttribute {
                attribute,
                reason: "expected a string".to_owned(),
            }),
    }
}

/// A required counter, as the bare decimal `DynamoDB` writes.
fn number<'a>(item: &'a Row, attribute: &'static str) -> Result<&'a str, QueryError> {
    item.get(attribute)
        .ok_or(QueryError::MissingAttribute { attribute })?
        .as_n()
        .map(String::as_str)
        .map_err(|_| QueryError::MalformedAttribute {
            attribute,
            reason: "expected a number".to_owned(),
        })
}

/// A required `u64` counter.
fn counter(item: &Row, attribute: &'static str) -> Result<u64, QueryError> {
    number(item, attribute)?
        .parse::<u64>()
        .map_err(|error| QueryError::MalformedAttribute {
            attribute,
            reason: error.to_string(),
        })
}

/// A contiguous sequence position.
///
/// Zero is the empty state rather than a position, which is why this is not a
/// plain [`AcceptedSequence::new`].
fn sequence(item: &Row, attribute: &'static str) -> Result<AcceptedSequence, QueryError> {
    let value = counter(item, attribute)?;
    if value == 0 {
        return Ok(AcceptedSequence::ORIGIN);
    }
    AcceptedSequence::new(value).map_err(|error| QueryError::MalformedAttribute {
        attribute,
        reason: error.to_string(),
    })
}

/// The priced category a row belongs to.
fn category(item: &Row) -> Result<PublicCategory, QueryError> {
    let stored = text(item, "publicCategory")?;
    PublicCategory::from_str(stored).map_err(|_| QueryError::MalformedAttribute {
        attribute: "publicCategory",
        reason: "outside the four priced categories".to_owned(),
    })
}

/// Decodes one stored rollup.
///
/// # Errors
///
/// [`QueryError::ItemTypeMismatch`] for a row of another item type, and the
/// missing- and malformed-attribute variants for anything the fold declares that
/// the row does not carry usably. A rollup that went below zero is refused
/// rather than read as a small number: `Quantity` is unsigned by construction
/// and a negative total is a corrupt fold.
pub fn decode_aggregate(item: &Row) -> Result<AggregateRow, QueryError> {
    expect_item_type(item, AGGREGATE)?;
    let quantity = Quantity::parse(number(item, "quantity")?).map_err(|error| {
        QueryError::MalformedAttribute {
            attribute: "quantity",
            reason: error.to_string(),
        }
    })?;
    Ok(AggregateRow {
        public_category: category(item)?,
        meter: opt_text(item, "meter")?.map(str::to_owned),
        bucket_start: text(item, "bucketStart")?.to_owned(),
        bucket_end: text(item, "bucketEnd")?.to_owned(),
        quantity,
        fact_count: counter(item, "factCount")?,
        dimensions: dimensions(item)?,
        highest_sequence: sequence(item, "highestSequence")?,
    })
}

/// Decodes one stored coarse rollup.
///
/// # Errors
///
/// [`QueryError::ItemTypeMismatch`] for a row of another item type — in
/// particular a per-tuple `usage_aggregate`, which shares this partition and
/// would silently under-report the bucket if it were read as a total — and the
/// missing- and malformed-attribute variants for anything the fold declares
/// that the row does not carry usably.
pub fn decode_coarse(item: &Row) -> Result<CoarseRow, QueryError> {
    expect_item_type(item, TOTAL)?;
    let quantity = Quantity::parse(number(item, "quantity")?).map_err(|error| {
        QueryError::MalformedAttribute {
            attribute: "quantity",
            reason: error.to_string(),
        }
    })?;
    let grain = match text(item, "grain")? {
        "H" => Grain::Hourly,
        "D" => Grain::Daily,
        other => {
            return Err(QueryError::MalformedAttribute {
                attribute: "grain",
                reason: format!("`{other}` is neither hourly nor daily"),
            });
        }
    };
    Ok(CoarseRow {
        public_category: category(item)?,
        grain,
        bucket: text(item, "bucketStart")?.to_owned(),
        quantity,
        highest_sequence: sequence(item, "highestSequence")?,
    })
}

/// The declared dimension tuple, ordered so two reads of one row agree.
fn dimensions(item: &Row) -> Result<Vec<(String, String)>, QueryError> {
    const ATTRIBUTE: &str = "dimensions";
    let members = item
        .get(ATTRIBUTE)
        .ok_or(QueryError::MissingAttribute {
            attribute: ATTRIBUTE,
        })?
        .as_m()
        .map_err(|_| QueryError::MalformedAttribute {
            attribute: ATTRIBUTE,
            reason: "expected a map".to_owned(),
        })?;
    let mut ordered = BTreeMap::new();
    for (name, member) in members {
        let value = member
            .as_s()
            .map_err(|_| QueryError::MalformedAttribute {
                attribute: ATTRIBUTE,
                reason: format!("member `{name}` is not a string"),
            })?
            .clone();
        ordered.insert(name.clone(), value);
    }
    Ok(ordered.into_iter().collect())
}

/// Decodes one copied frontier.
///
/// # Errors
///
/// As [`decode_aggregate`]. A quarantined frontier that names neither where it
/// stopped nor why is refused rather than reported as advancing: a stall nobody
/// can act on is worse than a read that failed.
pub fn decode_coverage(item: &Row) -> Result<CoverageRow, QueryError> {
    expect_item_type(item, COVERAGE)?;
    let state = match text(item, "state")? {
        "advancing" => FrontierState::Advancing,
        "quarantined" => {
            let at = sequence(item, "quarantineAt")?;
            let stored = text(item, "quarantineReason")?;
            let reason = PoisonReason::ALL
                .into_iter()
                .find(|candidate| candidate.id() == stored)
                .ok_or(QueryError::MalformedAttribute {
                    attribute: "quarantineReason",
                    reason: "outside the closed poison vocabulary".to_owned(),
                })?;
            FrontierState::Quarantined { at, reason }
        }
        other => {
            return Err(QueryError::MalformedAttribute {
                attribute: "state",
                reason: format!("`{other}` is neither advancing nor quarantined"),
            });
        }
    };
    let service_through = match opt_text(item, "serviceThrough")? {
        None => None,
        Some(stored) => {
            Some(
                Timestamp::parse(stored).map_err(|error| QueryError::MalformedAttribute {
                    attribute: "serviceThrough",
                    reason: error.to_string(),
                })?,
            )
        }
    };
    Ok(CoverageRow {
        public_category: category(item)?,
        accepted: sequence(item, "acceptedSequence")?,
        projected: sequence(item, "projectedSequence")?,
        published: sequence(item, "publishedSequence")?,
        settled: sequence(item, "settledSequence")?,
        service_through,
        state,
    })
}

/// Decodes the generation pointer.
///
/// # Errors
///
/// As [`decode_aggregate`], plus a refusal for a generation outside the
/// fixed-width key prefix, which would let one range query straddle two
/// generations.
pub fn decode_generation(item: &Row) -> Result<Generation, QueryError> {
    expect_item_type(item, GENERATION_POINTER)?;
    let value = counter(item, GENERATION)?;
    let narrowed = u16::try_from(value).map_err(|_| QueryError::MalformedAttribute {
        attribute: GENERATION,
        reason: "a projection generation is a small integer".to_owned(),
    })?;
    Generation::new(narrowed).map_err(|error| QueryError::MalformedAttribute {
        attribute: GENERATION,
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aex_usage_domain::frontier::{AcceptedSequence, FrontierState, PoisonReason};
    use aex_usage_domain::meter::PublicCategory;
    use aex_usage_domain::projection::Generation;
    use aws_sdk_dynamodb::types::AttributeValue;

    use super::{
        AGGREGATE, COVERAGE, GENERATION, GENERATION_POINTER, ITEM_TYPE, Row, decode_aggregate,
        decode_coverage, decode_generation,
    };
    use crate::expressions::QueryError;

    fn s(value: &str) -> AttributeValue {
        AttributeValue::S(value.to_owned())
    }

    fn n(value: u64) -> AttributeValue {
        AttributeValue::N(value.to_string())
    }

    /// The attribute set `aex_usage_app::projection` writes for a
    /// rollup, spelled exactly as it spells it.
    fn aggregate() -> Row {
        HashMap::from([
            (ITEM_TYPE.to_owned(), s(AGGREGATE)),
            (GENERATION.to_owned(), n(0)),
            ("workspaceId".to_owned(), s("ws-1")),
            ("organizationId".to_owned(), s("org-1")),
            ("publicCategory".to_owned(), s("compute")),
            ("meter".to_owned(), s("compute.cpu_ms")),
            ("bucketStart".to_owned(), s("2026-08-01T12")),
            ("bucketEnd".to_owned(), s("2026-08-01T12")),
            (
                "dimensions".to_owned(),
                AttributeValue::M(HashMap::from([
                    ("service".to_owned(), s("brain-mux")),
                    ("basis".to_owned(), s("consumed")),
                ])),
            ),
            ("highestSequence".to_owned(), n(9)),
            ("updatedAt".to_owned(), s("2026-08-01T12:00:00.000Z")),
            ("quantity".to_owned(), n(4_200)),
            ("factCount".to_owned(), n(3)),
        ])
    }

    /// The attribute set the fold writes for a coverage row.
    fn coverage() -> Row {
        HashMap::from([
            (ITEM_TYPE.to_owned(), s(COVERAGE)),
            (GENERATION.to_owned(), n(0)),
            ("workspaceId".to_owned(), s("ws-1")),
            ("organizationId".to_owned(), s("org-1")),
            ("publicCategory".to_owned(), s("compute")),
            ("acceptedSequence".to_owned(), n(11)),
            ("projectedSequence".to_owned(), n(9)),
            ("publishedSequence".to_owned(), n(7)),
            ("settledSequence".to_owned(), n(0)),
            ("state".to_owned(), s("advancing")),
            ("updatedAt".to_owned(), s("2026-08-01T12:00:00.000Z")),
        ])
    }

    #[test]
    fn a_rollup_decodes_the_attributes_the_fold_writes() {
        let decoded = decode_aggregate(&aggregate()).expect("decodes");
        assert_eq!(decoded.public_category, PublicCategory::Compute);
        assert_eq!(decoded.meter.as_deref(), Some("compute.cpu_ms"));
        assert_eq!(decoded.quantity.get(), 4_200);
        assert_eq!(decoded.fact_count, 3);
        assert_eq!(decoded.highest_sequence.get(), 9);
        assert_eq!(
            decoded.dimensions,
            vec![
                ("basis".to_owned(), "consumed".to_owned()),
                ("service".to_owned(), "brain-mux".to_owned()),
            ],
            "the declared tuple is ordered, so two reads of one row agree"
        );
    }

    #[test]
    fn a_rollup_read_as_another_item_type_is_refused_rather_than_coerced() {
        let mut mislabelled = aggregate();
        mislabelled.insert(ITEM_TYPE.to_owned(), s(COVERAGE));
        let error = decode_aggregate(&mislabelled).expect_err("another item type");
        assert!(
            matches!(
                error,
                QueryError::ItemTypeMismatch {
                    expected: AGGREGATE,
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_rollup_that_went_below_zero_is_corrupt_rather_than_a_small_number() {
        let mut negative = aggregate();
        negative.insert("quantity".to_owned(), AttributeValue::N("-1".to_owned()));
        let error = decode_aggregate(&negative).expect_err("a negative rollup");
        assert!(
            matches!(error, QueryError::MalformedAttribute { .. }),
            "{error}"
        );
    }

    #[test]
    fn a_rollup_missing_a_declared_attribute_is_refused_rather_than_defaulted() {
        for attribute in [
            "publicCategory",
            "bucketStart",
            "bucketEnd",
            "quantity",
            "factCount",
            "dimensions",
            "highestSequence",
        ] {
            let mut incomplete = aggregate();
            incomplete.remove(attribute);
            assert!(
                decode_aggregate(&incomplete).is_err(),
                "`{attribute}` was defaulted rather than refused"
            );
        }
    }

    #[test]
    fn a_zero_sequence_is_the_empty_state_and_never_a_position() {
        let decoded = decode_coverage(&coverage()).expect("decodes");
        assert_eq!(decoded.settled, AcceptedSequence::ORIGIN);
        assert_eq!(decoded.accepted.get(), 11);
        assert_eq!(decoded.projected.get(), 9);
        assert_eq!(decoded.published.get(), 7);
        assert!(!decoded.is_stalled());
        assert!(decoded.service_through.is_none());
    }

    #[test]
    fn a_quarantined_frontier_reports_where_it_stopped_and_why() {
        let mut parked = coverage();
        parked.insert("state".to_owned(), s("quarantined"));
        parked.insert("quarantineAt".to_owned(), n(10));
        parked.insert("quarantineReason".to_owned(), s("undecodable"));
        parked.insert("serviceThrough".to_owned(), s("2026-08-01T11:00:00.000Z"));
        let decoded = decode_coverage(&parked).expect("decodes");
        assert_eq!(
            decoded.state,
            FrontierState::Quarantined {
                at: AcceptedSequence::new(10).expect("a position"),
                reason: PoisonReason::Undecodable,
            }
        );
        assert!(decoded.is_stalled());
        assert_eq!(decoded.stall_reason(), Some(PoisonReason::Undecodable));
        assert!(decoded.service_through.is_some());
    }

    #[test]
    fn a_quarantine_without_its_reason_is_refused_rather_than_reported_as_advancing() {
        let mut bare = coverage();
        bare.insert("state".to_owned(), s("quarantined"));
        bare.insert("quarantineAt".to_owned(), n(10));
        assert!(decode_coverage(&bare).is_err());

        let mut invented = bare.clone();
        invented.insert("quarantineReason".to_owned(), s("the moon was wrong"));
        assert!(decode_coverage(&invented).is_err());
    }

    #[test]
    fn a_frontier_state_outside_the_closed_set_is_refused() {
        let mut invented = coverage();
        invented.insert("state".to_owned(), s("thinking about it"));
        assert!(decode_coverage(&invented).is_err());
    }

    #[test]
    fn the_generation_pointer_decodes_under_the_name_every_other_row_uses() {
        let item: Row = HashMap::from([
            (ITEM_TYPE.to_owned(), s(GENERATION_POINTER)),
            (GENERATION.to_owned(), n(7)),
        ]);
        assert_eq!(
            decode_generation(&item).expect("decodes"),
            Generation::new(7).expect("in range")
        );
    }

    #[test]
    fn a_generation_beyond_the_fixed_width_prefix_is_refused_rather_than_truncated() {
        for value in [10_000_u64, u64::from(u16::MAX) + 1] {
            let item: Row = HashMap::from([
                (ITEM_TYPE.to_owned(), s(GENERATION_POINTER)),
                (GENERATION.to_owned(), n(value)),
            ]);
            assert!(
                decode_generation(&item).is_err(),
                "generation {value} would straddle two key prefixes"
            );
        }
    }
}
