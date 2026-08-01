//! Key component validation and the fixed-width renderings every regional key
//! template is built from.
//!
//! `#` is the only separator in a regional key, so a component that could carry
//! one would let a crafted name, scope or identifier forge a key in another
//! partition. Every component placed into a key passes through
//! [`Component::parse`] first, which is why the range-scan sentinels
//! ([`LOWER_SENTINEL`], [`UPPER_SENTINEL`]) are unambiguous.

use std::fmt;

/// The one key separator.
pub const SEPARATOR: char = '#';

/// The lower range-scan sentinel. No component may contain it, so
/// `"{prefix}#\0"` sorts strictly below every real key with that prefix.
pub const LOWER_SENTINEL: char = '\0';

/// The upper range-scan sentinel. No component may contain it, so
/// `"{prefix}#\u{ffff}"` sorts strictly above every real key with that prefix.
pub const UPPER_SENTINEL: char = '\u{ffff}';

/// The widest a single key component may be, in UTF-8 bytes.
///
/// `DynamoDB` caps a partition key at 2,048 bytes and a sort key at 1,024. A
/// single component is capped well below both so a template with four
/// components can never assemble an over-long key.
pub const MAX_COMPONENT_BYTES: usize = 256;

/// Zero-padding width for a monotonic sequence placed in a sort key (D-32).
///
/// Twenty digits cover the whole `u64` range, so lexicographic order over the
/// rendered form equals numeric order over the value.
pub const SEQUENCE_WIDTH: usize = 20;

/// Why a value cannot be placed into a key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The component was empty; an empty component makes two keys collide.
    #[error("a key component is empty")]
    Empty,
    /// The component was longer than [`MAX_COMPONENT_BYTES`].
    #[error("a key component is {found} bytes; the maximum is {MAX_COMPONENT_BYTES}")]
    TooLong {
        /// The measured length in UTF-8 bytes.
        found: usize,
    },
    /// The component contained a byte that would make the key ambiguous.
    #[error(
        "a key component contains the forbidden character U+{codepoint:04X} at byte offset {offset}"
    )]
    Forbidden {
        /// Where the offending character starts.
        offset: usize,
        /// The offending scalar value.
        codepoint: u32,
    },
}

/// A string proved safe to place between `#` separators.
///
/// The type exists so a key template cannot accept a bare `&str`: every call
/// site has to name the validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Component<'a>(&'a str);

impl<'a> Component<'a> {
    /// Validates one key component.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::Empty`] for an empty value, [`KeyError::TooLong`]
    /// above [`MAX_COMPONENT_BYTES`], and [`KeyError::Forbidden`] for the
    /// separator, either range sentinel, or any control character.
    pub fn parse(text: &'a str) -> Result<Self, KeyError> {
        if text.is_empty() {
            return Err(KeyError::Empty);
        }
        if text.len() > MAX_COMPONENT_BYTES {
            return Err(KeyError::TooLong { found: text.len() });
        }
        for (offset, character) in text.char_indices() {
            if is_forbidden(character) {
                return Err(KeyError::Forbidden {
                    offset,
                    codepoint: character as u32,
                });
            }
        }
        Ok(Self(text))
    }

    /// The validated text.
    #[must_use]
    pub const fn as_str(&self) -> &'a str {
        self.0
    }
}

impl fmt::Display for Component<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// Whether `character` may never appear inside a key component.
#[must_use]
pub const fn is_forbidden(character: char) -> bool {
    matches!(character, SEPARATOR | LOWER_SENTINEL | UPPER_SENTINEL)
        || character.is_control()
        // U+FFFE is the other noncharacter in the BMP; excluding it keeps the
        // sentinel neighbourhood entirely outside the value space.
        || character == '\u{fffe}'
}

/// Renders a monotonic sequence at the pinned [`SEQUENCE_WIDTH`].
#[must_use]
pub fn sequence(value: u64) -> String {
    format!("{value:0SEQUENCE_WIDTH$}")
}

/// Renders a fanout page index at the pinned six-digit width.
#[must_use]
pub fn page(value: u32) -> String {
    format!("{value:06}")
}

/// Renders a join shard index at the pinned four-digit width.
#[must_use]
pub fn shard4(value: u16) -> String {
    format!("{value:04}")
}

