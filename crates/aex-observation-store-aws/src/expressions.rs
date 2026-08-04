//! Item shapes and `DynamoDB` expression builders for `observation-authority`.
//!
//! **Injection safety is structural, not textual.** Nothing customer-supplied
//! ever reaches an expression string: attribute names are emitted as generated
//! `#n0`, `#n1`… placeholders and values as `:v0`, `:v1`… placeholders. Key
//! templates are formatted only from components that `aex_observation_domain`
//! already validated to exclude `#`, `\0` and `\u{ffff}`.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

/// The reserved partition-key attribute name.
pub const PK: &str = "pk";

/// The reserved sort-key attribute name.
pub const SK: &str = "sk";

/// The discriminator every item carries.
pub const ITEM_TYPE: &str = "itemType";

/// The exhaustive `INCLUDE` projection of the three dense observation indexes.
///
/// Deliberately **slim**: three full copies plus PITR would multiply retained
/// bytes and break the Area 11 storage-rate assumption outright. `attrS`,
/// `attrN` and `attrB` are absent by design; a page that needs customer
/// attributes issues a base-table `BatchGetItem` (plan §4.6).
pub const DENSE_INDEX_PROJECTION: &[&str] = &[
    "observationId",
    "revision",
    "signal",
    "workspaceId",
    "sessionId",
    "runId",
    "agentId",
    "operationId",
    "time",
    "acceptedAt",
    "acceptedSeq",
    "indexed",
    "traceId",
    "spanId",
    "metricName",
    "bodyInline",
    "bodyS3Key",
    "bodySha256",
];

/// The `INCLUDE` projection of the sparse gap index.
pub const GAP_INDEX_PROJECTION: &[&str] = &[
    "gapId",
    "revision",
    "state",
    "scopeKey",
    "signals",
    "reason",
    "recoverable",
    "openedAt",
    "unbounded",
];

/// Every secondary index on `observation-authority`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Index {
    /// Session- (or workspace-) scoped event-time order.
    ScopeTime,
    /// Workspace-scoped accepted order.
    WorkspaceAccepted,
    /// Workspace-scoped event-time order.
    WorkspaceTime,
    /// Sparse: one exact metric name per UTC day.
    Metric,
    /// Sparse: one trace inside one scope.
    Trace,
    /// Sparse: reconciler and launcher due-scans.
    Control,
    /// Sparse: workspace-scoped gap queries.
    Gap,
}

impl Index {
    /// Every index, in declared order.
    pub const ALL: &'static [Index] = &[
        Index::ScopeTime,
        Index::WorkspaceAccepted,
        Index::WorkspaceTime,
        Index::Metric,
        Index::Trace,
        Index::Control,
        Index::Gap,
    ];

    /// The three indexes that apply to every observation.
    pub const DENSE: &'static [Index] = &[
        Index::ScopeTime,
        Index::WorkspaceAccepted,
        Index::WorkspaceTime,
    ];

    /// The index name in the table definition.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ScopeTime => "gsi_scope_time",
            Self::WorkspaceAccepted => "gsi_ws_accepted",
            Self::WorkspaceTime => "gsi_ws_time",
            Self::Metric => "gsi_metric",
            Self::Trace => "gsi_trace",
            Self::Control => "gsi_control",
            Self::Gap => "gsi_gap",
        }
    }

    /// The partition-key attribute.
    #[must_use]
    pub const fn partition_key(self) -> &'static str {
        match self {
            Self::ScopeTime => "tPk",
            Self::WorkspaceAccepted => "wPk",
            Self::WorkspaceTime => "wtPk",
            Self::Metric => "mPk",
            Self::Trace => "trPk",
            Self::Control => "cPk",
            Self::Gap => "gwPk",
        }
    }

    /// The sort-key attribute.
    #[must_use]
    pub const fn sort_key(self) -> &'static str {
        match self {
            Self::ScopeTime => "tSk",
            Self::WorkspaceAccepted => "wSk",
            Self::WorkspaceTime => "wtSk",
            Self::Metric => "mSk",
            Self::Trace => "trSk",
            Self::Control => "cSk",
            Self::Gap => "gwSk",
        }
    }

    /// Whether the index applies to every observation.
    #[must_use]
    pub const fn is_dense(self) -> bool {
        matches!(
            self,
            Self::ScopeTime | Self::WorkspaceAccepted | Self::WorkspaceTime
        )
    }

    /// The exhaustive `INCLUDE` projection.
    ///
    /// The sparse metric, trace and control indexes are `KEYS_ONLY` plus their
    /// own key attributes, so their projection list is empty.
    #[must_use]
    pub const fn projection(self) -> &'static [&'static str] {
        match self {
            Self::ScopeTime | Self::WorkspaceAccepted | Self::WorkspaceTime => {
                DENSE_INDEX_PROJECTION
            }
            Self::Gap => GAP_INDEX_PROJECTION,
            Self::Metric | Self::Trace | Self::Control => &[],
        }
    }
}

