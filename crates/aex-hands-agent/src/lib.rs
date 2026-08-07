//! `aex-hands-agent` owns the credential-free guest-root protocol agent: the command/result
//! journal, the process tree, reconnect, cancel and crash observation.
//!
//! It also owns the exact-generation frame codec, which both sides of the wire
//! share.
//!
//! # Invariants
//!
//! - the agent holds no `AEX` credential, role or private route; it is not a security
//!   authority
//! - a reconnect replays from the journal rather than losing or duplicating a command result
//! - a killed process tree is reported as observed, not inferred
//! - the generation binding is validated before any payload byte is interpreted, and the
//!   payload bound before any allocation
//! - `meta.json` is fsynced before a fork, and `terminal.json` is written once
//!
//! # What this is, and what it is not
//!
//! It is a **usability component, not a trust boundary**. H-BOUNDARY grants the customer
//! real root inside the `MicroVM`, so nothing this crate reports is authority:
//! guest-reported completion is customer-controlled observation that Brain bounds, digests
//! and stores as tool output. Killing or rewriting the agent fails the customer's own
//! operation and authorises nothing. The real ceilings live outside the guest — the
//! provider shape, the network bandwidth and connection caps, the eight-hour lifetime, the
//! operation and generation duration bounds, the open-operation count and the keepalive
//! maximum.
//!
//! # Not this crate's job
//!
//! - being a security boundary: isolation is the runtime's job, not the agent's
//! - the protocol definition (`aex-hands-protocol`)
//! - the tool executors (`aex-hands-tools`)
//! - runtime lifecycle decisions (`aex-runtime-control`)

pub mod boot;
pub mod capture;
pub mod crc;
pub mod image_contract;
pub mod journal;
pub mod session;
pub mod wire;

pub use boot::{
    BROWSER_CAPABILITY, GUEST_ROOT, GUEST_ROOT_VAR, JOURNAL_ROOT, JOURNAL_ROOT_VAR, LISTEN_ADDR,
    LISTEN_ADDR_VAR, REQUIRED_VARS, RUN_HOOK_KEYS, RunHook, RunHookBounds,
};
pub use capture::{ATTACH_STREAM_CAP_BYTES, Capture, CaptureOutcome, MirrorChunk};
pub use crc::crc32c;
pub use image_contract::{
    AGENT_PATH, AGENT_SBOM_PATH, FORBIDDEN_ROOTFS_PATHS, IMAGE_LOCK_PATH, ImageLock, LockVerdict,
    ROOTFS_CONTRACT, RPM_LIST_PATH, RootfsEntry, SBOM_DIR,
};
pub use journal::{
    DIRECTORY_SYNC_AVAILABLE, GuestBinding, Journal, JournalError, OperationMeta, ProcessProbe,
    ProcessRecord, ReplayEntry, ReplayVerdict,
};
pub use session::{
    CANCEL_GRACE_MS, CANCEL_REAP_MS, CancelStep, HEALTHZ_PATH, LifecycleHook, READYZ_PATH,
    StartDecision, StartInput, StartRefusal, Supervisor, cancel_step, empty_digest,
    interrupted_terminal,
};
pub use wire::{
    DECODE_STEPS, FLAG_TRAILING_BINARY, Frame, FrameError, FrameExpectation, PROTOCOL_V1,
    REQUEST_MAGIC, REQUEST_PREAMBLE_LEN, RESPONSE_MAGIC, RESPONSE_PREAMBLE_LEN, RequestPreamble,
    ResponsePreamble, ResponseStatus, Verb, decode_request, decode_response, encode_request,
    encode_response, split_result_payload, verify_body,
};
