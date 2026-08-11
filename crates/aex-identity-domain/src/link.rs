//! Provider identity links.
//!
//! AEX stores **no** provider access, refresh or id token. It needs no provider
//! API access, and the columns do not exist — a schema test asserts their
//! absence, so their reintroduction is a failing test rather than a review
//! comment.
//!
//! Unlinking is refused when it would leave a person with no way back in: either
//! another link survives, or the person's own email is verified.

use time::OffsetDateTime;
use uuid::Uuid;

/// Which identity provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provider {
    /// Google.
    Google,
}

impl Provider {
    /// Every provider.
    pub const ALL: [Self; 1] = [Self::Google];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }
}

/// Why an account identifier was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProviderAccountIdError {
    /// Outside the 1..=255 byte range.
    #[error("a provider account id is 1 to 255 bytes; got {length}")]
    Length {
        /// How long the supplied value was.
        length: usize,
    },
    /// A non-printable-ASCII byte.
    #[error("byte {offset} is not printable ASCII")]
    Character {
        /// Where the first offending byte sits.
        offset: usize,
    },
}

/// The provider's own identifier for one account.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderAccountId(String);

impl ProviderAccountId {
    /// Validates and wraps an identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderAccountIdError`] for a value the database `CHECK` would
    /// also refuse.
    pub fn parse(raw: &str) -> Result<Self, ProviderAccountIdError> {
        if raw.is_empty() || raw.len() > 255 {
            return Err(ProviderAccountIdError::Length { length: raw.len() });
        }
        if let Some(offset) = raw.bytes().position(|byte| !(0x21..=0x7e).contains(&byte)) {
            return Err(ProviderAccountIdError::Character { offset });
        }
        Ok(Self(raw.to_owned()))
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One provider link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIdentity {
    /// The row's own id.
    pub id: Uuid,
    /// Which person.
    pub user_id: Uuid,
    /// Which provider.
    pub provider: Provider,
    /// The provider's identifier, globally unique with `provider`.
    pub provider_account_id: ProviderAccountId,
    /// When it was linked.
    pub linked_at: OffsetDateTime,
}

/// Why an unlink was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UnlinkDenied {
    /// The link does not belong to that person.
    #[error("the link belongs to another person")]
    NotOwned,
    /// No such link.
    #[error("no such link")]
    NotFound,
    /// Unlinking would leave no way to sign in.
    #[error("unlinking would leave no way to sign in")]
    LastCredential,
}

/// Whether `target` may be unlinked from `links`.
///
/// # Errors
///
/// Returns [`UnlinkDenied::NotFound`] when the link is absent,
/// [`UnlinkDenied::NotOwned`] when it belongs to somebody else, and
/// [`UnlinkDenied::LastCredential`] when removing it would leave the person with
/// neither another link nor a verified email.
pub fn may_unlink(
    links: &[ExternalIdentity],
    user_id: Uuid,
    email_verified: bool,
    target: Uuid,
) -> Result<(), UnlinkDenied> {
    let link = links
        .iter()
        .find(|link| link.id == target)
        .ok_or(UnlinkDenied::NotFound)?;
    if link.user_id != user_id {
        return Err(UnlinkDenied::NotOwned);
    }
    let remaining = links
        .iter()
        .filter(|other| other.user_id == user_id && other.id != target)
        .count();
    if remaining == 0 && !email_verified {
        return Err(UnlinkDenied::LastCredential);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ExternalIdentity, Provider, ProviderAccountId, ProviderAccountIdError, UnlinkDenied,
        may_unlink,
    };
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn link(id: u128, user: u128, provider: Provider) -> ExternalIdentity {
        ExternalIdentity {
            id: Uuid::from_u128(id),
            user_id: Uuid::from_u128(user),
            provider,
            provider_account_id: ProviderAccountId::parse(&id.to_string()).expect("valid"),
            linked_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn the_last_link_survives_unless_the_email_is_verified() {
        let links = vec![link(1, 9, Provider::Google)];
        assert_eq!(
            may_unlink(&links, Uuid::from_u128(9), false, Uuid::from_u128(1)),
            Err(UnlinkDenied::LastCredential)
        );
        assert_eq!(
            may_unlink(&links, Uuid::from_u128(9), true, Uuid::from_u128(1)),
            Ok(())
        );
    }

    #[test]
    fn another_link_is_enough() {
        let links = vec![link(1, 9, Provider::Google), link(2, 9, Provider::Google)];
        assert_eq!(
            may_unlink(&links, Uuid::from_u128(9), false, Uuid::from_u128(1)),
            Ok(())
        );
    }

    #[test]
    fn a_link_belonging_to_somebody_else_is_never_unlinkable() {
        let links = vec![link(1, 9, Provider::Google), link(2, 8, Provider::Google)];
        assert_eq!(
            may_unlink(&links, Uuid::from_u128(9), true, Uuid::from_u128(2)),
            Err(UnlinkDenied::NotOwned)
        );
        assert_eq!(
            may_unlink(&links, Uuid::from_u128(9), true, Uuid::from_u128(3)),
            Err(UnlinkDenied::NotFound)
        );
    }

    #[test]
    fn the_account_identifier_grammar_matches_the_database_check() {
        assert!(ProviderAccountId::parse("1").is_ok());
        assert!(ProviderAccountId::parse(&"a".repeat(255)).is_ok());
        assert_eq!(
            ProviderAccountId::parse(""),
            Err(ProviderAccountIdError::Length { length: 0 })
        );
        assert_eq!(
            ProviderAccountId::parse(&"a".repeat(256)),
            Err(ProviderAccountIdError::Length { length: 256 })
        );
        assert!(matches!(
            ProviderAccountId::parse("a b"),
            Err(ProviderAccountIdError::Character { offset: 1 })
        ));
    }

    #[test]
    fn the_provider_spellings_round_trip() {
        for provider in Provider::ALL {
            assert_eq!(Provider::parse(provider.as_str()), Some(provider));
        }
        assert_eq!(Provider::parse("gitlab"), None);
    }
}
