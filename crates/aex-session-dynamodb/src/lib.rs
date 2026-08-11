//! `aex-session-dynamodb` owns the `session-authority` table adapter and, with
//! it, the machinery every regional `DynamoDB` adapter shares: key components,
//! attribute codecs, item measurement, the transaction plan compiler, the store
//! error vocabulary, bounded paging and the idempotent-replay combinator.
//!
//! There is exactly one transaction compiler in the workspace and it lives here.
//! Seven compilers would be seven chances to forget
//! `ReturnValuesOnConditionCheckFailure`, seven chances to write an
//! unconditional authority row, and seven different cancellation decodings.
//!
//! # Invariants
//!
//! - every mutation carries a condition expression; [`plan::TransactionPlan`]
//!   refuses an unconditional authority write rather than trusting review
//! - a transaction names every participant it touches, so a cancellation maps
//!   back to a reason rather than to an index
//! - every authority point read is strongly consistent
//! - decoding never coerces: an unexpected `itemType`, a missing attribute or a
//!   row from another tenant is an error, not a default
//! - `TTL` is never a fence; every `TTL`'d row also carries the explicit
//!   `expiresAt` the reader checks
//!
//! # Not this crate's job
//!
//! - the session fold, its invariants or any policy (`aex-session-domain`)
//! - runnable work and claims (`aex-work-dynamodb`), content
//!   (`aex-content-dynamodb`), secrets (`aex-secret-custody-dynamodb`)
//! - `HTTP`, authentication or queue publication
//!
//! # Features
//!
//! - `session-authority` (default): the key templates, row codecs and
//!   transactions for `session-authority`.
//! - `authz-projection` (default): the read-only `regional-authz-projection`
//!   reader. A deployable that needs only the projection depends on this crate
//!   with `default-features = false, features = ["authz-projection"]` and links
//!   no session write symbol.
//! - `authz-projection-write`: central control's placement, profile and
//!   revocation producer.
//! - `capacity-limit-projection-write`: the regional capacity authority's
//!   effective-limit transport. It owns no defaults or override policy.
//! - `create-preparation`: the private synchronous-create election and exact
//!   selected-file metadata authority.

#[cfg(feature = "session-authority")]
pub mod app_authority;
#[cfg(feature = "session-authority")]
pub mod application_plan;
pub mod attr;
#[cfg(feature = "session-authority")]
pub mod authority_codec;
pub mod component;
#[cfg(feature = "create-preparation")]
pub mod create_preparation;
#[cfg(feature = "session-authority")]
pub mod deletion;
pub mod error;
pub mod measure;
pub mod paging;
pub mod plan;
pub mod replay;
pub mod stream_keys;

#[cfg(feature = "session-authority")]
pub mod codec;
#[cfg(any(feature = "session-authority", feature = "authz-projection"))]
pub mod event;
#[cfg(any(feature = "session-authority", feature = "create-preparation"))]
pub mod keys;
#[cfg(feature = "session-authority")]
pub mod regional_control;
#[cfg(feature = "session-authority")]
pub mod runtime_effects;
#[cfg(feature = "session-authority")]
pub mod store;
#[cfg(feature = "session-authority")]
pub mod transactions;
// The placeholder row types are shared by the session codecs and by the
// projection reader, so the module follows either feature rather than one of
// them. Gating it on `session-authority` alone made `default-features = false,
// features = ["authz-projection"]` — the exact composition D-21 exists to
// permit — fail to compile, which nothing noticed because the default set turns
// both on.
#[cfg(any(
    feature = "session-authority",
    feature = "authz-projection",
    feature = "authz-projection-write",
    feature = "capacity-limit-projection-write"
))]
pub mod wire_pending;

#[cfg(feature = "authz-projection")]
pub mod projection;

#[cfg(any(
    feature = "authz-projection",
    feature = "capacity-limit-projection-write"
))]
mod projection_limit;

/// Central projection builds deliberately cannot name the capacity producer.
/// The feature-isolation CI lane runs this doctest with only
/// `authz-projection-write`; enabling the capacity feature through dependency
/// unification makes the snippet compile and therefore makes the lane fail.
///
/// ```compile_fail
/// use aex_session_dynamodb::capacity_limit_projection_write::{
///     CapacityLimitProjectionWriter, LimitWrite,
/// };
/// ```
#[cfg(feature = "authz-projection-write")]
pub mod projection_write;

#[cfg(feature = "capacity-limit-projection-write")]
pub mod capacity_limit_projection_write;

pub use attr::{CodecError, Item};
pub use component::{Component, KeyError};
pub use error::{Idempotence, Resolution, RetryPolicy, StoreError};
pub use plan::{Participant, RegionalTables, TransactionPlan};
pub use replay::{Backoff, ReceiptStore, Replayable, commit_or_replay};
