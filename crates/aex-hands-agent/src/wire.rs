//! The exact-generation frame codec.
//!
//! Every request and response body begins with a fixed-size big-endian preamble.
//! The preamble exists so the generation binding is validated **before** any
//! payload decode and with no HTTP dependency, which makes the hostile corpus pure
//! bytes.
//!
//! ```text
//! Request preamble — 40 bytes
//!   0..4    magic            b"AEXH"
//!   4..6    schema_version   u16
//!   6..7    verb             u8    1=start 2=status 3=cancel 4=result 5=attach 6=file
//!   7..8    flags            u8    bit0 = payload has a trailing binary segment
//!   8..24   generation       [u8;16]
//!  24..32   fence            u64
//!  32..36   payload_len      u32
//!  36..40   header_crc32c    u32   over bytes 0..36
//!
//! Response preamble — 56 bytes
//!   0..4    magic            b"AEXR"
//!   4..6    schema_version   u16
//!   6..7    verb             u8    echo
//!   7..8    status           u8    0 = typed payload, 1 = typed protocol error
//!   8..24   generation       [u8;16]
//!  24..32   fence            u64   highest fence the guest has observed
//!  32..36   payload_len      u32
//!  36..40   guest_revision   u32
//!  40..48   agent_build      [u8;8]
//!  48..52   reserved         [u8;4]  must be zero
//!  52..56   header_crc32c    u32   over bytes 0..52
//! ```
//!
//! # Why this module lives here
//!
//! `aex-hands-protocol` is generated from the contract stream's schemas and is not
//! edited by this stream, so the *semantics* — preamble layout, verb dispatch,
//! hostile-decode order — live beside the guest agent, which is the side that has
//! to survive hostile bytes. `aex-brain-hands` consumes the same module, so there
//! is one codec and not two that can disagree.
//!
//! # No negotiation
//!
//! `schema_version` is part of generation identity. A mismatch is
//! [`FrameError::UnsupportedVersion`] and terminates the generation; there is no
//! downgrade path and no forward compatibility.

use aex_hands_protocol::rpc::{CallHash, Fence, GenerationBinding, HandsOperationId};
use aex_internal_contracts::SchemaVersion;
use aex_wire::ids::{ContentHash, GenerationId, IdParseError, PrefixedId as _, Uuid7};

use crate::crc::crc32c;

/// The four magic bytes every request carries.
pub const REQUEST_MAGIC: [u8; 4] = *b"AEXH";

/// The four magic bytes every response carries.
pub const RESPONSE_MAGIC: [u8; 4] = *b"AEXR";

/// Fixed request preamble length.
pub const REQUEST_PREAMBLE_LEN: usize = 40;

/// Fixed response preamble length.
pub const RESPONSE_PREAMBLE_LEN: usize = 56;

/// `flags` bit 0: the payload carries a trailing binary segment. Only `result`
/// may set it.
pub const FLAG_TRAILING_BINARY: u8 = 0b0000_0001;

/// The launch protocol version.
pub const PROTOCOL_V1: SchemaVersion = SchemaVersion::V1;

/// The six exact-generation verbs.
///
/// `attach` is not a sixth verb in the protocol sense: it is the `Attached`
/// delivery mode of `start`, expressed as its own stream only because HTTP cannot
/// carry two response bodies. Liveness is not a verb either — every response
/// preamble carries `guest_revision` and `agent_build`, so any round trip is a
/// probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Verb {
    /// Start one operation.
    Start = 1,
    /// Ask about one operation.
    Status = 2,
    /// Cancel one operation.
    Cancel = 3,
    /// Pull terminal bytes, resumably.
    Result = 4,
    /// Open the attached delivery stream for one operation.
    Attach = 5,
    /// Transfer binary live-workspace files.
    File = 6,
}

impl Verb {
    /// Every verb, in wire order.
    pub const ALL: [Self; 6] = [
        Self::Start,
        Self::Status,
        Self::Cancel,
        Self::Result,
        Self::Attach,
        Self::File,
    ];

    /// The wire code.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// The fixed path this verb is posted to.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Start => "/aex/hands/v1/start",
            Self::Status => "/aex/hands/v1/status",
            Self::Cancel => "/aex/hands/v1/cancel",
            Self::Result => "/aex/hands/v1/result",
            Self::Attach => "/aex/hands/v1/attach",
            Self::File => "/aex/hands/v1/file",
        }
    }

    /// Parses a wire code.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Start),
            2 => Some(Self::Status),
            3 => Some(Self::Cancel),
            4 => Some(Self::Result),
            5 => Some(Self::Attach),
            6 => Some(Self::File),
            _ => None,
        }
    }
}

