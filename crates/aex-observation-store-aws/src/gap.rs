//! Canonical telemetry-gap persistence.
//!
//! Every writer and reader uses this codec. A malformed row is an authority
//! failure, never a reason to invent a default reason, signal, range or owner.

use std::collections::HashMap;

use aex_observation_domain::gap::{
    GapRecord, GapRevision, GapState, OrdinalRange, PRODUCIBLE_REASONS, TimeWindow,
};
use aex_observation_domain::keys::{self, ScopeKey};
use aex_observation_domain::signal::{Signal, SignalSet};
use aex_wire::ids::{PrefixedId as _, TelemetryGapId, WorkspaceId};
use aex_wire::models::TelemetryGapReason;
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::{AttributeValue, Put, TransactWriteItem};

use crate::expressions::{ITEM_TYPE, PK, SK};

/// The durable item discriminator registered by the regional table schema.
pub const GAP_ITEM_TYPE: &str = "telemetry_gap";

/// Why a gap row could not be encoded or decoded.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GapCodecError {
    /// A required attribute was absent or had the wrong DynamoDB type.
    #[error("gap attribute `{attribute}` is missing or has the wrong type")]
    Attribute {
        /// The malformed attribute.
        attribute: &'static str,
    },
    /// An attribute was present but contradicted another durable fact.
    #[error("gap attribute `{attribute}` is inconsistent: {reason}")]
    Inconsistent {
        /// The inconsistent attribute.
        attribute: &'static str,
        /// The failed invariant.
        reason: Box<str>,
    },
}

/// Why the durable gap store refused an append.
#[derive(Debug, thiserror::Error)]
pub enum GapStoreError {
    /// The row did not satisfy the canonical codec.
    #[error(transparent)]
    Codec(#[from] GapCodecError),
    /// Immutable history or revision ordering was contradicted.
    #[error("gap append conflicts with durable history: {reason}")]
    Conflict {
        /// The failed append invariant.
        reason: Box<str>,
    },
    /// DynamoDB did not establish an outcome that could be resolved by key.
    #[error("the observation authority is unavailable during {operation}: {reason}")]
    Provider {
        /// The provider operation.
        operation: &'static str,
        /// Sanitized provider failure.
        reason: Box<str>,
    },
}

/// Async DynamoDB adapter for immutable gap revisions.
#[derive(Clone, Debug)]
pub struct GapStore {
    dynamodb: aws_sdk_dynamodb::Client,
    table: String,
}

impl GapStore {
    /// Binds the adapter to one regional observation-authority table.
    #[must_use]
    pub fn new(dynamodb: aws_sdk_dynamodb::Client, table: impl Into<String>) -> Self {
        Self {
            dynamodb,
            table: table.into(),
        }
    }

    /// Appends one immutable contiguous revision.
    ///
    /// Replaying byte-equivalent evidence is success. A revision with different
    /// bytes at the same key is a conflict. Unknown write outcomes are resolved
    /// by a strongly consistent read of that exact key.
    ///
    /// # Errors
    ///
    /// Returns [`GapStoreError::Conflict`] for missing predecessors or unequal
    /// replays and [`GapStoreError::Provider`] when DynamoDB remains unknown.
    pub async fn append(&self, record: &GapRecord) -> Result<(), GapStoreError> {
        let item = encode(record)?;
        if let Some(found) = self.read_exact(record).await? {
            return equal_replay(&found, record);
        }
        if record.revision.revision > 0 {
            let mut predecessor = record.clone();
            predecessor.revision.revision -= 1;
            let Some(previous) = self.read_exact(&predecessor).await? else {
                return Err(GapStoreError::Conflict {
                    reason: "the predecessor revision is absent".into(),
                });
            };
            validate_successor(&previous, record)?;
        } else if record.revision.state == GapState::Repaired {
            return Err(GapStoreError::Conflict {
                reason: "revision zero cannot be repaired".into(),
            });
        }

        let mut builder = crate::expressions::ExpressionBuilder::new();
        let pk = builder.name(PK);
        let sk = builder.name(SK);
        let outcome = self
            .dynamodb
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .condition_expression(format!(
                "attribute_not_exists({pk}) AND attribute_not_exists({sk})"
            ))
            .set_expression_attribute_names(Some(builder.names()))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => match self.read_exact(record).await? {
                Some(found) => equal_replay(&found, record),
                None => Err(GapStoreError::Provider {
                    operation: "PutItem",
                    reason: error.to_string().into_boxed_str(),
                }),
            },
        }
    }

