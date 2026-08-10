//! The five verbs and their frames.
//!
//! Every request and response carries a [`GenerationBinding`], and the binding is
//! checked *before* the payload is decoded. A frame for a stale generation is
//! therefore never parsed at all, which is the difference between rejecting a
//! stale write and rejecting it after having already allocated for it.

use aex_internal_contracts::SchemaVersion;
use aex_wire::Uuid7;
use aex_wire::ids::{ContentHash, GenerationId};
use aex_wire::types::{JsonPointer, Timestamp};
use serde::{Deserialize, Serialize};

use crate::operation::{
    DeliveryMode, OperationBounds, OperationFailure, OperationRequest, TerminalMetadata,
    TerminalState,
};

mod attached;

pub use attached::AttachResponse;

/// A Brain-assigned operation identity.
///
/// The guest never mints one, and it is not derived from a model tool-use id or
/// a journal position. The retired protocol derived it from a journal position,
/// which made an operation unaddressable the moment the fold changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandsOperationId(pub Uuid7);

/// The canonical hash of the request Brain persisted before dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CallHash(pub ContentHash);

/// A monotonic per-generation fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Fence(pub u64);

/// The guest incarnation counter; it increments on guest agent restart.
///
/// Brain uses it to tell a restarted supervisor from a live process. What Brain
/// does on a bump is policy, and policy is not this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GuestRevision(pub u32);

/// Which stream a diagnostic chunk came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// Why Brain asked for a cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    /// A customer stop operation.
    CustomerStop,
    /// The run exceeded its spend ceiling.
    SpendCeiling,
    /// The run exceeded its wall bound.
    Deadline,
    /// An approval was denied.
    ApprovalDenied,
    /// The session is being deleted.
    SessionDeleting,
}

/// The generation and fence every frame carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GenerationBinding {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// Which exact generation the frame belongs to.
    pub generation: GenerationId,
    /// The fence at the time the frame was written.
    pub fence: Fence,
}

/// What a receiver requires of an incoming frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationExpectation {
    /// The generation the receiver is bound to.
    pub generation: GenerationId,
    /// The lowest fence the receiver will still accept.
    pub min_fence: Fence,
    /// The largest frame the receiver will allocate for.
    pub max_frame_bytes: u32,
    /// The envelope version the receiver understands.
    pub schema_version: SchemaVersion,
}

/// Start one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StartRequest {
    /// The generation binding.
    pub binding: GenerationBinding,
    /// Which operation.
    pub operation: HandsOperationId,
    /// The hash of the request Brain persisted before dispatch.
    pub call_hash: CallHash,
    /// What to do.
    pub request: OperationRequest,
    /// Bounds the guest cannot raise.
    pub bounds: OperationBounds,
    /// After this instant the guest must not start.
    pub deadline: Timestamp,
    /// How the terminal body is delivered.
    pub delivery: DeliveryMode,
}

/// What the guest answers a `start` with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "response",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum StartResponse {
    /// Accepted. `existing` distinguishes a first start from an exact replay.
    Accepted {
        /// Which operation.
        operation: HandsOperationId,
        /// The guest incarnation that accepted it.
        guest_revision: GuestRevision,
        /// Whether this operation was already running under the same call hash.
        existing: bool,
    },
    /// The operation exists under a different call hash. Nothing was started.
    Conflict {
        /// Which operation.
        operation: HandsOperationId,
        /// The hash the guest already has.
        recorded_call_hash: CallHash,
    },
    /// The operation already reached a terminal state.
    AlreadyTerminal {
        /// Which operation.
        operation: HandsOperationId,
        /// How it ended.
        state: TerminalState,
    },
    /// The guest refused.
    Rejected {
        /// Which operation.
        operation: HandsOperationId,
        /// Why.
        failure: OperationFailure,
    },
}

/// Ask about one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StatusRequest {
    /// The generation binding.
    pub binding: GenerationBinding,
    /// Which operation.
    pub operation: HandsOperationId,
}

