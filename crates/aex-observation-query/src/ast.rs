//! The bounded public filter AST and its field policy.
//!
//! `aex-wire` already enforces the structural registry bounds in
//! `ObservationFilter`'s `Deserialize` (contracts C-13): depth, leaf count,
//! boolean children, `in` values and string operand length. This module
//! re-validates **field policy only**, never structure, because two copies of a
//! structural bound is exactly how they come to disagree.
//!
//! A missing field makes an ordinary comparison **false**. `exists` is the only
//! way to observe absence.

use std::collections::BTreeMap;

use aex_observation_domain::canonical::CanonicalValue;
use aex_observation_domain::signal::Signal;

/// Why a query was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum QueryError {
    /// A field is not in the closed per-signal set.
    #[error("`{field}` is not a queryable field for the selected signals")]
    UnknownField {
        /// The offending path, named exactly as the client wrote it.
        field: Box<str>,
    },
    /// A field exists but may never be filtered on.
    #[error("`{field}` is protected and may not appear in a filter")]
    ProtectedField {
        /// The offending path.
        field: Box<str>,
    },
    /// A bound outside the field policy was violated.
    #[error("{what} is {observed}, above the {limit} ceiling")]
    Bound {
        /// The bound's name.
        what: &'static str,
        /// The measured value.
        observed: usize,
        /// The effective ceiling.
        limit: usize,
    },
    /// The time range is inverted or empty.
    #[error("the time range must be a non-empty half-open interval")]
    TimeRange,
    /// The aggregation request is not answerable.
    #[error("invalid metric aggregation: {reason}")]
    InvalidMetricAggregation {
        /// Why.
        reason: &'static str,
    },
}

/// The common fields every signal carries.
pub const COMMON_FIELDS: &[&str] = &[
    "id",
    "revision",
    "time",
    "acceptedAt",
    "sessionId",
    "runId",
    "agentId",
    "operationId",
];

/// Protected identity and internal fields, refused by name.
pub const PROTECTED_FIELDS: &[&str] = &["workspaceId", "organizationId", "batchId", "acceptedSeq"];

/// The closed per-signal field sets, carried forward from the accepted 34-name
/// allowlist.
#[must_use]
pub fn signal_fields(signal: Signal) -> &'static [&'static str] {
    match signal {
        Signal::Events => &[
            "type",
            "name",
            "source",
            "outcome",
            "messageId",
            "toolCallId",
        ],
        Signal::Logs => &[
            "severityNumber",
            "severityText",
            "body",
            "stream",
            "logger",
            "traceId",
            "spanId",
        ],
        Signal::Spans => &[
            "traceId",
            "spanId",
            "parentSpanId",
            "name",
            "kind",
            "status",
            "durationNs",
            "serviceName",
            "scopeName",
        ],
        Signal::Metrics => &["name", "kind", "unit", "temporality", "monotonic", "value"],
        Signal::Traces => &[
            "traceId",
            "state",
            "rootName",
            "durationNs",
            "spanCount",
            "errorCount",
            "serviceName",
            "revision",
        ],
    }
}

/// How many `(signal, field)` pairs the allowlist admits.
///
/// The replaced implementation counted 34 *names*; the accepted per-signal sets
/// are 36 pairs over 28 distinct names, because `traceId`, `spanId`, `name`,
/// `durationNs`, `serviceName` and `revision` appear on more than one signal.
/// Both counts are asserted rather than one being quietly restated.
#[must_use]
pub fn allowlist_pairs() -> usize {
    Signal::ALL
        .iter()
        .map(|signal| signal_fields(*signal).len())
        .sum()
}

/// The number of distinct allowlisted signal field names across every signal.
#[must_use]
pub fn allowlist_size() -> usize {
    Signal::ALL
        .iter()
        .flat_map(|signal| signal_fields(*signal).iter().copied())
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

/// A resolved reference to something a filter may read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldRef {
    /// One of [`COMMON_FIELDS`].
    Common(Box<str>),
    /// A field in the closed set of one signal.
    Signal(Signal, Box<str>),
    /// `attributes.<key>`, external-safe scalars only.
    ///
    /// Residual: the slim index projection cannot answer it, so a page fetches
    /// the base item before evaluating (plan §4.6).
    Attribute(Box<str>),
}

