//! Shared `workspace_limit` key and codec.
//!
//! The serving reader and the authority producer deliberately share this one
//! decoder. A producer-only composition must be able to resolve an ambiguous
//! write without linking the broader regional read capability, while both
//! sides must still agree byte-for-byte on the durable row.

use aex_wire::ids::WorkspaceId;
use aex_wire::limits::LimitId;
use aex_wire::models::{LimitSource, LimitValue};

use crate::attr::{CodecError, Item, Row};
use crate::wire_pending::ProjectedWorkspaceLimit;
#[cfg(feature = "authz-projection")]
use crate::wire_pending::{ProjectedLimitBundle, ProjectedLimitBundleHead};

/// The `itemType` of a durable effective workspace limit.
pub const WORKSPACE_LIMIT: &str = "workspace_limit";
/// The strongly read revision fence for a complete workspace limit set.
pub const WORKSPACE_LIMIT_BUNDLE_HEAD: &str = "workspace_limit_bundle_head";
/// The complete payload selected by [`WORKSPACE_LIMIT_BUNDLE_HEAD`].
pub const WORKSPACE_LIMIT_BUNDLE: &str = "workspace_limit_bundle";

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

/// `LIMIT#WS#{workspace_id}` / `BUNDLE#HEAD`.
#[must_use]
pub fn limit_bundle_head_key(workspace: WorkspaceId) -> (String, String) {
    (format!("LIMIT#WS#{workspace}"), "BUNDLE#HEAD".to_owned())
}

/// `LIMIT#WS#{workspace_id}` / `BUNDLE#VALUE`.
#[must_use]
pub fn limit_bundle_key(workspace: WorkspaceId) -> (String, String) {
    (format!("LIMIT#WS#{workspace}"), "BUNDLE#VALUE".to_owned())
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
    let actual_shape = effective_value.shape();
    if !effective_value.is_complete_for(id) {
        return Err(CodecError::Malformed {
            item_type: WORKSPACE_LIMIT,
            attribute: "effectiveValue",
            reason: format!(
                "value with shape {actual_shape:?} is incomplete or invalid for registered {:?}",
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

/// Decodes the strong completeness/revision fence.
///
/// # Errors
///
/// [`CodecError`] for any identity, type or value drift.
#[cfg(feature = "authz-projection")]
pub fn decode_limit_bundle_head(
    item: &Item,
    workspace: WorkspaceId,
) -> Result<ProjectedLimitBundleHead, CodecError> {
    let row = Row::bind(item, WORKSPACE_LIMIT_BUNDLE_HEAD)?;
    row.owned_by("workspaceId", &workspace.to_string())?;
    let (pk, sk) = limit_bundle_head_key(workspace);
    exact_key(&row, "pk", &pk)?;
    exact_key(&row, "sk", &sk)?;
    Ok(ProjectedLimitBundleHead {
        workspace,
        revision: row.u64("revision")?,
        defaults_revision: row.u64("defaultsRevision")?,
        changed_at: row.timestamp("changedAt")?,
    })
}

/// Decodes and proves one complete revision-bound payload.
///
/// # Errors
///
/// [`CodecError`] when identity, revision, registry order, completeness or
/// value shape differs from the authored contract.
#[cfg(feature = "authz-projection")]
pub fn decode_limit_bundle(
    item: &Item,
    workspace: WorkspaceId,
) -> Result<ProjectedLimitBundle, CodecError> {
    let row = Row::bind(item, WORKSPACE_LIMIT_BUNDLE)?;
    row.owned_by("workspaceId", &workspace.to_string())?;
    let (pk, sk) = limit_bundle_key(workspace);
    exact_key(&row, "pk", &pk)?;
    exact_key(&row, "sk", &sk)?;
    let revision = row.u64("revision")?;
    let limits = serde_json::from_str::<Vec<aex_wire::models::EffectiveWorkspaceLimit>>(
        row.string("limits")?,
    )
    .map_err(|error| CodecError::Malformed {
        item_type: WORKSPACE_LIMIT_BUNDLE,
        attribute: "limits",
        reason: error.to_string(),
    })?;
    if limits.len() != LimitId::ALL.len() {
        return Err(CodecError::Malformed {
            item_type: WORKSPACE_LIMIT_BUNDLE,
            attribute: "limits",
            reason: format!(
                "expected {} registered limits, found {}",
                LimitId::ALL.len(),
                limits.len()
            ),
        });
    }
    for (expected, limit) in LimitId::ALL.iter().copied().zip(&limits) {
        if limit.id != expected
            || limit.revision != revision
            || !limit.effective_value.is_complete_for(expected)
        {
            return Err(CodecError::Malformed {
                item_type: WORKSPACE_LIMIT_BUNDLE,
                attribute: "limits",
                reason: format!(
                    "entry `{}` disagrees with expected `{}` at revision {revision}",
                    limit.id.as_str(),
                    expected.as_str()
                ),
            });
        }
    }
    Ok(ProjectedLimitBundle {
        workspace,
        revision,
        limits,
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
