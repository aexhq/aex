//! Public schemas and typed validators for private release values.
//!
//! The public repository owns these shapes; the private hosted repository owns
//! every value inside them. Validation is deliberately side-effect free so an
//! acquire job can refuse tampering, plaintext secrets and stale plans before
//! it requests any cloud credential.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation};

/// Maximum time between creating and expiring a saved plan.
pub const SAVED_PLAN_MAX_TTL_MINUTES: i64 = 60;

/// A private environment binding governed by the public
/// `aex.environment-binding.v1` schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentBinding {
    /// Schema identity.
    pub schema: String,
    /// Self digest over canonical bytes with this field removed.
    pub binding_digest: String,
    /// Hosted plane.
    pub plane: String,
    /// Enabled regions.
    pub regions: Vec<String>,
    /// Desired public composition identity.
    pub desired_release_id: String,
    /// Private root-module version.
    pub root_module_version: String,
    /// Exact public module bundle.
    pub infra_module_bundle_digest: String,
    /// Logical-to-physical hosted resources.
    pub resources: BTreeMap<String, ResourceBinding>,
    /// Secret references only.
    pub secrets: BTreeMap<String, SecretReference>,
    /// Non-secret configuration identity.
    pub config: ConfigBinding,
    /// Optional hosted project identities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<ProjectBindings>,
    /// Optional business-data revisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_versions: Option<DataVersions>,
    /// Approval policy identity, not an approval decision.
    pub approval: ApprovalBinding,
}

/// One hosted resource bound to a public logical name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceBinding {
    /// Resource kind.
    pub kind: String,
    /// AWS account id.
    pub account_id: String,
    /// AWS region.
    pub region: String,
    /// Optional exact ARN.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arn: Option<String>,
    /// Optional exact URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Optional immutable version or digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immutable_version: Option<String>,
}

/// A reference to a secret. No plaintext field exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretReference {
    /// Secrets Manager ARN.
    pub arn: String,
    /// Exact secret version id.
    pub version_id: String,
    /// Named secret version stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_stage: Option<String>,
    /// Public secret-purpose vocabulary.
    pub kind: String,
}

/// Non-secret configuration identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigBinding {
    /// Digest of the configuration bundle.
    pub bundle_digest: String,
    /// Per-unit configuration schema versions.
    pub schema_versions: BTreeMap<String, u64>,
}

/// Hosted project identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectBindings {
    /// Vercel project identities by logical name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vercel: Option<BTreeMap<String, VercelProjectBinding>>,
}

/// One Vercel project identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VercelProjectBinding {
    /// Team id.
    pub team_id: String,
    /// Project id.
    pub project_id: String,
}

/// Private business-data revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataVersions {
    /// Price-book revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_book_revision: Option<String>,
    /// Tax-table revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tax_table_revision: Option<String>,
    /// Risk-threshold revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_threshold_revision: Option<String>,
}

/// Approval policy identity carried by a binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalBinding {
    /// Policy id.
    pub policy: String,
    /// Groups or identities permitted to approve.
    pub approvers: Vec<String>,
    /// Optional maintenance windows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maintenance_windows: Vec<MaintenanceWindow>,
    /// Optional break-glass role name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_glass_role: Option<String>,
}

/// One maintenance window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaintenanceWindow {
    /// Cron expression.
    pub cron: String,
    /// Window duration.
    pub duration_minutes: u64,
}

/// Exact hosted placement for all artifacts in one binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedPlacement {
    /// Schema identity.
    pub schema: String,
    /// Self digest over canonical bytes with this field removed.
    pub placement_digest: String,
    /// Public release identity.
    pub release_id: String,
    /// Private binding identity.
    pub binding_digest: String,
    /// Per-unit exact placement.
    pub artifacts: BTreeMap<String, ArtifactPlacement>,
}

/// One content-addressed artifact placement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactPlacement {
    /// Public content digest.
    pub artifact_digest: String,
    /// Public content size.
    pub size_bytes: u64,
    /// Exact provider image ARN/version pair, when this artifact is a MicroVM image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microvm_image: Option<MicrovmImagePlacement>,
    /// Exact hosted destination and readback identity.
    pub destination: PlacementDestination,
}

/// One immutable Lambda MicroVM image identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MicrovmImagePlacement {
    /// Image resource ARN. Versions are separate provider values, not ARNs.
    pub image_arn: String,
    /// Exact immutable image version returned by the provider.
    pub image_version: String,
}

/// Supported exact hosted destinations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum PlacementDestination {
    /// Versioned S3 object with checksum readback.
    S3 {
        /// Bucket name.
        bucket: String,
        /// Immutable object key.
        key: String,
        /// Exact version returned by S3.
        #[serde(rename = "objectVersionId")]
        object_version_id: String,
        /// S3 checksum normalized to the release digest form.
        #[serde(rename = "checksumSha256")]
        checksum_sha256: String,
    },
    /// OCI repository placement by manifest digest.
    Oci {
        /// Repository URI without a tag.
        repository: String,
        /// Exact manifest digest read back from the registry.
        #[serde(rename = "manifestDigest")]
        manifest_digest: String,
    },
    /// Immutable Vercel deployment identity.
    Vercel {
        /// Project id.
        #[serde(rename = "projectId")]
        project_id: String,
        /// Deployment id.
        #[serde(rename = "deploymentId")]
        deployment_id: String,
    },
}

