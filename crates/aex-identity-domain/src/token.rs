//! Account tokens.
//!
//! `Active → {Revoked, Expired}`, TTL 90 days, minted only by the device flow.
//!
//! The format is `aex_at_<26>_<43>` — one codec for every credential, 256 bits,
//! key-addressed. The system this replaces used `aexu_<48 hex>_<32 hex tag>`:
//! 192 bits, with the tag acting as a pre-database filter over a plaintext HMAC.
//! An id-addressed lookup is a primary-key miss for a forged id, which is
//! cheaper than the filter it replaces and does not need the plaintext.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use aex_control_domain::scope::ScopeSet;

use crate::credential::PepperVersion;

/// How long an account token lives.
pub const ACCOUNT_TOKEN_TTL: Duration = Duration::days(90);

/// How a token came to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenOrigin {
    /// The RFC 8628 device flow. The only origin at launch.
    DeviceFlow,
}

impl TokenOrigin {
    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeviceFlow => "device_flow",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        (text == "device_flow").then_some(Self::DeviceFlow)
    }
}

/// An account token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountToken {
    /// The token's own id, which is the credential lookup key.
    pub id: Uuid,
    /// Whose token.
    pub user_id: Uuid,
    /// Display name, shown in the token list.
    pub name: String,
    /// The scopes it carries.
    pub scopes: ScopeSet,
    /// How it came to exist.
    pub origin: TokenOrigin,
    /// Which pepper version the verifier was computed under.
    pub pepper_version: PepperVersion,
    /// When it was minted.
    pub issued_at: OffsetDateTime,
    /// When it lapses.
    pub expires_at: OffsetDateTime,
    /// When it was revoked.
    pub revoked_at: Option<OffsetDateTime>,
}

/// Why a token transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenTransition {
    /// The token is already revoked.
    #[error("the token is already revoked")]
    AlreadyRevoked,
    /// The token already lapsed.
    #[error("the token already expired")]
    AlreadyExpired,
}

impl AccountToken {
    /// The longest accepted display name.
    pub const MAX_NAME_LEN: usize = 128;

    /// Whether the token is usable at `now`.
    #[must_use]
    pub fn is_live_at(&self, now: OffsetDateTime) -> bool {
        self.revoked_at.is_none_or(|at| at > now) && self.expires_at > now
    }

    /// Revokes the token.
    ///
    /// # Errors
    ///
    /// Returns [`TokenTransition::AlreadyRevoked`] for a repeat and
    /// [`TokenTransition::AlreadyExpired`] for a lapsed token, so a caller can
    /// tell whether this revocation is the one that should advance the `user`
    /// epoch.
    pub fn revoke(&self, at: OffsetDateTime) -> Result<Self, TokenTransition> {
        if self.revoked_at.is_some() {
            return Err(TokenTransition::AlreadyRevoked);
        }
        if self.expires_at <= at {
            return Err(TokenTransition::AlreadyExpired);
        }
        Ok(Self {
            revoked_at: Some(at),
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ACCOUNT_TOKEN_TTL, AccountToken, TokenOrigin, TokenTransition};
    use crate::credential::PepperVersion;
    use aex_control_domain::scope::ScopeSet;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    fn token() -> AccountToken {
        AccountToken {
            id: Uuid::from_u128(1),
            user_id: Uuid::from_u128(2),
            name: "cli".to_owned(),
            scopes: ScopeSet::MEMBER,
            origin: TokenOrigin::DeviceFlow,
            pepper_version: PepperVersion::new(1),
            issued_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH + ACCOUNT_TOKEN_TTL,
            revoked_at: None,
        }
    }

    #[test]
    fn the_ttl_is_ninety_days_and_liveness_is_exact_at_the_boundary() {
        assert_eq!(ACCOUNT_TOKEN_TTL, Duration::days(90));
        let token = token();
        assert!(token.is_live_at(token.expires_at - Duration::milliseconds(1)));
        assert!(!token.is_live_at(token.expires_at));
    }

    #[test]
    fn revocation_happens_once_and_is_distinguishable_from_expiry() {
        let at = OffsetDateTime::UNIX_EPOCH + Duration::days(1);
        let revoked = token().revoke(at).expect("revokes");
        assert_eq!(revoked.revoked_at, Some(at));
        assert!(!revoked.is_live_at(at));
        assert_eq!(revoked.revoke(at), Err(TokenTransition::AlreadyRevoked));

        let lapsed = token();
        assert_eq!(
            lapsed.revoke(lapsed.expires_at),
            Err(TokenTransition::AlreadyExpired)
        );
    }

    #[test]
    fn the_only_origin_is_the_device_flow() {
        assert_eq!(
            TokenOrigin::parse("device_flow"),
            Some(TokenOrigin::DeviceFlow)
        );
        assert_eq!(TokenOrigin::parse("dashboard"), None);
        assert_eq!(TokenOrigin::parse("password"), None);
    }
}
