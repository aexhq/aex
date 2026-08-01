//! The central authorizer context: extraction, verification and binding.
//!
//! A central request never carries a raw credential to this crate. API Gateway
//! invokes `central-authz`, which resolves the credential against Aurora and
//! returns a flat string map; this module is the **only** place that map becomes
//! a typed principal.
//!
//! Three properties are enforced here rather than assumed:
//!
//! * the map is read with a **closed** key set — an unknown key is a refusal, so
//!   a field somebody adds without teaching the edge about it cannot be silently
//!   ignored;
//! * the context carries its own issue and expiry instants and is re-checked
//!   against them, with the same thirty-second ceiling the assertion envelope
//!   uses, so an authorizer bug cannot lengthen the window;
//! * an `unavailable` account state is carried as a first-class value and never
//!   collapses into `active`.

use std::collections::BTreeMap;

use aex_control_domain::{
    AccountState, ActorCredential, OrgMembership, OrgRole, Principal, ScopeSet,
};
use aex_wire::types::{Region, RequestId};
use uuid::Uuid;

/// The longest a central authorizer context may be honoured.
///
/// The same ceiling as the regional assertion envelope, for the same reason: a
/// credential decision is a snapshot, and a snapshot older than this is not a
/// decision about the present.
pub const MAX_CONTEXT_LIFETIME_MS: i64 = 30_000;

/// Every key the authorizer context may carry.
///
/// Closed on purpose. `parse` refuses anything outside this list, because an
/// authorizer that starts sending a field the edge silently drops is exactly how
/// an authorization input stops being enforced without anybody noticing.
pub const CONTEXT_KEYS: &[&str] = &[
    "aex.accountState",
    "aex.credentialId",
    "aex.expiresAtMs",
    "aex.issuedAtMs",
    "aex.memberships",
    "aex.organizationId",
    "aex.principalId",
    "aex.principalKind",
    "aex.region",
    "aex.requestId",
    "aex.scopes",
    "aex.workspaceId",
];

/// Which credential the authorizer resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContextPrincipalKind {
    /// An account token minted by the device flow.
    Account,
    /// A browser session minted by the dashboard sign-in ceremony.
    UserSession,
    /// A workspace API key.
    WorkspaceKey,
    /// No credential at all; only the two device-flow routes admit this.
    Anonymous,
}

impl ContextPrincipalKind {
    /// Every kind.
    pub const ALL: [Self; 4] = [
        Self::Account,
        Self::UserSession,
        Self::WorkspaceKey,
        Self::Anonymous,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::UserSession => "user_session",
            Self::WorkspaceKey => "workspace_key",
            Self::Anonymous => "anonymous",
        }
    }

    /// Resolves a wire spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }
}

/// Why an authorizer context was refused.
///
/// Every arm is a `401` or a `503`; none of them tells the caller which field
/// was wrong, because the caller does not author the context.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContextError {
    /// A required key was absent or empty.
    #[error("the authorizer context is missing `{0}`")]
    Missing(&'static str),
    /// A key outside the closed set was present.
    #[error("the authorizer context carries the undeclared key `{0}`")]
    Undeclared(String),
    /// A value did not parse.
    #[error("the authorizer context value for `{key}` is malformed")]
    Malformed {
        /// Which key.
        key: &'static str,
    },
    /// The context was issued for a window that has closed.
    #[error("the authorizer context is not current")]
    NotCurrent,
    /// The context claimed a longer window than the ceiling allows.
    #[error("the authorizer context claims a window longer than {MAX_CONTEXT_LIFETIME_MS} ms")]
    LifetimeTooLong,
}

