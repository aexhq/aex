//! The continuation cursor and page bounds.
//!
//! The cursor is bounded to [`CURSOR_MAX_ENCODED_BYTES`] canonical bytes and
//! `encode` **fails** above that rather than truncating (D-16): a silently
//! truncated cursor loses progress that the operation then repeats or skips.
//! `decode` rejects an unknown discriminant, a version mismatch and any trailing
//! byte.
//!
//! The encoding is hand-written, fixed-width little-endian and length-prefixed,
//! for the same reason every other encoding in this stream is: a `serde` bump
//! must not be able to move a persisted value.

use aex_content_domain::NormalizedPath;
use aex_wire::ids::{AgentId, PrefixedId as _, Uuid7};

/// The one cursor envelope version.
pub const CURSOR_VERSION: u16 = 1;

/// Largest encoded cursor.
pub const CURSOR_MAX_ENCODED_BYTES: usize = 4096;

/// Default page size when the caller does not ask for one.
pub const PAGE_DEFAULT: u16 = 100;

/// Which continued operation a cursor belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CursorKind {
    /// A session or workspace purge.
    Purge,
    /// A telemetry export.
    Export,
    /// A content garbage collection.
    Gc,
    /// An escalated persist.
    Persist,
    /// An escalated session stop (D-2): more agents than one bounded batch can
    /// settle, so the run/head barrier moves only on the final step.
    Stop,
}

impl CursorKind {
    /// Every kind, in canonical order.
    pub const ALL: [Self; 5] = [
        Self::Purge,
        Self::Export,
        Self::Gc,
        Self::Persist,
        Self::Stop,
    ];

    const fn discriminant(self) -> u8 {
        match self {
            Self::Purge => 1,
            Self::Export => 2,
            Self::Gc => 3,
            Self::Persist => 4,
            Self::Stop => 5,
        }
    }

    const fn from_discriminant(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Purge),
            2 => Some(Self::Export),
            3 => Some(Self::Gc),
            4 => Some(Self::Persist),
            5 => Some(Self::Stop),
            _ => None,
        }
    }
}

/// How far a purge has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PurgeStage {
    /// Closing admission and cancelling in-flight work.
    Fencing,
    /// Terminating provider generations.
    Generations,
    /// Removing owner and root edges.
    Edges,
    /// Sweeping unpinned content.
    Content,
    /// Detaching or cascading descendants.
    Descendants,
    /// Writing and projecting the denial fact.
    Denial,
}

impl PurgeStage {
    /// Every stage, in execution order.
    pub const ALL: [Self; 6] = [
        Self::Fencing,
        Self::Generations,
        Self::Edges,
        Self::Content,
        Self::Descendants,
        Self::Denial,
    ];

    const fn discriminant(self) -> u8 {
        match self {
            Self::Fencing => 1,
            Self::Generations => 2,
            Self::Edges => 3,
            Self::Content => 4,
            Self::Descendants => 5,
            Self::Denial => 6,
        }
    }

    const fn from_discriminant(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Fencing),
            2 => Some(Self::Generations),
            3 => Some(Self::Edges),
            4 => Some(Self::Content),
            5 => Some(Self::Descendants),
            6 => Some(Self::Denial),
            _ => None,
        }
    }
}

/// Which signal an export is streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExportMember {
    /// Trace spans.
    Traces,
    /// Log records.
    Logs,
    /// Metric points.
    Metrics,
}

impl ExportMember {
    /// Every member, in canonical order.
    pub const ALL: [Self; 3] = [Self::Traces, Self::Logs, Self::Metrics];

    const fn discriminant(self) -> u8 {
        match self {
            Self::Traces => 1,
            Self::Logs => 2,
            Self::Metrics => 3,
        }
    }

    const fn from_discriminant(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Traces),
            2 => Some(Self::Logs),
            3 => Some(Self::Metrics),
            _ => None,
        }
    }
}

/// An opaque provider page token a scan resumes from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageToken(Vec<u8>);

impl PageToken {
    /// Largest accepted token.
    pub const MAX_BYTES: usize = 2048;

    /// Wraps a provider token.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::TokenTooLong`] above [`PageToken::MAX_BYTES`].
    pub fn new(bytes: Vec<u8>) -> Result<Self, CursorError> {
        if bytes.len() > Self::MAX_BYTES {
            return Err(CursorError::TokenTooLong { bytes: bytes.len() });
        }
        Ok(Self(bytes))
    }

