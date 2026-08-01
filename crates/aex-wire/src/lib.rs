//! `aex-wire` is the public v1 wire vocabulary: validated identifier newtypes,
//! the closed error vocabulary, the request and response models, the route
//! registry, and their deterministic codecs.
//!
//! It is the base crate of the contract layer. `aex-internal-contracts`,
//! `aex-payment-contracts` and `aex-hands-protocol` all depend on it for shared
//! primitives; nothing depends the other way.
//!
//! # Invariants
//!
//! - every public identifier is a validated newtype, never a bare `String`, and
//!   an id of one kind never parses as another;
//! - every request decoder rejects unknown members, so a typo is a `400` and not
//!   a silently dropped field;
//! - no quantity the wire renders as a decimal string is ever a JSON number, so
//!   a `JavaScript` client cannot lose precision above 2^53;
//! - serialization is byte-deterministic under RFC 8785 JCS, which is what makes
//!   idempotency identity and the golden corpus meaningful.
//!
//! # Not this crate's job
//!
//! - routing, transport, authentication or middleware (`aex-central-http`,
//!   `aex-regional-http`);
//! - retry, backoff, clocks or randomness: a client method never sleeps, never
//!   retries and never reads a clock, because policy belongs to the SDK and the
//!   CLI;
//! - business rules, state machines or the policy of any authority crate.
//!
//! Everything under `src/generated/` is produced by `tools/aex-contract-gen`
//! from `api/`. Edit the authored tree, not the output; a hand edit fails
//! `cargo test -p aex-contract-gen`.

mod generated;

pub mod canonical;
pub mod cursor;
pub mod error;
pub mod idempotency;
pub mod ids;
pub mod limits;
pub mod page;
pub mod provider;
pub mod routes;
pub mod scopes;
pub mod types;

/// The generated public request and response models.
pub mod models {
    pub use crate::generated::models::*;
}

#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "testing")]
pub mod testing;

pub use canonical::{CanonicalJson, intent_digest, to_jcs_bytes};
pub use cursor::Cursor;
pub use error::{ApiError, ApiErrorBody, ErrorCode, ObservedErrorCode, WireError, WireResult};
pub use ids::{ContentHash, FilePath, PrefixedId, ResourceName, SpanId, TraceId, Uuid7};
pub use page::Page;
pub use types::{
    ByteRange, Cents, ComputeSize, DecimalU128, ETag, HttpMethod, HttpsUrl, JsonPointer,
    MetadataValue, Region, RequestId, Timestamp, ValueError,
};
