//! This binary's capability declaration, and the compile-time tokens that make
//! it more than a comment.
//!
//! [`Grant<C>`] is unforgeable outside this crate: [`Composition`] is the only
//! type implementing [`Declares`] here, so only this composition root can mint
//! one. [`crate::session::Stores::build`] demands a `Grant<WorkClaim>`, and the
//! manifest is checked before any AWS client is opened.

use std::collections::{BTreeMap, BTreeSet};

use aex_regional_http::capability::{
    Capability as _, CapabilityBinding, CompositionError, CompositionManifest, ContentEncrypt,
    Declares, DeployableId, ResolvedConfig, SecretPlaintextAdmission, SessionOperationInvoke,
    WorkClaim,
};

use crate::config::{self, Config};

/// This binary's capability declaration.
///
/// Four capabilities, and deliberately no general secret-decrypt capability.
/// `SecretPlaintextAdmission` is the narrow provider-credential sealing path;
/// reveal and rewrap remain outside this public edge.
#[allow(
    dead_code,
    reason = "the declaration is the capability list; its only use is the type-level `Declares` bound"
)]
pub struct Composition;

impl Declares<WorkClaim> for Composition {}
impl Declares<SessionOperationInvoke> for Composition {}
impl Declares<ContentEncrypt> for Composition {}
impl Declares<SecretPlaintextAdmission> for Composition {}

/// The manifest the start-up check runs against.
///
/// # Errors
///
/// Returns [`CompositionError::InvalidDeployable`] if the deployable name ever
/// stops matching the closed kebab-case grammar.
pub fn manifest() -> Result<CompositionManifest, CompositionError> {
    Ok(CompositionManifest {
        deployable: DeployableId::new(config::DEPLOYABLE)?,
        capabilities: BTreeSet::from([
            WorkClaim::ID,
            SessionOperationInvoke::ID,
            ContentEncrypt::ID,
            SecretPlaintextAdmission::ID,
        ]),
        bindings: vec![
            // The one write handle in the process. Naming it here is what makes
            // "which resource is the write" answerable without reading handlers.
            CapabilityBinding::resource(config::WORK_TABLE, WorkClaim::ID),
            CapabilityBinding::arn(
                config::SESSION_MAINTENANCE_WORKER_FUNCTION_ARN,
                SessionOperationInvoke::ID,
            ),
            CapabilityBinding::arn(config::CONTENT_KMS_KEY_ARN, ContentEncrypt::ID),
            CapabilityBinding::resource(
                config::SESSION_AUTHORITY_TABLE,
                SecretPlaintextAdmission::ID,
            ),
            CapabilityBinding::arn(config::SECRET_KMS_KEY_ARN, SecretPlaintextAdmission::ID),
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
                config::SESSION_MAINTENANCE_WORKER_FUNCTION_ARN.to_owned(),
                config.session_maintenance_worker.value.clone(),
            ),
            (
                config::CONTENT_KMS_KEY_ARN.to_owned(),
                config.content_kms_key.value.clone(),
            ),
            (
                config::SESSION_AUTHORITY_TABLE.to_owned(),
                config.session_table.clone(),
            ),
            (
                config::SECRET_KMS_KEY_ARN.to_owned(),
                config.secret_kms_key.value.clone(),
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
