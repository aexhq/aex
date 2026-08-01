//! Compile-time capability grants and the fail-closed startup denial.
//!
//! A central deployable declares the capabilities it is allowed to link. The
//! declaration is checked against its configuration **before** any client is
//! built, so a binary that was handed a resource it may not touch refuses to
//! start rather than discovering the denial on its first request.
//!
//! `Grant<C>` is unforgeable outside the declaring composition: its only
//! constructor is a default method on [`Declares`], which a crate can implement
//! only for a capability it names. A library that wants a privileged client must
//! therefore be handed a token by the composition root.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use crate::config::{CentralServiceId, DeploymentPlane};
use aex_wire::types::Region;

mod sealed {
    pub trait Sealed {}
}

/// A capability a composition root may grant.
pub trait Capability: sealed::Sealed {
    /// The stable manifest id.
    const ID: &'static str;
}

macro_rules! capability {
    ($name:ident, $id:literal, $doc:literal) => {
        #[doc = $doc]
        pub struct $name;
        impl sealed::Sealed for $name {}
        impl Capability for $name {
            const ID: &'static str = $id;
        }
    };
}

capability!(
    ControlWrite,
    "control.write",
    "Full `rds-data` transaction set against the control schema."
);
capability!(
    IdentityWrite,
    "identity.write",
    "Full `rds-data` transaction set against the identity schema."
);
capability!(
    AuthorizationRead,
    "authorization.read",
    "Read-only `rds-data` statements as `aex_authz`; no transaction API at all."
);
capability!(
    AssertionSign,
    "assertion.sign",
    "Unwrapping the Ed25519 assertion private key, once per cold start."
);
capability!(
    RegionalControlInvoke,
    "regional.control_invoke",
    "Invoking the regional control authorities."
);
capability!(
    ControlQueuePublish,
    "control.queue_publish",
    "Publishing to the central control FIFO queue."
);
capability!(
    ControlQueueConsume,
    "control.queue_consume",
    "Receiving from and deleting on the central control FIFO queue."
);
capability!(
    MailSend,
    "mail.send",
    "Sending an invitation notification from the configured identity."
);
capability!(
    SigningKeyAdminister,
    "signing_key.administer",
    "Creating and retiring assertion signing-key secrets."
);

/// A token proving the holder's composition declared `C`.
pub struct Grant<C: Capability>(PhantomData<C>);

/// A composition that declares a capability.
pub trait Declares<C: Capability> {
    /// Mints the token. Callable only inside the declaring composition.
    #[must_use]
    fn grant() -> Grant<C> {
        Grant(PhantomData)
    }
}

/// How one configuration key is bound to a capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityBinding {
    /// The configuration key.
    pub key: &'static str,
    /// The capability it enables.
    pub capability: &'static str,
    /// Whether the value must be a plane-bound ARN.
    pub arn: bool,
}

impl CapabilityBinding {
    /// A region- and account-bound ARN.
    #[must_use]
    pub const fn arn(key: &'static str, capability: &'static str) -> Self {
        Self {
            key,
            capability,
            arn: true,
        }
    }

    /// A required logical resource that is not an ARN.
    #[must_use]
    pub const fn resource(key: &'static str, capability: &'static str) -> Self {
        Self {
            key,
            capability,
            arn: false,
        }
    }
}

/// What a composition root declares about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionManifest {
    /// Which deployable.
    pub deployable: CentralServiceId,
    /// The capability ids it may link.
    pub capabilities: BTreeSet<&'static str>,
    /// The configuration keys it may be given.
    pub bindings: Vec<CapabilityBinding>,
}

/// A resolved environment, before any client is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    /// Which deployable the environment says this is.
    pub deployable: String,
    /// Which deployment plane.
    pub plane: DeploymentPlane,
    /// Which region.
    pub region: Region,
    /// The plane's account id.
    pub account_id: String,
    /// Every resource-bearing value, by key.
    pub values: BTreeMap<String, String>,
}

