//! Minimal identifier and timestamp types this crate needs before the contract
//! crates land.
//!
//! `TODO(cross-stream)`: every type here is replaced by its
//! `aex_wire::ids::*` / `aex_internal_contracts::usage::*` equivalent at merge.
//! Nothing in this module carries behaviour the contract crates do not already
//! own; it exists so the usage domain can be written, tested and reviewed
//! without waiting on a peer branch.
//!
//! The only rules enforced here are the ones the usage authority depends on: an
//! identifier is a non-empty, bounded, separator-free UTF-8 string, and a
//! timestamp is a fixed-width millisecond RFC 3339 instant.

use std::fmt;

use serde::{Deserialize, Serialize};
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime};

/// Longest accepted identifier, in UTF-8 bytes.
///
/// Matches `identifier.material`'s "registered name at most 128 UTF-8 bytes"
/// row in the accepted limits registry.
pub const MAX_ID_BYTES: usize = 128;

/// Why an identifier was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// The value was empty or entirely whitespace.
    #[error("`{kind}` identifier is empty")]
    Empty {
        /// The identifier type that refused the value.
        kind: &'static str,
    },
    /// The value exceeded [`MAX_ID_BYTES`].
    #[error("`{kind}` identifier is {len} bytes, over the {MAX_ID_BYTES}-byte ceiling")]
    TooLong {
        /// The identifier type that refused the value.
        kind: &'static str,
        /// The length that was refused.
        len: usize,
    },
    /// The value contained a character reserved by the key or canonical grammar.
    #[error("`{kind}` identifier contains reserved character {character:?}")]
    Reserved {
        /// The identifier type that refused the value.
        kind: &'static str,
        /// The offending character.
        character: char,
    },
}

/// Characters no identifier may contain.
///
/// `#` is the `DynamoDB` key separator, `/` is the canonical authority-key
/// separator, and the two control code points are the classic key-forging
/// vectors. Refusing them at construction is what makes a canonical string
/// unambiguous rather than merely conventional.
pub const RESERVED: [char; 4] = ['#', '/', '\0', '\u{ffff}'];

/// Validates one identifier body against the shared grammar.
///
/// # Errors
///
/// Returns [`IdError`] when the value is empty, over [`MAX_ID_BYTES`], or
/// contains a [`RESERVED`] character.
pub fn validate_id(kind: &'static str, value: &str) -> Result<(), IdError> {
    if value.trim().is_empty() {
        return Err(IdError::Empty { kind });
    }
    if value.len() > MAX_ID_BYTES {
        return Err(IdError::TooLong {
            kind,
            len: value.len(),
        });
    }
    if let Some(character) = value.chars().find(|c| RESERVED.contains(c)) {
        return Err(IdError::Reserved { kind, character });
    }
    Ok(())
}

macro_rules! bounded_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(Box<str>);

        impl $name {
            /// Parses one identifier under the shared bounded grammar.
            ///
            /// # Errors
            ///
            /// Returns [`IdError`] when the value is empty, oversized, or
            /// contains a [`RESERVED`] character.
            pub fn parse(value: &str) -> Result<Self, IdError> {
                validate_id(stringify!($name), value)?;
                Ok(Self(Box::from(value)))
            }

            /// Borrows the identifier body.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;

            fn try_from(value: String) -> Result<Self, IdError> {
                Self::parse(&value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0.into_string()
            }
        }
    };
}

bounded_id! {
    /// The AWS region a fact was measured and admitted in.
    RegionId
}
bounded_id! {
    /// The workspace a fact is attributed to; it is the fact partition key.
    WorkspaceId
}
bounded_id! {
    /// The organization that owes the money for a fact; the settlement key.
    OrganizationId
}
bounded_id! {
    /// The deployable that produced a fact (`brain-mux`, `regional-stream`, ...).
    ServiceId
}
bounded_id! {
    /// A session a fact is attributed to.
    SessionId
}
bounded_id! {
    /// An agent a fact is attributed to.
    AgentId
}
bounded_id! {
    /// A run a fact is attributed to.
    RunId
}
bounded_id! {
    /// A durable operation a fact is attributed to.
    OperationId
}
bounded_id! {
    /// The escrow reservation a fact draws down.
    ReservationId
}
bounded_id! {
    /// The rate book pinned at admission. Never the string `current`.
    PricingVersion
}
bounded_id! {
    /// A correction case grouping one or more correction facts.
    CaseId
}
bounded_id! {
    /// A model provider (`openai`, `anthropic`, ...) on an observability fact.
    ProviderId
}
bounded_id! {
    /// A provider model slug on an observability fact.
    ModelId
}
bounded_id! {
    /// The `pcr_` provider-credential binding an observability fact used.
    CredentialBindingId
}

