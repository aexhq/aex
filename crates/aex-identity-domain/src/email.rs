//! Email normalization.
//!
//! One normalization, applied once, in Rust. The database stores the normalized
//! form and a `CHECK (email = lower(email))` refuses anything else, so there is
//! no second normalization for the two to disagree about — which is what makes
//! "a normalized email belongs to at most one user" enforceable by a unique
//! index rather than by care.

use std::fmt;

/// Why an email address was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EmailError {
    /// Outside the 3..=254 byte range.
    #[error("an email address is 3 to 254 bytes; got {length}")]
    Length {
        /// How long the supplied value was.
        length: usize,
    },
    /// Not exactly one `@`.
    #[error("an email address has exactly one `@`; got {count}")]
    AtCount {
        /// How many were found.
        count: usize,
    },
    /// An empty local part or domain.
    #[error("an email address needs a non-empty local part and domain")]
    EmptyPart,
    /// A control character or whitespace.
    #[error("byte {offset} is a control character or whitespace")]
    Character {
        /// Where the first offending byte sits.
        offset: usize,
    },
    /// The domain has no dot, so it cannot be a public address.
    #[error("the domain has no label separator")]
    DomainNotQualified,
}

/// A normalized email address: lowercase, trimmed, exactly one `@`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NormalizedEmail(String);

impl NormalizedEmail {
    /// The shortest accepted address, `a@b`.
    pub const MIN_LEN: usize = 3;
    /// The longest accepted address.
    pub const MAX_LEN: usize = 254;

    /// Normalizes and validates an address.
    ///
    /// Lowercasing is ASCII-only on purpose: Unicode case folding is
    /// locale-sensitive and not idempotent for every input, and an identifier
    /// whose normalization depends on a locale is not an identifier.
    ///
    /// # Errors
    ///
    /// Returns [`EmailError`] for a value the database `CHECK` would also
    /// refuse, plus the structural rules the database cannot express.
    pub fn parse(raw: &str) -> Result<Self, EmailError> {
        let trimmed = raw.trim();
        let lowered = trimmed.to_ascii_lowercase();

        if lowered.len() < Self::MIN_LEN || lowered.len() > Self::MAX_LEN {
            return Err(EmailError::Length {
                length: lowered.len(),
            });
        }
        if let Some(offset) = lowered.bytes().position(|byte| byte < 0x21 || byte == 0x7f) {
            return Err(EmailError::Character { offset });
        }
        let count = lowered.matches('@').count();
        if count != 1 {
            return Err(EmailError::AtCount { count });
        }
        let (local, domain) = lowered
            .split_once('@')
            .ok_or(EmailError::AtCount { count })?;
        if local.is_empty() || domain.is_empty() {
            return Err(EmailError::EmptyPart);
        }
        if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
            return Err(EmailError::DomainNotQualified);
        }
        Ok(Self(lowered))
    }

    /// The address as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The domain part.
    ///
    /// # Panics
    ///
    /// Never: the constructor admits exactly one `@`.
    #[must_use]
    pub fn domain(&self) -> &str {
        self.0
            .split_once('@')
            .expect("a normalized email has exactly one `@`")
            .1
    }
}

/// An email address is personal data, so it never renders in a diagnostic.
///
/// The `Display` impl is deliberately absent for the same reason: a value that
/// is easy to interpolate into a log line eventually is.
impl fmt::Debug for NormalizedEmail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "<redacted:{} bytes>", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::{EmailError, NormalizedEmail};

    #[test]
    fn normalization_is_lowercase_and_trimmed() {
        let email = NormalizedEmail::parse("  Alice@Example.COM  ").expect("a valid address");
        assert_eq!(email.as_str(), "alice@example.com");
        assert_eq!(email.domain(), "example.com");
    }

    #[test]
    fn normalization_is_idempotent() {
        let once = NormalizedEmail::parse("Alice@Example.com").expect("valid");
        let twice = NormalizedEmail::parse(once.as_str()).expect("valid");
        assert_eq!(once, twice);
    }

    #[test]
    fn the_structural_rules_are_enforced() {
        assert_eq!(
            NormalizedEmail::parse("ab"),
            Err(EmailError::Length { length: 2 })
        );
        assert_eq!(
            NormalizedEmail::parse(&format!("{}@example.com", "a".repeat(250))),
            Err(EmailError::Length { length: 262 })
        );
        assert_eq!(
            NormalizedEmail::parse("a@@b.test"),
            Err(EmailError::AtCount { count: 2 })
        );
        assert_eq!(
            NormalizedEmail::parse("nobody.example.com"),
            Err(EmailError::AtCount { count: 0 })
        );
        assert_eq!(
            NormalizedEmail::parse("@example.com"),
            Err(EmailError::EmptyPart)
        );
        assert_eq!(
            NormalizedEmail::parse("a@localhost"),
            Err(EmailError::DomainNotQualified)
        );
        assert_eq!(
            NormalizedEmail::parse("a@example.com."),
            Err(EmailError::DomainNotQualified)
        );
    }

    #[test]
    fn whitespace_and_control_characters_inside_the_address_are_refused() {
        assert!(matches!(
            NormalizedEmail::parse("a b@example.com"),
            Err(EmailError::Character { .. })
        ));
        assert!(matches!(
            NormalizedEmail::parse("a\u{7f}@example.com"),
            Err(EmailError::Character { .. })
        ));
        assert!(matches!(
            NormalizedEmail::parse("a\n@example.com"),
            Err(EmailError::Character { .. })
        ));
    }

    #[test]
    fn the_address_never_renders_in_a_diagnostic() {
        let email = NormalizedEmail::parse("alice@example.com").expect("valid");
        let rendered = format!("{email:?}");
        assert_eq!(rendered, "<redacted:17 bytes>");
        assert!(!rendered.contains("alice"));
    }
}