/// Identity of a content-addressed blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobIdentity {
    /// SHA-256 digest.
    pub digest: String,
    /// Exact byte length.
    pub size_bytes: u64,
}

/// Exact transported and semantic identities of the regional table bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegionalTablesIdentity {
    /// SHA-256 digest over the transported JSON bytes.
    pub digest: String,
    /// Exact transported byte length.
    pub size_bytes: u64,
    /// BLAKE3 identity over the canonical decoded definitions.
    pub definitions_digest: String,
}

/// Identity of the release tool binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolIdentity {
    /// Tool semantic version.
    pub version: String,
    /// SHA-256 digest.
    pub digest: String,
    /// Exact byte length.
    pub size_bytes: u64,
}

/// Identity and hosted version of an opaque saved plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedPlanIdentity {
    /// SHA-256 digest.
    pub digest: String,
    /// Exact byte length.
    pub size_bytes: u64,
    /// Exact encrypted private object to fetch.
    pub location: EncryptedPlanLocation,
}

/// Exact storage and encryption identity of a saved plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptedPlanLocation {
    /// Versioned S3 bucket.
    pub bucket: String,
    /// Exact object key.
    pub key: String,
    /// Bucket region.
    pub region: String,
    /// Exact object version.
    pub object_version_id: String,
    /// Exact KMS key ARN used for server-side encryption.
    pub kms_key_arn: String,
}

/// Runner compatibility and fixed paths embedded in a saved plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerIdentity {
    /// Operating system family.
    pub os: String,
    /// CPU architecture.
    pub arch: String,
    /// Absolute fixed checkout working directory.
    pub working_directory: String,
    /// Absolute fixed module extraction path.
    pub module_extraction_path: String,
}

/// One Terraform provider identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderIdentity {
    /// Provider semantic version.
    pub version: String,
    /// Provider package checksum.
    pub checksum: String,
}

/// Terraform configuration and provider closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerraformIdentity {
    /// Exact Terraform CLI binary identity.
    pub binary: ToolIdentity,
    /// Private root path inside its verified source archive.
    pub root: String,
    /// Digest of the exact root closure.
    pub root_digest: String,
    /// Digest of `.terraform.lock.hcl`.
    pub provider_lock_digest: String,
    /// Fully resolved providers.
    pub providers: BTreeMap<String, ProviderIdentity>,
}

/// Remote state snapshot the plan was made against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase", deny_unknown_fields)]
pub enum StateIdentity {
    /// A pre-existing state object was observed during planning.
    Present {
        /// Exact state backend identity.
        backend: StateBackendIdentity,
        /// Backend object key.
        #[serde(rename = "backendKey")]
        backend_key: String,
        /// Terraform state lineage.
        lineage: String,
        /// Terraform state serial.
        serial: u64,
        /// Exact backend object version.
        #[serde(rename = "objectVersionId")]
        object_version_id: String,
    },
    /// No state object existed at the exact backend key during planning.
    Absent {
        /// Exact state backend identity.
        backend: StateBackendIdentity,
        /// Backend object key checked for absence.
        #[serde(rename = "backendKey")]
        backend_key: String,
    },
}

/// Exact S3 state backend and canonical configuration identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateBackendIdentity {
    /// Backend kind; v1 supports only S3.
    pub kind: String,
    /// Exact state bucket.
    pub bucket: String,
    /// Bucket region.
    pub region: String,
    /// Digest of the canonical backend configuration excluding credentials.
    pub config_digest: String,
}

/// GitHub workflow identity that created the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowIdentity {
    /// Caller repository.
    pub repository: String,
    /// Immutable GitHub repository id.
    pub repository_id: String,
    /// Exact private commit.
    pub commit_sha: String,
    /// Exact reusable workflow path and ref.
    pub workflow_ref: String,
    /// Workflow run id.
    pub run_id: String,
    /// Workflow attempt.
    pub run_attempt: u64,
    /// Job name.
    pub job_name: String,
}

/// Immutable envelope around one opaque saved Terraform plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedPlanEnvelope {
    /// Schema identity.
    pub schema: String,
    /// Self digest over canonical bytes with this field removed.
    pub envelope_digest: String,
    /// Public release identity.
    pub release_id: String,
    /// Private binding digest.
    pub binding_digest: String,
    /// Exact private repository commit.
    pub binding_ref: String,
    /// Exact hosted placement identity.
    pub placement_digest: String,
    /// Saved plan identity.
    pub plan: SavedPlanIdentity,
    /// Release tool identity.
    pub tool: ToolIdentity,
    /// Private source archive identity.
    pub source_archive: BlobIdentity,
    /// Public Terraform module bundle identity.
    pub module_bundle: BlobIdentity,
    /// Public generated regional table bundle identity.
    pub regional_tables: RegionalTablesIdentity,
    /// Runner and fixed-path compatibility identity.
    pub runner: RunnerIdentity,
    /// Terraform and provider closure.
    pub terraform: TerraformIdentity,
    /// State snapshot identity.
    pub state: StateIdentity,
    /// Workflow identity.
    pub workflow: WorkflowIdentity,
    /// RFC 3339 creation instant.
    pub created_at: String,
    /// RFC 3339 expiry instant.
    pub expires_at: String,
}

