//! Read-only classification of `session-authority` stream keys.
//!
//! This module is feature-neutral so a `KEYS_ONLY` consumer can classify native
//! event hints without linking the session authority's write codecs.

use std::collections::HashMap;

use aex_wire::ids::{ObservationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::AttributeValue;

const SESSION_PREFIX: &str = "SESSION#";
const EVENT_PREFIX: &str = "EVT#";

/// Sparse workspace event index name.
pub const WORKSPACE_EVENT_INDEX: &str = "gsi_workspace_events";
/// Workspace event partition attribute.
pub const WORKSPACE_EVENT_PK: &str = "evPk";
/// Workspace event sort attribute.
pub const WORKSPACE_EVENT_SK: &str = "evSk";
/// Sparse session event-time index name.
pub const SESSION_EVENT_INDEX: &str = "gsi_session_events";
/// Session event-time partition attribute.
pub const SESSION_EVENT_PK: &str = "esPk";
/// Session event-time sort attribute.
pub const SESSION_EVENT_SK: &str = "esSk";

/// The deletion state a long-lived session-scoped read must enforce.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionReadState {
    /// The session may still be read.
    Active,
    /// Trash or purge is in progress.
    Deleting,
    /// The authority records it as purged.
    Deleted,
}

/// Returns the owning session for exactly one native event key.
#[must_use]
pub fn parse_event(pk: &str, sk: &str) -> Option<SessionId> {
    if !sk
        .strip_prefix(EVENT_PREFIX)?
        .bytes()
        .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    pk.strip_prefix(SESSION_PREFIX)?.parse().ok()
}

/// Exact strongly consistent point-read key for one session head.
#[must_use]
pub fn head(session: SessionId) -> (String, &'static str) {
    (format!("{SESSION_PREFIX}{session}"), "HEAD")
}

/// Decodes the immutable workspace binding from a session head point read.
#[must_use]
pub fn head_workspace<S: std::hash::BuildHasher>(
    item: &HashMap<String, AttributeValue, S>,
) -> Option<WorkspaceId> {
    item.get("workspaceId")
        .and_then(|value| value.as_s().ok())
        .and_then(|value| value.parse().ok())
}

/// Strictly decodes the workspace binding and lifecycle needed by a stream.
///
/// # Errors
///
/// Returns a static attribute name when the row is not a session head, belongs
/// to another workspace, or carries an unmodelled lifecycle.
pub fn session_read_state<S: std::hash::BuildHasher>(
    item: &HashMap<String, AttributeValue, S>,
    asserted_workspace: WorkspaceId,
) -> Result<SessionReadState, &'static str> {
    let item_type = item
        .get("itemType")
        .and_then(|value| value.as_s().ok())
        .ok_or("itemType")?;
    if item_type != "session_head" {
        return Err("itemType");
    }
    if head_workspace(item) != Some(asserted_workspace) {
        return Err("workspaceId");
    }
    match item
        .get("lifecycle")
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
    {
        Some("active") => Ok(SessionReadState::Active),
        Some("trashed" | "purging") => Ok(SessionReadState::Deleting),
        Some("purged") => Ok(SessionReadState::Deleted),
        _ => Err("lifecycle"),
    }
}

/// Strictly decodes the monotonic deletion epoch from a validated session head.
///
/// # Errors
///
/// Returns the same fail-closed attribute names as [`session_read_state`], or
/// `deletionEpoch` when the authoritative numeric fence is absent or malformed.
pub fn session_deletion_epoch<S: std::hash::BuildHasher>(
    item: &HashMap<String, AttributeValue, S>,
    asserted_workspace: WorkspaceId,
) -> Result<u64, &'static str> {
    session_read_state(item, asserted_workspace)?;
    item.get("deletionEpoch")
        .and_then(|value| value.as_n().ok())
        .and_then(|value| value.parse().ok())
        .ok_or("deletionEpoch")
}

/// `EVTW#{workspace_id}#{YYYY-MM-DDTHH}`.
#[must_use]
pub fn workspace_event_partition(workspace: WorkspaceId, occurred_at: Timestamp) -> String {
    let instant = occurred_at.to_wire();
    workspace_event_partition_hour(workspace, instant.get(..13).unwrap_or(&instant))
}

/// `EVTW#{workspace_id}#{YYYY-MM-DDTHH}` for one already-normalized hour.
#[must_use]
pub fn workspace_event_partition_hour(workspace: WorkspaceId, hour: &str) -> String {
    format!("EVTW#{workspace}#{hour}")
}

/// `EVTS#{session_id}#{YYYY-MM-DDTHH}`.
#[must_use]
pub fn session_event_partition(session: SessionId, occurred_at: Timestamp) -> String {
    let instant = occurred_at.to_wire();
    session_event_partition_hour(session, instant.get(..13).unwrap_or(&instant))
}

/// `EVTS#{session_id}#{YYYY-MM-DDTHH}` for one already-normalized hour.
#[must_use]
pub fn session_event_partition_hour(session: SessionId, hour: &str) -> String {
    format!("EVTS#{session}#{hour}")
}

/// The canonical observation merge tuple for one native event.
#[must_use]
pub fn workspace_event_sort(
    _session: SessionId,
    event: ObservationId,
    occurred_at: Timestamp,
) -> String {
    aex_observation_domain::order::order_sort_key(aex_observation_domain::order::OrderTuple::new(
        occurred_at,
        aex_observation_domain::signal::Signal::Events,
        event,
        1,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7};
    use aws_sdk_dynamodb::types::AttributeValue;

    use super::{SessionReadState, head, parse_event, session_deletion_epoch, session_read_state};

    #[test]
    fn only_native_event_keys_classify() {
        let session = SessionId::from_uuid7(Uuid7::compose(1, [2; 10]));
        let (pk, sk) = head(session);
        assert_eq!(parse_event(&pk, "EVT#00000000000000000001"), Some(session));
        assert_eq!(parse_event(&pk, sk), None);
        assert_eq!(parse_event(&pk, "EVT#not-a-sequence"), None);
        assert_eq!(parse_event("BRAINAGENT#irrelevant", "EVT#0001"), None);
    }

    #[test]
    fn stream_session_state_fails_closed_and_distinguishes_deletion() {
        let workspace = aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(1, [3; 10]));
        let mut item = HashMap::from([
            (
                "itemType".to_owned(),
                AttributeValue::S("session_head".to_owned()),
            ),
            (
                "workspaceId".to_owned(),
                AttributeValue::S(workspace.to_string()),
            ),
            (
                "lifecycle".to_owned(),
                AttributeValue::S("active".to_owned()),
            ),
            (
                "deletionEpoch".to_owned(),
                AttributeValue::N("3".to_owned()),
            ),
        ]);
        assert_eq!(
            session_read_state(&item, workspace),
            Ok(SessionReadState::Active)
        );
        assert_eq!(session_deletion_epoch(&item, workspace), Ok(3));
        item.insert(
            "lifecycle".to_owned(),
            AttributeValue::S("purging".to_owned()),
        );
        assert_eq!(
            session_read_state(&item, workspace),
            Ok(SessionReadState::Deleting)
        );
        item.insert(
            "lifecycle".to_owned(),
            AttributeValue::S("purged".to_owned()),
        );
        assert_eq!(
            session_read_state(&item, workspace),
            Ok(SessionReadState::Deleted)
        );
        item.insert(
            "lifecycle".to_owned(),
            AttributeValue::S("future_state".to_owned()),
        );
        assert_eq!(session_read_state(&item, workspace), Err("lifecycle"));
    }
}
