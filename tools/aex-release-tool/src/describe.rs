//! Assembling an artifact envelope from a build that happened here.
//!
//! The envelope type, its schema and its verification were complete before this
//! module existed; what was missing was the thing that fills one in from bytes
//! on disk rather than from a workflow run. Every field that a local build can
//! establish — the artifact digest and size, the toolchain, the exact argv, the
//! input closure, the lockfile — is read, never guessed.
//!
//! Every field that only a CI run can establish is set to [`UNEARNED`] (or `0`,
//! or `false`) and named in the returned ledger. That is the whole design point:
//! an envelope that carries a plausible-looking run id nobody issued is worse
//! than no envelope, because `artifact verify` would then pass on a claim
//! nothing backs. A locally described envelope is refused by `artifact verify`
//! by construction — its location is not immutable and its provenance is not
//! attested — and the ledger says which fields caused that and why.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::artifact::{
    Adjacent, ArtifactEnvelope, BaseImage, BuildCommand, BuildPlan, Composition, Identities,
    Inputs, Licenses, Location, Media, MigrationIdentity, Output, Provenance, ReceiptRef,
    Retention, Signature, Source, Target, Toolchain, UnitIdentity, Vulnerabilities, Workflow,
};
use crate::error::{Exit, Result, ToolError, io};
use crate::graph::inputs::Unit;

/// The value every field a local build cannot establish carries.
///
/// It is deliberately not a well-formed digest, run id or identity: anything
/// that parsed as one of those would be indistinguishable from a real value in
/// a document somebody reads six months from now.
pub const UNEARNED: &str = "unearned";

/// One envelope field that could not be established here, and why.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnearnedField {
    /// RFC 6901 JSON pointer into the envelope.
    pub pointer: String,
    /// What would have to happen for the field to hold a value.
    pub reason: String,
}

/// Everything a local build knows about itself.
#[derive(Debug, Clone)]
pub struct LocalBuild<'a> {
    /// The unit that was built.
    pub unit: &'a Unit,
    /// The recipe that was executed.
    pub plan: &'a BuildPlan,
    /// The packaged artifact bytes.
    pub artifact: &'a Path,
    /// Verified OCI layout identity when the artifact bytes are an image
    /// manifest rather than a packaged blob.
    pub oci_identity: Option<&'a crate::oci::OciImageIdentity>,
    /// `owner/repo`.
    pub repository: String,
    /// The commit built.
    pub commit_sha: String,
    /// Whether the tree was clean.
    pub tree_clean: bool,
    /// The git ref built, where one is known.
    pub git_ref: Option<String>,
    /// The toolchain that produced the bytes.
    pub toolchain: Toolchain,
    /// `Cargo.lock` or `bun.lock` digest.
    pub lockfile_digest: String,
    /// The generated contract bundle digest.
    pub contract_digest: String,
    /// The argv that actually produced the bytes, where it differs from the
    /// recipe's.
    ///
    /// Absent is the normal case and means the recipe was executed as written.
    /// Present means the caller ran something else, and [`describe`] records
    /// what ran and says so in the ledger — an envelope that reported the
    /// recipe when the recipe was not what ran would be the one field in the
    /// document nobody could check.
    pub actual_argv: Option<Vec<String>>,
    /// Repository-relative path to blob digest, for every input in the closure.
    pub closure: BTreeMap<String, String>,
    /// Where the bytes are on this machine, repository-relative.
    pub location_uri: String,
    /// Receipts already earned for this artifact.
    pub receipts: Vec<ReceiptRef>,
    /// Envelope creation time, RFC 3339.
    pub created_at: String,
}

/// Split a recorded build target into the platform triple the envelope carries.
///
/// Two forms occur: a Rust triple with an optional glibc floor appended
/// (`aarch64-unknown-linux-gnu.2.34`), and `none`, which the two `TypeScript`
/// edges carry because a bundle is compiled for no triple at all. Its
/// architecture is a deployment property the Lambda function declares, not a
/// property of the bytes, and recording one here would claim the bundle could
/// not run on the other.
#[must_use]
pub fn target_of(recorded: &str) -> Target {
    if recorded == "none" {
        return Target {
            os: "none".to_owned(),
            architecture: "none".to_owned(),
            triple: "none".to_owned(),
            libc_version: None,
        };
    }
    let architecture = if recorded.contains("aarch64") || recorded.contains("arm64") {
        "arm64"
    } else {
        "amd64"
    };
    // The glibc floor is the tail after the last `-gnu.`; a musl or Node target
    // pins no floor, and inventing one would claim a compatibility guarantee
    // the bytes do not carry.
    let libc_version = recorded
        .split_once("-gnu.")
        .map(|(_, floor)| floor.to_owned());
    Target {
        os: "linux".to_owned(),
        architecture: architecture.to_owned(),
        triple: recorded.to_owned(),
        libc_version,
    }
}