    /// The raw token.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Where a continued operation resumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorPosition {
    /// A purge, at a stage.
    Purge(PurgeStage),
    /// An export, at a byte offset inside one part of one member.
    Export {
        /// Which part.
        part: u32,
        /// Where inside it.
        byte_offset: u64,
        /// Which signal.
        member: ExportMember,
    },
    /// A collection, at a provider page token.
    Gc {
        /// Where the scan got to.
        scanned_through: PageToken,
    },
    /// A persist, at a path.
    Persist {
        /// The next path to consider.
        next_path: NormalizedPath,
    },
    /// A paged stop, at the next agent to settle.
    ///
    /// The position is exclusive of everything already settled and inclusive of
    /// `next_agent`, so a duplicate step re-reads exactly the same batch and its
    /// `OperationCursorAt` guard refuses the second commit.
    Stop {
        /// The next agent to settle, in canonical agent order.
        next_agent: AgentId,
    },
}

impl CursorPosition {
    /// Which kind of operation this position belongs to.
    #[must_use]
    pub const fn kind(&self) -> CursorKind {
        match self {
            Self::Purge(_) => CursorKind::Purge,
            Self::Export { .. } => CursorKind::Export,
            Self::Gc { .. } => CursorKind::Gc,
            Self::Persist { .. } => CursorKind::Persist,
            Self::Stop { .. } => CursorKind::Stop,
        }
    }
}

/// One resumption point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationCursor {
    /// The envelope version.
    pub version: u16,
    /// Which kind of operation.
    pub kind: CursorKind,
    /// Where it resumes.
    pub position: CursorPosition,
    /// How many units are done.
    pub processed: u64,
    /// How many are expected, when that is known.
    pub total_hint: Option<u64>,
}

impl ContinuationCursor {
    /// Builds a cursor at the current version.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::KindMismatch`] when `kind` and `position`
    /// disagree.
    pub fn new(
        position: CursorPosition,
        processed: u64,
        total_hint: Option<u64>,
    ) -> Result<Self, CursorError> {
        Ok(Self {
            version: CURSOR_VERSION,
            kind: position.kind(),
            position,
            processed,
            total_hint,
        })
    }

    /// The canonical encoding.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::TooLarge`] above [`CURSOR_MAX_ENCODED_BYTES`] and
    /// [`CursorError::KindMismatch`] when the discriminant and position
    /// disagree.
    pub fn encode(&self) -> Result<Vec<u8>, CursorError> {
        if self.kind != self.position.kind() {
            return Err(CursorError::KindMismatch {
                declared: self.kind,
                position: self.position.kind(),
            });
        }
        let mut out = Vec::new();
        out.extend_from_slice(&self.version.to_le_bytes());
        out.push(self.kind.discriminant());
        match &self.position {
            CursorPosition::Purge(stage) => out.push(stage.discriminant()),
            CursorPosition::Export {
                part,
                byte_offset,
                member,
            } => {
                out.extend_from_slice(&part.to_le_bytes());
                out.extend_from_slice(&byte_offset.to_le_bytes());
                out.push(member.discriminant());
            }
            CursorPosition::Gc { scanned_through } => {
                push_bytes(&mut out, scanned_through.as_bytes());
            }
            CursorPosition::Persist { next_path } => {
                push_bytes(&mut out, next_path.as_bytes());
            }
            CursorPosition::Stop { next_agent } => {
                out.extend_from_slice(next_agent.uuid7().as_bytes());
            }
        }
        out.extend_from_slice(&self.processed.to_le_bytes());
        match self.total_hint {
            Some(total) => {
                out.push(1);
                out.extend_from_slice(&total.to_le_bytes());
            }
            None => out.push(0),
        }
        if out.len() > CURSOR_MAX_ENCODED_BYTES {
            return Err(CursorError::TooLarge {
                bytes: out.len(),
                max: CURSOR_MAX_ENCODED_BYTES,
            });
        }
        Ok(out)
    }