    async fn read_exact(&self, key: &GapRecord) -> Result<Option<GapRecord>, GapStoreError> {
        let response = self
            .dynamodb
            .get_item()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(keys::gap_pk(&key.scope)))
            .key(
                SK,
                AttributeValue::S(keys::gap_sk(key.revision.gap_id, key.revision.revision)),
            )
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| GapStoreError::Provider {
                operation: "GetItem",
                reason: error.to_string().into_boxed_str(),
            })?;
        response
            .item
            .as_ref()
            .map(decode)
            .transpose()
            .map_err(GapStoreError::from)
    }
}

#[async_trait::async_trait]
impl aex_observation_application::ports::GapSink for GapStore {
    async fn append_gap(
        &self,
        record: &GapRecord,
    ) -> Result<(), aex_observation_application::ports::PortError> {
        self.append(record).await.map_err(|error| {
            aex_observation_application::ports::PortError::Unavailable {
                authority: "observation",
                reason: error.to_string().into_boxed_str(),
            }
        })
    }
}

impl GapCodecError {
    fn attribute(attribute: &'static str) -> Self {
        Self::Attribute { attribute }
    }

    fn inconsistent(attribute: &'static str, reason: impl Into<Box<str>>) -> Self {
        Self::Inconsistent {
            attribute,
            reason: reason.into(),
        }
    }
}

/// Encodes one complete immutable gap revision.
///
/// # Errors
///
/// Returns [`GapCodecError`] when public fields were mutated into a shape that
/// contradicts the durable gap invariants.
pub fn encode(record: &GapRecord) -> Result<HashMap<String, AttributeValue>, GapCodecError> {
    validate(record)?;
    let revision = &record.revision;
    let mut item = HashMap::new();
    item.insert(
        PK.to_owned(),
        AttributeValue::S(keys::gap_pk(&record.scope)),
    );
    item.insert(
        SK.to_owned(),
        AttributeValue::S(keys::gap_sk(revision.gap_id, revision.revision)),
    );
    item.insert(
        ITEM_TYPE.to_owned(),
        AttributeValue::S(GAP_ITEM_TYPE.to_owned()),
    );
    item.insert(
        "gapId".to_owned(),
        AttributeValue::S(revision.gap_id.to_string()),
    );
    item.insert(
        "revision".to_owned(),
        AttributeValue::N(revision.revision.to_string()),
    );
    item.insert(
        "state".to_owned(),
        AttributeValue::S(revision.state.as_str().to_owned()),
    );
    item.insert(
        "scopeKey".to_owned(),
        AttributeValue::S(record.scope.to_key()),
    );
    item.insert(
        "workspaceId".to_owned(),
        AttributeValue::S(record.workspace.to_string()),
    );
    if let Some(session) = record.scope.session() {
        item.insert(
            "sessionId".to_owned(),
            AttributeValue::S(session.to_string()),
        );
    }
    item.insert(
        "signals".to_owned(),
        AttributeValue::L(
            revision
                .signals
                .iter()
                .map(|signal| AttributeValue::S(signal.as_str().to_owned()))
                .collect(),
        ),
    );
    item.insert(
        "reason".to_owned(),
        AttributeValue::S(revision.reason.as_str().to_owned()),
    );
    if let Some(range) = revision.ordinal_range {
        item.insert(
            "ordinalRange".to_owned(),
            AttributeValue::M(HashMap::from([
                ("lo".to_owned(), AttributeValue::N(range.lo().to_string())),
                ("hi".to_owned(), AttributeValue::N(range.hi().to_string())),
            ])),
        );
    }
    if let Some(range) = revision.time_range {
        item.insert(
            "timeRange".to_owned(),
            AttributeValue::M(HashMap::from([
                ("gte".to_owned(), AttributeValue::S(range.from().to_wire())),
                ("lt".to_owned(), AttributeValue::S(range.to().to_wire())),
            ])),
        );
    }
    item.insert(
        "unbounded".to_owned(),
        AttributeValue::Bool(revision.unbounded),
    );
    optional_number(&mut item, "attemptedRecords", record.attempted_records);
    optional_number(&mut item, "attemptedBytes", record.attempted_bytes);
    item.insert(
        "recoverable".to_owned(),
        AttributeValue::Bool(record.recoverable),
    );
    item.insert(
        "openedAt".to_owned(),
        AttributeValue::S(revision.opened_at.to_wire()),
    );
    item.insert(
        "revisedAt".to_owned(),
        AttributeValue::S(revision.revised_at.to_wire()),
    );
    if let Some(source) = &revision.repair_source {
        item.insert(
            "repairSource".to_owned(),
            AttributeValue::S(source.to_string()),
        );
        item.insert(
            "repairedAt".to_owned(),
            AttributeValue::S(revision.revised_at.to_wire()),
        );
    }
    item.insert(
        "gwPk".to_owned(),
        AttributeValue::S(format!("GAPW#{}", record.workspace)),
    );
    item.insert(
        "gwSk".to_owned(),
        AttributeValue::S(workspace_sort_key(revision)),
    );
    Ok(item)
}

