//! Identity and time vocabulary the Hands stream needs before the contracts stream
//! publishes the generated wire types.
//!
//! `TODO(cross-stream): replaced by aex_wire::<Path> / aex_hands_protocol::<Path> at merge.`
//! Every type here is a plain newtype with no behaviour the generated type will not
//! also carry, so the merge is a re-export swap rather than a rewrite.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// A wall-clock instant expressed as exact epoch milliseconds.
///
/// Millisecond integers rather than [`OffsetDateTime`] because the true-idle
/// boundary is defined at exactly 180 000 ms and a floating or nanosecond-backed
/// representation makes `179_999` versus `180_000` a rounding question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    /// The instant `millis` milliseconds after the Unix epoch.
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Epoch milliseconds.
    #[must_use]
    pub const fn millis(self) -> i64 {
        self.0
    }

    /// Milliseconds elapsed since `earlier`, saturating at zero.
    ///
    /// Saturation is deliberate: a clock that moves backwards must never present
    /// as a huge elapsed interval, which would make a busy generation look idle.
    #[must_use]
    pub const fn saturating_millis_since(self, earlier: Self) -> u64 {
        let delta = self.0.saturating_sub(earlier.0);
        if delta <= 0 { 0 } else { delta as u64 }
    }

    /// This instant advanced by `millis`, saturating at the representable range.
    #[must_use]
    pub const fn saturating_add_millis(self, millis: u64) -> Self {
        Self(self.0.saturating_add_unsigned(millis))
    }

    /// This instant moved back by `millis`, saturating at the representable range.
    #[must_use]
    pub const fn saturating_sub_millis(self, millis: u64) -> Self {
        Self(self.0.saturating_sub_unsigned(millis))
    }

    /// Converts from the `time` crate representation used by the store peers.
    #[must_use]
    pub const fn from_offset_date_time(value: OffsetDateTime) -> Self {
        Self((value.unix_timestamp_nanos() / 1_000_000) as i64)
    }

    /// Converts to the `time` crate representation used by the store peers.
    ///
    /// # Errors
    ///
    /// Returns [`time::error::ComponentRange`] when the millisecond value is
    /// outside the representable range of [`OffsetDateTime`].
    pub fn to_offset_date_time(self) -> Result<OffsetDateTime, time::error::ComponentRange> {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(self.0) * 1_000_000)
    }
}

/// A monotonically advancing lifecycle fence for one generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Fence(u64);

impl Fence {
    /// The fence every freshly allocated generation starts at.
    pub const ZERO: Self = Self(0);

    /// A fence at `value`.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The raw fence value.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    /// The next fence, saturating so a fence can never wrap back into the past.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for Fence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An optimistic-concurrency revision on the generation head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    /// The revision a freshly written head starts at.
    pub const ZERO: Self = Self(0);

    /// A revision at `value`.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The raw revision value.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    /// The next revision, saturating.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

macro_rules! uuid_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Wraps an already-allocated identifier.
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// The wrapped identifier.
            #[must_use]
            pub const fn uuid(self) -> Uuid {
                self.0
            }

            /// The identifier's canonical 16 bytes, in the order the wire preamble uses.
            #[must_use]
            pub const fn to_bytes(self) -> [u8; 16] {
                *self.0.as_bytes()
            }

            /// Reconstructs the identifier from its canonical 16 bytes.
            #[must_use]
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(Uuid::from_bytes(bytes))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(text)?))
            }
        }
    };
}

uuid_newtype! {
    /// The identity of one exact Hands generation. Time-ordered (UUID v7), so a
    /// later generation always compares greater than an earlier one.
    GenerationId
}
uuid_newtype! {
    /// The session that owns a generation.
    SessionId
}
uuid_newtype! {
    /// The workspace a generation is attributed to.
    WorkspaceId
}
uuid_newtype! {
    /// The organization a generation is billed to.
    OrganizationId
}
uuid_newtype! {
    /// One Brain-minted Hands operation. Never derived from a model tool-use id.
    HandsOperationId
}
uuid_newtype! {
    /// One recorded lifecycle intent.
    LifecycleIntentId
}
uuid_newtype! {
    /// One issued keepalive lease.
    KeepaliveLeaseId
}