/// Whether a response body is a typed payload or a typed protocol error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ResponseStatus {
    /// A typed payload for the echoed verb.
    Payload = 0,
    /// A typed protocol error.
    ProtocolError = 1,
}

impl ResponseStatus {
    /// Parses a wire code.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Payload),
            1 => Some(Self::ProtocolError),
            _ => None,
        }
    }
}

/// What a receiver requires of an inbound frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameExpectation {
    /// The generation the receiver is bound to.
    pub generation: GenerationId,
    /// The lowest fence the receiver still accepts.
    pub min_fence: Fence,
    /// The envelope version the receiver understands. There is no negotiation.
    pub schema_version: SchemaVersion,
    /// The largest payload the receiver will look at, checked before allocation.
    pub max_frame_bytes: u32,
}

/// Why a frame was refused.
///
/// Every variant names what was expected and what arrived, because a hostile
/// sender's frame is exactly the thing an operator will want to read back.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// The bytes are not a frame at all.
    #[error("malformed frame at `{at}`: {reason}")]
    Malformed {
        /// Which decode step failed.
        at: &'static str,
        /// Why.
        reason: String,
    },
    /// The frame declares an envelope version the receiver does not understand.
    #[error("unsupported schema version {found}, expected {expected}")]
    UnsupportedVersion {
        /// The version the receiver understands.
        expected: u32,
        /// The version on the frame.
        found: u32,
    },
    /// The frame belongs to a different generation.
    #[error("frame is for generation {found}, expected {expected}")]
    WrongGeneration {
        /// The generation the receiver is bound to.
        expected: GenerationId,
        /// The generation on the frame.
        found: GenerationId,
    },
    /// The frame's fence is below the highest the receiver has observed.
    #[error("frame fence {found} is older than {expected}")]
    StaleFence {
        /// The lowest accepted fence.
        expected: u64,
        /// The fence on the frame.
        found: u64,
    },
    /// The declared payload exceeds the receiver's bound. Refused before any
    /// allocation, which `tests/hostile_frames.rs` asserts structurally.
    #[error("frame declares {actual} payload bytes, limit is {limit}")]
    Oversize {
        /// The receiver bound.
        limit: u32,
        /// The declared payload length.
        actual: u64,
    },
    /// The frame's real length disagrees with its declared length.
    #[error("length mismatch: declared {declared}, received {received}")]
    LengthMismatch {
        /// What the sender declared.
        declared: u64,
        /// What arrived.
        received: u64,
    },
    /// Assembled bytes did not match the declared digest.
    #[error("digest mismatch: declared {declared}, computed {computed}")]
    DigestMismatch {
        /// What the sender declared.
        declared: ContentHash,
        /// What the receiver computed.
        computed: ContentHash,
    },
}

/// A decoded request preamble.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPreamble {
    /// The envelope version.
    pub schema_version: SchemaVersion,
    /// Which verb.
    pub verb: Verb,
    /// Frame flags.
    pub flags: u8,
    /// The generation the frame is bound to.
    pub generation: GenerationId,
    /// The fence the frame was written at.
    pub fence: Fence,
    /// The declared payload length.
    pub payload_len: u32,
}

impl RequestPreamble {
    /// The generation binding this preamble carries, in contract form.
    #[must_use]
    pub const fn binding(&self) -> GenerationBinding {
        GenerationBinding {
            schema_version: self.schema_version,
            generation: self.generation,
            fence: self.fence,
        }
    }

    /// Whether the payload carries a trailing binary segment.
    #[must_use]
    pub const fn has_trailing_binary(&self) -> bool {
        self.flags & FLAG_TRAILING_BINARY != 0
    }
}

/// A decoded response preamble.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponsePreamble {
    /// The envelope version.
    pub schema_version: SchemaVersion,
    /// The echoed verb.
    pub verb: Verb,
    /// Whether the payload is a typed value or a typed protocol error.
    pub status: ResponseStatus,
    /// The generation.
    pub generation: GenerationId,
    /// The highest fence the guest has observed.
    pub fence: Fence,
    /// The declared payload length.
    pub payload_len: u32,
    /// The guest incarnation that answered.
    pub guest_revision: u32,
    /// The first eight bytes of the agent binary's `blake3`.
    pub agent_build: [u8; 8],
}

