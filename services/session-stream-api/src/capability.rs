//! This binary's capability declaration, and the compile-time tokens that make
//! it more than a comment.
//!
//! # Why this module exists
//!
//! `regional-stream` used to prove "the stream admits no durable work" by
//! refusing to start if `AEX_WORK_TABLE` was bound at all. That proof was cheap
//! and genuinely strong, and merging the stream into the session API destroys
//! it: the session half *requires* the same variable, and one process cannot
//! both require and forbid it.
//!
//! What replaces it is narrower in what it inspects and wider in what it proves.
//! The environment guard said "no process holding this binary may know the work
//! table's name". This says "no code path reachable from a stream route may
//! reach a write handle, and the compiler checks it". The second is what anyone
//! actually wanted; the first was a proxy for it that happened to be enforceable
//! in `main`.
//!
//! # The three mechanisms
//!
//! 1. [`Grant<C>`] is unforgeable outside this crate: [`Composition`] is the only
//!    type implementing [`Declares`] here, so only this composition root can mint
//!    one. [`crate::session::Stores::build`] demands a `Grant<WorkClaim>`, which
//!    means the `WorkStore` write handle cannot be constructed anywhere else —
//!    not in a stream handler, not in a test helper, not in a future refactor
//!    that "just needs the table name".
//! 2. The stream half's state
//!    ([`crate::stream::mount::AppState`]) has no field of a write-capable type
//!    and no field from which one can be derived. It holds an edge, a read-only
//!    observation service, decode limits, socket quotas and the drain flag. A
//!    stream handler that wanted a write would have to widen that struct, which
//!    is a visible diff in a file called `mount.rs` rather than an invisible
//!    consequence of an environment variable appearing in a task definition.
//! 3. [`manifest`] is checked by [`aex_regional_http::capability::admit`] before
//!    any AWS client is opened, and refuses a start-up whose declared
//!    capabilities and configured resources disagree.
//!
//! Mechanism 2 is the one that carries the guarantee the deleted `FORBIDDEN`
//! entry used to carry. Mechanisms 1 and 3 are what stop it from silently
//! regressing.

use std::collections::{BTreeMap, BTreeSet};

use aex_regional_http::capability::{
    Capability as _, CapabilityBinding, CompositionError, CompositionManifest, ContentEncrypt,
    Declares, DeployableId, ResolvedConfig, StreamSocket, WorkClaim,
};

use crate::config::{self, Config};

/// This binary's capability declaration.
///
/// Two capabilities, and deliberately not a third. There is no
/// `SecretPlaintextAdmission` and no `SecretDecrypt`: this edge reads ciphertext
/// metadata and never decrypts, which is why `AEX_SECRET_KMS_KEY_ARN` stays in
/// [`crate::config::FORBIDDEN`].
#[allow(
    dead_code,
    reason = "the declaration is the capability list; its only use is the type-level `Declares` bound"
)]
pub struct Composition;

impl Declares<WorkClaim> for Composition {}
impl Declares<StreamSocket> for Composition {}
impl Declares<ContentEncrypt> for Composition {}

/// The manifest the start-up check runs against.
///
/// # Errors
///
/// Returns [`CompositionError::InvalidDeployable`] if the deployable name ever
/// stops matching the closed kebab-case grammar.
pub fn manifest() -> Result<CompositionManifest, CompositionError> {
    Ok(CompositionManifest {
        deployable: DeployableId::new(config::DEPLOYABLE)?,
        capabilities: BTreeSet::from([WorkClaim::ID, StreamSocket::ID, ContentEncrypt::ID]),
        bindings: vec![
            // The one write handle in the process. Naming it here is what makes
            // "which resource is the write" answerable without reading handlers.
            CapabilityBinding::resource(config::WORK_TABLE, WorkClaim::ID),
            // The sockets' authority. A stream reads it; nothing writes it here.
            CapabilityBinding::resource(config::OBSERVATION_TABLE, StreamSocket::ID),
            CapabilityBinding::arn(config::CONTENT_KMS_KEY_ARN, ContentEncrypt::ID),
        ],
    })
}

/// The resolved values the composition check runs over.
///
/// Only the capability-bearing keys appear. A key added to [`manifest`] without
/// being added here fails [`aex_regional_http::capability::admit`] as a missing
/// binding, and one added here without a manifest entry fails it as an
/// undeclared key, so the two lists cannot drift apart silently.
#[must_use]
pub fn resolved(config: &Config) -> ResolvedConfig {
    ResolvedConfig {
        deployable: config::DEPLOYABLE.to_owned(),
        plane: config.plane.as_str().to_owned(),
        region: config.region,
        account_id: config.content_bucket_owner.clone(),
        values: BTreeMap::from([
            (config::WORK_TABLE.to_owned(), config.work_table.clone()),
            (
                config::OBSERVATION_TABLE.to_owned(),
                config.observation_table.clone(),
            ),
            (
                config::CONTENT_KMS_KEY_ARN.to_owned(),
                config.content_kms_key.value.clone(),
            ),
        ]),
    }
}

/// Admits the resolved configuration against the compiled manifest.
///
/// # Errors
///
/// Returns the first [`CompositionError`]; every one of them is a refusal to
/// start, before a single AWS client is constructed.
pub fn admit(config: &Config) -> Result<(), CompositionError> {
    aex_regional_http::capability::admit(&manifest()?, &resolved(config))
}