/// Parse and validate one environment binding.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] for malformed, secret-bearing or tampered
/// bindings.
pub fn parse_environment_binding(text: &str) -> Result<EnvironmentBinding> {
    let value = parse_release_value(text, "environment-binding")?;
    reject_plaintext_secrets(&value)?;
    let binding: EnvironmentBinding = deserialize(value.clone(), "environment-binding")?;
    let mut violations = Vec::new();

    require_equal(
        &mut violations,
        "binding-schema",
        &binding.schema,
        "aex.environment-binding.v1",
    );
    validate_self_digest(
        &value,
        "bindingDigest",
        &binding.binding_digest,
        "binding-digest-mismatch",
        &mut violations,
    )?;
    validate_sha256(
        "release-id-invalid",
        &binding.desired_release_id,
        &mut violations,
    );
    validate_sha256(
        "module-bundle-digest-invalid",
        &binding.infra_module_bundle_digest,
        &mut violations,
    );
    if !matches!(binding.plane.as_str(), "dev" | "prd") {
        violations.push(Violation::new(
            "binding-plane-invalid",
            format!("`{}` is not dev or prd", binding.plane),
        ));
    }
    validate_regions(&binding.regions, &mut violations);
    if binding.root_module_version.is_empty() {
        violations.push(Violation::new(
            "root-module-version-empty",
            "rootModuleVersion must identify the private root",
        ));
    }
    validate_binding_resources(&binding.resources, &mut violations);
    validate_binding_secrets(&binding.secrets, &mut violations);
    validate_sha256(
        "config-bundle-digest-invalid",
        &binding.config.bundle_digest,
        &mut violations,
    );
    if binding.approval.approvers.is_empty() {
        violations.push(Violation::new(
            "binding-approvers-empty",
            "approval.approvers must contain at least one identity",
        ));
    }
    finish(Exit::ManifestInvalid, violations, binding)
}

fn validate_binding_resources(
    resources: &BTreeMap<String, ResourceBinding>,
    violations: &mut Vec<Violation>,
) {
    for (logical, resource) in resources {
        validate_logical_name(logical, '_', violations);
        if !is_account_id(&resource.account_id) {
            violations.push(Violation::new(
                "resource-account-invalid",
                format!("resource `{logical}` has an invalid accountId"),
            ));
        }
        if !is_region(&resource.region) {
            violations.push(Violation::new(
                "resource-region-invalid",
                format!("resource `{logical}` has an invalid region"),
            ));
        }
        if resource.arn.is_none() && resource.url.is_none() {
            violations.push(Violation::new(
                "resource-location-missing",
                format!("resource `{logical}` has neither arn nor url"),
            ));
        }
        if resource
            .arn
            .as_deref()
            .is_some_and(|arn| !arn.starts_with("arn:aws"))
        {
            violations.push(Violation::new(
                "resource-arn-invalid",
                format!("resource `{logical}` has an invalid ARN"),
            ));
        }
        if resource
            .url
            .as_deref()
            .is_some_and(|url| !is_safe_https_endpoint(url))
        {
            violations.push(Violation::new(
                "resource-url-invalid",
                format!(
                    "resource `{logical}` URL must be HTTPS without userinfo, query or fragment"
                ),
            ));
        }
    }
}

fn validate_binding_secrets(
    secrets: &BTreeMap<String, SecretReference>,
    violations: &mut Vec<Violation>,
) {
    for (logical, secret) in secrets {
        validate_logical_name(logical, '_', violations);
        if !secret.arn.starts_with("arn:aws") || !secret.arn.contains(":secretsmanager:") {
            violations.push(Violation::new(
                "secret-reference-invalid",
                format!("secret `{logical}` is not a Secrets Manager ARN"),
            ));
        }
        if secret.version_id.is_empty() {
            violations.push(Violation::new(
                "secret-version-id-missing",
                format!("secret `{logical}` must pin an immutable versionId"),
            ));
        }
    }
}