/// Who requested a correction.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActorRef {
    /// An AEX operator acting through the admin console.
    Operator {
        /// The operator's stable subject identifier.
        subject: OrganizationId,
    },
    /// An automated reconciler acting on a revised provider receipt.
    Reconciler {
        /// The deployable that raised the case.
        service: ServiceId,
    },
}

/// A millisecond-resolution instant with one canonical textual form.
///
/// The wire and key form is always exactly `YYYY-MM-DDTHH:MM:SS.mmmZ`, which is
/// the fixed width every regional sort key depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Timestamp(i64);

/// The single canonical timestamp format.
const TIMESTAMP_FORMAT: &[BorrowedFormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

/// Why a timestamp was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimestampError {
    /// The value did not match the fixed-width canonical form.
    #[error("`{value}` is not a canonical `YYYY-MM-DDTHH:MM:SS.mmmZ` instant")]
    Malformed {
        /// The value that was refused.
        value: String,
    },
    /// The value was outside the representable range.
    #[error("timestamp {millis} ms is outside the representable range")]
    OutOfRange {
        /// The offending epoch-millisecond value.
        millis: i64,
    },
}

impl Timestamp {
    /// Builds a timestamp from whole milliseconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Returns [`TimestampError::OutOfRange`] when the value cannot be
    /// represented as a calendar instant.
    pub fn from_unix_millis(millis: i64) -> Result<Self, TimestampError> {
        let nanos = i128::from(millis) * 1_000_000;
        OffsetDateTime::from_unix_timestamp_nanos(nanos)
            .map_err(|_| TimestampError::OutOfRange { millis })?;
        Ok(Self(millis))
    }

    /// Whole milliseconds since the Unix epoch.
    #[must_use]
    pub const fn unix_millis(self) -> i64 {
        self.0
    }

    /// Parses the canonical fixed-width form.
    ///
    /// # Errors
    ///
    /// Returns [`TimestampError::Malformed`] for anything that is not exactly
    /// `YYYY-MM-DDTHH:MM:SS.mmmZ`.
    pub fn parse(value: &str) -> Result<Self, TimestampError> {
        // The canonical form pins UTC with a literal `Z` rather than a parsed
        // offset, so the text alone cannot build an `OffsetDateTime`. Parsing as
        // primitive and asserting UTC is what makes `+01:00` a rejection rather
        // than a silently accepted second spelling of the same instant.
        let parsed = PrimitiveDateTime::parse(value, &TIMESTAMP_FORMAT)
            .map_err(|_| TimestampError::Malformed {
                value: value.to_owned(),
            })?
            .assume_utc();
        let millis = i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).map_err(|_| {
            TimestampError::Malformed {
                value: value.to_owned(),
            }
        })?;
        Ok(Self(millis))
    }

    /// Renders the canonical fixed-width form.
    ///
    /// # Panics
    ///
    /// Panics only if the value passed construction and then failed to format,
    /// which the constructor makes unreachable.
    #[must_use]
    pub fn to_canonical(self) -> String {
        let nanos = i128::from(self.0) * 1_000_000;
        let instant = OffsetDateTime::from_unix_timestamp_nanos(nanos)
            .expect("a constructed Timestamp is always representable");
        instant
            .format(&TIMESTAMP_FORMAT)
            .expect("the canonical format never fails on a representable instant")
    }

    /// Milliseconds from `self` to `other`, or `None` when `other` precedes `self`.
    #[must_use]
    pub const fn millis_until(self, other: Self) -> Option<u64> {
        match other.0.checked_sub(self.0) {
            Some(delta) if delta >= 0 => Some(delta.unsigned_abs()),
            _ => None,
        }
    }

    /// The instant floored to the start of its containing whole minute.
    #[must_use]
    pub const fn floor_minute(self) -> Self {
        Self(self.0.div_euclid(60_000) * 60_000)
    }

    /// The instant raised to the start of the next whole minute, or itself when
    /// it already sits exactly on a minute boundary.
    #[must_use]
    pub const fn ceil_minute(self) -> Self {
        let floored = self.floor_minute();
        if floored.0 == self.0 {
            floored
        } else {
            Self(floored.0 + 60_000)
        }
    }

    /// Whole minutes from `self` to `other`, or `None` when `other` precedes
    /// `self`.
    #[must_use]
    pub const fn whole_minutes_until(self, other: Self) -> Option<u64> {
        match self.millis_until(other) {
            Some(delta) => Some(delta / 60_000),
            None => None,
        }
    }

    /// The `YYYY-MM` bucket this instant falls in.
    #[must_use]
    pub fn month_bucket(self) -> String {
        self.to_canonical()[..7].to_owned()
    }

    /// The `YYYY-MM-DD` bucket this instant falls in.
    #[must_use]
    pub fn day_bucket(self) -> String {
        self.to_canonical()[..10].to_owned()
    }

    /// The `YYYY-MM-DDTHH` bucket this instant falls in.
    #[must_use]
    pub fn hour_bucket(self) -> String {
        self.to_canonical()[..13].to_owned()
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_canonical())
    }
}