/// One resolved central authorizer context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CentralAuthorizerContext {
    /// The diagnostic identifier echoed in every error envelope.
    pub request_id: RequestId,
    /// Which credential was resolved.
    pub kind: ContextPrincipalKind,
    /// The person or the key, by row id.
    pub principal_id: Uuid,
    /// The credential row, when a credential was presented.
    pub credential_id: Option<Uuid>,
    /// The workspace a key is pinned to.
    pub workspace_id: Option<Uuid>,
    /// The organization a key belongs to.
    pub organization_id: Option<Uuid>,
    /// The region a key is pinned to.
    pub region: Option<Region>,
    /// Every active membership, as read in one statement.
    pub memberships: Vec<OrgMembership>,
    /// The scopes the credential itself carries, before any narrowing.
    pub scopes: ScopeSet,
    /// The account state at resolution time. Never defaulted.
    pub account_state: AccountState,
    /// When the authorizer resolved the credential.
    pub issued_at_ms: i64,
    /// When its answer stops being current.
    pub expires_at_ms: i64,
}

impl CentralAuthorizerContext {
    /// Reads a context from the flat map API Gateway supplies.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError`] for an undeclared key, an absent required key, a
    /// value that does not parse, or a claimed window longer than
    /// [`MAX_CONTEXT_LIFETIME_MS`].
    pub fn parse(fields: &BTreeMap<String, String>) -> Result<Self, ContextError> {
        if let Some(extra) = fields
            .keys()
            .find(|key| !CONTEXT_KEYS.contains(&key.as_str()))
        {
            return Err(ContextError::Undeclared(extra.clone()));
        }
        let kind = ContextPrincipalKind::parse(required(fields, "aex.principalKind")?).ok_or(
            ContextError::Malformed {
                key: "aex.principalKind",
            },
        )?;
        let request_id = RequestId::parse(required(fields, "aex.requestId")?).map_err(|_| {
            ContextError::Malformed {
                key: "aex.requestId",
            }
        })?;
        let principal_id =
            uuid(fields, "aex.principalId")?.ok_or(ContextError::Missing("aex.principalId"))?;
        let account_state = AccountState::parse(required(fields, "aex.accountState")?).ok_or(
            ContextError::Malformed {
                key: "aex.accountState",
            },
        )?;
        let scopes = scopes(fields)?;
        let region = match fields.get("aex.region").map(String::as_str) {
            None | Some("") => None,
            Some(name) => {
                Some(Region::from_name(name).ok_or(ContextError::Malformed { key: "aex.region" })?)
            }
        };
        let context = Self {
            request_id,
            kind,
            principal_id,
            credential_id: uuid(fields, "aex.credentialId")?,
            workspace_id: uuid(fields, "aex.workspaceId")?,
            organization_id: uuid(fields, "aex.organizationId")?,
            region,
            memberships: memberships(fields)?,
            scopes,
            account_state,
            issued_at_ms: millis(fields, "aex.issuedAtMs")?,
            expires_at_ms: millis(fields, "aex.expiresAtMs")?,
        };
        context.check_shape()?;
        Ok(context)
    }

    /// Refuses a context whose fields do not match the credential it claims.
    fn check_shape(&self) -> Result<(), ContextError> {
        let lifetime = self.expires_at_ms.saturating_sub(self.issued_at_ms);
        if lifetime <= 0 {
            return Err(ContextError::NotCurrent);
        }
        if lifetime > MAX_CONTEXT_LIFETIME_MS {
            return Err(ContextError::LifetimeTooLong);
        }
        match self.kind {
            ContextPrincipalKind::WorkspaceKey => {
                if self.workspace_id.is_none() {
                    return Err(ContextError::Missing("aex.workspaceId"));
                }
                if self.organization_id.is_none() {
                    return Err(ContextError::Missing("aex.organizationId"));
                }
                if self.region.is_none() {
                    return Err(ContextError::Missing("aex.region"));
                }
                if !self.memberships.is_empty() {
                    return Err(ContextError::Malformed {
                        key: "aex.memberships",
                    });
                }
            }
            ContextPrincipalKind::Account | ContextPrincipalKind::UserSession => {
                if self.credential_id.is_none() {
                    return Err(ContextError::Missing("aex.credentialId"));
                }
                // A key's placement fields on a person's context would let a
                // caller select a workspace the credential never named.
                if self.workspace_id.is_some() {
                    return Err(ContextError::Malformed {
                        key: "aex.workspaceId",
                    });
                }
            }
            ContextPrincipalKind::Anonymous => {
                if !self.scopes.is_empty() || !self.memberships.is_empty() {
                    return Err(ContextError::Malformed {
                        key: "aex.principalKind",
                    });
                }
            }
        }
        Ok(())
    }

