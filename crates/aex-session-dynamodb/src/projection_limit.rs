//! Shared `workspace_limit` key and codec.
//!
//! The serving reader and the authority producer deliberately share this one
//! decoder. A producer-only composition must be able to resolve an ambiguous
//! write without linking the broader regional read capability, while both
//! sides must still agree byte-for-byte on the durable row.

use aex_wire::ids::WorkspaceId;
use aex_wire::limits::{LimitId, LimitShape};
use aex_wire::models::{LimitSource, LimitValue};

use crate::attr::{CodecError, Item, Row};
use crate::wire_pending::ProjectedWorkspaceLimit;

/// The `itemType` of a durable effective workspace limit.
pub const WORKSPACE_LIMIT: &str = "workspace_limit";

/// `LIMIT#WS#{workspace_id}` / `LIMIT#{limit_id}`.
///
/// The authority prefix is deliberately on the partition key: `DynamoDB` IAM
/// can restrict writes with `dynamodb:LeadingKeys`, while a shared `WS#`
/// partition would let central control persist capacity-owned rows.
#[must_use]
pub fn limit_key(workspace: WorkspaceId, limit: LimitId) -> (String, String) {
    (
        format!("LIMIT#WS#{workspace}"),
        format!("LIMIT#{}", limit.as_str()),
    )
}

/// Decodes one durable effective limit and checks its registered shape.
///
/// # Errors
///
/// [`CodecError`] for a missing, mistyped, foreign-tenant, unknown-limit or
/// wrong-shape field.
pub fn decode_limit(
    item: &Item,
    asserted: WorkspaceId,
) -> Result<ProjectedWorkspaceLimit, CodecError> {
    let row = Row::bind(item, WORKSPACE_LIMIT)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let id = LimitId::parse(row.string("limitId")?).ok_or_else(|| CodecError::Malformed {
        item_type: WORKSPACE_LIMIT,
        attribute: "limitId",
        reason: "outside the generated limit registry".to_owned(),
    })?;
    let (partition_key, sort_key) = limit_key(asserted, id);
    exact_key(&row, "pk", &partition_key)?;
    exact_key(&row, "sk", &sort_key)?;
    let effective_value = serde_json::from_str::<LimitValue>(row.string("effectiveValue")?)
        .map_err(|error| CodecError::Malformed {
            item_type: WORKSPACE_LIMIT,
            attribute: "effectiveValue",
            reason: error.to_string(),
        })?;
    let actual_shape = match &effective_value {
        LimitValue::Scalar(_) => LimitShape::Scalar,
        LimitValue::Map(_) => LimitShape::Map,
    };
    if actual_shape != id.shape() {
        return Err(CodecError::Malformed {
            item_type: WORKSPACE_LIMIT,
            attribute: "effectiveValue",
            reason: format!(
                "shape {actual_shape:?} does not match registered {:?}",
                id.shape()
            ),
        });
    }
    let source = match row.string("source")? {
        "default" => LimitSource::Default,
        "workspace_override" => LimitSource::WorkspaceOverride,
        _ => {
            return Err(CodecError::Malformed {
                item_type: WORKSPACE_LIMIT,
                attribute: "source",
                reason: "expected `default` or `workspace_override`".to_owned(),
            });
        }
    };
    Ok(ProjectedWorkspaceLimit {
        workspace: asserted,
        id,
        effective_value,
        source,
        revision: row.u64("revision")?,
        changed_at: row.timestamp("changedAt")?,
    })
}

/// Decodes the row returned for one requested point-read key.
///
/// # Errors
///
/// As [`decode_limit`], plus [`CodecError::Malformed`] when the stored
/// `limitId` does not name the limit whose key was requested.
pub fn decode_limit_at(
    item: &Item,
    asserted: WorkspaceId,
    requested: LimitId,
) -> Result<ProjectedWorkspaceLimit, CodecError> {
    let decoded = decode_limit(item, asserted)?;
    if decoded.id == requested {
        return Ok(decoded);
    }
    Err(CodecError::Malformed {
        item_type: WORKSPACE_LIMIT,
        attribute: "limitId",
        reason: format!(
            "stored `{}` does not match requested `{}`",
            decoded.id.as_str(),
            requested.as_str()
        ),
    })
}

fn exact_key(row: &Row<'_>, attribute: &'static str, expected: &str) -> Result<(), CodecError> {
    let stored = row.string(attribute)?;
    if stored == expected {
        return Ok(());
    }
    Err(CodecError::Malformed {
        item_type: WORKSPACE_LIMIT,
        attribute,
        reason: format!("stored `{stored}` does not match identity `{expected}`"),
    })
}
