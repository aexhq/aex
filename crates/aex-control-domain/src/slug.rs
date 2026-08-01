//! Organization and workspace slugs.
//!
//! The grammar matches the database `CHECK` exactly — `^[a-z0-9][a-z0-9-]{1,62}[a-z0-9]$` —
//! so a value that parses here cannot be refused by the constraint, and a value
//! the constraint would refuse cannot be built here.

use std::fmt;

/// Why a slug was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlugError {
    /// Outside the 3..=64 character range.
    #[error("a slug is 3 to 64 characters; got {length}")]
    Length {
        /// How long the supplied value was.
        length: usize,
    },
    /// A character outside `[a-z0-9-]`.
    #[error("a slug may only hold `a`-`z`, `0`-`9` and `-`; byte {offset} is not one")]
    Character {
        /// Where the first offending byte sits.
        offset: usize,
    },
    /// A leading or trailing hyphen.
    #[error("a slug may not start or end with `-`")]
    Boundary,
}

/// A validated slug.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slug(String);

impl Slug {
    /// The shortest accepted slug.
    pub const MIN_LEN: usize = 3;
    /// The longest accepted slug.
    pub const MAX_LEN: usize = 64;

    /// Validates and wraps a slug.
    ///
    /// # Errors
    ///
    /// Returns [`SlugError`] for a value the database `CHECK` would also refuse.
    pub fn parse(raw: &str) -> Result<Self, SlugError> {
        let bytes = raw.as_bytes();
        if bytes.len() < Self::MIN_LEN || bytes.len() > Self::MAX_LEN {
            return Err(SlugError::Length {
                length: bytes.len(),
            });
        }
        if let Some(offset) = bytes
            .iter()
            .position(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-'))
        {
            return Err(SlugError::Character { offset });
        }
        if bytes[0] == b'-' || bytes[bytes.len() - 1] == b'-' {
            return Err(SlugError::Boundary);
        }
        Ok(Self(raw.to_owned()))
    }

    /// The slug as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{Slug, SlugError};

    #[test]
    fn the_grammar_matches_the_database_check() {
        assert!(Slug::parse("acme").is_ok());
        assert!(Slug::parse("a-1").is_ok());
        assert!(Slug::parse(&"a".repeat(64)).is_ok());
        assert_eq!(Slug::parse("ab"), Err(SlugError::Length { length: 2 }));
        assert_eq!(
            Slug::parse(&"a".repeat(65)),
            Err(SlugError::Length { length: 65 })
        );
        assert_eq!(Slug::parse("-ab"), Err(SlugError::Boundary));
        assert_eq!(Slug::parse("ab-"), Err(SlugError::Boundary));
        assert_eq!(Slug::parse("Acme"), Err(SlugError::Character { offset: 0 }));
        assert_eq!(
            Slug::parse("ac_me"),
            Err(SlugError::Character { offset: 2 })
        );
        assert_eq!(
            Slug::parse("ac me"),
            Err(SlugError::Character { offset: 2 })
        );
    }
}