/// Builds the immutable Put action used when a source terminalizes atomically
/// with one or more gap revisions.
///
/// # Errors
///
/// Returns [`GapCodecError`] when the record is not canonical.
pub fn append_action(table: &str, record: &GapRecord) -> Result<TransactWriteItem, GapCodecError> {
    let mut builder = crate::expressions::ExpressionBuilder::new();
    let pk = builder.name(PK);
    let sk = builder.name(SK);
    let put = Put::builder()
        .table_name(table)
        .set_item(Some(encode(record)?))
        .condition_expression(format!(
            "attribute_not_exists({pk}) AND attribute_not_exists({sk})"
        ))
        .set_expression_attribute_names(Some(builder.names()))
        .build()
        .map_err(|error| GapCodecError::inconsistent("pk", error.to_string()))?;
    Ok(TransactWriteItem::builder().put(put).build())
}

/// Decodes and validates one complete base-table gap row.
///
/// # Errors
///
/// Returns [`GapCodecError`] for every missing, mistyped or contradictory
/// durable fact. No field is defaulted.
pub fn decode(item: &HashMap<String, AttributeValue>) -> Result<GapRecord, GapCodecError> {
    if string(item, ITEM_TYPE)? != GAP_ITEM_TYPE {
        return Err(GapCodecError::inconsistent(
            ITEM_TYPE,
            "expected telemetry_gap",
        ));
    }
    let workspace = WorkspaceId::parse(string(item, "workspaceId")?)
        .map_err(|_| GapCodecError::attribute("workspaceId"))?;
    let scope = ScopeKey::parse(string(item, "scopeKey")?)
        .map_err(|_| GapCodecError::attribute("scopeKey"))?;
    let gap_id = TelemetryGapId::parse(string(item, "gapId")?)
        .map_err(|_| GapCodecError::attribute("gapId"))?;
    let revision_number = number(item, "revision")?;
    let state = parse_state(string(item, "state")?)?;
    let reason = parse_reason(string(item, "reason")?)?;
    let signals = parse_signals(item)?;
    let time_range = optional_time_range(item)?;
    let ordinal_range = optional_ordinal_range(item)?;
    let unbounded = boolean(item, "unbounded")?;
    let opened_at = timestamp(item, "openedAt")?;
    let revised_at = timestamp(item, "revisedAt")?;
    let repair_source = optional_string(item, "repairSource")?.map(Box::<str>::from);
    let record = GapRecord {
        workspace,
        scope,
        revision: GapRevision {
            gap_id,
            revision: revision_number,
            state,
            signals,
            reason,
            time_range,
            ordinal_range,
            unbounded,
            revised_at,
            opened_at,
            repair_source,
        },
        attempted_records: optional_number_value(item, "attemptedRecords")?,
        attempted_bytes: optional_number_value(item, "attemptedBytes")?,
        recoverable: boolean(item, "recoverable")?,
    };
    validate(&record)?;
    validate_keys(item, &record)?;
    Ok(record)
}

/// Workspace-index sort key for one immutable revision.
#[must_use]
pub fn workspace_sort_key(revision: &GapRevision) -> String {
    format!(
        "{}#{}#{:020}",
        revision.opened_at.to_wire(),
        revision.gap_id,
        revision.revision
    )
}