/// What the guest knows about one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum StatusResponse {
    /// The guest has never heard of it.
    Unknown {
        /// Which operation.
        operation: HandsOperationId,
    },
    /// Accepted, not started.
    Accepted {
        /// Which operation.
        operation: HandsOperationId,
        /// The guest incarnation.
        guest_revision: GuestRevision,
    },
    /// Running.
    Running {
        /// Which operation.
        operation: HandsOperationId,
        /// The guest incarnation.
        guest_revision: GuestRevision,
        /// When it started.
        started_at: Timestamp,
        /// A coarse phase name, when the operation reports one.
        phase: Option<String>,
        /// How many terminal bytes exist so far.
        produced_bytes: u64,
    },
    /// Terminal.
    Terminal {
        /// Which operation.
        operation: HandsOperationId,
        /// The guest incarnation.
        guest_revision: GuestRevision,
        /// How it ended.
        terminal: TerminalMetadata,
    },
}

/// Cancel one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CancelRequest {
    /// The generation binding.
    pub binding: GenerationBinding,
    /// Which operation.
    pub operation: HandsOperationId,
    /// Why.
    pub reason: CancelReason,
}

/// What the guest answers a `cancel` with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "response",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CancelResponse {
    /// Cancellation is in progress.
    Cancelling {
        /// Which operation.
        operation: HandsOperationId,
    },
    /// It had already finished.
    AlreadyTerminal {
        /// Which operation.
        operation: HandsOperationId,
        /// How it ended.
        terminal: TerminalMetadata,
    },
    /// The guest has never heard of it.
    Unknown {
        /// Which operation.
        operation: HandsOperationId,
    },
}

/// Pull terminal bytes, resumably.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResultRequest {
    /// The generation binding.
    pub binding: GenerationBinding,
    /// Which operation.
    pub operation: HandsOperationId,
    /// The first byte Brain has not incorporated.
    pub from_offset: u64,
    /// How many bytes Brain will accept in this pull.
    pub max_bytes: u64,
}

/// One chunk of a terminal body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResultChunk {
    /// Which operation.
    pub operation: HandsOperationId,
    /// Where the chunk starts.
    pub offset: u64,
    /// The bytes, base64-encoded on the wire.
    ///
    /// Base64 here, not serde's default integer array: a 180 KB pull chunk
    /// serialized as integers is ~650 KB — brushing the 1 MiB frame bound —
    /// where base64 is ~240 KB.
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
    /// Whether this is the final chunk.
    pub last: bool,
}

/// What the guest answers a `result` with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "response",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ResultResponse {
    /// Terminal, with the requested slice of the body.
    Terminal {
        /// How it ended.
        terminal: TerminalMetadata,
        /// The slice, absent when the caller asked past the end.
        chunk: Option<ResultChunk>,
    },
    /// Not terminal yet.
    NotTerminal {
        /// Which operation.
        operation: HandsOperationId,
        /// What the guest does know.
        state: Box<StatusResponse>,
    },
    /// The guest has never heard of it.
    Unknown {
        /// Which operation.
        operation: HandsOperationId,
    },
}

/// A diagnostic event on an attached connection.
///
/// Diagnostics only. The authoritative body is always pulled, so a lost attached
/// event never loses a result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AttachedEvent {
    /// Output as it happens.
    Output {
        /// Which operation.
        operation: HandsOperationId,
        /// Which stream.
        stream: OutputStream,
        /// The bytes, base64-encoded on the wire. See [`ResultChunk::bytes`].
        #[serde(with = "base64_bytes")]
        chunk: Vec<u8>,
        /// Whether the guest dropped some.
        truncated: bool,
    },
    /// A coarse phase change.
    Progress {
        /// Which operation.
        operation: HandsOperationId,
        /// The phase name.
        phase: String,
    },
    /// The operation reached a terminal state.
    Terminal {
        /// Which operation.
        operation: HandsOperationId,
        /// How it ended.
        terminal: TerminalMetadata,
    },
}