/// Build an envelope from a local build, plus the ledger of what it could not
/// establish.
///
/// # Errors
/// Returns [`crate::error::Exit::Usage`] when the artifact bytes cannot be read,
/// and propagates canonicalization failure from sealing.
pub fn describe(build: &LocalBuild<'_>) -> Result<(ArtifactEnvelope, Vec<UnearnedField>)> {
    let bytes = std::fs::read(build.artifact)
        .map_err(|err| io(&build.artifact.display().to_string(), &err))?;
    validate_oci_identity(build, &bytes)?;
    let closure_digest = crate::artifact::input_closure_digest(
        &build.closure,
        build.plan,
        &build.toolchain,
        build.unit.base_image.as_deref(),
    )?;

    let mut unearned = unearned_fields();
    if let Some(actual) = &build.actual_argv
        && *actual != build.plan.argv
    {
        unearned.push(UnearnedField {
            pointer: "/inputs/buildCommand/argv".to_owned(),
            reason: format!(
                "the recipe is `{}` and it did not run here; these bytes were produced by `{}`, \
                 which is what the envelope records",
                build.plan.argv.join(" "),
                actual.join(" ")
            ),
        });
        unearned.sort();
    }

    let envelope = build_envelope(build, &bytes, closure_digest)?;

    Ok((envelope, unearned))
}

fn validate_oci_identity(build: &LocalBuild<'_>, bytes: &[u8]) -> Result<()> {
    let is_oci = build.unit.kind.starts_with("rust-oci-");
    let Some(identity) = build.oci_identity else {
        return if is_oci {
            Err(crate::error::ToolError::single(
                crate::error::Exit::ArtifactMismatch,
                "oci-identity-missing",
                format!(
                    "OCI unit `{}` requires an inspected reproducible image identity",
                    build.unit.id
                ),
            ))
        } else {
            Ok(())
        };
    };
    if !is_oci {
        return Err(crate::error::ToolError::single(
            crate::error::Exit::Usage,
            "oci-identity-unexpected",
            format!(
                "non-OCI unit `{}` cannot use an OCI identity",
                build.unit.id
            ),
        ));
    }
    let expected_base = build.unit.base_image.as_deref().and_then(|reference| {
        reference.rsplit_once('@').map(|(_, digest)| BaseImage {
            r#ref: reference.to_owned(),
            digest: digest.to_owned(),
        })
    });
    let valid = identity.unit == build.unit.id
        && identity.kind == build.unit.kind
        && identity.bin.as_str() == build.unit.bin.as_deref().unwrap_or_default()
        && identity.target == build.unit.target
        && identity.source.repository == build.repository
        && identity.source.commit_sha == build.commit_sha
        && Some(&identity.base_image) == expected_base.as_ref()
        && identity.build_plan_digest == build.plan.digest
        && identity.recipe_digest == identity.toolchain.recipe_digest(build.plan)?
        && identity.toolchain_digest == identity.toolchain.digest()?
        && identity.output_digest == identity.manifest.digest
        && identity.manifest.size_bytes == bytes.len() as u64
        && identity.manifest.digest == crate::canon::digest_bytes(bytes);
    if valid {
        Ok(())
    } else {
        Err(crate::error::ToolError::single(
            crate::error::Exit::ArtifactMismatch,
            "oci-identity-mismatch",
            format!(
                "OCI identity does not bind the exact unit, source, recipe, base and manifest for `{}`",
                build.unit.id
            ),
        ))
    }
}

/// Every envelope field no local build can fill, and why.
fn unearned_fields() -> Vec<UnearnedField> {
    vec![
        UnearnedField {
            pointer: "/source/workflow/runId".to_owned(),
            reason: "no workflow run produced these bytes; the field is issued by the lane that \
                     builds them"
                .to_owned(),
        },
        UnearnedField {
            pointer: "/source/workflow/builderId".to_owned(),
            reason: "the SLSA builder identity is the workflow's own identity token, which only \
                     GitHub issues"
                .to_owned(),
        },
        UnearnedField {
            pointer: "/output/location".to_owned(),
            reason: "the bytes have not reached their immutable public location, so the local \
                     filesystem path is not a publication identity"
                .to_owned(),
        },
        UnearnedField {
            pointer: "/sbom".to_owned(),
            reason: "no SBOM generator is wired; a component count of zero is the absence, not a \
                     bill of materials with no components"
                .to_owned(),
        },
        UnearnedField {
            pointer: "/licenses/verdict".to_owned(),
            reason: "no licence scan ran here; recording `allowed` would be a verdict nobody \
                     reached"
                .to_owned(),
        },
        UnearnedField {
            pointer: "/vulnerabilities".to_owned(),
            reason: "no advisory database was consulted; zero unapproved advisories would claim a \
                     scan that did not happen"
                .to_owned(),
        },
        UnearnedField {
            pointer: "/provenance".to_owned(),
            reason: "attestation requires the workflow's identity token; `attested: false` is why \
                     `artifact verify` refuses this envelope with exit 22"
                .to_owned(),
        },
        UnearnedField {
            pointer: String::new(),
            reason: "the document as a whole is deliberately not valid against \
                     api/schemas/release/artifact-envelope.json: that schema pins \
                     `source.treeClean`, `output.location.immutable`, `licenses.verdict` and \
                     `provenance.attested` as constants a published artifact must satisfy, and \
                     nothing here was published"
                .to_owned(),
        },
    ]
}

