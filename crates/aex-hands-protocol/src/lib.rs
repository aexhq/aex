//! `aex-hands-protocol` is the exact-generation Brain-to-Hands RPC.
//!
//! Hands is customer root, so this crate is the hostile-input boundary. Every
//! decoder here is written on the assumption that the sender is adversarial.
//!
//! # The six verbs
//!
//! ```text
//! start(operation, call_hash, fence, bounds) -> accepted(operation)
//! status(operation)
//! cancel(operation, fence)
//! result(operation, from_offset, max_bytes) -> terminal metadata + checksummed bytes
//! ```
//!
//! This is a clean replacement, not a port. The retired surface —
//! `hello`/`execute`/`output`/`result`/`heartbeat`/`shutdown` messages keyed by
//! `callId = "<assistantSeq>:<toolUseId>"`, generation carried out of band on the
//! the request envelope, guest-stamped usage counters, and a guest-written S3
//! completion marker — has **no successor**. The old `callId` encoded a journal
//! position into a transport identity, so an operation became unaddressable
//! after a fold change; [`HandsOperationId`] is Brain-minted and independent.
//!
//! # Invariants
//!
//! - the HTTP body is bounded before typed JSON decode;
//! - [`GenerationBinding`] is validated before operation dispatch;
//! - a repeated `start` with an equal `(operation, call_hash)` returns
//!   `Accepted { existing: true }`; an unequal hash returns `Conflict` and starts
//!   no process;
//! - there is no guest-reported usage type at all. Customer root can falsify any
//!   in-guest counter, so `RuntimeReceipt` is constructible only from a provider
//!   control-plane response, asserted by `tests/no_guest_billing.rs`.

pub mod files;
pub mod lifecycle;
pub mod operation;
pub mod rpc;

pub use files::{
    FILE_FRAME_BYTES, FILE_TRANSFER_PART_BYTES, FileDownloadChunk, FileDownloadId,
    FileDownloadState, FileFailureCode, FilePartReceipt, FileRequest, FileResponse, FileUploadId,
    FileUploadState, LiveFileEntry, LiveFileEntryKind, LiveFileListing, MAX_FILE_BYTES,
    MAX_FILE_LIST_ENTRIES,
};
pub use lifecycle::{
    KeepaliveLease, LifecycleIntent, LifecycleOutcome, ProviderFailure, ProviderReceiptId,
    ProviderRequestId, ReceiptError, RuntimeReceipt, TrueIdleEvidence,
};
pub use operation::{
    ByteRangeRequest, DeliveryMode, EnvName, EnvValue, FileMode, GuestPath, GuestPathError,
    GuestProcessId, GuestRoot, OperationBounds, OperationExit, OperationFailure, OperationRequest,
    Patch, PatchHunk, SearchPattern, StopSignal, TerminalMetadata, TerminalState,
};
pub use rpc::{
    AttachedEvent, CallHash, CancelReason, CancelRequest, CancelResponse, Fence, GenerationBinding,
    GuestRequest, GuestResponse, HandsOperationId, MAX_GUEST_BODY_BYTES, MAX_RESULT_CHUNK_BYTES,
    OutputStream, PROTOCOL_V1, ResultChunk, ResultRequest, ResultResponse, StartRequest,
    StartResponse, StatusRequest, StatusResponse, Verb,
};

pub use aex_wire::ids::GenerationId;