/// A frame decoded into its preamble and a borrowed payload.
///
/// The payload is borrowed, never copied. That is not an optimisation: it is what
/// makes "the length bound is checked before any allocation" a structural fact
/// rather than a review promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a, P> {
    /// The preamble.
    pub preamble: P,
    /// The payload bytes.
    pub payload: &'a [u8],
}

/// Encodes a request frame.
#[must_use]
pub fn encode_request(preamble: &RequestPreamble, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(REQUEST_PREAMBLE_LEN + payload.len());
    out.extend_from_slice(&REQUEST_MAGIC);
    out.extend_from_slice(&preamble.schema_version_u16().to_be_bytes());
    out.push(preamble.verb.code());
    out.push(preamble.flags);
    out.extend_from_slice(preamble.generation.uuid7().as_bytes());
    out.extend_from_slice(&preamble.fence.0.to_be_bytes());
    out.extend_from_slice(&preamble.payload_len.to_be_bytes());
    let check = crc32c(&out[..36]);
    out.extend_from_slice(&check.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

impl RequestPreamble {
    /// The envelope version narrowed to the two wire bytes.
    ///
    /// The contract's `SchemaVersion` is a `u32`; the preamble field is a `u16`.
    /// A version above `u16::MAX` cannot be spelled on the wire and saturates, so
    /// it will fail the receiver's equality check rather than silently truncate
    /// into a version the receiver does accept.
    fn schema_version_u16(&self) -> u16 {
        u16::try_from(self.schema_version.0).unwrap_or(u16::MAX)
    }
}

impl ResponsePreamble {
    /// The envelope version narrowed to the two wire bytes. See
    /// [`RequestPreamble::schema_version_u16`].
    fn schema_version_u16(&self) -> u16 {
        u16::try_from(self.schema_version.0).unwrap_or(u16::MAX)
    }
}

/// Encodes a response frame.
#[must_use]
pub fn encode_response(preamble: &ResponsePreamble, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(RESPONSE_PREAMBLE_LEN + payload.len());
    out.extend_from_slice(&RESPONSE_MAGIC);
    out.extend_from_slice(&preamble.schema_version_u16().to_be_bytes());
    out.push(preamble.verb.code());
    out.push(preamble.status as u8);
    out.extend_from_slice(preamble.generation.uuid7().as_bytes());
    out.extend_from_slice(&preamble.fence.0.to_be_bytes());
    out.extend_from_slice(&preamble.payload_len.to_be_bytes());
    out.extend_from_slice(&preamble.guest_revision.to_be_bytes());
    out.extend_from_slice(&preamble.agent_build);
    out.extend_from_slice(&[0u8; 4]);
    let check = crc32c(&out[..52]);
    out.extend_from_slice(&check.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// The named decode steps, in the order [`decode_request`] evaluates them.
///
/// Published so a test can assert the order is what the plan pins rather than
/// whatever the implementation happens to do.
pub const DECODE_STEPS: [&str; 9] = [
    "length",
    "magic",
    "schema_version",
    "verb",
    "header_crc32c",
    "generation",
    "fence",
    "payload_len",
    "frame_length",
];

/// Reads sixteen generation bytes.
fn read_generation(bytes: &[u8]) -> Result<GenerationId, IdParseError> {
    let mut raw = [0u8; 16];
    raw.copy_from_slice(&bytes[8..24]);
    Uuid7::from_bytes(raw).map(GenerationId::from_uuid7)
}

/// Reads a big-endian `u64`.
fn read_u64(bytes: &[u8], at: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[at..at + 8]);
    u64::from_be_bytes(raw)
}

/// Reads a big-endian `u32`.
fn read_u32(bytes: &[u8], at: usize) -> u32 {
    let mut raw = [0u8; 4];
    raw.copy_from_slice(&bytes[at..at + 4]);
    u32::from_be_bytes(raw)
}

/// Decodes one request frame.
///
/// The order is the point, and it is exactly [`DECODE_STEPS`]. Length, magic,
/// envelope version, verb and header check come first, then the generation
/// binding, then the payload bound — all before a single payload byte is
/// interpreted or copied. A hostile sender therefore cannot make the receiver
/// allocate for, or interpret, a frame it was never going to accept.
///
/// # Errors
///
/// Returns [`FrameError`] for a short or truncated frame, a wrong magic, an
/// unsupported envelope version, an unknown verb, a failed header check, a wrong
/// generation, a stale fence, an oversize declared payload, or a declared length
/// that disagrees with the real one.
pub fn decode_request<'a>(
    bytes: &'a [u8],
    expected: &FrameExpectation,
) -> Result<Frame<'a, RequestPreamble>, FrameError> {
    // 1. length
    if bytes.len() < REQUEST_PREAMBLE_LEN {
        return Err(FrameError::Malformed {
            at: "length",
            reason: format!("a request preamble is {REQUEST_PREAMBLE_LEN} bytes"),
        });
    }
    // 2. magic
    if bytes[..4] != REQUEST_MAGIC {
        return Err(FrameError::Malformed {
            at: "magic",
            reason: "not an AEXH frame".to_owned(),
        });
    }
    // 3. schema_version — no negotiation, no downgrade
    let version = u32::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    if version != expected.schema_version.0 {
        return Err(FrameError::UnsupportedVersion {
            expected: expected.schema_version.0,
            found: version,
        });
    }
    // 4. verb
    let Some(verb) = Verb::from_code(bytes[6]) else {
        return Err(FrameError::Malformed {
            at: "verb",
            reason: format!("verb {} is not one of the six", bytes[6]),
        });
    };
    // 5. header_crc32c
    let declared_check = read_u32(bytes, 36);
    let computed_check = crc32c(&bytes[..36]);
    if declared_check != computed_check {
        return Err(FrameError::Malformed {
            at: "header_crc32c",
            reason: format!("header check {declared_check:#010x} != {computed_check:#010x}"),
        });
    }
    // 6. generation
    let generation = read_generation(bytes).map_err(|error| FrameError::Malformed {
        at: "generation",
        reason: error.to_string(),
    })?;
    if generation != expected.generation {
        return Err(FrameError::WrongGeneration {
            expected: expected.generation,
            found: generation,
        });
    }
    // 7. fence
    let fence = Fence(read_u64(bytes, 24));
    if fence < expected.min_fence {
        return Err(FrameError::StaleFence {
            expected: expected.min_fence.0,
            found: fence.0,
        });
    }
    // 8. payload_len — before any allocation
    let payload_len = read_u32(bytes, 32);
    if payload_len > expected.max_frame_bytes {
        return Err(FrameError::Oversize {
            limit: expected.max_frame_bytes,
            actual: u64::from(payload_len),
        });
    }
    // 9. frame_length
    let declared_total = REQUEST_PREAMBLE_LEN as u64 + u64::from(payload_len);
    if bytes.len() as u64 != declared_total {
        return Err(FrameError::LengthMismatch {
            declared: declared_total,
            received: bytes.len() as u64,
        });
    }
    Ok(Frame {
        preamble: RequestPreamble {
            schema_version: expected.schema_version,
            verb,
            flags: bytes[7],
            generation,
            fence,
            payload_len,
        },
        payload: &bytes[REQUEST_PREAMBLE_LEN..],
    })
}

/// Decodes one response frame, in the same order as [`decode_request`].
///
/// # Errors
///
/// See [`decode_request`]. A response additionally fails when its four reserved
/// bytes are not zero: reserved space that is quietly ignored is reserved space
/// that becomes a compatibility hazard the first time someone uses it.
pub fn decode_response<'a>(
    bytes: &'a [u8],
    expected: &FrameExpectation,
) -> Result<Frame<'a, ResponsePreamble>, FrameError> {
    let preamble = decode_response_preamble(bytes, expected)?;
    let declared_total = RESPONSE_PREAMBLE_LEN as u64 + u64::from(preamble.payload_len);
    if bytes.len() as u64 != declared_total {
        return Err(FrameError::LengthMismatch {
            declared: declared_total,
            received: bytes.len() as u64,
        });
    }
    Ok(Frame {
        preamble,
        payload: &bytes[RESPONSE_PREAMBLE_LEN..],
    })
}