#[allow(clippy::too_many_lines)]
fn build_envelope(
    build: &LocalBuild<'_>,
    bytes: &[u8],
    closure_digest: String,
) -> Result<ArtifactEnvelope> {
    let oci = build.oci_identity;
    ArtifactEnvelope {
        schema: "aex.artifact-envelope.v1".to_owned(),
        envelope_digest: "sha256:0".to_owned(),
        artifact_subject_digest: "sha256:0".to_owned(),
        unit: UnitIdentity {
            id: build.unit.id.clone(),
            kind: build.unit.kind.clone(),
            plane: build.unit.plane.clone(),
            family: None,
        },
        media: Media {
            media_type: media_type_of(&build.plan.form).to_owned(),
            form: build.unit.form.clone(),
        },
        source: Source {
            repository: build.repository.clone(),
            commit_sha: build.commit_sha.clone(),
            tree_clean: build.tree_clean,
            r#ref: build.git_ref.clone(),
            workflow: Workflow {
                repository: build.repository.clone(),
                r#ref: UNEARNED.to_owned(),
                path: UNEARNED.to_owned(),
                run_id: UNEARNED.to_owned(),
                run_attempt: 0,
                job_name: UNEARNED.to_owned(),
                builder_id: UNEARNED.to_owned(),
            },
        },
        inputs: Inputs {
            lockfile_digest: build.lockfile_digest.clone(),
            toolchain: build.toolchain.clone(),
            build_profile: build.plan.profile.clone(),
            build_command: BuildCommand {
                argv: build
                    .actual_argv
                    .clone()
                    .unwrap_or_else(|| build.plan.argv.clone()),
                env: build.plan.env.clone(),
                digest: oci.map_or_else(
                    || build.plan.digest.clone(),
                    |image| image.recipe_digest.clone(),
                ),
            },
            input_closure_digest: closure_digest,
            input_closure_count: Some(build.closure.len() as u64),
            base_image: build.unit.base_image.as_ref().and_then(|reference| {
                reference.rsplit_once('@').map(|(_, digest)| BaseImage {
                    r#ref: reference.clone(),
                    digest: digest.to_owned(),
                })
            }),
            build_args: oci.map_or_else(BTreeMap::new, |image| image.toolchain.build_args()),
            source_date_epoch: Some(0),
        },
        output: Output {
            digest: oci.map_or_else(
                || crate::canon::digest_bytes(bytes),
                |image| image.output_digest.clone(),
            ),
            size_bytes: bytes.len() as u64,
            target: target_of(&build.unit.target),
            oci_index_digest: None,
            oci_child_digest: oci.map(|image| image.manifest.digest.clone()),
            oci_config_digest: oci.map(|image| image.config.digest.clone()),
            oci_layer_digests: oci
                .map(|image| {
                    image
                        .layers
                        .iter()
                        .map(|layer| layer.digest.clone())
                        .collect()
                })
                .unwrap_or_default(),
            location: Location {
                kind: "local".to_owned(),
                uri: build.location_uri.clone(),
                // A path on a developer's disk is not an identity, and saying so
                // is what makes `artifact verify` refuse this envelope.
                immutable: false,
                object_version_id: None,
            },
            symbols: None,
        },
        identities: Identities {
            contract_digest: build.contract_digest.clone(),
            telemetry_schema_digest: None,
            config_schema_version: build.unit.config_schema_version,
            config_env_namespace: Some(build.unit.config_env_namespace.clone()),
            migration: migration_identity(&build.unit.id, &build.closure)?,
            catalogs: None,
        },
        composition: Composition {
            minimum: Vec::new(),
            adjacent: Adjacent {
                storage_compatible: true,
                protocol_compatible: true,
                rollback_eligible: false,
                rationale: Some(
                    "nothing is deployed, so no adjacent version exists to roll back to".to_owned(),
                ),
            },
        },
        sbom: crate::artifact::Sbom {
            format: UNEARNED.to_owned(),
            digest: UNEARNED.to_owned(),
            uri: UNEARNED.to_owned(),
            component_count: 0,
        },
        licenses: Licenses {
            policy_digest: UNEARNED.to_owned(),
            verdict: UNEARNED.to_owned(),
            denials: Vec::new(),
            inventory_digest: None,
        },
        vulnerabilities: Vulnerabilities {
            scanner: UNEARNED.to_owned(),
            database: UNEARNED.to_owned(),
            scanned_at: UNEARNED.to_owned(),
            unapproved_critical: 0,
            unapproved_high: 0,
            approved_exceptions: Vec::new(),
        },
        provenance: Provenance {
            predicate_type: "https://slsa.dev/provenance/v1".to_owned(),
            bundle_digest: String::new(),
            uri: None,
            builder_id: UNEARNED.to_owned(),
            attested: false,
        },
        signature: Signature {
            present: false,
            kind: "none".to_owned(),
            key_id: None,
            bundle_digest: None,
        },
        receipts: build.receipts.clone(),
        retention: Retention {
            class: "artifact-bytes".to_owned(),
            expires_at: None,
        },
        created_at: build.created_at.clone(),
    }
    .seal()
}

