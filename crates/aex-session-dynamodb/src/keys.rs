//! The `session-authority` key templates.
//!
//! This module is the single owner of these keys. `aex-brain-store-dynamodb` calls it
//! rather than forking the item shapes, which is the whole reason Brain and the
//! session API cannot drift apart on row shape.
//!
//! Brain's session-level items live in this table under the `BRAIN#` sort-key
//! prefix inside `SESSION#{session_id}`; Brain's per-agent items get their own
//! partition. Both are reserved here so a future Brain key cannot collide with
//! a session key.

use aex_wire::ids::{AgentId, ApprovalId, MessageId, OperationId, RunId, SessionId, WorkspaceId};

use crate::component::{Component, KeyError, sequence};

/// One composite key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// The partition key.
    pub pk: String,
    /// The sort key.
    pub sk: String,
}

impl Key {
    fn new(pk: String, sk: String) -> Self {
        Self { pk, sk }
    }
}

/// The sort-key prefix reserved for Brain's session-level items.
///
/// Nothing in this module ever writes one; the constant exists so the reserved
/// space is visible here and a session key can be proved not to enter it.
pub const BRAIN_PREFIX: &str = "BRAIN#";

/// The partition prefix reserved for Brain's per-agent items.
pub const BRAIN_AGENT_PARTITION_PREFIX: &str = "BRAINAGENT#";

/// `SESSION#{session_id}`.
#[must_use]
pub fn session_partition(session: SessionId) -> String {
    format!("SESSION#{session}")
}

/// `AGENT#{session_id}#{agent_id}`.
#[must_use]
pub fn agent_partition(session: SessionId, agent: AgentId) -> String {
    format!("AGENT#{session}#{agent}")
}

/// The session head.
#[must_use]
pub fn head(session: SessionId) -> Key {
    Key::new(session_partition(session), "HEAD".to_owned())
}

/// One message.
#[must_use]
pub fn message(session: SessionId, message: MessageId) -> Key {
    Key::new(session_partition(session), format!("MSG#{message}"))
}

/// The lower bound of the message range inside one session.
#[must_use]
pub fn message_prefix() -> &'static str {
    "MSG#"
}

/// One run.
#[must_use]
pub fn run(session: SessionId, run: RunId) -> Key {
    Key::new(session_partition(session), format!("RUN#{run}"))
}

/// The run range prefix.
#[must_use]
pub fn run_prefix() -> &'static str {
    "RUN#"
}

/// One native event, ordered by its contiguous sequence.
#[must_use]
pub fn event(session: SessionId, event_seq: u64) -> Key {
    Key::new(
        session_partition(session),
        format!("EVT#{}", sequence(event_seq)),
    )
}

/// The event range prefix.
#[must_use]
pub fn event_prefix() -> &'static str {
    "EVT#"
}

/// One approval.
#[must_use]
pub fn approval(session: SessionId, approval: ApprovalId) -> Key {
    Key::new(session_partition(session), format!("APPROVAL#{approval}"))
}

/// The approval range prefix.
///
/// Approval identities are time-ordered, so one `begins_with` range over this
/// prefix lists a session's approvals oldest first without a filter expression
/// and without an index.
#[must_use]
pub fn approval_prefix() -> &'static str {
    "APPROVAL#"
}

/// The bounded agent registry entry inside the session partition.
#[must_use]
pub fn agent_index(session: SessionId, agent: AgentId) -> Key {
    Key::new(session_partition(session), format!("AGENT#{agent}"))
}

/// The clone child edge.
#[must_use]
pub fn clone_edge(session: SessionId, child: SessionId) -> Key {
    Key::new(session_partition(session), format!("CLONE#{child}"))
}

/// The session-partition edge to a durable operation.
#[must_use]
pub fn operation_edge(session: SessionId, operation: OperationId) -> Key {
    Key::new(session_partition(session), format!("OP#{operation}"))
}

/// One spend reservation.
///
/// # Errors
///
/// [`KeyError`] when the reservation identity could not enter a key.
pub fn reservation(session: SessionId, reservation: &str) -> Result<Key, KeyError> {
    let reservation = Component::parse(reservation)?;
    Ok(Key::new(
        session_partition(session),
        format!("RSV#{reservation}"),
    ))
}

