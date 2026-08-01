//! Memberships.
//!
//! One person's role inside one organization. The last-owner rule is enforced
//! twice: here, so the application gets a typed refusal, and by a
//! `DEFERRABLE INITIALLY DEFERRED` constraint trigger, so two transactions
//! racing to remove the last two owners cannot both win.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::Revision;
use crate::authz::OrgRole;

/// Whether the membership is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MembershipStatus {
    /// The person is a member.
    Active,
    /// The person was removed.
    Removed,
}

impl MembershipStatus {
    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Removed => "removed",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "active" => Some(Self::Active),
            "removed" => Some(Self::Removed),
            _ => None,
        }
    }
}

/// One person's membership of one organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    /// Public id payload.
    pub id: Uuid,
    /// Which organization.
    pub organization_id: Uuid,
    /// Which person.
    pub user_id: Uuid,
    /// The role held.
    pub role: OrgRole,
    /// Whether the membership is live.
    pub status: MembershipStatus,
    /// Optimistic-concurrency revision.
    pub revision: Revision,
    /// When it was created.
    pub created_at: OffsetDateTime,
    /// When it last changed.
    pub updated_at: OffsetDateTime,
}

/// Why a membership transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MembershipTransition {
    /// The membership is already removed.
    #[error("the membership is already removed")]
    AlreadyRemoved,
    /// Removing or demoting would leave the organization with no active owner.
    #[error("an organization always has at least one active owner")]
    LastOwnerRequired,
    /// The membership belongs to a different organization.
    #[error("the membership belongs to a different organization")]
    WrongOrganization,
}

impl Membership {
    /// Removes the membership.
    ///
    /// `other_active_owners` is how many *other* active owners the organization
    /// has; the caller reads it in the same transaction as the write.
    ///
    /// # Errors
    ///
    /// Returns [`MembershipTransition::AlreadyRemoved`] for a repeat, and
    /// [`MembershipTransition::LastOwnerRequired`] when this is the last owner.
    pub fn remove(
        &self,
        other_active_owners: usize,
        at: OffsetDateTime,
    ) -> Result<Self, MembershipTransition> {
        if self.status == MembershipStatus::Removed {
            return Err(MembershipTransition::AlreadyRemoved);
        }
        if self.role == OrgRole::Owner && other_active_owners == 0 {
            return Err(MembershipTransition::LastOwnerRequired);
        }
        Ok(Self {
            status: MembershipStatus::Removed,
            revision: self.revision.next(),
            updated_at: at,
            ..self.clone()
        })
    }

    /// Changes the role.
    ///
    /// # Errors
    ///
    /// Returns [`MembershipTransition::AlreadyRemoved`] for a removed
    /// membership, and [`MembershipTransition::LastOwnerRequired`] when the
    /// change would demote the last owner.
    pub fn set_role(
        &self,
        role: OrgRole,
        other_active_owners: usize,
        at: OffsetDateTime,
    ) -> Result<Self, MembershipTransition> {
        if self.status == MembershipStatus::Removed {
            return Err(MembershipTransition::AlreadyRemoved);
        }
        if self.role == OrgRole::Owner && role != OrgRole::Owner && other_active_owners == 0 {
            return Err(MembershipTransition::LastOwnerRequired);
        }
        Ok(Self {
            role,
            revision: self.revision.next(),
            updated_at: at,
            ..self.clone()
        })
    }

    /// Raises the role to `role`, never lowering it.
    ///
    /// Invitation acceptance uses this: accepting a `member` invitation must
    /// never demote somebody who is already an admin.
    #[must_use]
    pub fn raise_role_to(&self, role: OrgRole, at: OffsetDateTime) -> Self {
        if self.role.at_least(role) {
            return self.clone();
        }
        Self {
            role,
            revision: self.revision.next(),
            updated_at: at,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Membership, MembershipStatus, MembershipTransition};
    use crate::Revision;
    use crate::authz::OrgRole;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn membership(role: OrgRole) -> Membership {
        Membership {
            id: Uuid::from_u128(1),
            organization_id: Uuid::from_u128(2),
            user_id: Uuid::from_u128(3),
            role,
            status: MembershipStatus::Active,
            revision: Revision::INITIAL,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn the_last_owner_cannot_be_removed() {
        assert_eq!(
            membership(OrgRole::Owner).remove(0, OffsetDateTime::UNIX_EPOCH),
            Err(MembershipTransition::LastOwnerRequired)
        );
        assert!(
            membership(OrgRole::Owner)
                .remove(1, OffsetDateTime::UNIX_EPOCH)
                .is_ok()
        );
        assert!(
            membership(OrgRole::Admin)
                .remove(0, OffsetDateTime::UNIX_EPOCH)
                .is_ok()
        );
    }

    #[test]
    fn the_last_owner_cannot_be_demoted() {
        assert_eq!(
            membership(OrgRole::Owner).set_role(OrgRole::Admin, 0, OffsetDateTime::UNIX_EPOCH),
            Err(MembershipTransition::LastOwnerRequired)
        );
        assert!(
            membership(OrgRole::Owner)
                .set_role(OrgRole::Owner, 0, OffsetDateTime::UNIX_EPOCH)
                .is_ok(),
            "re-setting the same role is not a demotion"
        );
    }

    #[test]
    fn removal_is_not_idempotent_it_is_a_typed_refusal() {
        let removed = membership(OrgRole::Admin)
            .remove(1, OffsetDateTime::UNIX_EPOCH)
            .expect("the first removal succeeds");
        assert_eq!(
            removed.remove(1, OffsetDateTime::UNIX_EPOCH),
            Err(MembershipTransition::AlreadyRemoved)
        );
    }

    #[test]
    fn accepting_an_invitation_never_demotes() {
        let admin = membership(OrgRole::Admin);
        let after = admin.raise_role_to(OrgRole::Member, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(after.role, OrgRole::Admin);
        assert_eq!(after.revision, admin.revision, "no write, no revision bump");

        let member = membership(OrgRole::Member);
        let after = member.raise_role_to(OrgRole::Admin, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(after.role, OrgRole::Admin);
        assert_eq!(after.revision, member.revision.next());
    }

    #[test]
    fn the_status_spellings_round_trip() {
        for status in [MembershipStatus::Active, MembershipStatus::Removed] {
            assert_eq!(MembershipStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(MembershipStatus::parse("pending"), None);
    }
}
