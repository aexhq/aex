//! The storage minute cursor — the `M-STOR-CLOSE` accounting transformation.
//!
//! Storage is the only meter whose base unit is coarser than its observations,
//! so its closing rule is explicit, named and asserted rather than implied.
//!
//! An interior transition **floors** to the whole minute: nothing is
//! double-charged and nothing is dropped, because the sub-minute residue simply
//! carries into the next segment and is charged at the bytes that follow the
//! transition. The hard-delete close **ceils**, so no residence ends with an
//! unbilled tail.
//!
//! The terminal ceil is the one place a customer charge can exceed physical
//! residence. It is bounded by `bytes x 1 minute` per residence, it is asserted
//! by [`tests::the_charged_minutes_never_exceed_the_ceiled_residence`], and it
//! is reported in every shadow close so finance can sign it off explicitly.
//!
//! One refinement of the accepted text: `charged_through` is the residence's own
//! open instant advanced by whole minutes, rather than an absolute
//! wall-clock-minute boundary. Anchoring the grid to the residence instead of to
//! the wall clock keeps the total error at *at most one* minute per residence;
//! anchoring it to the wall clock would also round the head of the residence
//! outward and double the bound.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::IntervalError;
use crate::identity::{AuthorityId, SegmentOrdinal};
use crate::measurement::{
    Evidence, FactBasis, Measurement, ReceiptKind, ServiceTime, SourceReceipt,
};
use crate::meter::{Meter, UnknownMeter};
use crate::wire_pending::Timestamp;

/// Milliseconds in one whole minute.
pub const MINUTE_MS: u64 = 60_000;

/// What holds retained bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageOwnerKind {
    /// A customer content object.
    ContentObject,
    /// A customer content root revision.
    ContentRoot,
    /// A retained public observation series.
    PublicObservation,
    /// A temporary observation export.
    TemporaryExport,
    /// A retained Hands MicroVM snapshot.
    MicrovmSnapshot,
}

impl StorageOwnerKind {
    /// Every owner kind, in a stable order.
    pub const ALL: [Self; 5] = [
        Self::ContentObject,
        Self::ContentRoot,
        Self::PublicObservation,
        Self::TemporaryExport,
        Self::MicrovmSnapshot,
    ];

    /// The stable identifier written to a cursor row and an authority key.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::ContentObject => "content_object",
            Self::ContentRoot => "content_root",
            Self::PublicObservation => "public_observation",
            Self::TemporaryExport => "temporary_export",
            Self::MicrovmSnapshot => "microvm_snapshot",
        }
    }
}

impl fmt::Display for StorageOwnerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for StorageOwnerKind {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "storage owner kind",
                value: value.to_owned(),
            })
    }
}

/// The exact thing whose retained bytes are billed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StorageOwner {
    /// What kind of thing it is.
    pub kind: StorageOwnerKind,
    /// Its identifier.
    pub id: AuthorityId,
    /// Its immutable generation; never a mutable name.
    pub generation: u64,
}

/// Which durable store holds the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageSource {
    /// S3, using provider content length.
    S3,
    /// DynamoDB, using canonical logical encoded bytes.
    DynamoDb,
    /// Aurora, using canonical logical encoded bytes.
    Aurora,
}

impl StorageSource {
    /// Every storage source, in a stable order.
    pub const ALL: [Self; 3] = [Self::S3, Self::DynamoDb, Self::Aurora];

    /// The stable identifier written to a row.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::S3 => "s3",
            Self::DynamoDb => "dynamodb",
            Self::Aurora => "aurora",
        }
    }
}

impl fmt::Display for StorageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for StorageSource {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "storage source",
                value: value.to_owned(),
            })
    }
}

/// How a storage segment was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageClose {
    /// An interior transition; the elapsed time floors to whole minutes.
    InteriorFloor,
    /// The terminal hard delete; the elapsed time ceils to whole minutes.
    TerminalCeil,
}

impl StorageClose {
    /// The stable identifier written to a row.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::InteriorFloor => "interior_floor",
            Self::TerminalCeil => "terminal_ceil",
        }
    }
}