/// Decodes and validates only a response preamble.
///
/// A streaming client calls this as soon as the fixed 56-byte preamble arrives.
/// In particular, the declared payload bound is checked before that client
/// allocates a payload-sized buffer. [`decode_response`] uses the same function,
/// so the streaming and whole-frame paths cannot disagree about the protocol.
///
/// # Errors
///
/// Returns [`FrameError`] for a short preamble, wrong magic or version, unknown
/// verb or status, failed header check, non-zero reserved bytes, wrong generation,
/// stale fence, or oversized declared payload.
pub fn decode_response_preamble(
    bytes: &[u8],
    expected: &FrameExpectation,
) -> Result<ResponsePreamble, FrameError> {
    if bytes.len() < RESPONSE_PREAMBLE_LEN {
        return Err(FrameError::Malformed {
            at: "length",
            reason: format!("a response preamble is {RESPONSE_PREAMBLE_LEN} bytes"),
        });
    }
    if bytes[..4] != RESPONSE_MAGIC {
        return Err(FrameError::Malformed {
            at: "magic",
            reason: "not an AEXR frame".to_owned(),
        });
    }
    let version = u32::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    if version != expected.schema_version.0 {
        return Err(FrameError::UnsupportedVersion {
            expected: expected.schema_version.0,
            found: version,
        });
    }
    let Some(verb) = Verb::from_code(bytes[6]) else {
        return Err(FrameError::Malformed {
            at: "verb",
            reason: format!("verb {} is not one of the six", bytes[6]),
        });
    };
    let Some(status) = ResponseStatus::from_code(bytes[7]) else {
        return Err(FrameError::Malformed {
            at: "verb",
            reason: format!("status {} is neither payload nor error", bytes[7]),
        });
    };
    let declared_check = read_u32(bytes, 52);
    let computed_check = crc32c(&bytes[..52]);
    if declared_check != computed_check {
        return Err(FrameError::Malformed {
            at: "header_crc32c",
            reason: format!("header check {declared_check:#010x} != {computed_check:#010x}"),
        });
    }
    if bytes[48..52] != [0u8; 4] {
        return Err(FrameError::Malformed {
            at: "header_crc32c",
            reason: "the four reserved bytes must be zero".to_owned(),
        });
    }
    let generation = read_generation(bytes).map_err(|error| FrameError::Malformed {
        at: "generation",
        reason: error.to_string(),
    })?;
    if generation != expected.generation {
        return Err(FrameError::WrongGeneration {
            expected: expected.generation,
            found: generation,
        });
    }
    let fence = Fence(read_u64(bytes, 24));
    if fence < expected.min_fence {
        return Err(FrameError::StaleFence {
            expected: expected.min_fence.0,
            found: fence.0,
        });
    }
    let payload_len = read_u32(bytes, 32);
    if payload_len > expected.max_frame_bytes {
        return Err(FrameError::Oversize {
            limit: expected.max_frame_bytes,
            actual: u64::from(payload_len),
        });
    }
    let mut agent_build = [0u8; 8];
    agent_build.copy_from_slice(&bytes[40..48]);
    Ok(ResponsePreamble {
        schema_version: expected.schema_version,
        verb,
        status,
        generation,
        fence,
        payload_len,
        guest_revision: read_u32(bytes, 36),
        agent_build,
    })
}

