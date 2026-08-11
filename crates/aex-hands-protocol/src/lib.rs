//! `aex-hands-protocol` is the exact-generation Brain-to-Hands RPC.
//!
//! Hands is customer root, so this crate is the hostile-input boundary. Every
//! decoder here is written on the assumption that the sender is adversarial.
//!
//! # The six verbs
//!
//! ```text
//! start(operation, call_hash, fence, bounds) -> accepted(operation, guest_revision)
//! status(operation)
//! cancel(operation, fence)
//! result(operation, from_offset, max_bytes) -> terminal metadata + checksummed bytes
//! ```
//!
//! This is a clean replacement, not a port. The retired surface —
//! `hello`/`execute`/`output`/`result`/`heartbeat`/`shutdown` frames keyed by
//! `callId = "<assistantSeq>:<toolUseId>"`, generation carried out of band on the
//! transport envelope, guest-stamped usage counters, and a guest-written S3
//! completion marker — has **no successor**. The old `callId` encoded a journal
//! position into a transport identity, so an operation became unaddressable
//! after a fold change; [`HandsOperationId`] is Brain-minted and independent.
//!
//! # Invariants
//!
//! - byte length is checked against `max_frame_bytes` before any allocation;
//! - [`GenerationBinding`] is validated before payload decode;
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
    FILE_PART_BYTES, FileDownloadChunk, FileDownloadId, FileDownloadState, FileFailureCode,
    FilePartReceipt, FileRequest, FileResponse, FileUploadState, MAX_FILE_BYTES,
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
    GenerationExpectation, GuestRevision, HandsOperationId, MessageDecodeError, OutputStream,
    ResultChunk, ResultRequest, ResultResponse, StartRequest, StartResponse, StatusRequest,
    StatusResponse, decode_agent_message,
};

pub use aex_wire::ids::GenerationId;