impl HandsOperationId {
    /// The canonical no-op probe operation: `status(NIL)` returns `Unknown` and
    /// costs the guest nothing, so liveness needs no sixth verb.
    pub const NIL: Self = Self(Uuid::nil());
}

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Box<str>);

        impl $name {
            /// Wraps a provider-supplied value verbatim; AEX never normalizes it.
            #[must_use]
            pub fn new(value: impl Into<Box<str>>) -> Self {
                Self(value.into())
            }

            /// The wrapped value.
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
    };
}

string_newtype! {
    /// The provider's identifier for one MicroVM.
    MicrovmId
}
string_newtype! {
    /// The provider's request identifier, the only evidence an empty-bodied
    /// lifecycle response carries.
    ProviderRequestId
}
string_newtype! {
    /// A published MicroVM image identifier.
    ImageIdentifier
}
string_newtype! {
    /// A published MicroVM image version.
    ImageVersion
}
string_newtype! {
    /// The provider quota a capacity failure named.
    ProviderQuotaId
}

/// A 32-byte content digest (`blake3` for guest bodies, `sha256` for content objects).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Wraps 32 raw digest bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lower-case hexadecimal rendering.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
        }
        out
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.to_hex())
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// The exact protocol version pinned into a generation's identity. There is no
/// negotiation: a mismatch terminates the generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(u16);

impl SchemaVersion {
    /// The launch protocol version.
    pub const V1: Self = Self(1);

    /// A version at `value`.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// The raw version number.
    #[must_use]
    pub const fn value(self) -> u16 {
        self.0
    }
}

/// The revision of the resolved H-BOUNDARY effective limits policy a generation pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LimitsRevision(u64);

impl LimitsRevision {
    /// A revision at `value`.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The raw revision value.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentHash, Fence, GenerationId, Timestamp};
    use time::OffsetDateTime;

    #[test]
    fn elapsed_saturates_when_the_clock_moves_backwards() {
        let later = Timestamp::from_millis(1_000);
        let earlier = Timestamp::from_millis(4_000);
        assert_eq!(later.saturating_millis_since(earlier), 0);
    }

    #[test]
    fn elapsed_is_exact_at_the_idle_boundary() {
        let since = Timestamp::from_millis(0);
        assert_eq!(
            Timestamp::from_millis(179_999).saturating_millis_since(since),
            179_999
        );
        assert_eq!(
            Timestamp::from_millis(180_000).saturating_millis_since(since),
            180_000
        );
    }

    #[test]
    fn offset_date_time_round_trips_at_millisecond_resolution() {
        let original = Timestamp::from_millis(1_754_000_000_123);
        let converted: OffsetDateTime = original
            .to_offset_date_time()
            .expect("a 2025 instant is representable");
        assert_eq!(Timestamp::from_offset_date_time(converted), original);
    }

    #[test]
    fn a_fence_never_wraps() {
        assert_eq!(Fence::new(u64::MAX).next(), Fence::new(u64::MAX));
        assert_eq!(Fence::ZERO.next(), Fence::new(1));
    }

    #[test]
    fn a_generation_id_round_trips_through_its_wire_bytes() {
        let id = GenerationId::from_bytes([7; 16]);
        assert_eq!(GenerationId::from_bytes(id.to_bytes()), id);
    }

    #[test]
    fn a_content_hash_renders_as_lower_case_hex() {
        let mut bytes = [0_u8; 32];
        bytes[0] = 0xab;
        bytes[31] = 0x0f;
        let hash = ContentHash::from_bytes(bytes);
        assert!(hash.to_hex().starts_with("ab00"));
        assert!(hash.to_hex().ends_with("0f"));
        assert_eq!(hash.to_hex().len(), 64);
    }
}