impl fmt::Display for StorageClose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// The durable minute cursor of one `(owner, generation)` residence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageCursor {
    /// Bytes currently retained.
    pub bytes: u64,
    /// How far this residence has already been charged.
    pub charged_through: Timestamp,
    /// The next segment ordinal to mint.
    pub next_ordinal: SegmentOrdinal,
    /// Whether the residence has been hard-deleted.
    pub sealed: bool,
}

/// An authoritative change to a residence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StorageTransition {
    /// The residence opens with `bytes` retained.
    Put {
        /// Bytes now retained.
        bytes: u64,
    },
    /// The retained size changes.
    Resize {
        /// Bytes now retained.
        bytes: u64,
    },
    /// The bytes move to recoverable trash; they stay billable.
    Trash,
    /// The bytes are recovered from trash.
    Restore,
    /// The bytes are permanently removed; the cursor seals.
    HardDelete,
}

impl StorageTransition {
    /// The stable identifier written to a deferred-measurement payload.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Put { .. } => "put",
            Self::Resize { .. } => "resize",
            Self::Trash => "trash",
            Self::Restore => "restore",
            Self::HardDelete => "hard_delete",
        }
    }

    /// How a segment closed by this transition rounds.
    #[must_use]
    pub const fn close(self) -> StorageClose {
        match self {
            Self::HardDelete => StorageClose::TerminalCeil,
            _ => StorageClose::InteriorFloor,
        }
    }
}

/// What one transition produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageAccrualOutcome {
    /// The closed segment, when the elapsed time covered at least one minute.
    pub closed: Option<Measurement>,
    /// The segment ordinal the closed measurement occupies. It is minted even
    /// when nothing closed, so a caller never reuses one.
    pub segment_ordinal: SegmentOrdinal,
    /// The cursor state to persist.
    pub cursor: StorageCursor,
}

