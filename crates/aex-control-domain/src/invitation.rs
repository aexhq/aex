//! Invitations.
//!
//! There is **no invitation secret**. The system this replaces minted a token,
//! hashed it, stored the hash and discarded the plaintext — with no route
//! anywhere able to redeem it — and carried two incompatible invite formats in
//! one `token_hash` column.
//!
//! Acceptance is a verified-email match instead: the invitation names an email,
//! the signed-in person's email must equal it, and that person's email must be
//! verified. The email itself is a notification, not a credential, so losing it
//! costs nothing and intercepting it grants nothing.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::authz::OrgRole;

/// How long an invitation stays pending.
pub const INVITATION_TTL: Duration = Duration::days(14);

/// How many invitations one acceptance may redeem.
///
/// Acceptance names no invitation — it redeems everything pending for one
/// verified address — so the transaction needs a ceiling, or one request could
/// be made unboundedly large by inviting a single address from arbitrarily many
/// organizations. The selection statement carries the same `LIMIT`, the caller
/// preassigns this many membership ids, and the published response bounds its
/// array here too. Anything beyond it is redeemed by asking again.
pub const MAX_ACCEPTABLE_INVITATIONS: usize = 100;

/// The lifecycle of an invitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InvitationStatus {
    /// Waiting for the named email to sign in.
    Pending,
    /// Redeemed into a membership.
    Accepted,
    /// Withdrawn by an admin.
    Revoked,
    /// Passed its expiry without being accepted.
    Expired,
}

impl InvitationStatus {
    /// Every status.
    pub const ALL: [Self; 4] = [Self::Pending, Self::Accepted, Self::Revoked, Self::Expired];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }

    /// Whether the status can never change again.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

/// An invitation to join an organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation {
    /// Public id payload.
    pub id: Uuid,
    /// Which organization.
    pub organization_id: Uuid,
    /// The normalized email the invitation names.
    pub email: String,
    /// The role the invitation offers. Never `owner`.
    pub role: OrgRole,
    /// Lifecycle.
    pub status: InvitationStatus,
    /// Who invited.
    pub invited_by_user_id: Uuid,
    /// Who accepted, once accepted.
    pub accepted_user_id: Option<Uuid>,
    /// When it was created.
    pub created_at: OffsetDateTime,
    /// When it stops being redeemable.
    pub expires_at: OffsetDateTime,
    /// When it left `pending`.
    pub resolved_at: Option<OffsetDateTime>,
}

/// Why an invitation transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvitationTransition {
    /// The invitation already left `pending`.
    #[error("the invitation is already resolved")]
    AlreadyResolved,
    /// The invitation passed its expiry.
    #[error("the invitation expired")]
    Expired,
    /// The signed-in person's email is not the invited one.
    #[error("the invitation names a different email address")]
    EmailMismatch,
    /// The signed-in person has not verified their email.
    #[error("an unverified email address cannot accept an invitation")]
    EmailUnverified,
    /// An invitation may not offer ownership.
    #[error("an invitation may not offer the `owner` role")]
    OwnerNotInvitable,
}

impl Invitation {
    /// Whether the invitation is redeemable at `now`.
    #[must_use]
    pub fn is_pending_at(&self, now: OffsetDateTime) -> bool {
        self.status == InvitationStatus::Pending && self.expires_at > now
    }

    /// Checks the role an invitation may offer.
    ///
    /// # Errors
    ///
    /// Returns [`InvitationTransition::OwnerNotInvitable`] for `owner`.
    /// Ownership is transferred deliberately, never handed out by email.
    pub const fn check_offered_role(role: OrgRole) -> Result<(), InvitationTransition> {
        match role {
            OrgRole::Owner => Err(InvitationTransition::OwnerNotInvitable),
            OrgRole::Admin | OrgRole::Member => Ok(()),
        }
    }

    /// Accepts the invitation on behalf of a signed-in person.
    ///
    /// # Errors
    ///
    /// Returns [`InvitationTransition`] when the invitation is resolved, has
    /// expired, names a different email, or the person's email is unverified.
    pub fn accept(
        &self,
        user_id: Uuid,
        user_email: &str,
        email_verified: bool,
        now: OffsetDateTime,
    ) -> Result<Self, InvitationTransition> {
        if self.status.is_terminal() {
            return Err(InvitationTransition::AlreadyResolved);
        }
        if self.expires_at <= now {
            return Err(InvitationTransition::Expired);
        }
        if self.email != user_email {
            return Err(InvitationTransition::EmailMismatch);
        }
        if !email_verified {
            return Err(InvitationTransition::EmailUnverified);
        }
        Ok(Self {
            status: InvitationStatus::Accepted,
            accepted_user_id: Some(user_id),
            resolved_at: Some(now),
            ..self.clone()
        })
    }

