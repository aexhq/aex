//! Finite regional session API and long-lived NDJSON stream, composed into one deployable.
//!
//! # Why one package
//!
//! Two Rust services, each with a production floor of two Fargate tasks, is four
//! tasks holding two idle listeners in every region. Both are regional
//! request-path edges on the same plane, with the same CPU and memory shape, the
//! same stop timeout, the same `central-authz` dependency and the same
//! authority tables. Merging them halves the regional floor.
//!
//! # What stayed separate, on purpose
//!
//! **The route contract.** `RouteOwner::SessionApi` and `RouteOwner::Stream` are
//! untouched, and the generated `servingArtifact` strings still say
//! `regional-session-api` and `regional-stream`. The two halves mount
//! independently and are joined with `Router::merge` over URL prefixes that were
//! already disjoint. Splitting them back apart is therefore a deployment change
//! and not a contract change — which is the property that made merging safe to
//! do in the first place.
//!
//! **The assertion audiences.** The session half admits
//! `AssertionAudience::RegionalSession` and the stream half
//! `AssertionAudience::RegionalStream`, exactly as before. The merged binary
//! runs two edges and selects between them by which route matched. There is no
//! combined audience: inventing one would reach into `central-authz` and
//! `aex-internal-contracts` and would let an assertion minted for one surface
//! admit a request to the other.
//!
//! **The write capability.** See [`capability`]. The session half's [`session::Stores`]
//! is the whole of what this process may mutate, it is unconstructible without a
//! `Grant<WorkClaim>`, and the stream half's state cannot name it.

pub mod capability;
pub mod config;
pub mod session;
pub mod stream;

pub use config::Config;