/// The agent's control item.
#[must_use]
pub fn agent_control(session: SessionId, agent: AgentId) -> Key {
    Key::new(agent_partition(session, agent), "CONTROL".to_owned())
}

/// One immutable journal entry.
#[must_use]
pub fn journal(session: SessionId, agent: AgentId, seq: u64) -> Key {
    Key::new(
        agent_partition(session, agent),
        format!("J#{}", sequence(seq)),
    )
}

/// The journal range prefix.
#[must_use]
pub fn journal_prefix() -> &'static str {
    "J#"
}

/// The journal sort key a range query starts from.
#[must_use]
pub fn journal_sort_key(seq: u64) -> String {
    format!("J#{}", sequence(seq))
}

/// One agent effect.
///
/// # Errors
///
/// [`KeyError`] when the effect identity could not enter a key.
pub fn effect(session: SessionId, agent: AgentId, effect: &str) -> Result<Key, KeyError> {
    let effect = Component::parse(effect)?;
    Ok(Key::new(
        agent_partition(session, agent),
        format!("EFFECT#{effect}"),
    ))
}

/// One fanout page.
///
/// # Errors
///
/// [`KeyError`] when `intent` could not enter a key.
pub fn fanout_page(
    session: SessionId,
    agent: AgentId,
    intent: &str,
    page: u32,
) -> Result<Key, KeyError> {
    let intent = Component::parse(intent)?;
    Ok(Key::new(
        agent_partition(session, agent),
        format!("FANOUT#{intent}#{}", crate::component::page(page)),
    ))
}

/// One join membership shard.
///
/// # Errors
///
/// [`KeyError`] when `join` could not enter a key.
pub fn join_shard(
    session: SessionId,
    agent: AgentId,
    join: &str,
    shard: u16,
) -> Result<Key, KeyError> {
    let join = Component::parse(join)?;
    Ok(Key::new(
        agent_partition(session, agent),
        format!("JOIN#{join}#{}", crate::component::shard4(shard)),
    ))
}

/// One child budget return.
#[must_use]
pub fn budget_return(session: SessionId, parent: AgentId, child: AgentId) -> Key {
    Key::new(
        agent_partition(session, parent),
        format!("GRANTRET#{child}"),
    )
}

/// The durable operation record.
#[must_use]
pub fn operation(operation: OperationId) -> Key {
    Key::new(format!("OP#{operation}"), "STATE".to_owned())
}

/// The idempotency receipt.
///
/// # Errors
///
/// [`KeyError`] when the rendered scope could not enter a key, which can only
/// happen if a scope subject bypassed validation.
pub fn receipt(workspace: WorkspaceId, scope: &str, key_sha256_hex: &str) -> Result<Key, KeyError> {
    let scope = Component::parse(scope)?;
    let digest = Component::parse(key_sha256_hex)?;
    Ok(Key::new(
        format!("IDEM#{workspace}#{scope}#{digest}"),
        "RECEIPT".to_owned(),
    ))
}

/// The sparse workspace-axis index.
pub mod workspace_index {
    use aex_wire::ids::{OperationId, SessionId, WorkspaceId};
    use aex_wire::types::Timestamp;

    /// The index name.
    pub const NAME: &str = "gsi_workspace_index";

    /// The index partition key attribute.
    pub const PK: &str = "wsIndexPk";

    /// The index sort key attribute.
    pub const SK: &str = "wsIndexSk";

    /// The index partition for session heads at one lifecycle.
    ///
    /// `lifecycle` lives inside the partition key, so trashing moves the item to
    /// a different index partition and purging removes the attributes entirely.
    /// Ordinary list therefore needs no filter expression, and a purged session
    /// is physically absent from the index rather than filtered out of it.
    #[must_use]
    pub fn session_partition(workspace: WorkspaceId, lifecycle: &str) -> String {
        format!("WS#{workspace}#SESSION#{lifecycle}")
    }

    /// The index partition for durable operations.
    #[must_use]
    pub fn operation_partition(workspace: WorkspaceId) -> String {
        format!("WS#{workspace}#OP")
    }