/// The sealed set of frames a guest may send.
///
/// Sealing it here is what lets either framing — length-prefixed or otherwise —
/// be expressed without changing a single byte of the payload.
pub trait HandsMessage: serde::de::DeserializeOwned + Sized {
    /// The verb this frame belongs to.
    const VERB: &'static str;
}

impl HandsMessage for StartResponse {
    const VERB: &'static str = "start";
}

impl HandsMessage for StatusResponse {
    const VERB: &'static str = "status";
}

impl HandsMessage for CancelResponse {
    const VERB: &'static str = "cancel";
}

impl HandsMessage for ResultResponse {
    const VERB: &'static str = "result";
}

impl HandsMessage for AttachedEvent {
    const VERB: &'static str = "attached";
}

/// Why a frame from the guest was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageDecodeError {
    /// The frame exceeded `max_frame_bytes`. Checked before any allocation.
    #[error("frame is {actual} bytes, limit is {limit}")]
    Oversize {
        /// The receiver bound.
        limit: u32,
        /// What arrived.
        actual: usize,
    },
    /// The frame belongs to a different generation.
    #[error("frame is for generation {found}, expected {expected}")]
    WrongGeneration {
        /// The generation the receiver is bound to.
        expected: GenerationId,
        /// The generation on the frame.
        found: GenerationId,
    },
    /// The frame is older than the receiver will accept.
    #[error("frame fence {found} is older than {expected}")]
    StaleFence {
        /// The lowest accepted fence.
        expected: u64,
        /// The fence on the frame.
        found: u64,
    },
    /// The frame declares an envelope version the receiver does not understand.
    #[error("unsupported schema version {found}")]
    UnsupportedVersion {
        /// The version on the frame.
        found: u32,
    },
    /// Assembled bytes did not match the declared digest.
    #[error("digest mismatch: declared {declared}, computed {computed}")]
    DigestMismatch {
        /// What the guest declared.
        declared: ContentHash,
        /// What Brain computed.
        computed: ContentHash,
    },
    /// Assembled bytes did not match the declared length.
    #[error("length mismatch: declared {declared}, received {received}")]
    LengthMismatch {
        /// What the guest declared.
        declared: u64,
        /// What Brain received.
        received: u64,
    },
    /// The frame was not decodable at all.
    #[error("malformed frame at `{pointer}`: {reason}")]
    Malformed {
        /// Where it failed.
        pointer: JsonPointer,
        /// Why.
        reason: String,
    },
}

/// Body bytes as a padded standard-alphabet base64 string on the wire.
///
/// The same convention as `aex-model-catalog`'s signature payloads, restated
/// here rather than imported: this crate is in the credential-free guest
/// closure and takes no dependency on a catalog crate for one encoder.
mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize as _, Deserializer, Serializer};

    /// Serializes bytes as a padded standard-alphabet base64 string.
    ///
    /// # Errors
    ///
    /// Propagates the serializer's own failure only.
    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(value))
    }

    /// Deserializes a padded standard-alphabet base64 string.
    ///
    /// # Errors
    ///
    /// Returns a deserializer error for a malformed encoding.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        use serde::de::Error as _;
        let raw = String::deserialize(deserializer)?;
        STANDARD.decode(raw.as_bytes()).map_err(D::Error::custom)
    }
}

/// The generation binding a frame declares, read without decoding the payload.
#[derive(Debug, Deserialize)]
struct BindingPreamble {
    /// The binding.
    binding: GenerationBinding,
}

