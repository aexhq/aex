//! Audit events.
//!
//! Append-only, and written in the same transaction as the mutation it records.
//! No application role holds `UPDATE` or `DELETE`, so an audit row cannot be
//! quietly corrected. The `detail` document is redacted by construction: it is
//! built from a closed key set, never from a request body.

use time::OffsetDateTime;
use uuid::Uuid;

/// What kind of actor caused the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActorKind {
    /// A person, through either credential.
    User,
    /// A workspace API key.
    WorkspaceKey,
    /// A worker or scheduled job.
    System,
}

impl ActorKind {
    /// Every kind.
    pub const ALL: [Self; 3] = [Self::User, Self::WorkspaceKey, Self::System];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::WorkspaceKey => "workspace_key",
            Self::System => "system",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }
}

/// What kind of resource the event was about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceKind {
    /// An organization.
    Organization,
    /// A membership.
    Membership,
    /// An invitation.
    Invitation,
    /// A workspace.
    Workspace,
    /// A workspace API key.
    ApiKey,
    /// A durable operation.
    Operation,
    /// A person.
    User,
    /// An assertion signing key.
    SigningKey,
}

impl ResourceKind {
    /// Every kind.
    pub const ALL: [Self; 8] = [
        Self::Organization,
        Self::Membership,
        Self::Invitation,
        Self::Workspace,
        Self::ApiKey,
        Self::Operation,
        Self::User,
        Self::SigningKey,
    ];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Organization => "organization",
            Self::Membership => "membership",
            Self::Invitation => "invitation",
            Self::Workspace => "workspace",
            Self::ApiKey => "api_key",
            Self::Operation => "operation",
            Self::User => "user",
            Self::SigningKey => "signing_key",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }
}

/// Whether the action was allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AuditOutcome {
    /// The action ran.
    Allowed,
    /// The action was refused.
    Denied,
}

impl AuditOutcome {
    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Denied => "denied",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "allowed" => Some(Self::Allowed),
            "denied" => Some(Self::Denied),
            _ => None,
        }
    }
}

/// One append-only audit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    /// The row's own id.
    pub id: Uuid,
    /// Which organization, when the action had one.
    pub organization_id: Option<Uuid>,
    /// Which workspace, when the action had one.
    pub workspace_id: Option<Uuid>,
    /// What kind of actor.
    pub actor_kind: ActorKind,
    /// Which actor.
    pub actor_id: Option<Uuid>,
    /// The action, as a stable `<resource>.<verb>` token.
    pub action: String,
    /// What kind of resource.
    pub resource_kind: ResourceKind,
    /// Which resource.
    pub resource_id: Option<Uuid>,
    /// Whether it ran.
    pub outcome: AuditOutcome,
    /// The request that caused it.
    pub request_id: String,
    /// The durable operation it belongs to, when there is one.
    pub operation_id: Option<Uuid>,
    /// A redacted detail document, built from a closed key set.
    pub detail: serde_json::Value,
    /// When it happened.
    pub occurred_at: OffsetDateTime,
}

/// The keys an audit `detail` document may carry.
///
/// Closed by design: a detail built from a request body would eventually carry
/// a secret, and an append-only table is the worst place to discover that.
pub const DETAIL_KEYS: &[&str] = &[
    "denial",
    "fence",
    "from_status",
    "intent_hash",
    "kind",
    "name",
    "region",
    "role",
    "scopes",
    "slug",
    "to_status",
];

/// Whether every member of `detail` is a permitted key.
#[must_use]
pub fn detail_is_permitted(detail: &serde_json::Value) -> bool {
    match detail {
        serde_json::Value::Object(members) => members
            .keys()
            .all(|key| DETAIL_KEYS.contains(&key.as_str())),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{ActorKind, AuditOutcome, DETAIL_KEYS, ResourceKind, detail_is_permitted};

    #[test]
    fn the_detail_key_set_is_closed_and_sorted() {
        let mut sorted = DETAIL_KEYS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, DETAIL_KEYS, "keys are listed in sorted order");
        sorted.dedup();
        assert_eq!(sorted.len(), DETAIL_KEYS.len(), "no key is listed twice");
    }

    #[test]
    fn a_detail_naming_an_unlisted_key_is_refused() {
        assert!(detail_is_permitted(&serde_json::json!({ "role": "admin" })));
        assert!(detail_is_permitted(&serde_json::json!({})));
        assert!(!detail_is_permitted(
            &serde_json::json!({ "token": "aex_wk_x" })
        ));
        assert!(!detail_is_permitted(
            &serde_json::json!({ "email": "a@b.test" })
        ));
        assert!(!detail_is_permitted(&serde_json::json!("a string")));
    }

    #[test]
    fn the_enum_spellings_round_trip() {
        for kind in ActorKind::ALL {
            assert_eq!(ActorKind::parse(kind.as_str()), Some(kind));
        }
        for kind in ResourceKind::ALL {
            assert_eq!(ResourceKind::parse(kind.as_str()), Some(kind));
        }
        for outcome in [AuditOutcome::Allowed, AuditOutcome::Denied] {
            assert_eq!(AuditOutcome::parse(outcome.as_str()), Some(outcome));
        }
        assert_eq!(ActorKind::parse("robot"), None);
        assert_eq!(ResourceKind::parse("session"), None);
        assert_eq!(AuditOutcome::parse("maybe"), None);
    }
}
