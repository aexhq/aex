//! The scalar vocabulary every public wire shape is built from.
//!
//! Nothing here is generated: these are the primitives the generator itself
//! emits references to, so they are authored once and proved by the accept and
//! reject matrices in `tests/primitives.rs`.
//!
//! Two rules run through the whole module. First, a value that the contract
//! renders as a canonical decimal string ([`DecimalU128`], [`Cents`]) never has
//! a numeric JSON form, so a `JavaScript` client cannot silently lose precision.
//! Second, every parser is total and strict: an input that is merely *close* to
//! the grammar is a typed error, never a coerced value.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A value that did not match its wire grammar.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueError {
    /// A bounded string was empty or longer than its declared maximum.
    #[error("`{kind}` must be {min}..={max} bytes, found {found}")]
    Length {
        /// The type that rejected the value.
        kind: &'static str,
        /// Smallest accepted byte length.
        min: usize,
        /// Largest accepted byte length.
        max: usize,
        /// The offending byte length.
        found: usize,
    },
    /// A string contained a byte the grammar forbids.
    #[error("`{kind}` contains a forbidden character at byte {offset}")]
    Character {
        /// The type that rejected the value.
        kind: &'static str,
        /// Byte offset of the first offending character.
        offset: usize,
    },
    /// A string did not match its declared shape.
    #[error("`{kind}` does not match its grammar: {reason}")]
    Grammar {
        /// The type that rejected the value.
        kind: &'static str,
        /// Why the value was rejected, named precisely enough to fix.
        reason: &'static str,
    },
    /// A numeric value did not fit its declared range.
    #[error("`{kind}` is out of range")]
    Range {
        /// The type that rejected the value.
        kind: &'static str,
    },
}

/// Rejects any ASCII control character, including DEL.
fn reject_control_characters(kind: &'static str, text: &str) -> Result<(), ValueError> {
    match text.bytes().position(|byte| byte < 0x20 || byte == 0x7f) {
        Some(offset) => Err(ValueError::Character { kind, offset }),
        None => Ok(()),
    }
}

/// Deserializes a string field and maps a [`ValueError`] onto a serde error.
pub(crate) fn from_str_field<'de, D, T, F>(deserializer: D, parse: F) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    F: FnOnce(&str) -> Result<T, ValueError>,
{
    let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
    parse(text.as_ref()).map_err(D::Error::custom)
}

// ---------------------------------------------------------------------------
// Region
// ---------------------------------------------------------------------------

/// The five AWS regions a workspace may be placed in.
///
/// Placement is immutable, so this enum is closed: an unknown region name is a
/// decode error rather than a value a caller can smuggle through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Region {
    /// `us-east-1`, short code `use1`.
    #[serde(rename = "us-east-1")]
    UsEast1,
    /// `us-east-2`, short code `use2`.
    #[serde(rename = "us-east-2")]
    UsEast2,
    /// `us-west-2`, short code `usw2`.
    #[serde(rename = "us-west-2")]
    UsWest2,
    /// `ap-northeast-1`, short code `apne1`.
    #[serde(rename = "ap-northeast-1")]
    ApNortheast1,
    /// `eu-west-1`, short code `euw1`.
    #[serde(rename = "eu-west-1")]
    EuWest1,
}

impl Region {
    /// Every region, in wire-contract order.
    pub const ALL: [Self; 5] = [
        Self::UsEast1,
        Self::UsEast2,
        Self::UsWest2,
        Self::ApNortheast1,
        Self::EuWest1,
    ];

    /// The public AWS region name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UsEast1 => "us-east-1",
            Self::UsEast2 => "us-east-2",
            Self::UsWest2 => "us-west-2",
            Self::ApNortheast1 => "ap-northeast-1",
            Self::EuWest1 => "eu-west-1",
        }
    }

    /// The short code embedded in a workspace API key.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UsEast1 => "use1",
            Self::UsEast2 => "use2",
            Self::UsWest2 => "usw2",
            Self::ApNortheast1 => "apne1",
            Self::EuWest1 => "euw1",
        }
    }

    /// Resolves a short code. Only the five codes above are accepted.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|region| region.code() == code)
    }

    /// Resolves a public AWS region name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|region| region.as_str() == name)
    }
}