impl TryFrom<String> for Timestamp {
    type Error = TimestampError;

    fn try_from(value: String) -> Result<Self, TimestampError> {
        Self::parse(&value)
    }
}

impl From<Timestamp> for String {
    fn from(value: Timestamp) -> Self {
        value.to_canonical()
    }
}

#[cfg(test)]
mod tests {
    use super::{IdError, MAX_ID_BYTES, RegionId, Timestamp, WorkspaceId};

    #[test]
    fn rejects_every_reserved_character() {
        for reserved in ['#', '/', '\0', '\u{ffff}'] {
            let candidate = format!("ws{reserved}1");
            assert!(
                matches!(
                    WorkspaceId::parse(&candidate),
                    Err(IdError::Reserved { .. })
                ),
                "{reserved:?} must be refused"
            );
        }
    }

    #[test]
    fn rejects_empty_and_oversize() {
        assert!(matches!(
            WorkspaceId::parse("  "),
            Err(IdError::Empty { .. })
        ));
        let long = "w".repeat(MAX_ID_BYTES + 1);
        assert!(matches!(
            WorkspaceId::parse(&long),
            Err(IdError::TooLong { .. })
        ));
        assert!(RegionId::parse(&"r".repeat(MAX_ID_BYTES)).is_ok());
    }

    #[test]
    fn timestamps_round_trip_through_the_canonical_form() {
        let stamp = Timestamp::parse("2026-08-01T12:34:56.789Z").expect("canonical form parses");
        assert_eq!(stamp.to_canonical(), "2026-08-01T12:34:56.789Z");
        assert_eq!(
            Timestamp::from_unix_millis(stamp.unix_millis()).expect("round trip"),
            stamp
        );
    }

    #[test]
    fn refuses_a_non_canonical_instant() {
        for candidate in [
            "2026-08-01T12:34:56Z",
            "2026-08-01T12:34:56.7891Z",
            "2026-08-01T12:34:56.789+01:00",
            "not-a-time",
        ] {
            assert!(
                Timestamp::parse(candidate).is_err(),
                "`{candidate}` must be refused"
            );
        }
    }

    #[test]
    fn minute_boundaries_floor_and_ceil() {
        let at = Timestamp::parse("2026-08-01T12:34:56.789Z").expect("parses");
        assert_eq!(at.floor_minute().to_canonical(), "2026-08-01T12:34:00.000Z");
        assert_eq!(at.ceil_minute().to_canonical(), "2026-08-01T12:35:00.000Z");
        let aligned = Timestamp::parse("2026-08-01T12:34:00.000Z").expect("parses");
        assert_eq!(aligned.floor_minute(), aligned);
        assert_eq!(aligned.ceil_minute(), aligned);
    }
}