impl FieldRef {
    /// Whether evaluating this reference needs the base item.
    #[must_use]
    pub const fn is_residual(&self) -> bool {
        matches!(self, Self::Attribute(_))
    }

    /// The path as the client wrote it.
    #[must_use]
    pub fn path(&self) -> String {
        match self {
            Self::Common(name) | Self::Signal(_, name) => name.to_string(),
            Self::Attribute(key) => format!("attributes.{key}"),
        }
    }
}

/// Resolves one field path against the selected signals.
///
/// # Errors
///
/// Returns [`QueryError::ProtectedField`] for identity and internal fields and
/// [`QueryError::UnknownField`] for anything outside the closed sets, naming the
/// offending path in both cases.
pub fn resolve(path: &str, signals: &[Signal]) -> Result<FieldRef, QueryError> {
    if PROTECTED_FIELDS.contains(&path) {
        return Err(QueryError::ProtectedField { field: path.into() });
    }
    if let Some(key) = path.strip_prefix("attributes.") {
        if key.is_empty() || key.contains('.') {
            return Err(QueryError::UnknownField { field: path.into() });
        }
        return Ok(FieldRef::Attribute(key.into()));
    }
    if COMMON_FIELDS.contains(&path) {
        return Ok(FieldRef::Common(path.into()));
    }
    for signal in signals {
        if signal_fields(*signal).contains(&path) {
            return Ok(FieldRef::Signal(*signal, path.into()));
        }
    }
    Err(QueryError::UnknownField { field: path.into() })
}

/// The comparisons a leaf may use.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CmpOp {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Lte,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Gte,
}

impl CmpOp {
    /// Every operator, in declared order.
    pub const ALL: &'static [CmpOp] = &[
        CmpOp::Eq,
        CmpOp::Ne,
        CmpOp::Lt,
        CmpOp::Lte,
        CmpOp::Gt,
        CmpOp::Gte,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::Lt => "lt",
            Self::Lte => "lte",
            Self::Gt => "gt",
            Self::Gte => "gte",
        }
    }
}

/// The text operators a leaf may use, case-sensitive over UTF-8.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TextOp {
    /// String prefix.
    Prefix,
    /// Substring.
    Contains,
}

/// One node of the resolved filter.
#[derive(Clone, Debug, PartialEq)]
pub enum Predicate {
    /// Every child must match.
    And(Vec<Predicate>),
    /// At least one child must match.
    Or(Vec<Predicate>),
    /// The child must not match.
    Not(Box<Predicate>),
    /// A comparison against one field.
    Cmp {
        /// What to read.
        field: FieldRef,
        /// How to compare.
        op: CmpOp,
        /// The operand.
        value: CanonicalValue,
    },
    /// Membership of a bounded value set.
    In {
        /// What to read.
        field: FieldRef,
        /// Whether the membership is negated.
        negated: bool,
        /// The operands.
        values: Vec<CanonicalValue>,
    },
    /// A text match.
    Text {
        /// What to read.
        field: FieldRef,
        /// Which text operator.
        op: TextOp,
        /// The operand.
        value: Box<str>,
    },
    /// Presence of a field. The **only** way to observe absence.
    Exists {
        /// What to read.
        field: FieldRef,
    },
}

/// What one observation exposes to a filter.
pub trait FilterRow {
    /// Reads one resolved field, or `None` when the row does not carry it.
    fn read(&self, field: &FieldRef) -> Option<&CanonicalValue>;
}

/// A row backed by two ordinary maps, used by the residual evaluator after a
/// base-table fetch and by tests.
#[derive(Clone, Debug, Default)]
pub struct MapRow {
    /// The indexed and common scalars.
    pub fields: BTreeMap<String, CanonicalValue>,
    /// The customer attributes.
    pub attributes: BTreeMap<String, CanonicalValue>,
}

impl FilterRow for MapRow {
    fn read(&self, field: &FieldRef) -> Option<&CanonicalValue> {
        match field {
            FieldRef::Common(name) | FieldRef::Signal(_, name) => self.fields.get(name.as_ref()),
            FieldRef::Attribute(key) => self.attributes.get(key.as_ref()),
        }
    }
}

