//! Download grants.
//!
//! A grant is a five-minute capability over an exact byte range of one **object**,
//! delivered as a presigned S3 `GET`. It mints a `Pin::Grant` in the same
//! transaction so garbage collection cannot delete a pinned body before the grant
//! expires.
//!
//! # There is no redemption
//!
//! The wire `DownloadGrant` carries `url: https_url`, three download-grant routes
//! already ship exactly that, and there is **no redeem route among the 146**
//! (E D-11). The bearer-token model this type used to carry — a stored
//! `token_hash`, a `redeem()` and a `GrantPlacement::InlineRedemption` arm — is
//! deleted rather than left as a second, unreachable grant vocabulary.
//!
//! An inline body (≤ 32 KiB, held in `regional-content` and sealed by the
//! `aex-secret-aws` envelope) has no object to sign, so it is **promoted on first
//! grant**: unsealed, `put_immutable`'d to its content-addressed key, and the
//! resulting [`ContentObjectLocation`] passed here. `put_immutable` is
//! content-addressed and conditional, so promotion is idempotent and a repeat is
//! a cheap no-op.
//!
//! No URL appears in any `Debug`, `Display` or serialized form.

use core::fmt;

use aex_content_domain::{
    ContentDescriptor, ContentDigest, ContentObjectKey, Crc32c, GrantId, Pin,
};
use aex_wire::ids::{MeasurementId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use time::Duration;

/// How long a grant is valid.
pub const GRANT_TTL: Duration = Duration::minutes(5);

/// Largest range a single grant may authorize.
pub const MAX_SIGNED_RANGE_BYTES: u64 = 5_000_000_000_000;

/// A half-open byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteRange {
    /// First byte, inclusive.
    pub start: u64,
    /// Last byte, exclusive.
    pub end_exclusive: u64,
}

impl ByteRange {
    /// How many bytes the range covers.
    #[must_use]
    pub const fn len(self) -> u64 {
        self.end_exclusive.saturating_sub(self.start)
    }

    /// Whether the range covers nothing.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.end_exclusive <= self.start
    }
}

/// Who a grant is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GrantSubject {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The session the download is attributed to, when there is one.
    pub session: Option<SessionId>,
}

/// The exact object a grant is signed over.
///
/// A grant is always over an object: an inline body is promoted to one before a
/// grant is minted (E D-11), so there is no second placement arm to get wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentObjectLocation {
    /// The exact unversioned key.
    pub key: ContentObjectKey,
    /// The checksum the object store recorded.
    pub checksum: Crc32c,
}

/// Where a grant reads from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantPlacement {
    /// The body is an object and the reader is given a range of it.
    ObjectRange {
        /// The exact unversioned key.
        key: ContentObjectKey,
        /// The checksum the object store recorded.
        checksum: Crc32c,
    },
}

/// One minted grant.
///
/// `Debug` is written by hand so a leaked log line cannot contain the placement;
/// the derived form would print the exact object key.
#[derive(Clone, PartialEq, Eq)]
pub struct DownloadGrant {
    /// Its identity.
    pub id: GrantId,
    /// Who it is for.
    pub subject: GrantSubject,
    /// What it covers.
    pub range: ByteRange,
    /// How many bytes it authorizes, and therefore charges.
    pub authorized_bytes: u64,
    /// The immutable whole-object digest.
    pub whole_sha256: ContentDigest,
    /// Where a redemption reads from.
    pub placement: GrantPlacement,
    /// The measurement the download is recorded under.
    pub measurement: MeasurementId,
    /// When it lapses.
    pub expires_at: Timestamp,
}

impl fmt::Debug for DownloadGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DownloadGrant")
            .field("id", &self.id)
            .field("subject", &self.subject)
            .field("range", &self.range)
            .field("authorized_bytes", &self.authorized_bytes)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Why a grant was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GrantRejection {
    /// The range falls outside the object.
    #[error("range {requested:?} is outside an object of {size} bytes")]
    InvalidRange {
        /// What was asked for.
        requested: ByteRange,
        /// How large the object is.
        size: u64,
    },
    /// The range is larger than a single grant may authorize.
    #[error("range of {bytes} bytes exceeds the {max} byte maximum")]
    RangeTooLarge {
        /// How many bytes were asked for.
        bytes: u64,
        /// The maximum.
        max: u64,
    },
    /// The grant has lapsed.
    #[error("grant expired at {at:?}")]
    Expired {
        /// When it lapsed.
        at: Timestamp,
    },
}

