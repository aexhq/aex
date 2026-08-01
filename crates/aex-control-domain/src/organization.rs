//! Organizations.
//!
//! The billing and ownership boundary. `status` admits exactly one value at
//! launch, because organization deletion is absent from the accepted wire
//! contract: a `CHECK` that admits only `active` makes that absence explicit
//! rather than a forgotten state somebody adds a transition to later.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::Revision;
use crate::slug::Slug;

/// The lifecycle of an organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OrganizationStatus {
    /// The only status at launch.
    Active,
}

impl OrganizationStatus {
    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        (text == "active").then_some(Self::Active)
    }
}

/// An organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Organization {
    /// Public id payload.
    pub id: Uuid,
    /// Display name.
    pub name: String,
    /// Globally unique slug.
    pub slug: Slug,
    /// Lifecycle.
    pub status: OrganizationStatus,
    /// Optimistic-concurrency revision.
    pub revision: Revision,
    /// When it was created.
    pub created_at: OffsetDateTime,
    /// When it last changed.
    pub updated_at: OffsetDateTime,
    /// Who created it.
    pub created_by_user_id: Uuid,
}

impl Organization {
    /// The longest accepted display name.
    pub const MAX_NAME_LEN: usize = 128;

    /// Whether `name` is inside the database `CHECK`.
    #[must_use]
    pub fn name_is_valid(name: &str) -> bool {
        !name.is_empty() && name.chars().count() <= Self::MAX_NAME_LEN
    }
}

#[cfg(test)]
mod tests {
    use super::{Organization, OrganizationStatus};

    #[test]
    fn the_only_status_is_active() {
        assert_eq!(
            OrganizationStatus::parse("active"),
            Some(OrganizationStatus::Active)
        );
        assert_eq!(OrganizationStatus::parse("deleted"), None);
        assert_eq!(OrganizationStatus::parse("suspended"), None);
    }

    #[test]
    fn the_name_bound_matches_the_database_check() {
        assert!(Organization::name_is_valid("a"));
        assert!(Organization::name_is_valid(&"a".repeat(128)));
        assert!(!Organization::name_is_valid(""));
        assert!(!Organization::name_is_valid(&"a".repeat(129)));
    }
}
