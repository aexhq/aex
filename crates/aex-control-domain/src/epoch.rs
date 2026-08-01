//! Revocation epochs.
//!
//! An epoch is a monotone counter per `(subject_kind, subject_id)`. A regional
//! edge compares the epoch carried by an assertion against the one its local
//! projection holds; a projection that is ahead means the credential was revoked
//! inside the assertion's thirty-second window and the request is refused.
//!
//! [`Epoch`] has [`Epoch::advance`] and no decrement constructor, and the
//! database backs that up: no application role holds `UPDATE` on
//! `control.authorization_epoch`, and the five `SECURITY DEFINER` functions are
//! the only writers. Monotonicity is therefore a property of both layers rather
//! than a convention in one.

/// Which kind of subject an epoch belongs to.
///
/// The discriminants are the assertion envelope's wire bytes, so reordering this
/// enum is a wire change. `Empty` marks an unused trailing slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum EpochSubjectKind {
    /// An unused slot. Never carries an id or an epoch.
    Empty = 0,
    /// A person; advanced by disablement and by last-token revocation.
    User = 1,
    /// A person's role in one organization; advanced by role change or removal.
    Membership = 2,
    /// A workspace; advanced by deletion.
    Workspace = 3,
    /// A workspace API key; advanced by revocation.
    Key = 4,
    /// An organization's billing account; advanced by pause and resume.
    Account = 5,
}

impl EpochSubjectKind {
    /// Every writable kind, in wire order. `Empty` is excluded: it is a slot
    /// marker, not a subject.
    pub const WRITABLE: [Self; 5] = [
        Self::User,
        Self::Membership,
        Self::Workspace,
        Self::Key,
        Self::Account,
    ];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::User => "user",
            Self::Membership => "membership",
            Self::Workspace => "workspace",
            Self::Key => "key",
            Self::Account => "account",
        }
    }

    /// The `SECURITY DEFINER` function that advances this kind.
    ///
    /// There is deliberately none for `Empty`, and no caller anywhere may reach
    /// `control.bump_epoch(text, uuid)` directly.
    #[must_use]
    pub const fn bump_function(self) -> Option<&'static str> {
        match self {
            Self::Empty => None,
            Self::User => Some("control.bump_user_epoch"),
            Self::Membership => Some("control.bump_membership_epoch"),
            Self::Workspace => Some("control.bump_workspace_epoch"),
            Self::Key => Some("control.bump_key_epoch"),
            Self::Account => Some("control.bump_account_epoch"),
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::WRITABLE
            .iter()
            .copied()
            .find(|kind| kind.as_str() == text)
    }

    /// Resolves a wire discriminant.
    #[must_use]
    pub const fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Empty),
            1 => Some(Self::User),
            2 => Some(Self::Membership),
            3 => Some(Self::Workspace),
            4 => Some(Self::Key),
            5 => Some(Self::Account),
            _ => None,
        }
    }
}

/// A monotone revocation counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Epoch(u64);

impl Epoch {
    /// The epoch of a subject that has never been revoked.
    pub const NEVER: Self = Self(0);
    /// The epoch a subject reaches on its first revocation.
    pub const FIRST: Self = Self(1);

    /// Wraps a stored epoch.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The stored value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The next epoch.
    ///
    /// Saturating rather than wrapping: an epoch that wrapped would make a
    /// revoked credential valid again, which is the one outcome this type
    /// exists to prevent.
    #[must_use]
    pub const fn advance(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// Whether an assertion claiming `self` is stale against `projected`.
    #[must_use]
    pub const fn is_stale_against(self, projected: Self) -> bool {
        projected.0 > self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Epoch, EpochSubjectKind};

    #[test]
    fn an_epoch_only_moves_forward() {
        let epoch = Epoch::NEVER;
        assert_eq!(epoch.advance(), Epoch::FIRST);
        assert_eq!(epoch.advance().advance().get(), 2);
        assert_eq!(Epoch::new(u64::MAX).advance(), Epoch::new(u64::MAX));
    }

    #[test]
    fn a_projection_ahead_of_the_claim_is_stale() {
        assert!(Epoch::new(3).is_stale_against(Epoch::new(4)));
        assert!(!Epoch::new(4).is_stale_against(Epoch::new(4)));
        assert!(!Epoch::new(5).is_stale_against(Epoch::new(4)));
    }

    #[test]
    fn every_writable_kind_names_its_security_definer_function() {
        for kind in EpochSubjectKind::WRITABLE {
            let function = kind
                .bump_function()
                .expect("a writable kind has a function");
            assert!(function.starts_with("control.bump_"), "{function}");
            assert!(function.ends_with("_epoch"), "{function}");
        }
        assert_eq!(EpochSubjectKind::Empty.bump_function(), None);
    }

    #[test]
    fn the_wire_discriminants_round_trip_and_reject_the_unknown() {
        for kind in [
            EpochSubjectKind::Empty,
            EpochSubjectKind::User,
            EpochSubjectKind::Membership,
            EpochSubjectKind::Workspace,
            EpochSubjectKind::Key,
            EpochSubjectKind::Account,
        ] {
            assert_eq!(EpochSubjectKind::from_wire(kind as u8), Some(kind));
        }
        assert_eq!(EpochSubjectKind::from_wire(6), None);
        assert_eq!(EpochSubjectKind::from_wire(255), None);
    }

    #[test]
    fn the_database_spellings_round_trip_for_writable_kinds_only() {
        for kind in EpochSubjectKind::WRITABLE {
            assert_eq!(EpochSubjectKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(EpochSubjectKind::parse("empty"), None);
        assert_eq!(EpochSubjectKind::parse("session"), None);
    }
}