fn validate(record: &GapRecord) -> Result<(), GapCodecError> {
    let revision = &record.revision;
    if revision.signals.is_empty() {
        return Err(GapCodecError::inconsistent("signals", "empty set"));
    }
    if !PRODUCIBLE_REASONS.contains(&revision.reason) {
        return Err(GapCodecError::inconsistent(
            "reason",
            "reason is not producible",
        ));
    }
    if revision.unbounded != revision.time_range.is_none() {
        return Err(GapCodecError::inconsistent(
            "unbounded",
            "must equal timeRange absence",
        ));
    }
    if revision.revised_at < revision.opened_at {
        return Err(GapCodecError::inconsistent(
            "revisedAt",
            "precedes openedAt",
        ));
    }
    if revision.revision == 0 && revision.state == GapState::Repaired {
        return Err(GapCodecError::inconsistent(
            "revision",
            "revision zero cannot be repaired",
        ));
    }
    match (revision.state, revision.repair_source.as_deref()) {
        (GapState::Repaired, Some(source)) if !source.is_empty() && source.len() <= 128 => {}
        (GapState::Repaired, _) => {
            return Err(GapCodecError::inconsistent(
                "repairSource",
                "a repaired revision requires 1..=128 bytes",
            ));
        }
        (_, None) => {}
        (_, Some(_)) => {
            return Err(GapCodecError::inconsistent(
                "repairSource",
                "only repaired revisions carry provenance",
            ));
        }
    }
    if let ScopeKey::Workspace(scope_workspace) = record.scope
        && scope_workspace != record.workspace
    {
        return Err(GapCodecError::inconsistent(
            "workspaceId",
            "does not match workspace scope",
        ));
    }
    Ok(())
}

fn equal_replay(found: &GapRecord, offered: &GapRecord) -> Result<(), GapStoreError> {
    if found == offered {
        Ok(())
    } else {
        Err(GapStoreError::Conflict {
            reason: "the revision key already contains different evidence".into(),
        })
    }
}

fn validate_successor(previous: &GapRecord, next: &GapRecord) -> Result<(), GapStoreError> {
    let contiguous = previous.revision.revision.checked_add(1) == Some(next.revision.revision);
    let same_identity = previous.workspace == next.workspace
        && previous.scope == next.scope
        && previous.revision.gap_id == next.revision.gap_id;
    let same_evidence = previous.revision.opened_at == next.revision.opened_at
        && previous.revision.signals == next.revision.signals
        && previous.revision.reason == next.revision.reason
        && previous.revision.time_range == next.revision.time_range
        && previous.revision.ordinal_range == next.revision.ordinal_range
        && previous.revision.unbounded == next.revision.unbounded
        && previous.attempted_records == next.attempted_records
        && previous.attempted_bytes == next.attempted_bytes
        && previous.recoverable == next.recoverable;
    if contiguous
        && same_identity
        && same_evidence
        && next.revision.revised_at >= previous.revision.revised_at
    {
        Ok(())
    } else {
        Err(GapStoreError::Conflict {
            reason: "the revision is not a contiguous successor carrying the same evidence".into(),
        })
    }
}

fn validate_keys(
    item: &HashMap<String, AttributeValue>,
    record: &GapRecord,
) -> Result<(), GapCodecError> {
    let expected = [
        (PK, keys::gap_pk(&record.scope)),
        (
            SK,
            keys::gap_sk(record.revision.gap_id, record.revision.revision),
        ),
        ("gwPk", format!("GAPW#{}", record.workspace)),
        ("gwSk", workspace_sort_key(&record.revision)),
    ];
    for (attribute, expected) in expected {
        if string(item, attribute)? != expected {
            return Err(GapCodecError::inconsistent(attribute, "key mismatch"));
        }
    }
    match (record.scope.session(), optional_string(item, "sessionId")?) {
        (Some(expected), Some(found)) if found == expected.to_string() => {}
        (None, None) => {}
        _ => return Err(GapCodecError::inconsistent("sessionId", "scope mismatch")),
    }
    if record.revision.state == GapState::Repaired {
        if timestamp(item, "repairedAt")? != record.revision.revised_at {
            return Err(GapCodecError::inconsistent(
                "repairedAt",
                "must equal revisedAt",
            ));
        }
    } else if item.contains_key("repairedAt") {
        return Err(GapCodecError::inconsistent(
            "repairedAt",
            "only repaired revisions carry it",
        ));
    }
    Ok(())
}