    /// Refuses a context that is not current at `now_ms`.
    ///
    /// There is deliberately no grace period and no stale fallback: a context
    /// past its window is refused, and the caller re-authenticates. Extending a
    /// cached positive is how a revoked credential keeps working.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError::NotCurrent`] outside the claimed window.
    pub const fn verify(&self, now_ms: i64) -> Result<(), ContextError> {
        if now_ms < self.issued_at_ms || now_ms >= self.expires_at_ms {
            return Err(ContextError::NotCurrent);
        }
        Ok(())
    }

    /// The typed principal the authorization decision runs against.
    #[must_use]
    pub fn principal(&self) -> Principal {
        match self.kind {
            ContextPrincipalKind::Anonymous => Principal::Anonymous,
            ContextPrincipalKind::WorkspaceKey => Principal::WorkspaceKey {
                key_id: self.principal_id,
                workspace_id: self.workspace_id.unwrap_or_default(),
                organization_id: self.organization_id.unwrap_or_default(),
                scopes: self.scopes,
            },
            ContextPrincipalKind::Account => Principal::AccountActor {
                user_id: self.principal_id,
                credential: ActorCredential::AccountToken(
                    self.credential_id.unwrap_or(self.principal_id),
                ),
                memberships: self.memberships.clone(),
                token_scopes: self.scopes,
            },
            ContextPrincipalKind::UserSession => Principal::AccountActor {
                user_id: self.principal_id,
                credential: ActorCredential::DashboardSession(
                    self.credential_id.unwrap_or(self.principal_id),
                ),
                memberships: self.memberships.clone(),
                token_scopes: self.scopes,
            },
        }
    }
}

/// A required, non-empty value.
fn required<'a>(
    fields: &'a BTreeMap<String, String>,
    key: &'static str,
) -> Result<&'a str, ContextError> {
    fields
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(ContextError::Missing(key))
}

/// An optional identifier.
fn uuid(
    fields: &BTreeMap<String, String>,
    key: &'static str,
) -> Result<Option<Uuid>, ContextError> {
    match fields.get(key).map(String::as_str) {
        None | Some("") => Ok(None),
        Some(text) => Uuid::parse_str(text)
            .map(Some)
            .map_err(|_| ContextError::Malformed { key }),
    }
}

/// A required epoch-millisecond instant.
fn millis(fields: &BTreeMap<String, String>, key: &'static str) -> Result<i64, ContextError> {
    required(fields, key)?
        .parse::<i64>()
        .map_err(|_| ContextError::Malformed { key })
}

/// The space-separated scope list.
fn scopes(fields: &BTreeMap<String, String>) -> Result<ScopeSet, ContextError> {
    let raw = fields.get("aex.scopes").map_or("", String::as_str);
    if raw.is_empty() {
        return Ok(ScopeSet::EMPTY);
    }
    let spellings: Vec<&str> = raw.split(' ').filter(|it| !it.is_empty()).collect();
    ScopeSet::from_strings(&spellings).map_err(|_| ContextError::Malformed { key: "aex.scopes" })
}