/// Builds an expression with generated name and value placeholders.
///
/// A caller never writes a placeholder itself, so a customer string cannot
/// become one.
#[derive(Clone, Debug, Default)]
pub struct ExpressionBuilder {
    names: HashMap<String, String>,
    values: HashMap<String, AttributeValue>,
    next_name: usize,
    next_value: usize,
}

impl ExpressionBuilder {
    /// A fresh builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds an attribute name, returning its generated placeholder.
    pub fn name(&mut self, attribute: &str) -> String {
        let placeholder = format!("#n{}", self.next_name);
        self.next_name += 1;
        self.names.insert(placeholder.clone(), attribute.to_owned());
        placeholder
    }

    /// Binds a value, returning its generated placeholder.
    pub fn value(&mut self, value: AttributeValue) -> String {
        let placeholder = format!(":v{}", self.next_value);
        self.next_value += 1;
        self.values.insert(placeholder.clone(), value);
        placeholder
    }

    /// Binds a string value.
    pub fn string(&mut self, value: impl Into<String>) -> String {
        self.value(AttributeValue::S(value.into()))
    }

    /// Binds a numeric value.
    pub fn number(&mut self, value: impl std::fmt::Display) -> String {
        self.value(AttributeValue::N(value.to_string()))
    }

    /// Binds a boolean value.
    pub fn boolean(&mut self, value: bool) -> String {
        self.value(AttributeValue::Bool(value))
    }

    /// The accumulated `ExpressionAttributeNames`.
    #[must_use]
    pub fn names(&self) -> HashMap<String, String> {
        self.names.clone()
    }

    /// The accumulated `ExpressionAttributeValues`.
    #[must_use]
    pub fn values(&self) -> HashMap<String, AttributeValue> {
        self.values.clone()
    }

    /// How many distinct names and values are bound.
    #[must_use]
    pub fn len(&self) -> (usize, usize) {
        (self.names.len(), self.values.len())
    }

    /// Whether nothing is bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.values.is_empty()
    }
}

/// The grammar every built expression string must match.
///
/// Asserted by a property test over hostile attribute keys and operands: a
/// character outside this set can only have arrived by string interpolation,
/// which is the bug the placeholders exist to prevent.
#[must_use]
pub fn is_safe_expression(expression: &str) -> bool {
    expression.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || matches!(
                character,
                '#' | ':' | '_' | ' ' | '(' | ')' | '<' | '>' | '=' | ',' | '.'
            )
    })
}

/// `attribute_not_exists(#pk)` — the immutability condition every `OBS#` write
/// carries.
#[must_use]
pub fn immutable_condition(builder: &mut ExpressionBuilder) -> String {
    let pk = builder.name(PK);
    format!("attribute_not_exists({pk})")
}