/// Renders a runtime due shard index at the pinned two-digit width.
#[must_use]
pub fn shard2(value: u8) -> String {
    format!("{value:02}")
}

/// Renders a garbage-collection bucket index at the pinned three-digit width.
#[must_use]
pub fn bucket3(value: u16) -> String {
    format!("{value:03}")
}

/// Selects a due shard for `id` over `shards` partitions.
///
/// `xxh3_64` is a distribution function here, never a digest: nothing about the
/// system's integrity depends on it being hard to invert.
///
/// # Panics
///
/// Panics when `shards` is zero, which is a composition bug rather than a data
/// condition.
#[must_use]
pub fn due_shard(id: &str, shards: u64) -> u64 {
    assert!(shards > 0, "a due index always has at least one shard");
    xxhash_rust::xxh3::xxh3_64(id.as_bytes()) % shards
}

/// Selects a garbage-collection bucket from the leading bytes of a digest.
///
/// The bucket is taken from the digest rather than hashed again, because the
/// digest is already uniformly distributed and a second hash would only cost
/// time.
///
/// # Errors
///
/// Returns [`KeyError::Empty`] when `digest_hex` is shorter than the four hex
/// characters the bucket is taken from.
///
/// # Panics
///
/// Panics when `buckets` is zero, which is a composition bug rather than a data
/// condition.
pub fn gc_bucket(digest_hex: &str, buckets: u16) -> Result<u16, KeyError> {
    assert!(buckets > 0, "a garbage-collection scan always has a bucket");
    let leading = digest_hex.get(0..4).ok_or(KeyError::Empty)?;
    let value = u16::from_str_radix(leading, 16).map_err(|_| KeyError::Forbidden {
        offset: 0,
        codepoint: leading.chars().next().map_or(0, |first| first as u32),
    })?;
    Ok(value % buckets)
}

#[cfg(test)]
mod tests {
    use super::{
        Component, KeyError, MAX_COMPONENT_BYTES, bucket3, due_shard, gc_bucket, page, sequence,
        shard2, shard4,
    };

    #[test]
    fn the_separator_can_never_enter_a_component() {
        let error = Component::parse("ws#evil").expect_err("`#` is rejected");
        assert!(
            matches!(error, KeyError::Forbidden { offset: 2, .. }),
            "{error}"
        );
    }

    #[test]
    fn both_range_sentinels_are_rejected() {
        assert!(Component::parse("a\0b").is_err());
        assert!(Component::parse("a\u{ffff}b").is_err());
        assert!(Component::parse("a\u{fffe}b").is_err());
    }

    #[test]
    fn an_empty_or_over_long_component_is_rejected() {
        assert_eq!(Component::parse("").unwrap_err(), KeyError::Empty);
        let long = "a".repeat(MAX_COMPONENT_BYTES + 1);
        assert_eq!(
            Component::parse(&long).unwrap_err(),
            KeyError::TooLong {
                found: MAX_COMPONENT_BYTES + 1
            }
        );
    }

    #[test]
    fn a_sequence_renders_at_exactly_twenty_digits() {
        assert_eq!(sequence(0).len(), 20);
        assert_eq!(sequence(u64::MAX).len(), 20);
        assert_eq!(sequence(u64::MAX), "18446744073709551615");
        assert!(
            sequence(9) < sequence(10),
            "padding preserves numeric order"
        );
    }

    #[test]
    fn the_fixed_width_shard_renderings_keep_their_declared_widths() {
        assert_eq!(page(7), "000007");
        assert_eq!(shard4(63), "0063");
        assert_eq!(shard2(15), "15");
        assert_eq!(bucket3(255), "255");
    }

    #[test]
    fn a_due_shard_is_stable_and_inside_its_range() {
        let first = due_shard("wrk_01", 64);
        assert_eq!(first, due_shard("wrk_01", 64));
        assert!(first < 64);
    }

    #[test]
    fn a_gc_bucket_comes_from_the_leading_digest_bytes() {
        assert_eq!(gc_bucket("0100abcd", 256).expect("four hex digits"), 0);
        assert_eq!(gc_bucket("00ff0000", 256).expect("four hex digits"), 255);
        assert!(gc_bucket("ab", 256).is_err());
    }
}