/// Parse and validate one resolved placement.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] for malformed or tampered placement.
pub fn parse_resolved_placement(text: &str) -> Result<ResolvedPlacement> {
    let value = parse_release_value(text, "resolved-placement")?;
    reject_plaintext_secrets(&value)?;
    let placement: ResolvedPlacement = deserialize(value.clone(), "resolved-placement")?;
    let mut violations = Vec::new();

    require_equal(
        &mut violations,
        "placement-schema",
        &placement.schema,
        "aex.resolved-placement.v1",
    );
    validate_self_digest(
        &value,
        "placementDigest",
        &placement.placement_digest,
        "placement-digest-mismatch",
        &mut violations,
    )?;
    validate_sha256("release-id-invalid", &placement.release_id, &mut violations);
    validate_sha256(
        "binding-digest-invalid",
        &placement.binding_digest,
        &mut violations,
    );
    if placement.artifacts.is_empty() {
        violations.push(Violation::new(
            "placement-artifacts-empty",
            "a resolved placement must name at least one artifact",
        ));
    }
    validate_placements(&placement.artifacts, &mut violations);
    finish(Exit::ManifestInvalid, violations, placement)
}

fn validate_placements(
    artifacts: &BTreeMap<String, ArtifactPlacement>,
    violations: &mut Vec<Violation>,
) {
    for (unit, artifact) in artifacts {
        validate_logical_name(unit, '-', violations);
        validate_sha256(
            "placement-artifact-digest-invalid",
            &artifact.artifact_digest,
            violations,
        );
        if artifact.size_bytes == 0 {
            violations.push(Violation::new(
                "placement-artifact-size-invalid",
                format!("artifact `{unit}` has zero size"),
            ));
        }
        if let Some(image) = &artifact.microvm_image {
            let arn_parts = image.image_arn.split(':').collect::<Vec<_>>();
            if arn_parts.len() != 7
                || !arn_parts[0].starts_with("arn")
                || arn_parts[2] != "lambda"
                || arn_parts[3].is_empty()
                || arn_parts[4].len() != 12
                || !arn_parts[4].bytes().all(|byte| byte.is_ascii_digit())
                || arn_parts[5] != "microvm-image"
                || arn_parts[6].is_empty()
                || arn_parts[6].len() > 64
                || !arn_parts[6]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            {
                violations.push(Violation::new(
                    "placement-microvm-image-arn-invalid",
                    format!("artifact `{unit}` has an invalid Lambda MicroVM image ARN"),
                ));
            }
            if image.image_version.is_empty() || image.image_version.len() > 2_048 {
                violations.push(Violation::new(
                    "placement-microvm-image-version-invalid",
                    format!("artifact `{unit}` has an invalid Lambda MicroVM image version"),
                ));
            }
        }
        match &artifact.destination {
            PlacementDestination::S3 {
                bucket,
                key,
                object_version_id,
                checksum_sha256,
            } => {
                require_non_empty("placement-s3-bucket-empty", bucket, violations);
                require_safe_relative("placement-s3-key-invalid", key, violations);
                require_non_empty("placement-s3-version-empty", object_version_id, violations);
                validate_sha256("placement-s3-checksum-invalid", checksum_sha256, violations);
                if checksum_sha256 != &artifact.artifact_digest {
                    violations.push(Violation::new(
                        "placement-s3-checksum-mismatch",
                        format!("artifact `{unit}` checksum does not equal its public digest"),
                    ));
                }
            }
            PlacementDestination::Oci {
                repository,
                manifest_digest,
            } => {
                require_non_empty("placement-oci-repository-empty", repository, violations);
                if repository.contains('@')
                    || repository
                        .rsplit('/')
                        .next()
                        .is_some_and(|v| v.contains(':'))
                {
                    violations.push(Violation::new(
                        "placement-oci-repository-mutable",
                        format!("artifact `{unit}` repository must not contain a tag or digest"),
                    ));
                }
                validate_sha256("placement-oci-digest-invalid", manifest_digest, violations);
                if manifest_digest != &artifact.artifact_digest {
                    violations.push(Violation::new(
                        "placement-oci-digest-mismatch",
                        format!("artifact `{unit}` manifest does not equal its public digest"),
                    ));
                }
            }
            PlacementDestination::Vercel {
                project_id,
                deployment_id,
            } => {
                require_non_empty("placement-vercel-project-empty", project_id, violations);
                require_non_empty(
                    "placement-vercel-deployment-empty",
                    deployment_id,
                    violations,
                );
            }
        }
    }
}

/// Parse and validate one saved-plan envelope and, when supplied, its opaque
/// plan bytes.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] for malformed/tampered metadata,
/// [`Exit::ArtifactMismatch`] for plan-byte mismatch and
/// [`Exit::AdmissionDenied`] for a not-yet-valid or expired plan.
pub fn parse_saved_plan_envelope(
    text: &str,
    now: OffsetDateTime,
    plan_bytes: Option<&[u8]>,
) -> Result<SavedPlanEnvelope> {
    let value = parse_release_value(text, "saved-plan-envelope")?;
    reject_plaintext_secrets(&value)?;
    let envelope: SavedPlanEnvelope = deserialize(value.clone(), "saved-plan-envelope")?;
    let mut violations = Vec::new();

    validate_self_digest(
        &value,
        "envelopeDigest",
        &envelope.envelope_digest,
        "saved-plan-envelope-digest-mismatch",
        &mut violations,
    )?;
    if !violations.is_empty() {
        return Err(ToolError::many(Exit::ManifestInvalid, violations));
    }
    validate_saved_identity(&envelope, &mut violations);
    validate_terraform(&envelope.terraform, &mut violations);
    validate_state(&envelope.state, &mut violations);
    validate_plan_location(&envelope.plan.location, &mut violations);
    validate_workflow(&envelope.workflow, &mut violations);
    let times = validate_plan_times(&envelope, &mut violations);
    if !violations.is_empty() {
        return Err(ToolError::many(Exit::ManifestInvalid, violations));
    }
    validate_plan_bytes(&envelope.plan, plan_bytes)?;
    let Some((created, expires)) = times else {
        return Err(ToolError::single(
            Exit::ManifestInvalid,
            "saved-plan-time-invalid",
            "saved plan time validation produced no result",
        ));
    };
    validate_plan_freshness(&envelope, now, created, expires)?;
    Ok(envelope)
}