/// Splits a `result` payload into its metadata and body halves.
///
/// `flags.bit0 == 1` means the payload is
/// `u32 meta_len ‖ meta_json[meta_len] ‖ body[rest]`.
///
/// # Errors
///
/// Returns [`FrameError::LengthMismatch`] when the declared metadata length does
/// not fit inside the payload.
pub fn split_result_payload(payload: &[u8]) -> Result<(&[u8], &[u8]), FrameError> {
    if payload.len() < 4 {
        return Err(FrameError::LengthMismatch {
            declared: 4,
            received: payload.len() as u64,
        });
    }
    let meta_len = read_u32(payload, 0) as usize;
    let end = 4usize
        .checked_add(meta_len)
        .filter(|end| *end <= payload.len())
        .ok_or(FrameError::LengthMismatch {
            declared: 4 + meta_len as u64,
            received: payload.len() as u64,
        })?;
    Ok((&payload[4..end], &payload[end..]))
}

/// Verifies an assembled terminal body against its declared length and digest.
///
/// Called after reassembly, never per chunk: a partial body has neither the
/// declared length nor the declared digest, so checking early would reject every
/// resumable pull.
///
/// # Errors
///
/// Returns [`FrameError::LengthMismatch`] or [`FrameError::DigestMismatch`]. The
/// error retains the diagnostic values and the body is never journalled.
pub fn verify_body(
    body: &[u8],
    declared_len: u64,
    declared_digest: ContentHash,
) -> Result<(), FrameError> {
    if body.len() as u64 != declared_len {
        return Err(FrameError::LengthMismatch {
            declared: declared_len,
            received: body.len() as u64,
        });
    }
    let computed = ContentHash::from_bytes(*blake3::hash(body).as_bytes());
    if computed != declared_digest {
        return Err(FrameError::DigestMismatch {
            declared: declared_digest,
            computed,
        });
    }
    Ok(())
}