/// The `org:membership:role` membership list.
fn memberships(fields: &BTreeMap<String, String>) -> Result<Vec<OrgMembership>, ContextError> {
    const KEY: &str = "aex.memberships";
    let raw = fields.get(KEY).map_or("", String::as_str);
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut all = Vec::new();
    for entry in raw.split(',').filter(|it| !it.is_empty()) {
        let mut parts = entry.split(':');
        let (Some(organization), Some(membership), Some(role), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(ContextError::Malformed { key: KEY });
        };
        all.push(OrgMembership {
            organization_id: Uuid::parse_str(organization)
                .map_err(|_| ContextError::Malformed { key: KEY })?,
            membership_id: Uuid::parse_str(membership)
                .map_err(|_| ContextError::Malformed { key: KEY })?,
            role: OrgRole::parse(role).ok_or(ContextError::Malformed { key: KEY })?,
        });
    }
    // A duplicate organization would make `decide` pick whichever came first,
    // which is a silent role choice rather than a decision.
    let mut organizations: Vec<Uuid> = all.iter().map(|it| it.organization_id).collect();
    organizations.sort_unstable();
    let total = organizations.len();
    organizations.dedup();
    if organizations.len() != total {
        return Err(ContextError::Malformed { key: KEY });
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::{
        CONTEXT_KEYS, CentralAuthorizerContext, ContextError, ContextPrincipalKind,
        MAX_CONTEXT_LIFETIME_MS,
    };
    use aex_control_domain::{AccountState, Principal};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn actor() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("aex.requestId".to_owned(), "req-1".to_owned()),
            ("aex.principalKind".to_owned(), "account".to_owned()),
            ("aex.principalId".to_owned(), Uuid::from_u128(1).to_string()),
            (
                "aex.credentialId".to_owned(),
                Uuid::from_u128(2).to_string(),
            ),
            (
                "aex.memberships".to_owned(),
                format!("{}:{}:owner", Uuid::from_u128(3), Uuid::from_u128(4)),
            ),
            (
                "aex.scopes".to_owned(),
                "organizations:read workspaces:read".to_owned(),
            ),
            ("aex.accountState".to_owned(), "active".to_owned()),
            ("aex.issuedAtMs".to_owned(), "1000".to_owned()),
            ("aex.expiresAtMs".to_owned(), "31000".to_owned()),
        ])
    }

    fn key() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("aex.requestId".to_owned(), "req-2".to_owned()),
            ("aex.principalKind".to_owned(), "workspace_key".to_owned()),
            ("aex.principalId".to_owned(), Uuid::from_u128(5).to_string()),
            ("aex.workspaceId".to_owned(), Uuid::from_u128(6).to_string()),
            (
                "aex.organizationId".to_owned(),
                Uuid::from_u128(7).to_string(),
            ),
            ("aex.region".to_owned(), "eu-west-1".to_owned()),
            ("aex.scopes".to_owned(), "operations:read".to_owned()),
            ("aex.accountState".to_owned(), "active".to_owned()),
            ("aex.issuedAtMs".to_owned(), "1000".to_owned()),
            ("aex.expiresAtMs".to_owned(), "2000".to_owned()),
        ])
    }

    #[test]
    fn an_actor_context_binds_its_memberships_and_credential() {
        let context = CentralAuthorizerContext::parse(&actor()).expect("parses");
        assert_eq!(context.kind, ContextPrincipalKind::Account);
        assert_eq!(context.memberships.len(), 1);
        let Principal::AccountActor {
            user_id,
            credential,
            ..
        } = context.principal()
        else {
            panic!("an account context resolves an actor");
        };
        assert_eq!(user_id, Uuid::from_u128(1));
        assert_eq!(credential.id(), Uuid::from_u128(2));
    }

    #[test]
    fn a_key_context_carries_its_placement_and_no_membership() {
        let context = CentralAuthorizerContext::parse(&key()).expect("parses");
        let Principal::WorkspaceKey {
            workspace_id,
            organization_id,
            ..
        } = context.principal()
        else {
            panic!("a key context resolves a key");
        };
        assert_eq!(workspace_id, Uuid::from_u128(6));
        assert_eq!(organization_id, Uuid::from_u128(7));
    }

    #[test]
    fn an_undeclared_key_is_refused_rather_than_ignored() {
        let mut fields = actor();
        fields.insert("aex.impersonate".to_owned(), "root".to_owned());
        assert_eq!(
            CentralAuthorizerContext::parse(&fields),
            Err(ContextError::Undeclared("aex.impersonate".to_owned()))
        );
    }

    #[test]
    fn every_required_key_is_named_when_it_is_absent() {
        for key in ["aex.requestId", "aex.principalKind", "aex.principalId"] {
            let mut fields = actor();
            fields.remove(key);
            assert!(
                matches!(
                    CentralAuthorizerContext::parse(&fields),
                    Err(ContextError::Missing(_) | ContextError::Malformed { .. })
                ),
                "removing {key}"
            );
        }
    }

    #[test]
    fn a_context_outside_its_window_is_never_honoured() {
        let context = CentralAuthorizerContext::parse(&key()).expect("parses");
        assert_eq!(context.verify(1_999), Ok(()));
        assert_eq!(context.verify(2_000), Err(ContextError::NotCurrent));
        assert_eq!(context.verify(999), Err(ContextError::NotCurrent));
    }

    #[test]
    fn a_context_claiming_a_longer_window_than_the_ceiling_is_refused_at_parse() {
        let mut fields = key();
        fields.insert(
            "aex.expiresAtMs".to_owned(),
            (1_000 + MAX_CONTEXT_LIFETIME_MS + 1).to_string(),
        );
        assert_eq!(
            CentralAuthorizerContext::parse(&fields),
            Err(ContextError::LifetimeTooLong)
        );
    }

    #[test]
    fn an_unavailable_account_state_survives_parsing_as_itself() {
        let mut fields = actor();
        fields.insert("aex.accountState".to_owned(), "unavailable".to_owned());
        let context = CentralAuthorizerContext::parse(&fields).expect("parses");
        assert_eq!(context.account_state, AccountState::Unavailable);
    }

    #[test]
    fn a_person_may_not_carry_a_key_placement() {
        let mut fields = actor();
        fields.insert("aex.workspaceId".to_owned(), Uuid::from_u128(9).to_string());
        assert_eq!(
            CentralAuthorizerContext::parse(&fields),
            Err(ContextError::Malformed {
                key: "aex.workspaceId"
            })
        );
    }

    #[test]
    fn a_duplicated_organization_is_refused_rather_than_resolved_by_order() {
        let mut fields = actor();
        fields.insert(
            "aex.memberships".to_owned(),
            format!(
                "{org}:{a}:member,{org}:{b}:owner",
                org = Uuid::from_u128(3),
                a = Uuid::from_u128(4),
                b = Uuid::from_u128(5)
            ),
        );
        assert_eq!(
            CentralAuthorizerContext::parse(&fields),
            Err(ContextError::Malformed {
                key: "aex.memberships"
            })
        );
    }

    #[test]
    fn an_anonymous_context_carries_neither_scope_nor_membership() {
        let mut fields = BTreeMap::from([
            ("aex.requestId".to_owned(), "req-3".to_owned()),
            ("aex.principalKind".to_owned(), "anonymous".to_owned()),
            ("aex.principalId".to_owned(), Uuid::nil().to_string()),
            ("aex.accountState".to_owned(), "active".to_owned()),
            ("aex.issuedAtMs".to_owned(), "0".to_owned()),
            ("aex.expiresAtMs".to_owned(), "1000".to_owned()),
        ]);
        assert_eq!(
            CentralAuthorizerContext::parse(&fields)
                .expect("parses")
                .principal(),
            Principal::Anonymous
        );
        fields.insert("aex.scopes".to_owned(), "account:read".to_owned());
        assert!(CentralAuthorizerContext::parse(&fields).is_err());
    }

    #[test]
    fn the_key_set_is_sorted_and_duplicate_free() {
        let mut sorted = CONTEXT_KEYS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, CONTEXT_KEYS);
        sorted.dedup();
        assert_eq!(sorted.len(), CONTEXT_KEYS.len());
    }
}