/// Mints a grant and the pin that keeps its body alive.
///
/// The pin is returned rather than applied, so the caller writes both in one
/// transaction; a grant without its pin would be a capability over a body
/// garbage collection is free to delete. This is the one place E's
/// eventual-consistency default yields, because an unpinned grant lets GC delete
/// a body mid-download (E D-17).
///
/// The object location is an **explicit input** rather than something read off
/// the descriptor's placement: an inline body has to be promoted to an object
/// first, and making the caller name the location is what stops a grant being
/// minted over a body that has no object to sign.
///
/// # Errors
///
/// Returns [`GrantRejection`] for a range outside the object or above
/// [`MAX_SIGNED_RANGE_BYTES`].
pub fn mint_grant(
    subject: GrantSubject,
    descriptor: &ContentDescriptor,
    location: &ContentObjectLocation,
    requested: Option<ByteRange>,
    id: GrantId,
    measurement: MeasurementId,
    now: Timestamp,
) -> Result<(DownloadGrant, Pin), GrantRejection> {
    let whole = ByteRange {
        start: 0,
        end_exclusive: descriptor.size_bytes,
    };
    let range = requested.unwrap_or(whole);
    if range.start > range.end_exclusive || range.end_exclusive > descriptor.size_bytes {
        return Err(GrantRejection::InvalidRange {
            requested: range,
            size: descriptor.size_bytes,
        });
    }
    if range.len() > MAX_SIGNED_RANGE_BYTES {
        return Err(GrantRejection::RangeTooLarge {
            bytes: range.len(),
            max: MAX_SIGNED_RANGE_BYTES,
        });
    }

    let placement = GrantPlacement::ObjectRange {
        key: location.key.clone(),
        checksum: location.checksum,
    };

    let expires_at = advance(now, GRANT_TTL);
    let grant = DownloadGrant {
        id,
        subject,
        range,
        authorized_bytes: range.len(),
        whole_sha256: descriptor.digest,
        placement,
        measurement,
        expires_at,
    };
    let pin = Pin::Grant {
        grant: id,
        digest: descriptor.digest,
        expires_at,
    };
    Ok((grant, pin))
}

fn advance(now: Timestamp, ttl: Duration) -> Timestamp {
    let millis = now
        .unix_millis()
        .saturating_add(i64::try_from(ttl.whole_milliseconds()).unwrap_or(i64::MAX));
    Timestamp::from_unix_millis(millis)
        .unwrap_or_else(|_| unreachable!("a five-minute TTL keeps the instant in range"))
}

#[cfg(test)]
mod tests {
    use aex_content_domain::{
        CiphertextIdentity, ContentDescriptor, ContentDigest, ContentObjectKey, Crc32c, GrantId,
        Pin, Placement,
    };
    use aex_wire::ids::{MeasurementId, PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        ByteRange, ContentObjectLocation, GrantPlacement, GrantRejection, GrantSubject,
        MAX_SIGNED_RANGE_BYTES, mint_grant,
    };

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn descriptor(size: u64, object: bool) -> ContentDescriptor {
        ContentDescriptor {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            digest: ContentDigest::of(b"body"),
            size_bytes: size,
            media_type: None,
            placement: if object {
                Placement::Object {
                    key: ContentObjectKey::parse("wks/abc").expect("valid"),
                    checksum: Crc32c(7),
                }
            } else {
                Placement::Inline
            },
            ciphertext: CiphertextIdentity {
                key_generation: 1,
                wrapped_key: vec![1; 32],
                nonce: vec![1; 12],
            },
            created_at: moment(0),
        }
    }

    fn subject() -> GrantSubject {
        GrantSubject {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            session: None,
        }
    }

    fn location() -> ContentObjectLocation {
        ContentObjectLocation {
            key: ContentObjectKey::parse("wks/abc").expect("valid"),
            checksum: Crc32c(7),
        }
    }

    fn mint(
        size: u64,
        object: bool,
        range: Option<ByteRange>,
    ) -> Result<(super::DownloadGrant, Pin), GrantRejection> {
        mint_grant(
            subject(),
            &descriptor(size, object),
            &location(),
            range,
            GrantId(Uuid7::compose(1, [2; 10])),
            MeasurementId::from_uuid7(Uuid7::compose(1, [3; 10])),
            moment(0),
        )
    }

    #[test]
    fn every_grant_is_an_object_range_and_arrives_with_its_pin() {
        // A promoted inline body is signed exactly like any other object: there
        // is no second placement arm and no redemption route to reach one.
        let (grant, pin) = mint(1_024, false, None).expect("mints");
        assert!(matches!(grant.placement, GrantPlacement::ObjectRange { .. }));
        assert!(matches!(pin, Pin::Grant { .. }));
        assert_eq!(grant.whole_sha256, ContentDigest::of(b"body"));
    }

    #[test]
    fn a_range_must_lie_inside_the_object_and_under_the_maximum() {
        assert!(matches!(
            mint(
                1_024,
                true,
                Some(ByteRange {
                    start: 0,
                    end_exclusive: 2_048
                })
            ),
            Err(GrantRejection::InvalidRange { .. })
        ));
        assert!(matches!(
            mint(
                MAX_SIGNED_RANGE_BYTES + 1,
                true,
                Some(ByteRange {
                    start: 0,
                    end_exclusive: MAX_SIGNED_RANGE_BYTES + 1
                })
            ),
            Err(GrantRejection::RangeTooLarge { .. })
        ));
    }

    #[test]
    fn a_grant_charges_the_authorized_bytes_and_its_pin_shares_its_instant() {
        let (grant, pin) = mint(1_024, true, None).expect("mints");
        // Charging is for the authorized bytes, not the delivered ones: the bytes
        // leave through a presigned S3 URL and aex never observes the transfer.
        assert_eq!(grant.authorized_bytes, 1_024);

        // The pin outlives the grant by construction: both carry the same instant.
        let Pin::Grant { expires_at, .. } = pin else {
            panic!("a grant mints a grant pin");
        };
        assert_eq!(expires_at, grant.expires_at);
    }

    #[test]
    fn debug_can_render_neither_a_token_nor_an_object_key() {
        let (grant, _) = mint(1_024, true, None).expect("mints");
        let rendered = format!("{grant:?}");
        assert!(!rendered.contains("token"));
        assert!(!rendered.contains("wks/abc"));
    }
}
