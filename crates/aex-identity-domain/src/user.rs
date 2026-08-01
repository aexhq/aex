//! People.
//!
//! `Active ⇄ Disabled`, and `email_verified_at` is set once and never cleared.
//! Disabling advances the `user` revocation epoch in the same transaction, so
//! "any successful authentication implies the user was active at read time"
//! holds even inside a live assertion's thirty-second window.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::email::NormalizedEmail;

/// Whether a person may authenticate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UserStatus {
    /// May authenticate.
    Active,
    /// May not.
    Disabled,
}

impl UserStatus {
    /// Every status.
    pub const ALL: [Self; 2] = [Self::Active, Self::Disabled];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }
}

/// A person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// Public id payload.
    pub id: Uuid,
    /// The normalized address, unique across all people.
    pub email: NormalizedEmail,
    /// When the address was proven.
    pub email_verified_at: Option<OffsetDateTime>,
    /// Display name, when a provider supplied one.
    pub name: Option<String>,
    /// Avatar URL, when a provider supplied one.
    pub image_url: Option<String>,
    /// Whether the person may authenticate.
    pub status: UserStatus,
    /// Optimistic-concurrency revision.
    pub revision: aex_control_domain::Revision,
    /// When the row was created.
    pub created_at: OffsetDateTime,
    /// When the row last changed.
    pub updated_at: OffsetDateTime,
}

/// Why a user transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UserTransition {
    /// The person is already disabled.
    #[error("the person is already disabled")]
    AlreadyDisabled,
    /// The person is already active.
    #[error("the person is already active")]
    AlreadyActive,
    /// The address is already verified.
    #[error("the address is already verified")]
    AlreadyVerified,
}

impl User {
    /// The longest accepted display name.
    pub const MAX_NAME_LEN: usize = 128;

    /// Whether the person may authenticate at all.
    #[must_use]
    pub const fn may_authenticate(&self) -> bool {
        matches!(self.status, UserStatus::Active)
    }

    /// Whether the address has been proven.
    #[must_use]
    pub const fn email_is_verified(&self) -> bool {
        self.email_verified_at.is_some()
    }

    /// Disables the person.
    ///
    /// # Errors
    ///
    /// Returns [`UserTransition::AlreadyDisabled`] for a repeat, so the caller
    /// can tell a first disablement from a replay and bump the epoch once.
    pub fn disable(&self, at: OffsetDateTime) -> Result<Self, UserTransition> {
        if self.status == UserStatus::Disabled {
            return Err(UserTransition::AlreadyDisabled);
        }
        Ok(Self {
            status: UserStatus::Disabled,
            revision: self.revision.next(),
            updated_at: at,
            ..self.clone()
        })
    }

    /// Re-enables the person.
    ///
    /// # Errors
    ///
    /// Returns [`UserTransition::AlreadyActive`] for a repeat.
    pub fn enable(&self, at: OffsetDateTime) -> Result<Self, UserTransition> {
        if self.status == UserStatus::Active {
            return Err(UserTransition::AlreadyActive);
        }
        Ok(Self {
            status: UserStatus::Active,
            revision: self.revision.next(),
            updated_at: at,
            ..self.clone()
        })
    }

    /// Records that the address was proven.
    ///
    /// # Errors
    ///
    /// Returns [`UserTransition::AlreadyVerified`] for a repeat. There is no
    /// transition that clears the timestamp: verification is not reversible,
    /// because reversing it would silently revoke every invitation acceptance
    /// that depended on it.
    pub fn verify_email(&self, at: OffsetDateTime) -> Result<Self, UserTransition> {
        if self.email_verified_at.is_some() {
            return Err(UserTransition::AlreadyVerified);
        }
        Ok(Self {
            email_verified_at: Some(at),
            revision: self.revision.next(),
            updated_at: at,
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{User, UserStatus, UserTransition};
    use crate::email::NormalizedEmail;
    use aex_control_domain::Revision;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn user(status: UserStatus) -> User {
        User {
            id: Uuid::from_u128(1),
            email: NormalizedEmail::parse("a@b.test").expect("valid"),
            email_verified_at: None,
            name: None,
            image_url: None,
            status,
            revision: Revision::INITIAL,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn the_status_transitions_both_ways_and_repeats_are_typed() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let disabled = user(UserStatus::Active).disable(at).expect("disables");
        assert_eq!(disabled.status, UserStatus::Disabled);
        assert!(!disabled.may_authenticate());
        assert_eq!(disabled.disable(at), Err(UserTransition::AlreadyDisabled));

        let active = disabled.enable(at).expect("re-enables");
        assert!(active.may_authenticate());
        assert_eq!(active.enable(at), Err(UserTransition::AlreadyActive));
    }

    #[test]
    fn verification_happens_once_and_is_never_cleared() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let verified = user(UserStatus::Active).verify_email(at).expect("verifies");
        assert!(verified.email_is_verified());
        assert_eq!(verified.email_verified_at, Some(at));
        assert_eq!(
            verified.verify_email(at),
            Err(UserTransition::AlreadyVerified)
        );
    }

    #[test]
    fn every_transition_bumps_the_revision() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let before = user(UserStatus::Active);
        assert_eq!(
            before.disable(at).expect("disables").revision,
            before.revision.next()
        );
        assert_eq!(
            before.verify_email(at).expect("verifies").revision,
            before.revision.next()
        );
    }

    #[test]
    fn the_status_spellings_round_trip() {
        for status in UserStatus::ALL {
            assert_eq!(UserStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(UserStatus::parse("banned"), None);
    }
}