    /// Decodes a canonical cursor.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError`] for a wrong version, an unknown discriminant, a
    /// truncated buffer, an over-long buffer or any trailing byte.
    pub fn decode(bytes: &[u8]) -> Result<Self, CursorError> {
        if bytes.len() > CURSOR_MAX_ENCODED_BYTES {
            return Err(CursorError::TooLarge {
                bytes: bytes.len(),
                max: CURSOR_MAX_ENCODED_BYTES,
            });
        }
        let mut reader = Reader::new(bytes);
        let version = u16::from_le_bytes(reader.array::<2>()?);
        if version != CURSOR_VERSION {
            return Err(CursorError::VersionMismatch {
                expected: CURSOR_VERSION,
                found: version,
            });
        }
        let kind = CursorKind::from_discriminant(reader.byte()?)
            .ok_or(CursorError::UnknownDiscriminant)?;
        let position = match kind {
            CursorKind::Purge => CursorPosition::Purge(
                PurgeStage::from_discriminant(reader.byte()?)
                    .ok_or(CursorError::UnknownDiscriminant)?,
            ),
            CursorKind::Export => CursorPosition::Export {
                part: u32::from_le_bytes(reader.array::<4>()?),
                byte_offset: u64::from_le_bytes(reader.array::<8>()?),
                member: ExportMember::from_discriminant(reader.byte()?)
                    .ok_or(CursorError::UnknownDiscriminant)?,
            },
            CursorKind::Gc => CursorPosition::Gc {
                scanned_through: PageToken::new(reader.bytes()?.to_vec())?,
            },
            CursorKind::Persist => CursorPosition::Persist {
                next_path: NormalizedPath::parse(
                    std::str::from_utf8(reader.bytes()?).map_err(|_| CursorError::Malformed)?,
                )
                .map_err(|_| CursorError::Malformed)?,
            },
            CursorKind::Stop => CursorPosition::Stop {
                next_agent: AgentId::from_uuid7(
                    Uuid7::from_bytes(reader.array::<16>()?).map_err(|_| CursorError::Malformed)?,
                ),
            },
        };
        let processed = u64::from_le_bytes(reader.array::<8>()?);
        let total_hint = match reader.byte()? {
            0 => None,
            1 => Some(u64::from_le_bytes(reader.array::<8>()?)),
            _ => return Err(CursorError::UnknownDiscriminant),
        };
        if !reader.is_exhausted() {
            return Err(CursorError::TrailingBytes {
                bytes: reader.remaining(),
            });
        }
        Ok(Self {
            version,
            kind,
            position,
            processed,
            total_hint,
        })
    }
}

fn push_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    let length = u32::try_from(bytes.len())
        .unwrap_or_else(|_| unreachable!("cursor fields are bounded well below u32::MAX"));
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(bytes);
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn byte(&mut self) -> Result<u8, CursorError> {
        let value = *self.bytes.get(self.offset).ok_or(CursorError::Truncated)?;
        self.offset += 1;
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CursorError> {
        let slice = self
            .bytes
            .get(self.offset..self.offset + N)
            .ok_or(CursorError::Truncated)?;
        self.offset += N;
        let mut out = [0_u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn bytes(&mut self) -> Result<&'a [u8], CursorError> {
        let length = u32::from_le_bytes(self.array::<4>()?) as usize;
        let slice = self
            .bytes
            .get(self.offset..self.offset + length)
            .ok_or(CursorError::Truncated)?;
        self.offset += length;
        Ok(slice)
    }

    const fn is_exhausted(&self) -> bool {
        self.offset == self.bytes.len()
    }

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
}

/// Why a cursor was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    /// The encoding exceeded the bound.
    #[error("cursor needs {bytes} bytes, above the {max} byte bound")]
    TooLarge {
        /// Bytes needed.
        bytes: usize,
        /// The bound.
        max: usize,
    },
    /// The envelope version was not the current one.
    #[error("cursor version {found} is not {expected}")]
    VersionMismatch {
        /// The version this build writes.
        expected: u16,
        /// The version found.
        found: u16,
    },
    /// A discriminant outside the closed set appeared.
    #[error("cursor carries an unknown discriminant")]
    UnknownDiscriminant,
    /// The buffer ended early.
    #[error("cursor is truncated")]
    Truncated,
    /// Bytes remained after a complete cursor.
    #[error("cursor has {bytes} trailing byte(s)")]
    TrailingBytes {
        /// How many.
        bytes: usize,
    },
    /// A field's bytes were not a legal value.
    #[error("cursor carries a malformed field")]
    Malformed,
    /// A page token exceeded its bound.
    #[error("page token of {bytes} bytes is too long")]
    TokenTooLong {
        /// Bytes offered.
        bytes: usize,
    },
    /// The declared kind and the position disagreed.
    #[error("cursor declares {declared:?} but carries a {position:?} position")]
    KindMismatch {
        /// The declared kind.
        declared: CursorKind,
        /// The position's kind.
        position: CursorKind,
    },
}

/// Why a page request was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PageError {
    /// A page of zero items was asked for.
    #[error("page size must be at least 1")]
    Zero,
    /// The request exceeded the workspace's effective maximum.
    #[error("page size {requested} is above the effective maximum {max}")]
    AboveMaximum {
        /// What was asked for.
        requested: u16,
        /// The effective maximum.
        max: u16,
    },
}

