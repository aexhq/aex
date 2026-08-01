//! Download grants.
//!
//! A grant is a five-minute capability over an exact byte range. It stores a
//! token **hash**, never the token, and mints a `Pin::Grant` in the same
//! transaction so garbage collection cannot delete a pinned body before the
//! grant expires. A small body gets an inline redemption that references the
//! content and pins it — never a second copy of the bytes (D-15).
//!
//! Neither the token nor a URL appears in any `Debug`, `Display` or serialized
//! form: the type carries a hash and the hash is all it can render.

use core::fmt;

use aex_content_domain::{
    ContentDescriptor, ContentDigest, ContentMissing, ContentObjectKey, Crc32c, GrantId, Pin,
    PlacementClass,
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

/// Where a redemption reads from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantPlacement {
    /// The body is small enough to serve from content storage directly.
    InlineRedemption {
        /// The pinned body.
        content: ContentDigest,
    },
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
/// `Debug` is written by hand so a leaked log line cannot contain the token
/// hash; the derived form would print it.
#[derive(Clone, PartialEq, Eq)]
pub struct DownloadGrant {
    /// Its identity.
    pub id: GrantId,
    /// The hash of the bearer token. The token itself is never stored.
    token_hash: [u8; 32],
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

/// What a successful redemption authorizes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redemption {
    /// Which grant.
    pub grant: GrantId,
    /// What to read.
    pub placement: GrantPlacement,
    /// Which range.
    pub range: ByteRange,
    /// How many bytes are charged.
    pub charged_bytes: u64,
    /// The measurement the download is recorded under.
    pub measurement: MeasurementId,
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
    /// The presented token does not hash to the stored value.
    #[error("token does not match")]
    TokenMismatch,
    /// The body is not usable.
    #[error("content is missing")]
    ContentMissing(Box<ContentMissing>),
}

/// Mints a grant and the pin that keeps its body alive.
///
/// The pin is returned rather than applied, so the caller writes both in one
/// transaction; a grant without its pin would be a capability over a body
/// garbage collection is free to delete.
///
/// # Errors
///
/// Returns [`GrantRejection`] for a range outside the object or above
/// [`MAX_SIGNED_RANGE_BYTES`].
pub fn mint_grant(
    subject: GrantSubject,
    descriptor: &ContentDescriptor,
    requested: Option<ByteRange>,
    id: GrantId,
    token_hash: [u8; 32],
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

    let placement = match (&descriptor.placement, descriptor.placement.class()) {
        (_, PlacementClass::Inline) => GrantPlacement::InlineRedemption {
            content: descriptor.digest,
        },
        (aex_content_domain::Placement::Object { key, checksum }, PlacementClass::Object) => {
            GrantPlacement::ObjectRange {
                key: key.clone(),
                checksum: *checksum,
            }
        }
        (aex_content_domain::Placement::Inline, PlacementClass::Object) => {
            // A descriptor whose class and placement disagree was assembled
            // incorrectly; serving it would hand out bytes nobody can locate.
            return Err(GrantRejection::ContentMissing(Box::new(ContentMissing {
                workspace: descriptor.workspace,
                digest: descriptor.digest,
                placement: PlacementClass::Object,
                reason: aex_content_domain::MissingReason::ObjectAbsent,
            })));
        }
    };

    let expires_at = advance(now, GRANT_TTL);
    let grant = DownloadGrant {
        id,
        token_hash,
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

/// Redeems a grant.
///
/// Validates the token hash and the expiry, and nothing else: replay until
/// expiry is legal, and a transfer that has already started may finish after it.
/// Charging is for the **authorized** bytes, not the delivered ones.
///
/// # Errors
///
/// Returns [`GrantRejection::TokenMismatch`] or [`GrantRejection::Expired`].
pub fn redeem(
    grant: &DownloadGrant,
    presented: &[u8; 32],
    now: Timestamp,
) -> Result<Redemption, GrantRejection> {
    if grant.token_hash != *presented {
        return Err(GrantRejection::TokenMismatch);
    }
    if now.unix_millis() >= grant.expires_at.unix_millis() {
        return Err(GrantRejection::Expired {
            at: grant.expires_at,
        });
    }
    Ok(Redemption {
        grant: grant.id,
        placement: grant.placement.clone(),
        range: grant.range,
        charged_bytes: grant.authorized_bytes,
        measurement: grant.measurement,
    })
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
        ByteRange, GrantPlacement, GrantRejection, GrantSubject, MAX_SIGNED_RANGE_BYTES,
        mint_grant, redeem,
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

    fn mint(
        size: u64,
        object: bool,
        range: Option<ByteRange>,
    ) -> Result<(super::DownloadGrant, Pin), GrantRejection> {
        mint_grant(
            subject(),
            &descriptor(size, object),
            range,
            GrantId(Uuid7::compose(1, [2; 10])),
            [9; 32],
            MeasurementId::from_uuid7(Uuid7::compose(1, [3; 10])),
            moment(0),
        )
    }

    #[test]
    fn a_small_body_gets_a_content_reference_and_a_pin_not_a_copy() {
        let (grant, pin) = mint(1_024, false, None).expect("mints");
        assert!(matches!(
            grant.placement,
            GrantPlacement::InlineRedemption { .. }
        ));
        assert!(matches!(pin, Pin::Grant { .. }));
        // The grant carries a digest and a pin; there is no byte payload anywhere
        // in its type.
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
    fn redemption_checks_the_token_and_the_expiry_only() {
        let (grant, pin) = mint(1_024, true, None).expect("mints");
        assert!(matches!(
            redeem(&grant, &[0; 32], moment(1)),
            Err(GrantRejection::TokenMismatch)
        ));

        let redemption = redeem(&grant, &[9; 32], moment(1)).expect("redeems");
        assert_eq!(redemption.charged_bytes, 1_024);
        // Replay until expiry is legal.
        assert!(redeem(&grant, &[9; 32], moment(299_999)).is_ok());
        assert!(matches!(
            redeem(&grant, &[9; 32], moment(300_000)),
            Err(GrantRejection::Expired { .. })
        ));

        // The pin outlives the grant by construction: both carry the same instant.
        let Pin::Grant { expires_at, .. } = pin else {
            panic!("a grant mints a grant pin");
        };
        assert_eq!(expires_at, grant.expires_at);
    }

    #[test]
    fn neither_debug_nor_display_can_render_the_token() {
        let (grant, _) = mint(1_024, true, None).expect("mints");
        let rendered = format!("{grant:?}");
        assert!(!rendered.contains("token"));
        assert!(!rendered.contains("999999999"));
    }
}
