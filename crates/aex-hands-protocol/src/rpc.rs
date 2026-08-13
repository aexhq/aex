//! The bounded JSON verbs between Tool Mux and one Hands guest.
//!
//! Every request carries a [`GenerationBinding`] in one [`GuestRequest`] envelope.
//! The HTTP adapter bounds the complete body before decoding this envelope, then
//! checks the binding before dispatching its typed payload. Every successful
//! response carries the generation and fence observed after dispatch in a
//! [`GuestResponse`]; HTTP already owns message framing.

use aex_internal_contracts::SchemaVersion;
use aex_wire::Uuid7;
use aex_wire::ids::{ContentHash, GenerationId};
use aex_wire::types::Timestamp;
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

/// The exact maximum encoded request or response body.
pub const MAX_GUEST_BODY_BYTES: usize = 1_048_576;

/// The largest raw terminal-body chunk carried by one JSON response.
///
/// Base64 and the typed response metadata keep this comfortably below
/// [`MAX_GUEST_BODY_BYTES`].
pub const MAX_RESULT_CHUNK_BYTES: u64 = 180_000;

/// The launch protocol version.
pub const PROTOCOL_V1: SchemaVersion = SchemaVersion::V1;

/// The guest HTTP verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verb {
    /// Prove protocol, bounds, capabilities, and exact generation before use.
    Hello,
    /// Start one operation.
    Start,
    /// Ask about one operation.
    Status,
    /// Cancel one operation.
    Cancel,
    /// Pull terminal bytes resumably.
    Result,
    /// Hold one request until a short operation answers.
    Attach,
    /// Transfer binary live-workspace files.
    File,
}

impl Verb {
    /// Every verb, in route order.
    pub const ALL: [Self; 7] = [
        Self::Hello,
        Self::Start,
        Self::Status,
        Self::Cancel,
        Self::Result,
        Self::Attach,
        Self::File,
    ];

    /// The fixed versioned path this verb is posted to.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Hello => "/aex/hands/v1/hello",
            Self::Start => "/aex/hands/v1/start",
            Self::Status => "/aex/hands/v1/status",
            Self::Cancel => "/aex/hands/v1/cancel",
            Self::Result => "/aex/hands/v1/result",
            Self::Attach => "/aex/hands/v1/attach",
            Self::File => "/aex/hands/v1/file",
        }
    }
}

/// Empty exact-generation handshake request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HelloRequest {}

/// Exact bounds and capabilities a guest proves before Tool Mux dispatches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HelloResponse {
    /// Protocol revision implemented by this guest.
    pub protocol_version: SchemaVersion,
    /// Largest accepted encoded request or response body.
    pub max_body_bytes: u64,
    /// Largest result pull window.
    pub max_result_chunk_bytes: u64,
    /// Whether structured official filesystem operations are installed.
    pub filesystem_tools: bool,
    /// Whether the explicit shell/exec operation is installed.
    pub bash: bool,
    /// Whether registered processes can host sandbox MCP.
    pub sandbox_process_mcp: bool,
}

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

/// The generation and fence every request envelope carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GenerationBinding {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// Which exact generation the request belongs to.
    pub generation: GenerationId,
    /// The fence at the time the request was written.
    pub fence: Fence,
}

/// One bounded request to a guest HTTP verb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GuestRequest<T> {
    /// Exact generation, version and monotonic lifecycle fence.
    pub binding: GenerationBinding,
    /// The request specific to the route.
    pub request: T,
}

/// One bounded successful response from a guest HTTP verb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GuestResponse<T> {
    /// Exact generation, version and monotonic lifecycle fence observed by the guest.
    pub binding: GenerationBinding,
    /// The response specific to the route.
    pub response: T,
}

/// Start one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StartRequest {
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
    },
    /// Running.
    Running {
        /// Which operation.
        operation: HandsOperationId,
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
        /// How it ended.
        terminal: TerminalMetadata,
    },
}

/// Cancel one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CancelRequest {
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
    /// serialized as integers is ~650 KB — brushing the 1 MiB body bound —
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

#[cfg(test)]
mod tests {
    use super::{
        AttachedEvent, Fence, GenerationBinding, GuestRequest, GuestResponse, HandsOperationId,
        OutputStream, PROTOCOL_V1, ResultChunk, StatusRequest, StatusResponse, Verb,
    };
    use aex_wire::Uuid7;
    use aex_wire::ids::{GenerationId, PrefixedId as _};

    fn operation() -> HandsOperationId {
        HandsOperationId(Uuid7::compose(3, [3; 10]))
    }

    #[test]
    fn the_protocol_owns_one_stable_route_for_every_verb() {
        assert_eq!(
            Verb::ALL.map(Verb::path),
            [
                "/aex/hands/v1/hello",
                "/aex/hands/v1/start",
                "/aex/hands/v1/status",
                "/aex/hands/v1/cancel",
                "/aex/hands/v1/result",
                "/aex/hands/v1/attach",
                "/aex/hands/v1/file",
            ]
        );
    }

    #[test]
    fn a_typed_request_round_trips_with_its_generation_fence() {
        let request = GuestRequest {
            binding: GenerationBinding {
                schema_version: PROTOCOL_V1,
                generation: GenerationId::from_uuid7(Uuid7::compose(4, [5; 10])),
                fence: Fence(7),
            },
            request: StatusRequest {
                operation: operation(),
            },
        };
        let encoded = serde_json::to_vec(&request).expect("it serializes");
        let decoded: GuestRequest<StatusRequest> =
            serde_json::from_slice(&encoded).expect("it deserializes");
        assert_eq!(decoded, request);

        let mut with_unknown: serde_json::Value =
            serde_json::from_slice(&encoded).expect("it is JSON");
        with_unknown["unexpected"] = serde_json::Value::Bool(true);
        assert!(
            serde_json::from_value::<GuestRequest<StatusRequest>>(with_unknown).is_err(),
            "the bounded envelope rejects fields outside its public contract"
        );
    }

    #[test]
    fn a_typed_response_round_trips_with_its_generation_fence() {
        let response = GuestResponse {
            binding: GenerationBinding {
                schema_version: PROTOCOL_V1,
                generation: GenerationId::from_uuid7(Uuid7::compose(4, [6; 10])),
                fence: Fence(8),
            },
            response: StatusResponse::Unknown {
                operation: operation(),
            },
        };
        let encoded = serde_json::to_vec(&response).expect("it serializes");
        let decoded: GuestResponse<StatusResponse> =
            serde_json::from_slice(&encoded).expect("it deserializes");
        assert_eq!(decoded, response);
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
    fn a_result_pull_chunk_stays_well_inside_the_body_bound() {
        // 180 KB of body as an integer array is ~650 KB — brushing the 1 MiB
        // body bound before the rest of the response is even counted. Base64
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