fn optional_number(
    item: &mut HashMap<String, AttributeValue>,
    name: &'static str,
    value: Option<u64>,
) {
    if let Some(value) = value {
        item.insert(name.to_owned(), AttributeValue::N(value.to_string()));
    }
}

fn string<'a>(
    item: &'a HashMap<String, AttributeValue>,
    name: &'static str,
) -> Result<&'a str, GapCodecError> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
        .ok_or_else(|| GapCodecError::attribute(name))
}

fn optional_string<'a>(
    item: &'a HashMap<String, AttributeValue>,
    name: &'static str,
) -> Result<Option<&'a str>, GapCodecError> {
    let Some(value) = item.get(name) else {
        return Ok(None);
    };
    value
        .as_s()
        .map(String::as_str)
        .map(Some)
        .map_err(|_| GapCodecError::attribute(name))
}

fn number(
    item: &HashMap<String, AttributeValue>,
    name: &'static str,
) -> Result<u64, GapCodecError> {
    string_number(item, name)?.map_or_else(|| Err(GapCodecError::attribute(name)), Ok)
}

fn optional_number_value(
    item: &HashMap<String, AttributeValue>,
    name: &'static str,
) -> Result<Option<u64>, GapCodecError> {
    string_number(item, name)
}

fn string_number(
    item: &HashMap<String, AttributeValue>,
    name: &'static str,
) -> Result<Option<u64>, GapCodecError> {
    let Some(value) = item.get(name) else {
        return Ok(None);
    };
    let text = value.as_n().map_err(|_| GapCodecError::attribute(name))?;
    text.parse::<u64>()
        .map(Some)
        .map_err(|_| GapCodecError::attribute(name))
}

fn boolean(
    item: &HashMap<String, AttributeValue>,
    name: &'static str,
) -> Result<bool, GapCodecError> {
    item.get(name)
        .and_then(|value| value.as_bool().ok())
        .copied()
        .ok_or_else(|| GapCodecError::attribute(name))
}

fn timestamp(
    item: &HashMap<String, AttributeValue>,
    name: &'static str,
) -> Result<Timestamp, GapCodecError> {
    Timestamp::parse(string(item, name)?).map_err(|_| GapCodecError::attribute(name))
}

fn parse_state(value: &str) -> Result<GapState, GapCodecError> {
    GapState::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.as_str() == value)
        .ok_or_else(|| GapCodecError::attribute("state"))
}

fn parse_reason(value: &str) -> Result<TelemetryGapReason, GapCodecError> {
    PRODUCIBLE_REASONS
        .iter()
        .copied()
        .find(|candidate| candidate.as_str() == value)
        .ok_or_else(|| GapCodecError::attribute("reason"))
}

fn parse_signals(item: &HashMap<String, AttributeValue>) -> Result<SignalSet, GapCodecError> {
    let values = item
        .get("signals")
        .and_then(|value| value.as_l().ok())
        .ok_or_else(|| GapCodecError::attribute("signals"))?;
    let mut signals = SignalSet::EMPTY;
    for value in values {
        let word = value
            .as_s()
            .map_err(|_| GapCodecError::attribute("signals"))?;
        let signal = Signal::parse(word).ok_or_else(|| GapCodecError::attribute("signals"))?;
        if signals.contains(signal) {
            return Err(GapCodecError::inconsistent("signals", "duplicate signal"));
        }
        signals = signals.with(signal);
    }
    if signals.is_empty() {
        return Err(GapCodecError::inconsistent("signals", "empty set"));
    }
    Ok(signals)
}

fn optional_time_range(
    item: &HashMap<String, AttributeValue>,
) -> Result<Option<TimeWindow>, GapCodecError> {
    let Some(value) = item.get("timeRange") else {
        return Ok(None);
    };
    let map = value
        .as_m()
        .map_err(|_| GapCodecError::attribute("timeRange"))?;
    let from = timestamp(map, "gte")?;
    let to = timestamp(map, "lt")?;
    TimeWindow::new(from, to)
        .map(Some)
        .ok_or_else(|| GapCodecError::inconsistent("timeRange", "empty or inverted"))
}

