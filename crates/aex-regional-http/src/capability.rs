//! Compile-time grant tokens and fail-closed startup admission.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use aex_wire::types::Region;

mod sealed {
    pub trait Sealed {}
}

/// A capability that can be granted only by a composition root.
pub trait Capability: sealed::Sealed {
    /// Stable manifest id.
    const ID: &'static str;
}

macro_rules! capability {
    ($name:ident, $id:literal) => {
        #[doc = concat!("Capability `", $id, "`.")]
        pub struct $name;
        impl sealed::Sealed for $name {}
        impl Capability for $name {
            const ID: &'static str = $id;
        }
    };
}

capability!(SecretPlaintextAdmission, "secret.plaintext_admission");
capability!(SecretDecrypt, "secret.decrypt");
capability!(KeystoreAdminister, "keystore.administer");
capability!(ContentObjectDelete, "content.object_delete");
capability!(ContentEncrypt, "content.encrypt");
capability!(WorkClaim, "work.claim");
capability!(SessionOperationInvoke, "session.operation_invoke");
capability!(StreamSocket, "stream.socket");

/// Unforgeable-by-construction token passed to a privileged adapter.
pub struct Grant<C: Capability>(PhantomData<C>);

/// A composition explicitly declaring a capability.
pub trait Declares<C: Capability> {
    /// Mints the token inside the declaring composition.
    #[must_use]
    fn grant() -> Grant<C> {
        Grant(PhantomData)
    }
}

/// Closed deployable identity grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployableId(String);

impl DeployableId {
    /// Parses lowercase kebab-case.
    ///
    /// # Errors
    ///
    /// Returns [`CompositionError::InvalidDeployable`] for any other grammar.
    pub fn new(value: impl Into<String>) -> Result<Self, CompositionError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !value.starts_with('-')
            && !value.ends_with('-');
        if !valid {
            return Err(CompositionError::InvalidDeployable);
        }
        Ok(Self(value))
    }

    /// Stable spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How a configuration key is bound to a capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityBinding {
    key: &'static str,
    capability: &'static str,
    arn: bool,
}

impl CapabilityBinding {
    /// A region/account-bound ARN setting.
    #[must_use]
    pub const fn arn(key: &'static str, capability: &'static str) -> Self {
        Self {
            key,
            capability,
            arn: true,
        }
    }

    /// A required logical resource setting.
    #[must_use]
    pub const fn resource(key: &'static str, capability: &'static str) -> Self {
        Self {
            key,
            capability,
            arn: false,
        }
    }
}

/// Compile-time composition declaration rendered as startup data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionManifest {
    /// Binary identity.
    pub deployable: DeployableId,
    /// Closed capability ids.
    pub capabilities: BTreeSet<&'static str>,
    /// Configuration bindings allowed for this binary.
    pub bindings: Vec<CapabilityBinding>,
}

/// Fully resolved startup config, before a listener or client is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    /// `AEX_DEPLOYABLE`.
    pub deployable: String,
    /// `AEX_PLANE`.
    pub plane: String,
    /// `AEX_REGION`.
    pub region: Region,
    /// Plane account id.
    pub account_id: String,
    /// Resource-bearing variables in the deployable namespace.
    pub values: BTreeMap<String, String>,
}

/// A fail-closed startup denial.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompositionError {
    /// Deployable id did not match the closed grammar.
    #[error("invalid deployable id")]
    InvalidDeployable,
    /// Binary and configuration identify different deployables.
    #[error("resolved deployable does not match compiled manifest")]
    WrongDeployable,
    /// Plane was not `dev` or `prd`.
    #[error("unknown deployment plane")]
    UnknownPlane,
    /// A capability was named without a binding.
    #[error("capability `{0}` has no declared configuration binding")]
    CapabilityWithoutBinding(&'static str),
    /// A required binding was missing.
    #[error("required configuration `{0}` is missing")]
    MissingBinding(&'static str),
    /// An unknown resource-bearing key was supplied.
    #[error("undeclared configuration key `{0}`")]
    UndeclaredKey(String),
    /// A binding asked for a capability the manifest does not declare.
    #[error("configuration `{key}` requires undeclared capability `{capability}`")]
    UndeclaredCapability {
        /// Configuration key.
        key: &'static str,
        /// Capability id.
        capability: &'static str,
    },
    /// ARN did not bind to the configured plane account and region.
    #[error("configuration `{0}` is not bound to this plane")]
    OffPlaneArn(&'static str),
}

/// Validates the complete resource/capability graph before any side effect.
///
/// # Errors
///
/// Returns a [`CompositionError`] for every incomplete, extra or off-plane binding.
pub fn admit(
    manifest: &CompositionManifest,
    resolved: &ResolvedConfig,
) -> Result<(), CompositionError> {
    if manifest.deployable.as_str() != resolved.deployable {
        return Err(CompositionError::WrongDeployable);
    }
    if !matches!(resolved.plane.as_str(), "dev" | "prd") {
        return Err(CompositionError::UnknownPlane);
    }
    let known = manifest
        .bindings
        .iter()
        .map(|binding| binding.key)
        .collect::<BTreeSet<_>>();
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
            return Err(CompositionError::UndeclaredCapability {
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

fn arn_matches(value: &str, region: Region, account: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    matches!(fields.next(), Some("arn"))
        && matches!(fields.next(), Some("aws"))
        && fields.next().is_some_and(|service| !service.is_empty())
        && fields.next() == Some(region.as_str())
        && fields.next() == Some(account)
        && fields.next().is_some_and(|resource| !resource.is_empty())
}
