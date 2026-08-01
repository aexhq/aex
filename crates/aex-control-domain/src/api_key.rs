//! Workspace API key metadata.
//!
//! The row holds a 32-byte keyed verifier and an explicit pepper version, never
//! a secret and never anything derived from one that could be reversed. The
//! secret itself is returned exactly once, by the create route, and is not
//! recoverable afterwards.
//!
//! There is no `last_used_at`. The system this replaces wrote one on the
//! authorization path as a best-effort `UPDATE`, which put a write on the
//! hottest read in the platform for a value nothing consumes.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::Revision;
use crate::scope::{ScopeError, ScopeSet};

/// One workspace API key, as the control plane stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    /// The key's own id. This is the lookup key: verification is a primary-key
    /// hit, so a forged id is an index miss rather than a scan.
    pub id: Uuid,
    /// The workspace it belongs to.
    pub workspace_id: Uuid,
    /// The organization it belongs to.
    pub organization_id: Uuid,
    /// Display name.
    pub name: String,
    /// The scopes it carries. Always inside [`ScopeSet::WORKSPACE_KEY_MINTABLE`].
    pub scopes: ScopeSet,
    /// The region it is pinned to, matching its workspace.
    pub region: aex_wire::types::Region,
    /// Which pepper version the verifier was computed under.
    pub pepper_version: u16,
    /// When it was minted.
    pub created_at: OffsetDateTime,
    /// When it was revoked.
    pub revoked_at: Option<OffsetDateTime>,
    /// Optimistic-concurrency revision.
    pub revision: Revision,
    /// Who minted it.
    pub created_by_user_id: Uuid,
}

/// Why an API key transition was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiKeyTransition {
    /// The key is already revoked.
    #[error("the key is already revoked")]
    AlreadyRevoked,
    /// The requested scopes exceed what a workspace key may carry.
    #[error("a workspace key may not carry {excess:?}")]
    ScopesExceedCeiling {
        /// The scopes above the ceiling, in registry order.
        excess: Vec<String>,
    },
    /// The requested scopes were empty.
    #[error("a workspace key must carry at least one scope")]
    NoScopes,
    /// The scope list itself did not parse.
    #[error(transparent)]
    Scope(#[from] ScopeError),
}

impl ApiKey {
    /// The longest accepted display name.
    pub const MAX_NAME_LEN: usize = 128;

    /// Narrows a requested scope set to what a workspace key may carry.
    ///
    /// # Errors
    ///
    /// Returns [`ApiKeyTransition::NoScopes`] for an empty request and
    /// [`ApiKeyTransition::ScopesExceedCeiling`] when the request names a scope
    /// above the ceiling. It is deliberately a refusal rather than a silent
    /// narrowing: a caller that asked for `api_keys:write` and got a key without
    /// it would only find out at the first `403`.
    pub fn admissible_scopes(requested: ScopeSet) -> Result<ScopeSet, ApiKeyTransition> {
        if requested.is_empty() {
            return Err(ApiKeyTransition::NoScopes);
        }
        let excess = requested.difference(ScopeSet::WORKSPACE_KEY_MINTABLE);
        if !excess.is_empty() {
            return Err(ApiKeyTransition::ScopesExceedCeiling {
                excess: excess.to_strings(),
            });
        }
        Ok(requested)
    }

    /// Whether the key is live at `now`.
    #[must_use]
    pub fn is_live_at(&self, now: OffsetDateTime) -> bool {
        self.revoked_at.is_none_or(|at| at > now)
    }

    /// Revokes the key.
    ///
    /// # Errors
    ///
    /// Returns [`ApiKeyTransition::AlreadyRevoked`] for a repeat, so the caller
    /// can tell a first revocation from a replay and skip the epoch bump.
    pub fn revoke(&self, at: OffsetDateTime) -> Result<Self, ApiKeyTransition> {
        if self.revoked_at.is_some() {
            return Err(ApiKeyTransition::AlreadyRevoked);
        }
        Ok(Self {
            revoked_at: Some(at),
            revision: self.revision.next(),
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiKey, ApiKeyTransition};
    use crate::Revision;
    use crate::scope::{Scope, ScopeSet};
    use aex_wire::types::Region;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    fn key() -> ApiKey {
        ApiKey {
            id: Uuid::from_u128(1),
            workspace_id: Uuid::from_u128(2),
            organization_id: Uuid::from_u128(3),
            name: "ci".to_owned(),
            scopes: ScopeSet::of(&[Scope::SessionsRead]),
            region: Region::EuWest1,
            pepper_version: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            revoked_at: None,
            revision: Revision::INITIAL,
            created_by_user_id: Uuid::from_u128(4),
        }
    }

    #[test]
    fn a_scope_above_the_ceiling_is_refused_not_narrowed() {
        let requested = ScopeSet::of(&[Scope::SessionsRead, Scope::ApiKeysWrite]);
        assert_eq!(
            ApiKey::admissible_scopes(requested),
            Err(ApiKeyTransition::ScopesExceedCeiling {
                excess: vec!["api_keys:write".to_owned()]
            })
        );
    }

    #[test]
    fn an_empty_scope_request_is_refused() {
        assert_eq!(
            ApiKey::admissible_scopes(ScopeSet::EMPTY),
            Err(ApiKeyTransition::NoScopes)
        );
    }

    #[test]
    fn the_whole_ceiling_is_admissible() {
        assert_eq!(
            ApiKey::admissible_scopes(ScopeSet::WORKSPACE_KEY_MINTABLE),
            Ok(ScopeSet::WORKSPACE_KEY_MINTABLE)
        );
    }

    #[test]
    fn revocation_is_a_one_way_transition_with_a_typed_replay() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let revoked = key().revoke(at).expect("the first revocation succeeds");
        assert_eq!(revoked.revoked_at, Some(at));
        assert_eq!(revoked.revision, key().revision.next());
        assert_eq!(
            revoked.revoke(at),
            Err(ApiKeyTransition::AlreadyRevoked),
            "a replay is distinguishable, so the epoch is bumped once"
        );
    }

    #[test]
    fn liveness_is_evaluated_against_the_supplied_instant() {
        let at = OffsetDateTime::UNIX_EPOCH + Duration::hours(1);
        let revoked = key().revoke(at).expect("revocation");
        assert!(revoked.is_live_at(at - Duration::minutes(1)));
        assert!(!revoked.is_live_at(at));
        assert!(!revoked.is_live_at(at + Duration::minutes(1)));
        assert!(key().is_live_at(at));
    }
}
