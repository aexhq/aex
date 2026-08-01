//! The one shared substrate for every AEX test run that touches something it
//! must later clean up.
//!
//! `15-test-architecture.md` D-01: the cleanup ledger was declared in five
//! places across four plans with three conflicting homes. It lives here, once,
//! together with the run identity, resource prefix, budget, TTL, secret canary,
//! fault ports and the pinned container-image registry that every one of those
//! declarations also needed. `aex-central-test-support`,
//! `aex-regional-test-support`, `aex-brain-test-support` and
//! `aex-observation-test-support` re-export these types; they never define a
//! second one.
//!
//! # Invariants
//!
//! - a missing prerequisite is a failure, never a skip: [`required_env`] panics
//!   with the variable name rather than returning `None`;
//! - a resource that can outlive the run is recorded in the [`ledger`] *before*
//!   the create call returns, so residue is detectable even when the test
//!   process dies;
//! - a container image is referenced by digest or not at all
//!   ([`images::ImageError::Unpinned`]), and a started engine is waited for by
//!   a signal it emits itself, never by a sleep ([`containers::Readiness`]);
//! - the run's [`canary::SecretCanary`] value is never printed, not even in the
//!   failure message that reports a leak - only its location is.
//!
//! # Not this crate's job
//!
//! - product fixtures: those stay in the four `*-test-support` crates;
//! - talking to AWS: this crate links no SDK, so an integration target builds
//!   its own client against the endpoint a [`containers`] handle exposes;
//! - starting a container on the unit lane: [`containers`] sits behind the
//!   non-default `containers` feature, and with it off this crate is still pure
//!   data plus process-local state with no async runtime;
//! - deciding whether a lane passed: that is the release tool's receipt.

pub mod budget;
pub mod canary;
pub mod containers;
pub mod env;
pub mod fault;
pub mod images;
pub mod ledger;
pub mod run;

pub use budget::{Budget, BudgetError, Charge};
pub use canary::{LeakFinding, LeakShape, SecretCanary, scan_for_leaks};
pub use containers::{ContainerError, Engine, Readiness};
#[cfg(feature = "containers")]
pub use containers::{
    DynamoDbLocalContainer, LocalStackContainer, MinioContainer, PostgresContainer,
};
pub use fault::{Clock, FaultError, Proxy, ScriptedClock, ScriptedPort, Toxic};
pub use images::{ImageError, ImageRef, image};
pub use ledger::{CleanupLedger, Entry, LedgerError, ResourceKind, Terminal, TestCaseId};
pub use run::{Lane, TestRun, TestRunId, Ttl};

/// The policy document that pins the closed value sets and the per-lane TTLs.
///
/// It is embedded rather than read at run time so a test binary carries the
/// same policy the registry checker validated, with no working-directory
/// dependency.
pub(crate) const TEST_PROFILES_TOML: &str =
    include_str!("../../../../release/policy/test-profiles.toml");

/// The policy document that pins every permitted container image by digest.
pub(crate) const TEST_IMAGES_TOML: &str =
    include_str!("../../../../release/policy/test-images.toml");
