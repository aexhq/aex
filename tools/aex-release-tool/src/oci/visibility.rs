//! One-time, fail-closed GHCR public-namespace bootstrap decisions.

use serde::{Deserialize, Serialize};

use super::OciImageIdentity;
use crate::error::{Exit, ToolError};

/// Package state returned by GitHub's supported package-read API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OciPackageVisibility {
    /// No readable package exists.
    Missing,
    /// Package exists but anonymous pulls are forbidden.
    Private,
    /// Package is anonymously readable.
    Public,
}

impl std::str::FromStr for OciPackageVisibility {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "missing" => Ok(Self::Missing),
            "private" => Ok(Self::Private),
            "public" => Ok(Self::Public),
            _ => Err(format!("unsupported GHCR visibility `{value}`")),
        }
    }
}

/// Boundary at which package visibility is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OciVisibilityPhase {
    /// Before any registry mutation.
    BeforePush,
    /// Immediately after an explicitly authorized bootstrap push.
    AfterPush,
}

impl std::str::FromStr for OciVisibilityPhase {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "before-push" => Ok(Self::BeforePush),
            "after-push" => Ok(Self::AfterPush),
            _ => Err(format!("unsupported GHCR visibility phase `{value}`")),
        }
    }
}

/// Typed operator blocker retained even when a bootstrap run fails closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciVisibilityBlocker {
    /// Stable automation code.
    pub code: String,
    /// Exact package settings URL for the one-time operator action.
    pub package_settings_url: String,
    /// Human-readable fail-closed reason.
    pub message: String,
    /// Explicit acknowledgement required by the runbook.
    pub required_acknowledgement: String,
}

/// Decision consumed by the workflow before or after its only registry push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciVisibilityDecision {
    /// Schema discriminator.
    pub schema: String,
    /// OCI unit.
    pub unit: String,
    /// Digest-only image repository.
    pub image_repository: String,
    /// Observed package state.
    pub visibility: OciPackageVisibility,
    /// Evaluation boundary.
    pub phase: OciVisibilityPhase,
    /// Whether both protected bootstrap authorities were present.
    pub bootstrap_authorized: bool,
    /// `proceed`, `bootstrap-push`, or `blocked`.
    pub action: String,
    /// Present exactly when publication must stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<OciVisibilityBlocker>,
}

impl OciVisibilityDecision {
    /// Convert a typed blocker into the workflow's fail-closed error.
    #[must_use]
    pub fn blocked_error(&self) -> Option<ToolError> {
        self.blocker.as_ref().map(|blocker| {
            ToolError::single(
                Exit::ProvenanceMissing,
                &blocker.code,
                blocker.message.clone(),
            )
        })
    }
}

/// Decide whether a normal or explicitly authorized bootstrap run may push.
///
/// This function never changes visibility. GitHub documents no supported REST
/// mutation for that property, so a private package always becomes a typed
/// operator blocker after the initial digest exists.
#[must_use]
pub fn decide_visibility(
    image: &OciImageIdentity,
    visibility: OciPackageVisibility,
    phase: OciVisibilityPhase,
    bootstrap_authorized: bool,
) -> OciVisibilityDecision {
    let action = match (phase, visibility, bootstrap_authorized) {
        (_, OciPackageVisibility::Public, _) => "proceed",
        (
            OciVisibilityPhase::BeforePush,
            OciPackageVisibility::Missing | OciPackageVisibility::Private,
            true,
        ) => "bootstrap-push",
        _ => "blocked",
    };
    let blocker = (action == "blocked").then(|| blocker(image, visibility, phase));
    OciVisibilityDecision {
        schema: "aex.oci-visibility-decision.v1".to_owned(),
        unit: image.unit.clone(),
        image_repository: image.image_repository.clone(),
        visibility,
        phase,
        bootstrap_authorized,
        action: action.to_owned(),
        blocker,
    }
}

fn blocker(
    image: &OciImageIdentity,
    visibility: OciPackageVisibility,
    phase: OciVisibilityPhase,
) -> OciVisibilityBlocker {
    let (owner, _) = image
        .source
        .repository
        .split_once('/')
        .unwrap_or(("aexhq", "aex"));
    let package = format!("aex-units%2F{}", image.unit);
    let code = match (phase, visibility) {
        (OciVisibilityPhase::AfterPush, OciPackageVisibility::Private) => {
            "oci-ghcr-bootstrap-awaiting-public"
        }
        (OciVisibilityPhase::AfterPush, OciPackageVisibility::Missing) => {
            "oci-ghcr-bootstrap-package-missing"
        }
        (_, OciPackageVisibility::Private) => "oci-ghcr-package-private",
        (_, OciPackageVisibility::Missing) => "oci-ghcr-package-missing",
        (_, OciPackageVisibility::Public) => "oci-ghcr-visibility-internal",
    };
    OciVisibilityBlocker {
        code: code.to_owned(),
        package_settings_url: format!(
            "https://github.com/orgs/{owner}/packages/container/{package}/settings"
        ),
        message: format!(
            "GHCR package for `{}` is `{visibility:?}` at `{phase:?}`; anonymous readback and attestation are refused",
            image.unit
        ),
        required_acknowledgement:
            "I understand that public package visibility is a one-time operator action and that every published version becomes anonymously readable."
                .to_owned(),
    }
}