/// Resolves a page size.
///
/// Absent means [`PAGE_DEFAULT`]; zero is rejected; anything above the effective
/// maximum is rejected rather than clamped, because silently returning fewer
/// items than asked for makes a client's paging arithmetic wrong (D-19).
///
/// # Errors
///
/// Returns [`PageError`] for zero or an over-large request.
pub const fn page_limit(requested: Option<u16>, effective_max: u16) -> Result<u16, PageError> {
    let Some(requested) = requested else {
        if PAGE_DEFAULT > effective_max {
            return Ok(effective_max);
        }
        return Ok(PAGE_DEFAULT);
    };
    if requested == 0 {
        return Err(PageError::Zero);
    }
    if requested > effective_max {
        return Err(PageError::AboveMaximum {
            requested,
            max: effective_max,
        });
    }
    Ok(requested)
}

#[cfg(test)]
mod tests {
    use aex_content_domain::NormalizedPath;
    use aex_wire::ids::PrefixedId as _;

    use super::{
        CURSOR_MAX_ENCODED_BYTES, ContinuationCursor, CursorError, CursorPosition, ExportMember,
        PAGE_DEFAULT, PageError, PageToken, PurgeStage, page_limit,
    };

    fn every_position() -> Vec<CursorPosition> {
        let mut out: Vec<CursorPosition> = PurgeStage::ALL
            .into_iter()
            .map(CursorPosition::Purge)
            .collect();
        for member in ExportMember::ALL {
            out.push(CursorPosition::Export {
                part: 7,
                byte_offset: 1_234,
                member,
            });
        }
        out.push(CursorPosition::Gc {
            scanned_through: PageToken::new(b"page-token".to_vec()).expect("in range"),
        });
        out.push(CursorPosition::Persist {
            next_path: NormalizedPath::parse("a/b/c.txt").expect("valid"),
        });
        out.push(CursorPosition::Stop {
            next_agent: aex_wire::ids::AgentId::from_uuid7(aex_wire::ids::Uuid7::compose(
                1_700_000_000_000,
                [7; 10],
            )),
        });
        out
    }

    #[test]
    fn every_position_round_trips() {
        for position in every_position() {
            let cursor = ContinuationCursor::new(position, 42, Some(100)).expect("builds");
            let encoded = cursor.encode().expect("encodes");
            assert_eq!(ContinuationCursor::decode(&encoded), Ok(cursor));
        }
    }

    #[test]
    fn decode_rejects_trailing_bytes_and_unknown_discriminants() {
        let cursor = ContinuationCursor::new(CursorPosition::Purge(PurgeStage::Edges), 1, None)
            .expect("builds");
        let mut encoded = cursor.encode().expect("encodes");
        encoded.push(0);
        assert_eq!(
            ContinuationCursor::decode(&encoded),
            Err(CursorError::TrailingBytes { bytes: 1 })
        );

        let mut wrong_kind = cursor.encode().expect("encodes");
        wrong_kind[2] = 99;
        assert_eq!(
            ContinuationCursor::decode(&wrong_kind),
            Err(CursorError::UnknownDiscriminant)
        );

        let mut wrong_version = cursor.encode().expect("encodes");
        wrong_version[0] = 2;
        assert_eq!(
            ContinuationCursor::decode(&wrong_version),
            Err(CursorError::VersionMismatch {
                expected: 1,
                found: 2
            })
        );

        assert_eq!(
            ContinuationCursor::decode(&[1]),
            Err(CursorError::Truncated)
        );
    }

    #[test]
    fn encode_fails_rather_than_truncating() {
        let token = PageToken::new(vec![7; PageToken::MAX_BYTES]).expect("in range");
        let cursor = ContinuationCursor::new(
            CursorPosition::Gc {
                scanned_through: token,
            },
            0,
            None,
        )
        .expect("builds");
        assert!(cursor.encode().expect("encodes").len() <= CURSOR_MAX_ENCODED_BYTES);
        assert_eq!(
            PageToken::new(vec![7; PageToken::MAX_BYTES + 1]),
            Err(CursorError::TokenTooLong {
                bytes: PageToken::MAX_BYTES + 1
            })
        );
    }

    #[test]
    fn page_limits_default_and_reject() {
        assert_eq!(page_limit(None, 1_000), Ok(PAGE_DEFAULT));
        assert_eq!(page_limit(Some(1), 1_000), Ok(1));
        assert_eq!(page_limit(Some(1_000), 1_000), Ok(1_000));
        assert_eq!(page_limit(Some(0), 1_000), Err(PageError::Zero));
        assert_eq!(
            page_limit(Some(1_001), 1_000),
            Err(PageError::AboveMaximum {
                requested: 1_001,
                max: 1_000
            })
        );
        assert_eq!(page_limit(None, 10), Ok(10));
    }
}