impl fmt::Display for Region {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// ComputeSize
// ---------------------------------------------------------------------------

/// The only customer-selectable compute dimension.
///
/// The arity is five, pinned by the limits decision; peak CPU, memory, disk and
/// bandwidth are provider-derived from the token and are never selectable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ComputeSize {
    /// 512 MiB baseline.
    #[serde(rename = "512mb")]
    Mb512,
    /// 1 GiB baseline; the default.
    #[serde(rename = "1gb")]
    Gb1,
    /// 2 GiB baseline.
    #[serde(rename = "2gb")]
    Gb2,
    /// 4 GiB baseline.
    #[serde(rename = "4gb")]
    Gb4,
    /// 8 GiB baseline.
    #[serde(rename = "8gb")]
    Gb8,
}

impl ComputeSize {
    /// Every shape, smallest first.
    pub const ALL: [Self; 5] = [Self::Mb512, Self::Gb1, Self::Gb2, Self::Gb4, Self::Gb8];

    /// The shape a session gets when `compute.size` is omitted.
    pub const DEFAULT: Self = Self::Gb1;

    /// The public token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mb512 => "512mb",
            Self::Gb1 => "1gb",
            Self::Gb2 => "2gb",
            Self::Gb4 => "4gb",
            Self::Gb8 => "8gb",
        }
    }
}

impl fmt::Display for ComputeSize {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// HttpMethod
// ---------------------------------------------------------------------------

/// The four methods the contract uses, in the fixed bundling order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    /// Observational read.
    Get,
    /// Full replacement of a named resource.
    Put,
    /// Creation, bounded typed read, or durable-operation admission.
    Post,
    /// Idempotent removal.
    Delete,
}

impl HttpMethod {
    /// Every method, in the order the bundler sorts a path's operations by.
    pub const ALL: [Self; 4] = [Self::Get, Self::Put, Self::Post, Self::Delete];

    /// The uppercase HTTP token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Post => "POST",
            Self::Delete => "DELETE",
        }
    }

    /// Resolves an uppercase HTTP token.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|method| method.as_str() == token)
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Timestamp
// ---------------------------------------------------------------------------

/// An RFC 3339 UTC instant with exactly three fractional digits.
///
/// The wire spelling is fixed at `YYYY-MM-DDThh:mm:ss.sssZ`. There is no lossy
/// `From<OffsetDateTime>`: truncation is an explicit call so a caller cannot
/// drop sub-millisecond precision by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    /// Milliseconds since the Unix epoch.
    millis: i64,
}

/// The smallest instant the wire spelling can express, `0000-01-01T00:00:00.000Z`.
const TIMESTAMP_MIN_MILLIS: i64 = -62_167_219_200_000;
/// The largest instant the wire spelling can express, `9999-12-31T23:59:59.999Z`.
const TIMESTAMP_MAX_MILLIS: i64 = 253_402_300_799_999;

impl Timestamp {
    /// Builds a timestamp from Unix milliseconds.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::Range`] when the instant cannot be rendered in the
    /// four-digit-year wire spelling.
    pub const fn from_unix_millis(millis: i64) -> Result<Self, ValueError> {
        if millis < TIMESTAMP_MIN_MILLIS || millis > TIMESTAMP_MAX_MILLIS {
            return Err(ValueError::Range { kind: "Timestamp" });
        }
        Ok(Self { millis })
    }