    /// Withdraws the invitation.
    ///
    /// # Errors
    ///
    /// Returns [`InvitationTransition::AlreadyResolved`] for a resolved one.
    pub fn revoke(&self, now: OffsetDateTime) -> Result<Self, InvitationTransition> {
        if self.status.is_terminal() {
            return Err(InvitationTransition::AlreadyResolved);
        }
        Ok(Self {
            status: InvitationStatus::Revoked,
            resolved_at: Some(now),
            ..self.clone()
        })
    }

    /// Marks a lapsed invitation expired.
    ///
    /// # Errors
    ///
    /// Returns [`InvitationTransition::AlreadyResolved`] for a resolved one, and
    /// `Ok(self.clone())` is never produced for one that has not lapsed — the
    /// caller gets [`InvitationTransition::Expired`]'s inverse as a refusal.
    pub fn expire(&self, now: OffsetDateTime) -> Result<Self, InvitationTransition> {
        if self.status.is_terminal() {
            return Err(InvitationTransition::AlreadyResolved);
        }
        if self.expires_at > now {
            return Err(InvitationTransition::EmailMismatch);
        }
        Ok(Self {
            status: InvitationStatus::Expired,
            resolved_at: Some(now),
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Invitation, InvitationStatus, InvitationTransition};
    use crate::authz::OrgRole;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    fn invitation() -> Invitation {
        Invitation {
            id: Uuid::from_u128(1),
            organization_id: Uuid::from_u128(2),
            email: "a@b.test".to_owned(),
            role: OrgRole::Member,
            status: InvitationStatus::Pending,
            invited_by_user_id: Uuid::from_u128(3),
            accepted_user_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH + Duration::days(14),
            resolved_at: None,
        }
    }

    #[test]
    fn acceptance_requires_a_matching_verified_email() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::hours(1);
        let user = Uuid::from_u128(9);
        assert_eq!(
            invitation().accept(user, "other@b.test", true, now),
            Err(InvitationTransition::EmailMismatch)
        );
        assert_eq!(
            invitation().accept(user, "a@b.test", false, now),
            Err(InvitationTransition::EmailUnverified)
        );
        let accepted = invitation()
            .accept(user, "a@b.test", true, now)
            .expect("a matching verified email accepts");
        assert_eq!(accepted.status, InvitationStatus::Accepted);
        assert_eq!(accepted.accepted_user_id, Some(user));
        assert_eq!(accepted.resolved_at, Some(now));
    }

    #[test]
    fn an_invitation_can_only_be_accepted_once() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::hours(1);
        let accepted = invitation()
            .accept(Uuid::from_u128(9), "a@b.test", true, now)
            .expect("first acceptance");
        assert_eq!(
            accepted.accept(Uuid::from_u128(10), "a@b.test", true, now),
            Err(InvitationTransition::AlreadyResolved)
        );
    }

    #[test]
    fn an_expired_invitation_is_not_redeemable() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::days(15);
        assert_eq!(
            invitation().accept(Uuid::from_u128(9), "a@b.test", true, now),
            Err(InvitationTransition::Expired)
        );
        assert!(!invitation().is_pending_at(now));
    }

    #[test]
    fn ownership_is_never_invitable() {
        assert_eq!(
            Invitation::check_offered_role(OrgRole::Owner),
            Err(InvitationTransition::OwnerNotInvitable)
        );
        assert_eq!(Invitation::check_offered_role(OrgRole::Admin), Ok(()));
        assert_eq!(Invitation::check_offered_role(OrgRole::Member), Ok(()));
    }

    #[test]
    fn a_revoked_invitation_never_returns_to_pending() {
        let revoked = invitation()
            .revoke(OffsetDateTime::UNIX_EPOCH)
            .expect("revocation succeeds");
        assert!(revoked.status.is_terminal());
        assert_eq!(
            revoked.accept(
                Uuid::from_u128(9),
                "a@b.test",
                true,
                OffsetDateTime::UNIX_EPOCH
            ),
            Err(InvitationTransition::AlreadyResolved)
        );
    }

    #[test]
    fn the_status_spellings_round_trip() {
        for status in InvitationStatus::ALL {
            assert_eq!(InvitationStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(InvitationStatus::parse("sent"), None);
    }
}