/// The transaction P condition: create, or resume an identical preparation.
#[must_use]
pub fn prepare_condition(builder: &mut ExpressionBuilder, intent_digest: &str) -> String {
    let pk = builder.name(PK);
    let state = builder.name("state");
    let digest = builder.name("intentDigest");
    let preparing = builder.string("preparing");
    let expected = builder.string(intent_digest);
    format!("attribute_not_exists({pk}) OR ({state} = {preparing} AND {digest} = {expected})")
}

/// The transaction C condition on the scope frontier.
#[must_use]
pub fn frontier_condition(builder: &mut ExpressionBuilder, expected_revision: u64) -> String {
    let revision = builder.name("revision");
    let expected = builder.number(expected_revision);
    format!("{revision} = {expected}")
}

/// The deletion-epoch condition every admission, publication and grant carries.
#[must_use]
pub fn deletion_epoch_condition(builder: &mut ExpressionBuilder, pinned_epoch: u64) -> String {
    let epoch = builder.name("deletionEpoch");
    let pinned = builder.number(pinned_epoch);
    format!("{epoch} = {pinned}")
}

#[cfg(test)]
mod tests {
    use super::{
        DENSE_INDEX_PROJECTION, ExpressionBuilder, Index, deletion_epoch_condition,
        immutable_condition, is_safe_expression, prepare_condition,
    };

    #[test]
    fn the_dense_projection_is_exactly_the_declared_slim_set() {
        assert_eq!(DENSE_INDEX_PROJECTION.len(), 18);
        for forbidden in [
            "attrS",
            "attrN",
            "attrB",
            "signalRank",
            "scopeKey",
            "organizationId",
            "batchId",
            "logicalBytes",
            "attrDigest",
            "seriesHash",
        ] {
            assert!(
                !DENSE_INDEX_PROJECTION.contains(&forbidden),
                "`{forbidden}` must stay in the authority row"
            );
        }
        for index in Index::DENSE {
            assert_eq!(index.projection(), DENSE_INDEX_PROJECTION);
            assert!(index.is_dense());
        }
        assert_eq!(Index::ALL.len(), 7);
        assert!(!Index::Trace.is_dense());
        assert!(Index::Metric.projection().is_empty());
    }

    #[test]
    fn every_index_declares_distinct_key_attributes() {
        let mut seen = std::collections::BTreeSet::new();
        for index in Index::ALL {
            assert!(seen.insert(index.partition_key()), "{}", index.as_str());
            assert!(seen.insert(index.sort_key()), "{}", index.as_str());
        }
        assert_eq!(seen.len(), 14);
    }

    #[test]
    fn a_hostile_operand_never_reaches_the_expression_string() {
        let hostile = "x) OR attribute_not_exists(#pk) --";
        let mut builder = ExpressionBuilder::new();
        let expression = prepare_condition(&mut builder, hostile);
        assert!(is_safe_expression(&expression), "{expression}");
        assert!(
            !expression.contains("OR attribute_not_exists(#pk) --"),
            "the operand leaked into the expression: {expression}"
        );
        assert_eq!(builder.len(), (3, 2));
        assert!(!builder.is_empty());
        assert!(
            builder
                .values()
                .values()
                .any(|value| value.as_s().map(String::as_str) == Ok(hostile))
        );
    }

    #[test]
    fn the_conditions_are_the_declared_shapes() {
        let mut builder = ExpressionBuilder::new();
        assert_eq!(
            immutable_condition(&mut builder),
            "attribute_not_exists(#n0)"
        );
        let mut builder = ExpressionBuilder::new();
        assert_eq!(deletion_epoch_condition(&mut builder, 7), "#n0 = :v0");
        assert_eq!(
            builder
                .values()
                .get(":v0")
                .and_then(|value| value.as_n().ok())
                .cloned(),
            Some("7".to_owned())
        );
        let mut builder = ExpressionBuilder::new();
        assert_eq!(builder.boolean(true), ":v0");
    }
}
