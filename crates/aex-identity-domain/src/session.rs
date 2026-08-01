//! Browser sessions.
//!
//! `Active → {Expired, Revoked}`. Expiry is **derived** from `expires_at`, never
//! written: a status column that has to be swept is a status column that is
//! wrong between sweeps. Extension can never push `expires_at` past
//! `issued_at + 30 d`, which the database also enforces with a `CHECK`.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::credential::PepperVersion;

/// How long a browser session lives.
pub const DASHBOARD_SESSION_TTL: Duration = Duration::days(30);

/// Where a session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionState {
    /// Usable.
    Active,
    /// Past its expiry.
    Expired,
    /// Explicitly revoked.
    Revoked,
}

/// A browser session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardSession {
    /// The session's own id, which is the credential lookup key.
    pub id: Uuid,
    /// Whose session.
    pub user_id: Uuid,
    /// Which pepper version the verifier was computed under.
    pub pepper_version: PepperVersion,
    /// When it was minted.
    pub issued_at: OffsetDateTime,
    /// When it stops being usable.
    pub expires_at: OffsetDateTime,
    /// When it was revoked.
    pub revoked_at: Option<OffsetDateTime>,
}

/// Why a session transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionTransition {
    /// The session is already revoked.
    #[error("the session is already revoked")]
    AlreadyRevoked,
    /// The session already lapsed.
    #[error("the session already expired")]
    AlreadyExpired,
    /// The extension would push expiry past the hard cap.
    #[error("a session may not live past its issue time plus 30 days")]
    BeyondCap,
}

impl DashboardSession {
    /// Where the session is at `now`.
    ///
    /// Revocation is a recorded fact and wins over expiry at every instant: a
    /// revoked session is revoked even when observed with a clock that has moved
    /// backwards, and even after it would also have lapsed. That is both the
    /// safe answer and the one an audit reader needs.
    #[must_use]
    pub fn state_at(&self, now: OffsetDateTime) -> SessionState {
        if self.revoked_at.is_some() {
            return SessionState::Revoked;
        }
        if self.expires_at <= now {
            return SessionState::Expired;
        }
        SessionState::Active
    }

    /// The hard cap on this session's expiry.
    #[must_use]
    pub fn expiry_cap(&self) -> OffsetDateTime {
        self.issued_at + DASHBOARD_SESSION_TTL
    }

    /// Extends the session, never past the cap.
    ///
    /// # Errors
    ///
    /// Returns [`SessionTransition`] for a revoked or lapsed session, and
    /// [`SessionTransition::BeyondCap`] when the new expiry would exceed
    /// `issued_at + 30 d`.
    pub fn extend(
        &self,
        expires_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> Result<Self, SessionTransition> {
        match self.state_at(now) {
            SessionState::Revoked => return Err(SessionTransition::AlreadyRevoked),
            SessionState::Expired => return Err(SessionTransition::AlreadyExpired),
            SessionState::Active => {}
        }
        if expires_at > self.expiry_cap() {
            return Err(SessionTransition::BeyondCap);
        }
        if expires_at <= self.expires_at {
            return Ok(self.clone());
        }
        Ok(Self {
            expires_at,
            ..self.clone()
        })
    }

    /// Revokes the session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionTransition::AlreadyRevoked`] for a repeat and
    /// [`SessionTransition::AlreadyExpired`] for a lapsed session, so a caller
    /// can tell which revocations are the ones that should advance an epoch.
    pub fn revoke(&self, at: OffsetDateTime) -> Result<Self, SessionTransition> {
        if self.revoked_at.is_some() {
            return Err(SessionTransition::AlreadyRevoked);
        }
        if self.expires_at <= at {
            return Err(SessionTransition::AlreadyExpired);
        }
        Ok(Self {
            revoked_at: Some(at),
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{DASHBOARD_SESSION_TTL, DashboardSession, SessionState, SessionTransition};
    use crate::credential::PepperVersion;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    fn session() -> DashboardSession {
        DashboardSession {
            id: Uuid::from_u128(1),
            user_id: Uuid::from_u128(2),
            pepper_version: PepperVersion::new(1),
            issued_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH + DASHBOARD_SESSION_TTL,
            revoked_at: None,
        }
    }

    #[test]
    fn expiry_is_derived_at_the_exact_boundary() {
        let session = session();
        let expiry = session.expires_at;
        assert_eq!(
            session.state_at(expiry - Duration::milliseconds(1)),
            SessionState::Active
        );
        assert_eq!(session.state_at(expiry), SessionState::Expired);
        assert_eq!(
            session.state_at(expiry + Duration::milliseconds(1)),
            SessionState::Expired
        );
    }

    #[test]
    fn revocation_wins_over_expiry() {
        let at = OffsetDateTime::UNIX_EPOCH + Duration::days(1);
        let revoked = session().revoke(at).expect("revokes");
        assert_eq!(revoked.state_at(at), SessionState::Revoked);
        assert_eq!(
            revoked.state_at(revoked.expires_at + Duration::days(1)),
            SessionState::Revoked
        );
        assert_eq!(
            revoked.state_at(at - Duration::hours(1)),
            SessionState::Revoked,
            "revocation is a fact, not a comparison against the reader's clock"
        );
    }

    #[test]
    fn revocation_is_a_typed_refusal_on_repeat_and_after_expiry() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let revoked = session().revoke(at).expect("revokes");
        assert_eq!(revoked.revoke(at), Err(SessionTransition::AlreadyRevoked));
        let session = session();
        assert_eq!(
            session.revoke(session.expires_at),
            Err(SessionTransition::AlreadyExpired)
        );
    }

    #[test]
    fn extension_is_capped_at_thirty_days_from_issue() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::days(1);
        let mut session = session();
        session.expires_at = OffsetDateTime::UNIX_EPOCH + Duration::days(10);

        let extended = session
            .extend(OffsetDateTime::UNIX_EPOCH + DASHBOARD_SESSION_TTL, now)
            .expect("extension up to the cap is allowed");
        assert_eq!(extended.expires_at, session.expiry_cap());

        assert_eq!(
            session.extend(
                OffsetDateTime::UNIX_EPOCH + DASHBOARD_SESSION_TTL + Duration::seconds(1),
                now
            ),
            Err(SessionTransition::BeyondCap)
        );
    }

    #[test]
    fn expiry_is_monotone_non_increasing_under_extension() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::days(1);
        let session = session();
        let shortened = session
            .extend(OffsetDateTime::UNIX_EPOCH + Duration::days(2), now)
            .expect("a shorter expiry is accepted as a no-op");
        assert_eq!(
            shortened.expires_at, session.expires_at,
            "extension never shortens a session"
        );
    }

    #[test]
    fn a_lapsed_or_revoked_session_cannot_be_extended() {
        let session = session();
        let after = session.expires_at + Duration::days(1);
        assert_eq!(
            session.extend(session.expiry_cap(), after),
            Err(SessionTransition::AlreadyExpired)
        );
        let revoked = session.revoke(OffsetDateTime::UNIX_EPOCH).expect("revokes");
        assert_eq!(
            revoked.extend(revoked.expiry_cap(), OffsetDateTime::UNIX_EPOCH),
            Err(SessionTransition::AlreadyRevoked)
        );
    }
}