fn validate_saved_identity(envelope: &SavedPlanEnvelope, violations: &mut Vec<Violation>) {
    require_equal(
        violations,
        "saved-plan-schema",
        &envelope.schema,
        "aex.saved-plan-envelope.v1",
    );
    for (rule, digest) in [
        ("release-id-invalid", &envelope.release_id),
        ("binding-digest-invalid", &envelope.binding_digest),
        ("placement-digest-invalid", &envelope.placement_digest),
        ("saved-plan-digest-invalid", &envelope.plan.digest),
        ("release-tool-digest-invalid", &envelope.tool.digest),
        (
            "source-archive-digest-invalid",
            &envelope.source_archive.digest,
        ),
        (
            "module-bundle-digest-invalid",
            &envelope.module_bundle.digest,
        ),
        (
            "regional-tables-digest-invalid",
            &envelope.regional_tables.digest,
        ),
        (
            "terraform-root-digest-invalid",
            &envelope.terraform.root_digest,
        ),
        (
            "provider-lock-digest-invalid",
            &envelope.terraform.provider_lock_digest,
        ),
        (
            "terraform-binary-digest-invalid",
            &envelope.terraform.binary.digest,
        ),
    ] {
        validate_sha256(rule, digest, violations);
    }
    validate_sha1("binding-ref-invalid", &envelope.binding_ref, violations);
    for (rule, size) in [
        ("saved-plan-size-invalid", envelope.plan.size_bytes),
        ("release-tool-size-invalid", envelope.tool.size_bytes),
        (
            "source-archive-size-invalid",
            envelope.source_archive.size_bytes,
        ),
        (
            "module-bundle-size-invalid",
            envelope.module_bundle.size_bytes,
        ),
        (
            "regional-tables-size-invalid",
            envelope.regional_tables.size_bytes,
        ),
        (
            "terraform-binary-size-invalid",
            envelope.terraform.binary.size_bytes,
        ),
    ] {
        if size == 0 {
            violations.push(Violation::new(rule, "sizeBytes must be greater than zero"));
        }
    }
    if !valid_blake3(&envelope.regional_tables.definitions_digest) {
        violations.push(Violation::new(
            "regional-tables-definitions-digest-invalid",
            "regionalTables.definitionsDigest must be one lowercase BLAKE3 digest",
        ));
    }
    require_version(
        "release-tool-version-invalid",
        &envelope.tool.version,
        violations,
    );
    validate_runner(&envelope.runner, violations);
}

fn valid_blake3(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_terraform(terraform: &TerraformIdentity, violations: &mut Vec<Violation>) {
    require_version(
        "terraform-version-invalid",
        &terraform.binary.version,
        violations,
    );
    require_safe_relative("terraform-root-invalid", &terraform.root, violations);
    if terraform.providers.is_empty() {
        violations.push(Violation::new(
            "terraform-providers-empty",
            "the saved plan must bind at least one provider",
        ));
    }
    for (source, provider) in &terraform.providers {
        require_non_empty("terraform-provider-source-empty", source, violations);
        require_version(
            "terraform-provider-version-invalid",
            &provider.version,
            violations,
        );
        validate_sha256(
            "terraform-provider-checksum-invalid",
            &provider.checksum,
            violations,
        );
    }
}

fn validate_plan_times(
    envelope: &SavedPlanEnvelope,
    violations: &mut Vec<Violation>,
) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let created = parse_time(
        "saved-plan-created-at-invalid",
        &envelope.created_at,
        violations,
    );
    let expires = parse_time(
        "saved-plan-expires-at-invalid",
        &envelope.expires_at,
        violations,
    );
    let (Some(created), Some(expires)) = (created, expires) else {
        return None;
    };
    if expires <= created {
        violations.push(Violation::new(
            "saved-plan-expiry-order-invalid",
            "expiresAt must be later than createdAt",
        ));
    }
    if expires - created > time::Duration::minutes(SAVED_PLAN_MAX_TTL_MINUTES) {
        violations.push(Violation::new(
            "saved-plan-ttl-too-long",
            format!("saved plans may live for at most {SAVED_PLAN_MAX_TTL_MINUTES} minutes"),
        ));
    }
    Some((created, expires))
}