    /// The index sort key for a session head.
    #[must_use]
    pub fn session_sort(created_at: Timestamp, session: SessionId) -> String {
        format!("{}#{session}", created_at.to_wire())
    }

    /// The index sort key for an operation.
    #[must_use]
    pub fn operation_sort(created_at: Timestamp, operation: OperationId) -> String {
        format!("{}#{operation}", created_at.to_wire())
    }
}

/// Every session lifecycle value, in the order a session moves through them.
pub const LIFECYCLES: &[&str] = &["active", "trashed", "purging", "purged"];

/// Every session status value.
pub const STATUSES: &[&str] = &["idle", "running", "stopping"];

/// Every run status value.
pub const RUN_STATUSES: &[&str] = &[
    "queued",
    "running",
    "succeeded",
    "failed",
    "timed_out",
    "cancelled",
    "interrupted",
];

/// Every approval status value.
pub const APPROVAL_STATUSES: &[&str] = &["pending", "approved", "denied", "cancelled", "expired"];

/// Every reason a pending approval is withdrawn.
pub const APPROVAL_CANCEL_CAUSES: &[&str] = &[
    "stop_requested",
    "run_cancelled",
    "session_trashing",
    "account_paused",
    "continuity_lost",
    "tool_call_cancelled",
    "binding_drift",
];

/// Every effect state value.
pub const EFFECT_STATES: &[&str] = &["prepared", "dispatched", "settled", "unknown"];

/// Every native-event outbox state.
pub const OUTBOX_STATES: &[&str] = &["pending", "delivered"];

/// Every `itemType` this table may hold, as declared in
/// `migrations/regional/tables/session-authority.json`.
pub const ITEM_TYPES: &[&str] = &[
    "session_head",
    "message",
    "run",
    "session_event",
    "approval",
    "agent_index",
    "agent_control",
    "journal_entry",
    "agent_effect",
    "fanout_page",
    "join_shard",
    "budget_return",
    "spend_reservation",
    "operation",
    "idempotency_receipt",
    "regional_workspace_control",
];

#[cfg(test)]
mod tests {
    use aex_wire::ids::{AgentId, MessageId, PrefixedId, SessionId, Uuid7, WorkspaceId};

    use super::{
        BRAIN_PREFIX, agent_control, event, head, journal, journal_sort_key, message, receipt,
        workspace_index,
    };

    fn session() -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1_700_000_000_000, [1; 10]))
    }

    fn agent() -> AgentId {
        AgentId::from_uuid7(Uuid7::compose(1_700_000_000_001, [2; 10]))
    }

    #[test]
    fn a_session_key_never_enters_the_reserved_brain_space() {
        let session = session();
        for key in [
            head(session),
            message(session, MessageId::from_uuid7(Uuid7::compose(1, [3; 10]))),
            event(session, 4),
            agent_control(session, agent()),
        ] {
            assert!(
                !key.sk.starts_with(BRAIN_PREFIX),
                "`{}` collides with the reserved Brain prefix",
                key.sk
            );
        }
    }

    #[test]
    fn a_journal_sort_key_orders_lexicographically_as_it_orders_numerically() {
        assert!(journal_sort_key(9) < journal_sort_key(10));
        assert!(journal_sort_key(u64::MAX - 1) < journal_sort_key(u64::MAX));
        let key = journal(session(), agent(), 42);
        assert_eq!(key.sk, "J#00000000000000000042");
    }

    #[test]
    fn an_event_key_pads_to_the_pinned_width() {
        assert_eq!(event(session(), 1).sk, "EVT#00000000000000000001");
    }

    #[test]
    fn a_receipt_key_refuses_a_scope_carrying_the_separator() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [4; 10]));
        assert!(receipt(workspace, "session.message:ses#evil", &"0".repeat(64)).is_err());
        assert!(receipt(workspace, "session.create", &"0".repeat(64)).is_ok());
    }

    #[test]
    fn the_index_partition_carries_the_lifecycle_so_list_needs_no_filter() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [5; 10]));
        let active = workspace_index::session_partition(workspace, "active");
        let trashed = workspace_index::session_partition(workspace, "trashed");
        assert_ne!(active, trashed);
        assert!(active.ends_with("#SESSION#active"));
    }
}