/// Decodes one frame from the guest.
///
/// The order is the point. Length is checked before allocation, the envelope
/// version and generation binding are checked before the payload is decoded, and
/// only then is the message parsed. A hostile guest therefore cannot make Brain
/// allocate for, or interpret, a frame it was never going to accept.
///
/// # Errors
///
/// Returns [`MessageDecodeError`] for an oversize frame, an unsupported envelope
/// version, a wrong generation, a stale fence, or a malformed payload.
pub fn decode_agent_message<T: HandsMessage>(
    bytes: &[u8],
    expected: &GenerationExpectation,
) -> Result<T, MessageDecodeError> {
    if bytes.len() > expected.max_frame_bytes as usize {
        return Err(MessageDecodeError::Oversize {
            limit: expected.max_frame_bytes,
            actual: bytes.len(),
        });
    }
    let preamble: BindingPreamble =
        serde_json::from_slice(bytes).map_err(|error| MessageDecodeError::Malformed {
            pointer: JsonPointer::root().child("binding"),
            reason: error.to_string(),
        })?;
    if preamble.binding.schema_version != expected.schema_version {
        return Err(MessageDecodeError::UnsupportedVersion {
            found: preamble.binding.schema_version.0,
        });
    }
    if preamble.binding.generation != expected.generation {
        return Err(MessageDecodeError::WrongGeneration {
            expected: expected.generation,
            found: preamble.binding.generation,
        });
    }
    if preamble.binding.fence < expected.min_fence {
        return Err(MessageDecodeError::StaleFence {
            expected: expected.min_fence.0,
            found: preamble.binding.fence.0,
        });
    }
    serde_json::from_slice(bytes).map_err(|error| MessageDecodeError::Malformed {
        pointer: JsonPointer::root(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{AttachedEvent, HandsOperationId, OutputStream, ResultChunk};
    use aex_wire::Uuid7;

    fn operation() -> HandsOperationId {
        HandsOperationId(Uuid7::compose(3, [3; 10]))
    }

    #[test]
    fn chunk_bytes_ride_the_wire_as_base64_and_never_as_integer_arrays() {
        let chunk = ResultChunk {
            operation: operation(),
            offset: 4,
            bytes: vec![0, 159, 146, 150],
            last: true,
        };
        let encoded = serde_json::to_value(&chunk).expect("it serializes");
        assert_eq!(
            encoded["bytes"],
            serde_json::Value::String("AJ+Slg==".to_owned()),
            "padded standard-alphabet base64, working for arbitrary non-UTF-8 bytes"
        );
        let decoded: ResultChunk = serde_json::from_value(encoded).expect("it round-trips");
        assert_eq!(decoded, chunk);

        let event = AttachedEvent::Output {
            operation: operation(),
            stream: OutputStream::Stdout,
            chunk: b"hello".to_vec(),
            truncated: false,
        };
        let encoded = serde_json::to_value(&event).expect("it serializes");
        assert_eq!(
            encoded["chunk"],
            serde_json::Value::String("aGVsbG8=".to_owned())
        );
        let decoded: AttachedEvent = serde_json::from_value(encoded).expect("it round-trips");
        assert_eq!(decoded, event);
    }

    #[test]
    fn a_malformed_base64_chunk_is_refused_at_decode() {
        let raw = serde_json::json!({
            "operation": operation(),
            "offset": 0,
            "bytes": "not base64!!",
            "last": true,
        });
        assert!(serde_json::from_value::<ResultChunk>(raw).is_err());
    }

    #[test]
    fn a_result_pull_chunk_stays_well_inside_the_frame_bound() {
        // 180 KB of body as an integer array is ~650 KB — brushing the 1 MiB
        // frame bound before the rest of the response is even counted. Base64
        // keeps it at 4/3 plus padding.
        let chunk = ResultChunk {
            operation: operation(),
            offset: 0,
            bytes: vec![0xAB; 180_000],
            last: true,
        };
        let encoded = serde_json::to_string(&chunk).expect("it serializes");
        assert!(
            encoded.len() < 250_000,
            "a 180 KB chunk must stay ~240 KB on the wire, found {}",
            encoded.len()
        );
    }
}