/// Why a composition refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompositionError {
    /// The binary and its configuration name different deployables.
    #[error("the environment names `{found}` but this binary is `{expected}`")]
    WrongDeployable {
        /// What the binary is.
        expected: &'static str,
        /// What the environment said.
        found: String,
    },
    /// A declared capability has no configuration binding.
    #[error("capability `{0}` has no declared configuration binding")]
    CapabilityWithoutBinding(&'static str),
    /// A binding names a capability the manifest does not declare.
    ///
    /// This is the denial that matters: a deployable cannot be handed a resource
    /// for a capability it never declared, whatever the environment says.
    #[error("configuration `{key}` requires undeclared capability `{capability}`")]
    ForbiddenCapability {
        /// Which key.
        key: &'static str,
        /// Which capability.
        capability: &'static str,
    },
    /// A required binding was absent or blank.
    #[error("required configuration `{0}` is missing")]
    MissingBinding(&'static str),
    /// A resource-bearing key outside the manifest was supplied.
    #[error("undeclared configuration key `{0}`")]
    UndeclaredKey(String),
    /// An ARN did not belong to this plane's account and region.
    #[error("configuration `{0}` is not bound to this plane")]
    OffPlaneArn(&'static str),
}

/// Validates the whole resource and capability graph before any side effect.
///
/// # Errors
///
/// Returns the first [`CompositionError`] it finds. There is no partial
/// admission: a composition either links exactly what it declared or does not
/// start.
pub fn admit(
    manifest: &CompositionManifest,
    resolved: &ResolvedConfig,
) -> Result<(), CompositionError> {
    if manifest.deployable.as_str() != resolved.deployable {
        return Err(CompositionError::WrongDeployable {
            expected: manifest.deployable.as_str(),
            found: resolved.deployable.clone(),
        });
    }
    let known: BTreeSet<&str> = manifest.bindings.iter().map(|it| it.key).collect();
    if let Some(extra) = resolved
        .values
        .keys()
        .find(|key| !known.contains(key.as_str()))
    {
        return Err(CompositionError::UndeclaredKey(extra.clone()));
    }
    for capability in &manifest.capabilities {
        if !manifest
            .bindings
            .iter()
            .any(|binding| binding.capability == *capability)
        {
            return Err(CompositionError::CapabilityWithoutBinding(capability));
        }
    }
    for binding in &manifest.bindings {
        if !manifest.capabilities.contains(binding.capability) {
            return Err(CompositionError::ForbiddenCapability {
                key: binding.key,
                capability: binding.capability,
            });
        }
        let value = resolved
            .values
            .get(binding.key)
            .filter(|value| !value.trim().is_empty())
            .ok_or(CompositionError::MissingBinding(binding.key))?;
        if binding.arn && !arn_matches(value, resolved.region, &resolved.account_id) {
            return Err(CompositionError::OffPlaneArn(binding.key));
        }
    }
    Ok(())
}

/// Whether an ARN names this plane's account and region.
fn arn_matches(value: &str, region: Region, account: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    matches!(fields.next(), Some("arn"))
        && matches!(fields.next(), Some("aws"))
        && fields.next().is_some_and(|service| !service.is_empty())
        && fields.next() == Some(region.as_str())
        && fields.next() == Some(account)
        && fields.next().is_some_and(|resource| !resource.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        AssertionSign, AuthorizationRead, Capability as _, CapabilityBinding, CompositionError,
        CompositionManifest, ControlWrite, Declares, Grant, ResolvedConfig, admit,
    };
    use crate::config::{CentralServiceId, DeploymentPlane};
    use aex_wire::types::Region;
    use std::collections::{BTreeMap, BTreeSet};

    struct Authz;
    impl Declares<AuthorizationRead> for Authz {}
    impl Declares<AssertionSign> for Authz {}

    fn manifest() -> CompositionManifest {
        CompositionManifest {
            deployable: CentralServiceId::Authz,
            capabilities: BTreeSet::from([AuthorizationRead::ID, AssertionSign::ID]),
            bindings: vec![
                CapabilityBinding::arn("AEX_AURORA_CLUSTER_ARN", AuthorizationRead::ID),
                CapabilityBinding::resource("AEX_ASSERTION_SIGNING_SECRET_ID", AssertionSign::ID),
            ],
        }
    }

    fn resolved() -> ResolvedConfig {
        ResolvedConfig {
            deployable: "central-authz".to_owned(),
            plane: DeploymentPlane::Dev,
            region: Region::EuWest1,
            account_id: "000000000000".to_owned(),
            values: BTreeMap::from([
                (
                    "AEX_AURORA_CLUSTER_ARN".to_owned(),
                    "arn:aws:rds:eu-west-1:000000000000:cluster:aex".to_owned(),
                ),
                (
                    "AEX_ASSERTION_SIGNING_SECRET_ID".to_owned(),
                    "aex/dev/authz-signing/current".to_owned(),
                ),
            ]),
        }
    }

    #[test]
    fn a_complete_declaration_is_admitted() {
        assert_eq!(admit(&manifest(), &resolved()), Ok(()));
        let _: Grant<AuthorizationRead> = <Authz as Declares<AuthorizationRead>>::grant();
    }

    #[test]
    fn a_deployable_cannot_link_a_capability_it_did_not_declare() {
        let mut manifest = manifest();
        manifest.bindings.push(CapabilityBinding::arn(
            "AEX_CONTROL_QUEUE_URL",
            ControlWrite::ID,
        ));
        assert_eq!(
            admit(&manifest, &resolved()),
            Err(CompositionError::ForbiddenCapability {
                key: "AEX_CONTROL_QUEUE_URL",
                capability: ControlWrite::ID
            })
        );
    }

    #[test]
    fn a_declared_capability_with_no_binding_is_refused() {
        let mut manifest = manifest();
        manifest.capabilities.insert(ControlWrite::ID);
        assert_eq!(
            admit(&manifest, &resolved()),
            Err(CompositionError::CapabilityWithoutBinding(ControlWrite::ID))
        );
    }

    #[test]
    fn an_undeclared_configuration_key_is_refused_rather_than_ignored() {
        let mut resolved = resolved();
        resolved
            .values
            .insert("AEX_SES_IDENTITY".to_owned(), "aex.dev".to_owned());
        assert_eq!(
            admit(&manifest(), &resolved),
            Err(CompositionError::UndeclaredKey(
                "AEX_SES_IDENTITY".to_owned()
            ))
        );
    }

    #[test]
    fn an_absent_or_blank_binding_is_refused() {
        for value in [None, Some("   ")] {
            let mut resolved = resolved();
            match value {
                None => {
                    resolved.values.remove("AEX_AURORA_CLUSTER_ARN");
                }
                Some(blank) => {
                    resolved
                        .values
                        .insert("AEX_AURORA_CLUSTER_ARN".to_owned(), blank.to_owned());
                }
            }
            assert_eq!(
                admit(&manifest(), &resolved),
                Err(CompositionError::MissingBinding("AEX_AURORA_CLUSTER_ARN"))
            );
        }
    }

    #[test]
    fn an_arn_from_another_plane_is_refused() {
        let mut resolved = resolved();
        resolved.values.insert(
            "AEX_AURORA_CLUSTER_ARN".to_owned(),
            "arn:aws:rds:us-east-1:000000000000:cluster:aex".to_owned(),
        );
        assert_eq!(
            admit(&manifest(), &resolved),
            Err(CompositionError::OffPlaneArn("AEX_AURORA_CLUSTER_ARN"))
        );

        let mut resolved = super::tests::resolved();
        resolved.values.insert(
            "AEX_AURORA_CLUSTER_ARN".to_owned(),
            "arn:aws:rds:eu-west-1:999999999999:cluster:aex".to_owned(),
        );
        assert_eq!(
            admit(&manifest(), &resolved),
            Err(CompositionError::OffPlaneArn("AEX_AURORA_CLUSTER_ARN"))
        );
    }

    #[test]
    fn a_binary_refuses_an_environment_naming_another_deployable() {
        let mut resolved = resolved();
        resolved.deployable = "central-control-api".to_owned();
        assert_eq!(
            admit(&manifest(), &resolved),
            Err(CompositionError::WrongDeployable {
                expected: "central-authz",
                found: "central-control-api".to_owned()
            })
        );
    }
}