impl Predicate {
    /// Whether any leaf needs the base item.
    #[must_use]
    pub fn has_residual(&self) -> bool {
        match self {
            Self::And(children) | Self::Or(children) => {
                children.iter().any(Predicate::has_residual)
            }
            Self::Not(child) => child.has_residual(),
            Self::Cmp { field, .. }
            | Self::In { field, .. }
            | Self::Text { field, .. }
            | Self::Exists { field } => field.is_residual(),
        }
    }

    /// Evaluates the predicate against one row.
    ///
    /// A missing field makes an ordinary comparison false, which is why
    /// `not(eq(x, 1))` matches a row with no `x`: the inner comparison is false
    /// and negating it is the documented meaning. Only `exists` observes
    /// absence directly.
    #[must_use]
    pub fn evaluate(&self, row: &impl FilterRow) -> bool {
        match self {
            Self::And(children) => children.iter().all(|child| child.evaluate(row)),
            Self::Or(children) => children.iter().any(|child| child.evaluate(row)),
            Self::Not(child) => !child.evaluate(row),
            Self::Exists { field } => row.read(field).is_some(),
            Self::Cmp { field, op, value } => row
                .read(field)
                .and_then(|actual| compare(actual, value))
                .is_some_and(|ordering| match op {
                    CmpOp::Eq => ordering == std::cmp::Ordering::Equal,
                    CmpOp::Ne => ordering != std::cmp::Ordering::Equal,
                    CmpOp::Lt => ordering == std::cmp::Ordering::Less,
                    CmpOp::Lte => ordering != std::cmp::Ordering::Greater,
                    CmpOp::Gt => ordering == std::cmp::Ordering::Greater,
                    CmpOp::Gte => ordering != std::cmp::Ordering::Less,
                }),
            Self::In {
                field,
                negated,
                values,
            } => match row.read(field) {
                None => false,
                Some(actual) => {
                    let hit = values.iter().any(|candidate| {
                        compare(actual, candidate) == Some(std::cmp::Ordering::Equal)
                    });
                    hit != *negated
                }
            },
            Self::Text { field, op, value } => {
                match row.read(field).and_then(CanonicalValue::as_str) {
                    None => false,
                    Some(text) => match op {
                        TextOp::Prefix => text.starts_with(value.as_ref()),
                        TextOp::Contains => text.contains(value.as_ref()),
                    },
                }
            }
        }
    }
}

