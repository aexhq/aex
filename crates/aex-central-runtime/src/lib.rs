//! `aex-central-runtime` holds the central plane's non-Aurora port adapters.
//!
//! Everything here is a port the two central APIs and the control worker must
//! bind before they can serve: the ambient sources a domain is forbidden to
//! read, the credential pepper, the regional control authority and the
//! invitation notification.
//!
//! # Invariants
//!
//! - pepper material never reaches a log, an error body, a `Debug` rendering or
//!   telemetry; [`pepper`] carries the tests that say so
//! - a pepper version is resolved from the lifecycle table and fetched by that
//!   exact Secrets Manager version id, so a verifier reads the version its row
//!   names rather than whatever the secret holds now
//! - an unknown cross-plane outcome is [`aex_control_app::ports::EffectError::Unknown`]
//!   and never a timeout-inferred failure
//! - no adapter here reads the environment; every deployable passes what it
//!   resolved
//!
//! # Not this crate's job
//!
//! - SQL and transactions (`aex-control-aurora`, `aex-identity-aurora`)
//! - HTTP, status codes or headers (`aex-central-http`)
//! - choosing an email vendor: [`mail`] writes a durable intent and
//!   `central-control-worker` owns delivery

pub mod ambient;
pub mod directory;
pub mod mail;
pub mod pepper;
pub mod regional;

pub use ambient::{OsSecretRng, SystemClock, Uuid7Factory};
pub use directory::{DataApiPepperDirectory, PepperStatements};
pub use mail::{OutboxMailer, OutboxWriter, PendingNotification};
pub use pepper::{
    PepperDirectory, PepperRecord, PepperState, SecretsManagerPepperKeystore, pepper_cache_bound,
};
pub use regional::LambdaRegionalControl;