    /// Truncates an `OffsetDateTime` towards negative infinity to whole
    /// milliseconds and converts it to UTC.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::Range`] when the instant is outside the wire range.
    pub fn from_datetime_trunc_ms(value: time::OffsetDateTime) -> Result<Self, ValueError> {
        let millis = value.unix_timestamp_nanos().div_euclid(1_000_000);
        let millis = i64::try_from(millis).map_err(|_| ValueError::Range { kind: "Timestamp" })?;
        Self::from_unix_millis(millis)
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn unix_millis(self) -> i64 {
        self.millis
    }

    /// The instant as a `time::OffsetDateTime` in UTC.
    ///
    /// # Panics
    ///
    /// Never: the constructor already bounded the value to a representable range.
    #[must_use]
    pub fn to_datetime(self) -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(self.millis) * 1_000_000)
            .expect("a bounded Timestamp is always representable")
    }

    /// Renders the fixed 24-character wire spelling.
    #[must_use]
    pub fn to_wire(self) -> String {
        let datetime = self.to_datetime();
        let date = datetime.date();
        let clock = datetime.time();
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            date.year(),
            u8::from(date.month()),
            date.day(),
            clock.hour(),
            clock.minute(),
            clock.second(),
            clock.millisecond(),
        )
    }

    /// Parses the fixed 24-character wire spelling.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] for any other RFC 3339 spelling, including a
    /// missing or wider fractional part, a lowercase `t`/`z`, and a numeric UTC
    /// offset.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        const KIND: &str = "Timestamp";
        const SHAPE: &str = "expected exactly `YYYY-MM-DDThh:mm:ss.sssZ`";
        let shape_error = ValueError::Grammar {
            kind: KIND,
            reason: SHAPE,
        };
        let bytes = text.as_bytes();
        if bytes.len() != 24 {
            return Err(shape_error);
        }
        let punctuation_matches = bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':'
            && bytes[19] == b'.'
            && bytes[23] == b'Z';
        if !punctuation_matches {
            return Err(shape_error);
        }
        let digits = |range: std::ops::Range<usize>| -> Option<u32> {
            let mut value: u32 = 0;
            for &byte in &bytes[range] {
                if !byte.is_ascii_digit() {
                    return None;
                }
                value = value * 10 + u32::from(byte - b'0');
            }
            Some(value)
        };
        let (
            Some(year),
            Some(month),
            Some(day),
            Some(hour),
            Some(minute),
            Some(second),
            Some(milli),
        ) = (
            digits(0..4),
            digits(5..7),
            digits(8..10),
            digits(11..13),
            digits(14..16),
            digits(17..19),
            digits(20..23),
        )
        else {
            return Err(shape_error);
        };
        let month = u8::try_from(month)
            .ok()
            .and_then(|value| time::Month::try_from(value).ok())
            .ok_or(ValueError::Grammar {
                kind: KIND,
                reason: "month is out of range",
            })?;
        let year = i32::try_from(year).map_err(|_| ValueError::Range { kind: KIND })?;
        let day = u8::try_from(day).map_err(|_| ValueError::Range { kind: KIND })?;
        let date =
            time::Date::from_calendar_date(year, month, day).map_err(|_| ValueError::Grammar {
                kind: KIND,
                reason: "date is not a real day",
            })?;
        let hour = u8::try_from(hour).map_err(|_| ValueError::Range { kind: KIND })?;
        let minute = u8::try_from(minute).map_err(|_| ValueError::Range { kind: KIND })?;
        let second = u8::try_from(second).map_err(|_| ValueError::Range { kind: KIND })?;
        let milli = u16::try_from(milli).map_err(|_| ValueError::Range { kind: KIND })?;
        let clock = time::Time::from_hms_milli(hour, minute, second, milli).map_err(|_| {
            ValueError::Grammar {
                kind: KIND,
                reason: "time is out of range",
            }
        })?;
        Self::from_datetime_trunc_ms(date.with_time(clock).assume_utc())
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_wire())
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_str_field(deserializer, Self::parse)
    }
}

// ---------------------------------------------------------------------------
// Canonical decimal quantities
// ---------------------------------------------------------------------------

/// Parses a canonical non-negative decimal integer string into `u128`.
fn parse_canonical_decimal(kind: &'static str, text: &str) -> Result<u128, ValueError> {
    if text.is_empty() {
        return Err(ValueError::Grammar {
            kind,
            reason: "empty string",
        });
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err(ValueError::Grammar {
            kind,
            reason: "leading zero",
        });
    }
    if let Some(offset) = text.bytes().position(|byte| !byte.is_ascii_digit()) {
        return Err(ValueError::Character { kind, offset });
    }
    text.parse::<u128>().map_err(|_| ValueError::Range { kind })
}