fn validate_plan_bytes(plan: &SavedPlanIdentity, bytes: Option<&[u8]>) -> Result<()> {
    let Some(bytes) = bytes else { return Ok(()) };
    let mut mismatches = Vec::new();
    if bytes.len() as u64 != plan.size_bytes {
        mismatches.push(Violation::new(
            "saved-plan-size-mismatch",
            format!(
                "envelope records {} bytes but the saved plan has {}",
                plan.size_bytes,
                bytes.len()
            ),
        ));
    }
    let observed = canon::digest_bytes(bytes);
    if observed != plan.digest {
        mismatches.push(Violation::new(
            "saved-plan-digest-mismatch",
            format!(
                "envelope records {} but the saved plan is {observed}",
                plan.digest
            ),
        ));
    }
    finish(Exit::ArtifactMismatch, mismatches, ())
}

fn validate_plan_freshness(
    envelope: &SavedPlanEnvelope,
    now: OffsetDateTime,
    created: OffsetDateTime,
    expires: OffsetDateTime,
) -> Result<()> {
    if now < created {
        return Err(ToolError::single(
            Exit::AdmissionDenied,
            "saved-plan-not-yet-valid",
            format!(
                "plan was created at {} but evaluation time is {now}",
                envelope.created_at
            ),
        ));
    }
    if now >= expires {
        return Err(ToolError::single(
            Exit::AdmissionDenied,
            "saved-plan-expired",
            format!(
                "plan expired at {} and must be replanned",
                envelope.expires_at
            ),
        ));
    }
    Ok(())
}

fn parse_release_value(text: &str, kind: &str) -> Result<Value> {
    serde_json::from_str(text).map_err(|error| {
        ToolError::single(
            Exit::ManifestInvalid,
            format!("{kind}-unparseable"),
            error.to_string(),
        )
    })
}

fn deserialize<T: for<'de> Deserialize<'de>>(value: Value, kind: &str) -> Result<T> {
    serde_json::from_value(value).map_err(|error| {
        ToolError::single(
            Exit::ManifestInvalid,
            format!("{kind}-invalid"),
            error.to_string(),
        )
    })
}

fn reject_plaintext_secrets(value: &Value) -> Result<()> {
    let mut violations = Vec::new();
    scan_secret_shapes(value, "$", &mut violations);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::ManifestInvalid, violations))
    }
}