/// The identity of one operation as the guest records it.
///
/// Brain mints [`HandsOperationId`] and persists [`CallHash`] before dispatch, so
/// the guest is a cooperative cache and never an authority: a customer with root
/// can delete the journal, and doing so fails their own operation and authorises
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperationIdentity {
    /// Which operation.
    pub operation: HandsOperationId,
    /// The canonical hash of the request Brain persisted before dispatch.
    pub call_hash: CallHash,
}

#[cfg(test)]
mod tests {
    use super::{
        DECODE_STEPS, FLAG_TRAILING_BINARY, Frame, FrameError, FrameExpectation, PROTOCOL_V1,
        REQUEST_PREAMBLE_LEN, RESPONSE_PREAMBLE_LEN, RequestPreamble, ResponsePreamble,
        ResponseStatus, Verb, decode_request, decode_response, decode_response_preamble,
        encode_request, encode_response, split_result_payload, verify_body,
    };
    use aex_hands_protocol::rpc::Fence;
    use aex_internal_contracts::SchemaVersion;
    use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, Uuid7};

    /// The payload length as the preamble field type.
    fn len32(bytes: &[u8]) -> u32 {
        u32::try_from(bytes.len()).expect("a test payload is small")
    }

    fn generation(tag: u64) -> GenerationId {
        let seed = u8::try_from(tag & 0xff).unwrap_or(0);
        GenerationId::from_uuid7(Uuid7::compose(tag, [seed; 10]))
    }

    fn expectation() -> FrameExpectation {
        FrameExpectation {
            generation: generation(1),
            min_fence: Fence(4),
            schema_version: PROTOCOL_V1,
            max_frame_bytes: 1_048_576,
        }
    }

    fn request(payload_len: u32) -> RequestPreamble {
        RequestPreamble {
            schema_version: PROTOCOL_V1,
            verb: Verb::Start,
            flags: 0,
            generation: generation(1),
            fence: Fence(4),
            payload_len,
        }
    }

    fn response(payload_len: u32) -> ResponsePreamble {
        ResponsePreamble {
            schema_version: PROTOCOL_V1,
            verb: Verb::Start,
            status: ResponseStatus::Payload,
            generation: generation(1),
            fence: Fence(4),
            payload_len,
            guest_revision: 7,
            agent_build: [1, 2, 3, 4, 5, 6, 7, 8],
        }
    }

    #[test]
    fn a_request_round_trips_byte_for_byte() {
        let payload = br#"{"hello":"world"}"#;
        let encoded = encode_request(&request(len32(payload)), payload);
        assert_eq!(encoded.len(), REQUEST_PREAMBLE_LEN + payload.len());
        assert_eq!(&encoded[..4], b"AEXH");
        let Frame {
            preamble,
            payload: decoded,
        } = decode_request(&encoded, &expectation()).expect("a well-formed frame decodes");
        assert_eq!(preamble, request(len32(payload)));
        assert_eq!(decoded, payload);
    }

    #[test]
    fn a_response_round_trips_and_carries_the_liveness_fields() {
        let payload = br#"{"response":"accepted"}"#;
        let encoded = encode_response(&response(len32(payload)), payload);
        assert_eq!(encoded.len(), RESPONSE_PREAMBLE_LEN + payload.len());
        assert_eq!(&encoded[..4], b"AEXR");
        let frame = decode_response(&encoded, &expectation()).expect("a well-formed frame decodes");
        assert_eq!(frame.preamble.guest_revision, 7);
        assert_eq!(frame.preamble.agent_build, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(frame.payload, payload);
    }

    #[test]
    fn a_streaming_reader_can_refuse_a_declared_oversize_from_the_preamble_alone() {
        let valid = encode_response(&response(1_048_576), &[]);
        let preamble = decode_response_preamble(&valid[..RESPONSE_PREAMBLE_LEN], &expectation())
            .expect("the exact ceiling is accepted before a body is allocated");
        assert_eq!(preamble.payload_len, 1_048_576);

        let oversize = encode_response(&response(1_048_577), &[]);
        assert!(matches!(
            decode_response_preamble(&oversize[..RESPONSE_PREAMBLE_LEN], &expectation()),
            Err(FrameError::Oversize {
                limit: 1_048_576,
                actual: 1_048_577
            })
        ));
    }

    #[test]
    fn there_are_exactly_six_verbs_with_fixed_paths() {
        assert_eq!(Verb::ALL.len(), 6);
        assert_eq!(
            Verb::ALL.map(Verb::code),
            [1, 2, 3, 4, 5, 6],
            "the wire codes are pinned"
        );
        assert_eq!(
            Verb::ALL.map(Verb::path),
            [
                "/aex/hands/v1/start",
                "/aex/hands/v1/status",
                "/aex/hands/v1/cancel",
                "/aex/hands/v1/result",
                "/aex/hands/v1/attach",
                "/aex/hands/v1/file",
            ]
        );
        for code in [0u8, 7, 255] {
            assert!(
                Verb::from_code(code).is_none(),
                "verb {code} does not exist"
            );
        }
    }

    /// The section 3.3 order test: each step is reachable and the later steps are
    /// unreachable once an earlier one fails.
    #[test]
    fn the_hostile_decode_order_is_exactly_the_pinned_order() {
        let payload = b"{}";
        let good = encode_request(&request(len32(payload)), payload);

        // 1. length — a truncated frame never reaches the magic check, which is
        // proven by feeding bytes whose magic is also wrong.
        let short = &[0u8; REQUEST_PREAMBLE_LEN - 1];
        assert!(matches!(
            decode_request(short, &expectation()),
            Err(FrameError::Malformed { at: "length", .. })
        ));

        // 2. magic — wrong magic wins over a wrong version, wrong verb, bad check,
        // wrong generation and a stale fence, all of which are also wrong here.
        let mut bad_magic = vec![0u8; REQUEST_PREAMBLE_LEN];
        bad_magic[6] = 99;
        assert!(matches!(
            decode_request(&bad_magic, &expectation()),
            Err(FrameError::Malformed { at: "magic", .. })
        ));

        // 3. schema_version wins over verb, header check and generation.
        let mut bad_version = good.clone();
        bad_version[5] = 9;
        bad_version[6] = 99;
        bad_version[8] ^= 0xff;
        assert!(matches!(
            decode_request(&bad_version, &expectation()),
            Err(FrameError::UnsupportedVersion {
                expected: 1,
                found: 9
            })
        ));

        // 4. verb wins over the header check and the generation.
        let mut bad_verb = good.clone();
        bad_verb[6] = 6;
        bad_verb[8] ^= 0xff;
        assert!(matches!(
            decode_request(&bad_verb, &expectation()),
            Err(FrameError::Malformed { at: "verb", .. })
        ));

        // 5. header_crc32c wins over the generation, the fence and the length.
        let mut bad_check = encode_request(&request(9_999), payload);
        bad_check[8] ^= 0xff;
        assert!(matches!(
            decode_request(&bad_check, &expectation()),
            Err(FrameError::Malformed {
                at: "header_crc32c",
                ..
            })
        ));

        // 6. generation wins over the fence, the payload bound and the length.
        let mut foreign = request(u32::MAX);
        foreign.generation = generation(2);
        foreign.fence = Fence(0);
        let foreign = encode_request(&foreign, payload);
        assert!(matches!(
            decode_request(&foreign, &expectation()),
            Err(FrameError::WrongGeneration { .. })
        ));

        // 7. fence wins over the payload bound and the length.
        let mut stale = request(u32::MAX);
        stale.fence = Fence(3);
        let stale = encode_request(&stale, payload);
        assert!(matches!(
            decode_request(&stale, &expectation()),
            Err(FrameError::StaleFence {
                expected: 4,
                found: 3
            })
        ));

        // 8. payload_len wins over the real frame length.
        let oversize = encode_request(&request(u32::MAX), payload);
        assert!(matches!(
            decode_request(&oversize, &expectation()),
            Err(FrameError::Oversize {
                limit: 1_048_576,
                actual: 4_294_967_295
            })
        ));

        // 9. frame_length is last.
        let mut truncated = good.clone();
        truncated.pop();
        assert!(matches!(
            decode_request(&truncated, &expectation()),
            Err(FrameError::LengthMismatch {
                declared: 42,
                received: 41
            })
        ));

        assert_eq!(DECODE_STEPS.len(), 9, "every pinned step has a case above");
    }

    #[test]
    fn a_single_flipped_header_bit_is_refused() {
        let payload = b"{}";
        let good = encode_request(&request(len32(payload)), payload);
        for index in 0..36 {
            for bit in 0..8 {
                let mut corrupt = good.clone();
                corrupt[index] ^= 1 << bit;
                assert!(
                    decode_request(&corrupt, &expectation()).is_err(),
                    "byte {index} bit {bit} flipped and still decoded"
                );
            }
        }
    }

    #[test]
    fn a_response_with_non_zero_reserved_bytes_is_refused() {
        let payload = b"{}";
        let mut encoded = encode_response(&response(len32(payload)), payload);
        encoded[48] = 1;
        // Recompute the header check so the reserved-byte rule is what fails.
        let check = crate::crc::crc32c(&encoded[..52]);
        encoded[52..56].copy_from_slice(&check.to_be_bytes());
        assert!(matches!(
            decode_response(&encoded, &expectation()),
            Err(FrameError::Malformed { .. })
        ));
    }

    #[test]
    fn a_higher_fence_is_accepted_so_the_guest_can_adopt_it() {
        let payload = b"{}";
        let mut ahead = request(len32(payload));
        ahead.fence = Fence(9);
        let encoded = encode_request(&ahead, payload);
        let frame = decode_request(&encoded, &expectation()).expect("a higher fence is accepted");
        assert_eq!(frame.preamble.fence, Fence(9));
        assert_eq!(frame.preamble.binding().fence, Fence(9));
    }

    #[test]
    fn a_result_payload_splits_into_metadata_and_body() {
        let meta = br#"{"state":"succeeded"}"#;
        let body = b"the deliverable";
        let mut payload = Vec::new();
        payload.extend_from_slice(&len32(meta).to_be_bytes());
        payload.extend_from_slice(meta);
        payload.extend_from_slice(body);

        let mut preamble = request(len32(&payload));
        preamble.verb = Verb::Result;
        preamble.flags = FLAG_TRAILING_BINARY;
        let encoded = encode_request(&preamble, &payload);
        let frame = decode_request(&encoded, &expectation()).expect("a result frame decodes");
        assert!(frame.preamble.has_trailing_binary());

        let (decoded_meta, decoded_body) =
            split_result_payload(frame.payload).expect("the split succeeds");
        assert_eq!(decoded_meta, meta);
        assert_eq!(decoded_body, body);
    }

    #[test]
    fn a_lying_metadata_length_is_refused_rather_than_read_past_the_payload() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&u32::MAX.to_be_bytes());
        payload.extend_from_slice(b"short");
        assert!(matches!(
            split_result_payload(&payload),
            Err(FrameError::LengthMismatch { .. })
        ));
        assert!(matches!(
            split_result_payload(&[1, 2]),
            Err(FrameError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn a_body_is_verified_by_length_and_then_digest_and_diagnostics_survive() {
        let body = b"the deliverable";
        let digest = ContentHash::from_bytes(*blake3::hash(body).as_bytes());
        assert_eq!(verify_body(body, body.len() as u64, digest), Ok(()));

        // A forged terminal claiming a huge body fails on length, not on digest,
        // so the receiver never hashes bytes it was never sent.
        assert!(matches!(
            verify_body(body, 10 * 1024 * 1024 * 1024, digest),
            Err(FrameError::LengthMismatch {
                declared: 10_737_418_240,
                received: 15
            })
        ));

        let wrong = ContentHash::from_bytes([0xab; 32]);
        let Err(FrameError::DigestMismatch { declared, computed }) =
            verify_body(body, body.len() as u64, wrong)
        else {
            panic!("a wrong digest must be refused");
        };
        assert_eq!(declared, wrong);
        assert_eq!(computed, digest);
    }

    #[test]
    fn an_envelope_version_the_receiver_does_not_understand_is_never_negotiated() {
        let payload = b"{}";
        let mut future = request(len32(payload));
        future.schema_version = SchemaVersion(2);
        let encoded = encode_request(&future, payload);
        assert!(matches!(
            decode_request(&encoded, &expectation()),
            Err(FrameError::UnsupportedVersion {
                expected: 1,
                found: 2
            })
        ));
        // And the other direction: an older sender is refused just as hard.
        let mut older = request(len32(payload));
        older.schema_version = SchemaVersion(0);
        let encoded = encode_request(&older, payload);
        assert!(matches!(
            decode_request(&encoded, &expectation()),
            Err(FrameError::UnsupportedVersion {
                expected: 1,
                found: 0
            })
        ));
    }
}
