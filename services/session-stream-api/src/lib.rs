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
//! **The route contract.** `RouteOwner::SessionApi` and `RouteOwner::Stream`
//! remain distinct mount strategies behind the one generated
//! `session-stream-api` artifact. Provider-credential registration now shares
//! that same public owner; its plaintext capability stays isolated behind the
//! narrow [`session::secret_registration::ProviderCredentialRegistration`]
//! port.
//!
//! **The assertion audiences.** The session half admits
//! `AssertionAudience::RegionalSession` and the stream half
//! `AssertionAudience::RegionalStream`, exactly as before. The merged binary
//! runs two edges and selects between them by which route matched. There is no
//! combined audience: inventing one would reach into `central-authz` and
//! `aex-internal-contracts` and would let an assertion minted for one surface
//! admit a request to the other.
//!
//! **The write capability.** See [`capability`]. The session half's
//! [`session::Stores`] is the durable adapter set, it is unconstructible without
//! a `Grant<WorkClaim>`, and the stream half's state cannot name it. Plaintext
//! admission is declared separately and exposes no reveal operation.

pub mod capability;
pub mod config;
pub mod frontier;
pub mod release_catalog;
pub mod session;
pub mod stream;

pub use config::Config;