fn scan_secret_shapes(value: &Value, path: &str, violations: &mut Vec<Violation>) {
    match value {
        Value::Object(members) => {
            for (key, child) in members {
                let normalized: String = key
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .flat_map(char::to_lowercase)
                    .collect();
                if matches!(
                    normalized.as_str(),
                    "password"
                        | "plaintext"
                        | "secretvalue"
                        | "secretaccesskey"
                        | "apikeyvalue"
                        | "tokenvalue"
                        | "credentialvalue"
                        | "privatekey"
                ) {
                    violations.push(Violation::new(
                        "release-contract-plaintext-secret",
                        format!("{path}.{key} is a plaintext credential-shaped field"),
                    ));
                }
                scan_secret_shapes(child, &format!("{path}.{key}"), violations);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                scan_secret_shapes(child, &format!("{path}[{index}]"), violations);
            }
        }
        Value::String(text) => {
            if text.contains("-----BEGIN PRIVATE KEY-----")
                || is_aws_access_key(text)
                || has_uri_userinfo(text)
            {
                violations.push(Violation::new(
                    "release-contract-plaintext-secret",
                    format!("{path} contains a plaintext credential shape"),
                ));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn validate_self_digest(
    value: &Value,
    field: &str,
    recorded: &str,
    rule: &str,
    violations: &mut Vec<Violation>,
) -> Result<()> {
    validate_sha256(rule, recorded, violations);
    let expected = canon::digest_document_excluding(value, &[field])?;
    if expected != recorded {
        violations.push(Violation::new(
            rule,
            format!("recorded {recorded}, computed {expected}"),
        ));
    }
    Ok(())
}

fn validate_regions(regions: &[String], violations: &mut Vec<Violation>) {
    if regions.is_empty() {
        violations.push(Violation::new(
            "binding-regions-empty",
            "regions must contain at least one region",
        ));
    }
    let mut seen = BTreeSet::new();
    for region in regions {
        if !is_region(region) {
            violations.push(Violation::new(
                "binding-region-invalid",
                format!("`{region}` is not an AWS region"),
            ));
        }
        if !seen.insert(region) {
            violations.push(Violation::new(
                "binding-region-duplicate",
                format!("`{region}` appears more than once"),
            ));
        }
    }
    if regions.windows(2).any(|pair| pair[0] >= pair[1]) {
        violations.push(Violation::new(
            "binding-regions-not-sorted",
            "regions must be unique and in ascending lexical order",
        ));
    }
}

fn validate_runner(runner: &RunnerIdentity, violations: &mut Vec<Violation>) {
    require_equal(
        violations,
        "saved-plan-runner-os-invalid",
        &runner.os,
        "linux",
    );
    require_equal(
        violations,
        "saved-plan-runner-arch-invalid",
        &runner.arch,
        "x86_64",
    );
    for (rule, path) in [
        (
            "saved-plan-working-directory-invalid",
            runner.working_directory.as_str(),
        ),
        (
            "saved-plan-module-extraction-path-invalid",
            runner.module_extraction_path.as_str(),
        ),
    ] {
        if !is_absolute_posix_path(path) {
            violations.push(Violation::new(
                rule,
                format!("`{path}` is not a normalized absolute POSIX path"),
            ));
        }
    }
    if !runner.module_extraction_path.starts_with(&format!(
        "{}/",
        runner.working_directory.trim_end_matches('/')
    )) {
        violations.push(Violation::new(
            "saved-plan-module-path-outside-workdir",
            "runner.moduleExtractionPath must be below runner.workingDirectory",
        ));
    }
}

fn validate_state(state: &StateIdentity, violations: &mut Vec<Violation>) {
    let (backend, backend_key) = match state {
        StateIdentity::Present {
            backend,
            backend_key,
            lineage,
            object_version_id,
            ..
        } => {
            require_non_empty("terraform-state-lineage-empty", lineage, violations);
            require_non_empty(
                "terraform-state-version-empty",
                object_version_id,
                violations,
            );
            (backend, backend_key)
        }
        StateIdentity::Absent {
            backend,
            backend_key,
        } => (backend, backend_key),
    };

    require_equal(
        violations,
        "terraform-state-backend-kind-invalid",
        &backend.kind,
        "s3",
    );
    validate_bucket(
        "terraform-state-backend-bucket-invalid",
        &backend.bucket,
        violations,
    );
    if !is_region(&backend.region) {
        violations.push(Violation::new(
            "terraform-state-backend-region-invalid",
            "state.backend.region is not an AWS region",
        ));
    }
    validate_sha256(
        "terraform-backend-config-digest-invalid",
        &backend.config_digest,
        violations,
    );
    require_safe_relative("terraform-state-key-invalid", backend_key, violations);
}

fn validate_plan_location(location: &EncryptedPlanLocation, violations: &mut Vec<Violation>) {
    validate_bucket("saved-plan-bucket-invalid", &location.bucket, violations);
    require_safe_relative("saved-plan-key-invalid", &location.key, violations);
    if !is_region(&location.region) {
        violations.push(Violation::new(
            "saved-plan-region-invalid",
            "plan.location.region is not an AWS region",
        ));
    }
    require_non_empty(
        "saved-plan-object-version-empty",
        &location.object_version_id,
        violations,
    );
    let kms_prefix = format!("arn:aws:kms:{}:", location.region);
    if !kms_key_regex().is_match(&location.kms_key_arn)
        || !location.kms_key_arn.starts_with(&kms_prefix)
    {
        violations.push(Violation::new(
            "saved-plan-kms-key-invalid",
            "plan.location.kmsKeyArn must be an exact KMS key ARN in the plan region",
        ));
    }
}

fn validate_bucket(rule: &str, bucket: &str, violations: &mut Vec<Violation>) {
    let valid = (3..=63).contains(&bucket.len())
        && !bucket.starts_with('-')
        && !bucket.ends_with('-')
        && bucket
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '.');
    if !valid {
        violations.push(Violation::new(
            rule,
            format!("`{bucket}` is not an S3 bucket name"),
        ));
    }
}

fn validate_workflow(workflow: &WorkflowIdentity, violations: &mut Vec<Violation>) {
    if !repository_regex().is_match(&workflow.repository) {
        violations.push(Violation::new(
            "saved-plan-workflow-repository-invalid",
            "workflow.repository must be owner/repository",
        ));
    }
    if !workflow.repository_id.chars().all(|ch| ch.is_ascii_digit())
        || workflow.repository_id.is_empty()
    {
        violations.push(Violation::new(
            "saved-plan-workflow-repository-id-invalid",
            "workflow.repositoryId must be GitHub's numeric repository id",
        ));
    }
    validate_sha1(
        "saved-plan-workflow-commit-invalid",
        &workflow.commit_sha,
        violations,
    );
    if !workflow_ref_regex().is_match(&workflow.workflow_ref)
        || !workflow
            .workflow_ref
            .starts_with(&format!("{}/", workflow.repository))
    {
        violations.push(Violation::new(
            "saved-plan-workflow-ref-invalid",
            "workflow.workflowRef must pin a workflow in workflow.repository to an exact ref",
        ));
    }
    if workflow
        .workflow_ref
        .rsplit_once('@')
        .is_none_or(|(_, commit)| commit != workflow.commit_sha)
    {
        violations.push(Violation::new(
            "saved-plan-workflow-ref-commit-mismatch",
            "workflow.workflowRef must be pinned to workflow.commitSha",
        ));
    }
    if workflow.run_id.is_empty() || !workflow.run_id.chars().all(|ch| ch.is_ascii_digit()) {
        violations.push(Violation::new(
            "saved-plan-workflow-run-id-invalid",
            "workflow.runId must be numeric",
        ));
    }
    if workflow.run_attempt == 0 {
        violations.push(Violation::new(
            "saved-plan-workflow-attempt-invalid",
            "workflow.runAttempt must be at least one",
        ));
    }
    require_non_empty(
        "saved-plan-workflow-job-empty",
        &workflow.job_name,
        violations,
    );
}

fn validate_sha256(rule: &str, value: &str, violations: &mut Vec<Violation>) {
    if !sha256_regex().is_match(value) {
        violations.push(Violation::new(
            rule,
            format!("`{value}` is not a sha256 digest"),
        ));
    }
}

fn validate_sha1(rule: &str, value: &str, violations: &mut Vec<Violation>) {
    if !sha1_regex().is_match(value) {
        violations.push(Violation::new(
            rule,
            format!("`{value}` is not a 40-hex commit"),
        ));
    }
}

fn validate_logical_name(value: &str, separator: char, violations: &mut Vec<Violation>) {
    let valid = (3..=64).contains(&value.len())
        && value.starts_with(|ch: char| ch.is_ascii_lowercase())
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == separator);
    if !valid {
        violations.push(Violation::new(
            "release-contract-logical-name-invalid",
            format!("`{value}` is not a valid logical name"),
        ));
    }
}

fn require_equal(violations: &mut Vec<Violation>, rule: &str, observed: &str, expected: &str) {
    if observed != expected {
        violations.push(Violation::new(
            rule,
            format!("expected `{expected}`, observed `{observed}`"),
        ));
    }
}

fn require_non_empty(rule: &str, value: &str, violations: &mut Vec<Violation>) {
    if value.is_empty() {
        violations.push(Violation::new(rule, "value must not be empty"));
    }
}

fn require_safe_relative(rule: &str, value: &str, violations: &mut Vec<Violation>) {
    let path = Path::new(value);
    let unsafe_component = path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if value.is_empty() || value.contains('\\') || unsafe_component {
        violations.push(Violation::new(
            rule,
            format!("`{value}` is not a normalized repository-relative path"),
        ));
    }
}

fn require_version(rule: &str, value: &str, violations: &mut Vec<Violation>) {
    if !version_regex().is_match(value) {
        violations.push(Violation::new(
            rule,
            format!("`{value}` is not an exact semantic version"),
        ));
    }
}

fn parse_time(rule: &str, value: &str, violations: &mut Vec<Violation>) -> Option<OffsetDateTime> {
    match OffsetDateTime::parse(value, &Rfc3339) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            violations.push(Violation::new(rule, error.to_string()));
            None
        }
    }
}

fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.chars().all(|ch| ch.is_ascii_digit())
}

fn is_region(value: &str) -> bool {
    region_regex().is_match(value)
}

fn is_aws_access_key(value: &str) -> bool {
    value.len() == 20
        && (value.starts_with("AKIA") || value.starts_with("ASIA"))
        && value
            .chars()
            .skip(4)
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
}

fn has_uri_userinfo(value: &str) -> bool {
    value.split_once("://").is_some_and(|(_, rest)| {
        rest.split('/')
            .next()
            .is_some_and(|authority| authority.contains('@'))
    })
}

fn is_safe_https_endpoint(value: &str) -> bool {
    value.strip_prefix("https://").is_some_and(|rest| {
        !rest.is_empty()
            && !rest.contains(['?', '#', '\\'])
            && rest
                .split('/')
                .next()
                .is_some_and(|authority| !authority.is_empty() && !authority.contains('@'))
    })
}

fn is_absolute_posix_path(value: &str) -> bool {
    value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains(['\\', ':'])
        && value
            .split('/')
            .skip(1)
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn finish<T>(exit: Exit, violations: Vec<Violation>, value: T) -> Result<T> {
    if violations.is_empty() {
        Ok(value)
    } else {
        Err(ToolError::many(exit, violations))
    }
}

fn sha256_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| Regex::new(r"^sha256:[0-9a-f]{64}$").expect("static digest regex"))
}

fn sha1_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| Regex::new(r"^[0-9a-f]{40}$").expect("static commit regex"))
}

fn region_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| Regex::new(r"^[a-z]{2}-[a-z]+-[0-9]$").expect("static region regex"))
}

fn version_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$")
            .expect("static version regex")
    })
}

fn repository_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$").expect("static repository regex")
    })
}

fn workflow_ref_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(
            r"^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+/\.github/workflows/[A-Za-z0-9._-]+\.ya?ml@[0-9a-f]{40}$",
        )
        .expect("static workflow ref regex")
    })
}

fn kms_key_regex() -> &'static Regex {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE.get_or_init(|| {
        Regex::new(r"^arn:aws[a-z-]*:kms:[a-z]{2}-[a-z]+-[0-9]:[0-9]{12}:key/[A-Za-z0-9/_+=,.@-]+$")
            .expect("static KMS key ARN regex")
    })
}