fn optional_ordinal_range(
    item: &HashMap<String, AttributeValue>,
) -> Result<Option<OrdinalRange>, GapCodecError> {
    let Some(value) = item.get("ordinalRange") else {
        return Ok(None);
    };
    let map = value
        .as_m()
        .map_err(|_| GapCodecError::attribute("ordinalRange"))?;
    let lo = number(map, "lo")?;
    let hi = number(map, "hi")?;
    OrdinalRange::new(lo, hi)
        .map(Some)
        .ok_or_else(|| GapCodecError::inconsistent("ordinalRange", "inverted"))
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, equal_replay, validate_successor};
    use aex_observation_domain::gap::{GapRecord, GapRevision, OrdinalRange, TimeWindow};
    use aex_observation_domain::keys::ScopeKey;
    use aex_observation_domain::signal::{Signal, SignalSet};
    use aex_wire::ids::{PrefixedId as _, SessionId, TelemetryGapId, WorkspaceId};
    use aex_wire::models::TelemetryGapReason;
    use aex_wire::types::Timestamp;

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("fixture instant")
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::parse("wsp_0000000001e40r2081040g2081").expect("workspace")
    }

    fn session() -> SessionId {
        SessionId::parse("ses_0000000003ec1r60r30c1g60r3").expect("session")
    }

    fn record() -> GapRecord {
        let revision = GapRevision::open(
            TelemetryGapId::parse("gap_0000000001e40r2081040g2081").expect("gap"),
            SignalSet::from_signal(Signal::Logs),
            TelemetryGapReason::SpoolLost,
            TimeWindow::new(instant(10), instant(21)),
            instant(30),
        )
        .with_ordinals(OrdinalRange::new(4, 8).expect("range"));
        GapRecord::try_new(
            workspace(),
            ScopeKey::Session(session()),
            revision,
            Some(5),
            Some(512),
            false,
        )
        .expect("record")
    }

    #[test]
    fn a_gap_row_round_trips_without_losing_evidence() {
        let expected = record();
        let item = encode(&expected).expect("encodes");
        assert_eq!(
            item.get("gwPk").and_then(|value| value.as_s().ok()),
            Some(&format!("GAPW#{}", workspace()))
        );
        assert_eq!(decode(&item).expect("decodes"), expected);
    }

    #[test]
    fn an_unbounded_gap_has_no_invented_time_range() {
        let mut expected = record();
        expected.revision.time_range = None;
        expected.revision.unbounded = true;
        let item = encode(&expected).expect("encodes");
        assert!(!item.contains_key("timeRange"));
        assert_eq!(decode(&item).expect("decodes"), expected);
    }

    #[test]
    fn a_corrupt_reason_is_refused_instead_of_defaulted() {
        let mut item = encode(&record()).expect("encodes");
        item.insert(
            "reason".to_owned(),
            aws_sdk_dynamodb::types::AttributeValue::S("mystery".to_owned()),
        );
        assert!(decode(&item).is_err());
    }

    #[test]
    fn a_mistyped_optional_attribute_is_refused_instead_of_treated_as_absent() {
        let mut item = encode(&record()).expect("encodes");
        item.insert(
            "repairSource".to_owned(),
            aws_sdk_dynamodb::types::AttributeValue::Bool(false),
        );
        assert!(decode(&item).is_err());
    }

    #[test]
    fn a_repaired_revision_zero_is_not_canonical() {
        let mut record = record();
        record.revision.state = aex_observation_domain::gap::GapState::Repaired;
        record.revision.repair_source = Some("retained-stage".into());
        assert!(encode(&record).is_err());
    }

    #[test]
    fn an_identical_replay_is_success_and_different_evidence_conflicts() {
        let offered = record();
        assert!(equal_replay(&offered, &offered).is_ok());
        let mut different = offered.clone();
        different.attempted_bytes = Some(513);
        assert!(equal_replay(&offered, &different).is_err());
    }

    #[test]
    fn only_a_contiguous_revision_with_unchanged_evidence_is_a_successor() {
        let previous = record();
        let mut next = previous.clone();
        next.revision = previous.revision.revised(instant(31));
        assert!(validate_successor(&previous, &next).is_ok());

        next.attempted_records = Some(6);
        assert!(validate_successor(&previous, &next).is_err());
    }
}