/// Applies one authoritative transition to a residence.
///
/// Deterministic and total: the same cursor, instant and transition always
/// produce the same outcome, so a replayed `regional-work` payload converges.
///
/// # Errors
///
/// Returns [`IntervalError`] when the transition precedes the cursor, opens an
/// already-open residence, touches a sealed one, arrives without a residence,
/// or produces a quantity that does not fit.
pub fn accrue_storage(
    cursor: Option<&StorageCursor>,
    owner: &StorageOwner,
    source: StorageSource,
    at: Timestamp,
    transition: StorageTransition,
    commit_id: &str,
) -> Result<StorageAccrualOutcome, IntervalError> {
    let Some(cursor) = cursor else {
        let StorageTransition::Put { bytes } = transition else {
            return Err(IntervalError::NoResidence {
                transition: transition.id(),
            });
        };
        return Ok(StorageAccrualOutcome {
            closed: None,
            segment_ordinal: SegmentOrdinal::FIRST,
            cursor: StorageCursor {
                bytes,
                charged_through: at,
                next_ordinal: SegmentOrdinal::FIRST,
                sealed: false,
            },
        });
    };
    if cursor.sealed {
        return Err(IntervalError::Sealed);
    }
    if matches!(transition, StorageTransition::Put { .. }) {
        return Err(IntervalError::AlreadyOpen);
    }
    let elapsed_ms = cursor
        .charged_through
        .millis_until(at)
        .ok_or_else(|| IntervalError::Backwards {
            at: at.to_canonical(),
            charged_through: cursor.charged_through.to_canonical(),
        })?;

    let close = transition.close();
    let minutes = match close {
        StorageClose::InteriorFloor => elapsed_ms / MINUTE_MS,
        StorageClose::TerminalCeil => elapsed_ms.div_ceil(MINUTE_MS),
    };

    let charged_ms = minutes
        .checked_mul(MINUTE_MS)
        .ok_or(IntervalError::Quantity(
            crate::quantity::QuantityError::Overflow {
                operation: "minute accrual",
            },
        ))?;
    let segment_end = Timestamp::from_unix_millis(
        cursor
            .charged_through
            .unix_millis()
            .saturating_add(i64::try_from(charged_ms).unwrap_or(i64::MAX)),
    )
    .map_err(|_| {
        IntervalError::Quantity(crate::quantity::QuantityError::Overflow {
            operation: "segment end",
        })
    })?;

    let closed = if minutes == 0 {
        None
    } else {
        Some(Measurement::new(
            Meter::StorageByteMin,
            FactBasis::Consumed,
            ServiceTime::Interval {
                start: cursor.charged_through,
                end: segment_end,
            },
            SourceReceipt {
                kind: ReceiptKind::StorageCommit,
                id: Box::from(commit_id),
                digest: None,
            },
            Evidence::StorageResidence {
                owner: owner.clone(),
                source,
                bytes: cursor.bytes,
                minutes,
                commit_id: Box::from(commit_id),
                close,
            },
        )?)
    };

    let bytes_after = match transition {
        StorageTransition::Resize { bytes } => bytes,
        _ => cursor.bytes,
    };
    let next_ordinal = if closed.is_some() {
        cursor.next_ordinal.next().map_err(|_| {
            IntervalError::Quantity(crate::quantity::QuantityError::Overflow {
                operation: "segment ordinal",
            })
        })?
    } else {
        cursor.next_ordinal
    };

    Ok(StorageAccrualOutcome {
        closed,
        segment_ordinal: cursor.next_ordinal,
        cursor: StorageCursor {
            bytes: bytes_after,
            charged_through: segment_end,
            next_ordinal,
            sealed: matches!(transition, StorageTransition::HardDelete),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{
        MINUTE_MS, StorageOwner, StorageOwnerKind, StorageSource, StorageTransition, accrue_storage,
    };
    use crate::identity::AuthorityId;
    use crate::interval::IntervalError;
    use crate::wire_pending::Timestamp;

    fn owner() -> StorageOwner {
        StorageOwner {
            kind: StorageOwnerKind::ContentObject,
            id: AuthorityId::parse("obj-1").expect("id"),
            generation: 3,
        }
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    /// Replays a whole residence and returns the emitted byte-minutes.
    fn replay(script: &[(i64, StorageTransition)]) -> Vec<u128> {
        let mut cursor = None;
        let mut emitted = Vec::new();
        for (millis, transition) in script {
            let outcome = accrue_storage(
                cursor.as_ref(),
                &owner(),
                StorageSource::S3,
                at(*millis),
                *transition,
                "commit",
            )
            .expect("valid transition");
            if let Some(measurement) = outcome.closed {
                emitted.push(measurement.quantity().get());
            }
            cursor = Some(outcome.cursor);
        }
        emitted
    }

    #[test]
    fn an_interior_transition_floors_and_carries_the_residue() {
        let emitted = replay(&[
            (0, StorageTransition::Put { bytes: 1_000 }),
            (90_000, StorageTransition::Resize { bytes: 2_000 }),
            (210_000, StorageTransition::HardDelete),
        ]);
        // 90 s at 1000 bytes floors to one minute; the 30 s residue carries and
        // is charged at 2000 bytes. 210 s - 60 s = 150 s ceils to three minutes.
        assert_eq!(emitted, vec![1_000, 2_000 * 3]);
    }

    #[test]
    fn a_sub_minute_transition_emits_nothing_and_keeps_the_cursor() {
        let outcome = accrue_storage(
            None,
            &owner(),
            StorageSource::S3,
            at(0),
            StorageTransition::Put { bytes: 10 },
            "c1",
        )
        .expect("opens");
        let second = accrue_storage(
            Some(&outcome.cursor),
            &owner(),
            StorageSource::S3,
            at(30_000),
            StorageTransition::Trash,
            "c2",
        )
        .expect("interior");
        assert!(second.closed.is_none());
        assert_eq!(second.cursor.charged_through, at(0));
        assert_eq!(second.cursor.bytes, 10);
    }

    #[test]
    fn a_ten_second_residence_is_billed_one_whole_minute() {
        let emitted = replay(&[
            (0, StorageTransition::Put { bytes: 4_096 }),
            (10_000, StorageTransition::HardDelete),
        ]);
        assert_eq!(emitted, vec![4_096]);
    }

    #[test]
    fn the_charged_minutes_never_exceed_the_ceiled_residence() {
        // S-STOR-CEIL: the declared bound is `bytes x 1 minute` per residence.
        let bytes = 512u128;
        for residence_ms in [1i64, 59_999, 60_000, 60_001, 359_999] {
            let emitted = replay(&[
                (
                    0,
                    StorageTransition::Put {
                        bytes: u64::try_from(bytes).expect("small"),
                    },
                ),
                (residence_ms, StorageTransition::HardDelete),
            ]);
            let charged: u128 = emitted.iter().sum();
            let exact_minutes =
                u128::from(u64::try_from(residence_ms).expect("positive")) / u128::from(MINUTE_MS);
            assert!(
                charged >= bytes * exact_minutes,
                "no second may be dropped at {residence_ms} ms"
            );
            assert!(
                charged <= bytes * (exact_minutes + 1),
                "the excess is bounded by bytes x 1 minute at {residence_ms} ms"
            );
        }
    }

    #[test]
    fn trash_is_interior_and_keeps_the_bytes_billable() {
        let emitted = replay(&[
            (0, StorageTransition::Put { bytes: 100 }),
            (60_000, StorageTransition::Trash),
            (120_000, StorageTransition::Restore),
            (180_000, StorageTransition::HardDelete),
        ]);
        assert_eq!(emitted, vec![100, 100, 100]);
    }

    #[test]
    fn a_sealed_cursor_refuses_every_further_transition() {
        let opened = accrue_storage(
            None,
            &owner(),
            StorageSource::S3,
            at(0),
            StorageTransition::Put { bytes: 10 },
            "c1",
        )
        .expect("opens");
        let sealed = accrue_storage(
            Some(&opened.cursor),
            &owner(),
            StorageSource::S3,
            at(60_000),
            StorageTransition::HardDelete,
            "c2",
        )
        .expect("seals");
        assert!(sealed.cursor.sealed);
        for transition in [
            StorageTransition::Resize { bytes: 1 },
            StorageTransition::Trash,
            StorageTransition::Restore,
            StorageTransition::HardDelete,
        ] {
            assert_eq!(
                accrue_storage(
                    Some(&sealed.cursor),
                    &owner(),
                    StorageSource::S3,
                    at(120_000),
                    transition,
                    "c3",
                ),
                Err(IntervalError::Sealed)
            );
        }
    }

    #[test]
    fn a_transition_without_a_residence_or_before_the_cursor_is_refused() {
        assert!(matches!(
            accrue_storage(
                None,
                &owner(),
                StorageSource::S3,
                at(0),
                StorageTransition::Trash,
                "c1",
            ),
            Err(IntervalError::NoResidence { .. })
        ));
        let opened = accrue_storage(
            None,
            &owner(),
            StorageSource::S3,
            at(120_000),
            StorageTransition::Put { bytes: 1 },
            "c1",
        )
        .expect("opens");
        assert!(matches!(
            accrue_storage(
                Some(&opened.cursor),
                &owner(),
                StorageSource::S3,
                at(60_000),
                StorageTransition::Trash,
                "c2",
            ),
            Err(IntervalError::Backwards { .. })
        ));
        assert_eq!(
            accrue_storage(
                Some(&opened.cursor),
                &owner(),
                StorageSource::S3,
                at(180_000),
                StorageTransition::Put { bytes: 2 },
                "c2",
            ),
            Err(IntervalError::AlreadyOpen)
        );
    }

    #[test]
    fn every_closed_segment_mints_a_distinct_ordinal() {
        let mut cursor = None;
        let mut ordinals = Vec::new();
        for millis in [0i64, 60_000, 120_000, 180_000] {
            let transition = if millis == 0 {
                StorageTransition::Put { bytes: 8 }
            } else {
                StorageTransition::Resize {
                    bytes: 8 + u64::try_from(millis).expect("positive") / 60_000,
                }
            };
            let outcome = accrue_storage(
                cursor.as_ref(),
                &owner(),
                StorageSource::S3,
                at(millis),
                transition,
                "commit",
            )
            .expect("valid");
            if outcome.closed.is_some() {
                ordinals.push(outcome.segment_ordinal.get());
            }
            cursor = Some(outcome.cursor);
        }
        assert_eq!(ordinals, vec![0, 1, 2]);
    }
}