/// A non-negative integer quantity the wire renders as a canonical decimal string.
///
/// Every `*Sequence`, byte count, millisecond count and token count uses this
/// type. It is deliberately never `u64` (the projection side is `Decimal(38,0)`)
/// and never a JSON number (a `JavaScript` client would round above 2^53).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DecimalU128(u128);

impl DecimalU128 {
    /// The zero quantity.
    pub const ZERO: Self = Self(0);

    /// Wraps a `u128`.
    #[must_use]
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// The underlying value.
    #[must_use]
    pub const fn get(self) -> u128 {
        self.0
    }

    /// Adds two quantities, returning `None` on overflow.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Parses the canonical decimal spelling.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] for a leading zero, a sign, an exponent, a
    /// fractional part, a non-digit byte, or a value above `u128::MAX`.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        parse_canonical_decimal("DecimalU128", text).map(Self)
    }
}

impl fmt::Display for DecimalU128 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Serialize for DecimalU128 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DecimalU128 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_str_field(deserializer, Self::parse)
    }
}

/// A non-negative whole-cent USD amount, rendered as a canonical decimal string.
///
/// `Cents` is the only money type that crosses the public wire; internal
/// micro-USD lives in `aex-internal-contracts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Cents(u64);

impl Cents {
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// Wraps a whole-cent amount.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying whole-cent amount.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Adds two amounts, returning `None` on overflow.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Parses the canonical decimal spelling.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] for the same reasons as [`DecimalU128::parse`],
    /// plus a value above `u64::MAX`.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        let value = parse_canonical_decimal("Cents", text)?;
        u64::try_from(value)
            .map(Self)
            .map_err(|_| ValueError::Range { kind: "Cents" })
    }
}

impl fmt::Display for Cents {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Serialize for Cents {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Cents {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_str_field(deserializer, Self::parse)
    }
}

// ---------------------------------------------------------------------------
// Bounded opaque strings
// ---------------------------------------------------------------------------

/// Declares a bounded, control-character-free opaque string newtype.
macro_rules! opaque_string {
    ($(#[$meta:meta])* $name:ident, $kind:literal, $max:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Box<str>);

        impl $name {
            /// Largest accepted byte length.
            pub const MAX_BYTES: usize = $max;

            /// Parses the value, enforcing length and character bounds.
            ///
            /// # Errors
            ///
            /// Returns a [`ValueError`] when the value is empty, longer than
            /// `MAX_BYTES`, or contains a control character.
            pub fn parse(text: &str) -> Result<Self, ValueError> {
                if text.is_empty() || text.len() > Self::MAX_BYTES {
                    return Err(ValueError::Length {
                        kind: $kind,
                        min: 1,
                        max: Self::MAX_BYTES,
                        found: text.len(),
                    });
                }
                reject_control_characters($kind, text)?;
                Ok(Self(text.into()))
            }

            /// The value as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                from_str_field(deserializer, Self::parse)
            }
        }
    };
}

opaque_string!(
    /// A strong opaque entity tag for a mutable detail resource.
    ///
    /// Collections never carry one; the value is compared byte-for-byte and is
    /// never parsed into the revision it encodes.
    ETag,
    "ETag",
    256
);

opaque_string!(
    /// The per-request diagnostic identifier echoed in every error envelope.
    RequestId,
    "RequestId",
    128
);

opaque_string!(
    /// A stable provider-side code, carried verbatim but never interpreted.
    StableCode,
    "StableCode",
    128
);

/// An absolute `https://` URL.
///
/// Only the scheme, a non-empty authority and printable characters are enforced
/// here; destination policy (allowlists, SSRF screening) belongs to whoever
/// dials it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HttpsUrl(Box<str>);

impl HttpsUrl {
    /// Largest accepted byte length.
    pub const MAX_BYTES: usize = 2048;

    /// Parses an absolute `https://` URL.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when the scheme is not `https`, the authority is
    /// empty, the value is too long, or it contains a control character.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        const KIND: &str = "HttpsUrl";
        if text.is_empty() || text.len() > Self::MAX_BYTES {
            return Err(ValueError::Length {
                kind: KIND,
                min: 1,
                max: Self::MAX_BYTES,
                found: text.len(),
            });
        }
        reject_control_characters(KIND, text)?;
        let Some(rest) = text.strip_prefix("https://") else {
            return Err(ValueError::Grammar {
                kind: KIND,
                reason: "scheme must be `https`",
            });
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() {
            return Err(ValueError::Grammar {
                kind: KIND,
                reason: "authority is empty",
            });
        }
        Ok(Self(text.into()))
    }