fn migration_identity(
    unit: &str,
    closure: &BTreeMap<String, String>,
) -> Result<Option<MigrationIdentity>> {
    if unit != "central-schema-admin" {
        return Ok(None);
    }
    let digest = closure
        .get("migrations/central/bundle.lock.json")
        .filter(|digest| {
            digest
                .strip_prefix("sha256:")
                .is_some_and(|value| {
                    value.len() == 64
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                })
        })
        .ok_or_else(|| {
            ToolError::single(
                Exit::ArtifactMismatch,
                "central-migration-identity-missing",
                "central-schema-admin's input closure must contain the canonical central bundle lock",
            )
        })?;
    Ok(Some(MigrationIdentity {
        required_central_head: None,
        central_bundle_digest: Some(digest.clone()),
        regional_bundle_digest: None,
        regional_generation: None,
    }))
}

fn media_type_of(form: &str) -> &'static str {
    match form {
        "lambda-zip" | "microvm-zip" => "application/zip",
        "oci" => "application/vnd.oci.image.manifest.v1+json",
        _ => "application/gzip",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{UNEARNED, migration_identity, target_of};

    #[test]
    fn a_rust_triple_with_a_glibc_floor_keeps_the_floor() {
        let target = target_of("aarch64-unknown-linux-gnu.2.34");
        assert_eq!(target.architecture, "arm64");
        assert_eq!(target.libc_version.as_deref(), Some("2.34"));
        assert_eq!(target.triple, "aarch64-unknown-linux-gnu.2.34");
    }

    #[test]
    fn a_musl_target_pins_no_floor() {
        let target = target_of("x86_64-unknown-linux-musl");
        assert_eq!(target.architecture, "amd64");
        assert_eq!(
            target.libc_version, None,
            "a static binary carries no glibc guarantee to record"
        );
    }

    #[test]
    fn a_bundle_compiled_for_no_triple_claims_no_platform() {
        // The two TypeScript edges. Claiming `arm64` here would say the bundle
        // cannot run on the other architecture, which is not true of a
        // JavaScript file, and the architecture the function is created with is
        // Terraform's decision rather than the artifact's identity.
        let target = target_of("none");
        assert_eq!(target.architecture, "none");
        assert_eq!(target.os, "none");
        assert_eq!(target.triple, "none");
        assert_eq!(target.libc_version, None);
    }

    #[test]
    fn the_unearned_marker_is_not_a_digest_or_an_identity() {
        assert!(!UNEARNED.starts_with("sha256:"));
        assert!(!UNEARNED.starts_with("https://"));
        assert!(UNEARNED.parse::<u64>().is_err());
    }

    #[test]
    fn the_schema_admin_envelope_requires_the_embedded_bundle_identity() {
        let missing = migration_identity("central-schema-admin", &BTreeMap::new())
            .expect_err("an image with no central bundle input has no valid envelope");
        assert!(
            missing
                .rules()
                .contains(&"central-migration-identity-missing")
        );

        let digest = format!("sha256:{}", "a".repeat(64));
        let identity = migration_identity(
            "central-schema-admin",
            &BTreeMap::from([(
                "migrations/central/bundle.lock.json".to_owned(),
                digest.clone(),
            )]),
        )
        .expect("the exact input identity is accepted")
        .expect("schema admin carries migration identity");
        assert_eq!(
            identity.central_bundle_digest.as_deref(),
            Some(digest.as_str())
        );
    }
}