/// Compares two canonical values, or `None` when their types are not comparable.
#[allow(
    clippy::cast_precision_loss,
    reason = "a mixed numeric comparison is a double comparison by contract"
)]
fn compare(left: &CanonicalValue, right: &CanonicalValue) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (CanonicalValue::Str(a), CanonicalValue::Str(b)) => Some(a.cmp(b)),
        (CanonicalValue::Bool(a), CanonicalValue::Bool(b)) => Some(a.cmp(b)),
        (CanonicalValue::Int(a), CanonicalValue::Int(b)) => Some(a.cmp(b)),
        (CanonicalValue::Null, CanonicalValue::Null) => Some(std::cmp::Ordering::Equal),
        (CanonicalValue::Num(a), CanonicalValue::Num(b)) => a.partial_cmp(b),
        (CanonicalValue::Int(a), CanonicalValue::Num(b)) => (*a as f64).partial_cmp(b),
        (CanonicalValue::Num(a), CanonicalValue::Int(b)) => a.partial_cmp(&(*b as f64)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CanonicalValue, CmpOp, FieldRef, MapRow, Predicate, QueryError, TextOp, allowlist_pairs,
        allowlist_size, resolve, signal_fields,
    };
    use aex_observation_domain::signal::Signal;

    fn row() -> MapRow {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "severityText".to_owned(),
            CanonicalValue::Str("warn".into()),
        );
        fields.insert("severityNumber".to_owned(), CanonicalValue::Int(13));
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("route".to_owned(), CanonicalValue::Str("/v1/runs".into()));
        MapRow { fields, attributes }
    }

    #[test]
    fn the_allowlist_is_the_declared_closed_set() {
        assert_eq!(allowlist_pairs(), 36, "one entry per (signal, field) pair");
        assert_eq!(
            allowlist_size(),
            28,
            "distinct names, shared across signals"
        );
        assert_eq!(signal_fields(Signal::Metrics).len(), 6);
        for signal in Signal::ALL {
            let fields = signal_fields(*signal);
            let distinct: std::collections::BTreeSet<_> = fields.iter().copied().collect();
            assert_eq!(distinct.len(), fields.len(), "{signal:?} repeats a field");
        }
    }

    #[test]
    fn a_protected_field_is_refused_by_name() {
        for protected in ["workspaceId", "organizationId", "batchId", "acceptedSeq"] {
            assert_eq!(
                resolve(protected, &[Signal::Logs]),
                Err(QueryError::ProtectedField {
                    field: protected.into()
                })
            );
        }
    }

    #[test]
    fn an_unknown_or_nested_attribute_path_is_refused_by_name() {
        assert_eq!(
            resolve("nope", &[Signal::Logs]),
            Err(QueryError::UnknownField {
                field: "nope".into()
            })
        );
        assert!(resolve("attributes.", &[Signal::Logs]).is_err());
        assert!(resolve("attributes.a.b", &[Signal::Logs]).is_err());
        assert_eq!(
            resolve("attributes.route", &[Signal::Logs]).expect("resolves"),
            FieldRef::Attribute("route".into())
        );
    }

    #[test]
    fn a_field_of_another_signal_is_not_visible() {
        assert!(resolve("temporality", &[Signal::Logs]).is_err());
        assert!(resolve("temporality", &[Signal::Metrics]).is_ok());
        assert!(resolve("time", &[Signal::Logs]).is_ok());
    }

    #[test]
    fn a_missing_field_makes_an_ordinary_comparison_false() {
        let absent = Predicate::Cmp {
            field: FieldRef::Signal(Signal::Logs, "logger".into()),
            op: CmpOp::Eq,
            value: CanonicalValue::Str("x".into()),
        };
        assert!(!absent.evaluate(&row()));
        assert!(
            !Predicate::Exists {
                field: FieldRef::Signal(Signal::Logs, "logger".into())
            }
            .evaluate(&row())
        );
        assert!(
            Predicate::Exists {
                field: FieldRef::Signal(Signal::Logs, "severityText".into())
            }
            .evaluate(&row())
        );
    }

    #[test]
    fn comparisons_and_membership_evaluate_over_the_canonical_types() {
        let predicate = Predicate::And(vec![
            Predicate::Cmp {
                field: FieldRef::Signal(Signal::Logs, "severityNumber".into()),
                op: CmpOp::Gte,
                value: CanonicalValue::Int(13),
            },
            Predicate::In {
                field: FieldRef::Signal(Signal::Logs, "severityText".into()),
                negated: false,
                values: vec![
                    CanonicalValue::Str("warn".into()),
                    CanonicalValue::Str("error".into()),
                ],
            },
            Predicate::Text {
                field: FieldRef::Attribute("route".into()),
                op: TextOp::Prefix,
                value: "/v1".into(),
            },
        ]);
        assert!(predicate.evaluate(&row()));
        assert!(predicate.has_residual(), "the attribute leaf is residual");

        let negated = Predicate::In {
            field: FieldRef::Signal(Signal::Logs, "severityText".into()),
            negated: true,
            values: vec![CanonicalValue::Str("warn".into())],
        };
        assert!(!negated.evaluate(&row()));
    }

    #[test]
    fn a_type_mismatch_never_matches() {
        let mismatched = Predicate::Cmp {
            field: FieldRef::Signal(Signal::Logs, "severityText".into()),
            op: CmpOp::Eq,
            value: CanonicalValue::Int(13),
        };
        assert!(!mismatched.evaluate(&row()));
        assert_eq!(CmpOp::ALL.len(), 6);
        assert_eq!(CmpOp::Gte.as_str(), "gte");
        assert_eq!(
            FieldRef::Attribute("route".into()).path(),
            "attributes.route"
        );
        assert!(
            !Predicate::Cmp {
                field: FieldRef::Signal(Signal::Logs, "severityNumber".into()),
                op: CmpOp::Eq,
                value: CanonicalValue::Int(13),
            }
            .has_residual()
        );
    }
}