    /// The URL as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HttpsUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for HttpsUrl {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HttpsUrl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_str_field(deserializer, Self::parse)
    }
}

/// An RFC 6901 JSON pointer naming the exact position a decode failed at.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct JsonPointer(Box<str>);

impl JsonPointer {
    /// The document root.
    #[must_use]
    pub fn root() -> Self {
        Self(String::new().into_boxed_str())
    }

    /// Parses an RFC 6901 pointer.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when a non-empty pointer does not start with
    /// `/`, or when a `~` is not followed by `0` or `1`.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        const KIND: &str = "JsonPointer";
        if text.is_empty() {
            return Ok(Self::root());
        }
        if !text.starts_with('/') {
            return Err(ValueError::Grammar {
                kind: KIND,
                reason: "a non-empty pointer must start with `/`",
            });
        }
        reject_control_characters(KIND, text)?;
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'~' {
                match bytes.get(index + 1) {
                    Some(b'0' | b'1') => index += 1,
                    _ => {
                        return Err(ValueError::Character {
                            kind: KIND,
                            offset: index,
                        });
                    }
                }
            }
            index += 1;
        }
        Ok(Self(text.into()))
    }

    /// Appends an object member, escaping it per RFC 6901.
    #[must_use]
    pub fn child(&self, member: &str) -> Self {
        let escaped = member.replace('~', "~0").replace('/', "~1");
        Self(format!("{}/{escaped}", self.0).into_boxed_str())
    }

    /// Appends an array index.
    #[must_use]
    pub fn index(&self, position: usize) -> Self {
        Self(format!("{}/{position}", self.0).into_boxed_str())
    }

    /// The pointer as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JsonPointer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for JsonPointer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for JsonPointer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_str_field(deserializer, Self::parse)
    }
}

// ---------------------------------------------------------------------------
// MetadataValue
// ---------------------------------------------------------------------------

/// One value in a caller-defined metadata map.
///
/// The set is closed at the four JSON scalars: nesting metadata would make the
/// map an untyped document, and an untyped document on the wire is exactly what
/// this contract does not have.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetadataValue {
    /// A string.
    Text(String),
    /// A boolean.
    Bool(bool),
    /// A number. This is the one place a caller-supplied float is accepted.
    Number(f64),
    /// An explicit null.
    Null,
}

// ---------------------------------------------------------------------------
// ByteRange
// ---------------------------------------------------------------------------

/// An explicit inclusive byte range over an immutable object.
///
/// Both endpoints are canonical decimal strings because a 5 TB object exceeds
/// the exact integer range of a `JavaScript` number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ByteRange {
    /// First byte, inclusive.
    start: DecimalU128,
    /// Last byte, inclusive.
    end_inclusive: DecimalU128,
}

impl ByteRange {
    /// Builds an inclusive range.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::Range`] when `end_inclusive` precedes `start`; an
    /// empty range would sign a grant that authorises nothing.
    pub fn new(start: u128, end_inclusive: u128) -> Result<Self, ValueError> {
        if end_inclusive < start {
            return Err(ValueError::Range { kind: "ByteRange" });
        }
        Ok(Self {
            start: DecimalU128::new(start),
            end_inclusive: DecimalU128::new(end_inclusive),
        })
    }

    /// First byte, inclusive.
    #[must_use]
    pub const fn start(self) -> u128 {
        self.start.get()
    }

    /// Last byte, inclusive.
    #[must_use]
    pub const fn end_inclusive(self) -> u128 {
        self.end_inclusive.get()
    }

    /// Number of bytes the range covers; always at least one.
    #[must_use]
    pub const fn len(self) -> u128 {
        self.end_inclusive.get() - self.start.get() + 1
    }

    /// Always `false`: a range is never empty by construction.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }
}
