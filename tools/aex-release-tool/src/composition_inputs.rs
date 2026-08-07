//! Authoritative production of the non-envelope composition identities.
//!
//! The protected workflow supplies only identities it alone knows: the exact
//! GitHub run and the paths of bytes it has published. Everything else is
//! recomputed from those bytes or the checked-out source. This keeps
//! `composition-inputs.json` from becoming an unchecked shell-authored claim.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;

use crate::artifact::ArtifactEnvelope;
use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation};
use crate::graph::inputs::Units;
use crate::manifest::{
    CentralMigrations, CompositionInputs, Infra, Migrations, Policy, PublicSource,
    RegionalMigrations, ReleaseToolIdentity,
};

/// Files whose exact bytes enter the public composition.
#[derive(Debug, Clone, Copy)]
pub struct PublicInputFiles<'a> {
    /// Raw, directly executable release tool.
    pub release_tool: &'a Path,
    /// Deterministic Terraform module bundle.
    pub module_bundle: &'a Path,
    /// Generated regional table bundle.
    pub regional_tables: &'a Path,
}

/// Protected-main workflow identity.
#[derive(Debug, Clone)]
pub struct PublicRunIdentity {
    /// GitHub `owner/repository`.
    pub repository: String,
    /// Exact 40-character source commit.
    pub commit_sha: String,
    /// Positive GitHub Actions run id.
    pub workflow_run_id: String,
    /// Positive run attempt.
    pub workflow_run_attempt: u64,
    /// Version printed by the acquired release tool and checked against source.
    pub release_tool_version: String,
}

/// Identities that can only be derived after the complete envelope set exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeAuthorities {
    /// Exact central schema-admin image digest.
    pub central_admin_image_digest: String,
    /// Central migration bundle digest carried by the schema-admin artifact.
    pub central_bundle_digest: String,
    /// Catalogue names to immutable digests.
    pub catalogs: BTreeMap<String, String>,
}

/// Verify the complete envelope set and derive its composition authorities.
///
/// # Errors
/// Returns the handoff verifier's classified failure for an incomplete set,
/// or [`Exit::CompositionIncompatible`] when the schema admin or Brain omits
/// the identities only those artifacts can establish.
pub fn envelope_authorities(
    registry: &Units,
    envelopes: Vec<ArtifactEnvelope>,
) -> Result<(BTreeMap<String, ArtifactEnvelope>, EnvelopeAuthorities)> {
    let store = crate::manifest::verify_handoff_envelopes(registry, envelopes)?;
    let admin = store.get("central-schema-admin").ok_or_else(|| {
        ToolError::single(
            Exit::CompositionIncompatible,
            "composition-central-admin-missing",
            "the complete registry has no certified central-schema-admin artifact",
        )
    })?;
    let central_admin_image_digest = admin.output.digest.clone();
    let central_bundle_digest = admin
        .identities
        .migration
        .as_ref()
        .and_then(|migration| migration.central_bundle_digest.as_ref())
        .ok_or_else(|| {
            ToolError::single(
                Exit::CompositionIncompatible,
                "composition-central-admin-bundle-missing",
                "the certified central-schema-admin artifact carries no central migration bundle digest",
            )
        })?
        .clone();
    require_sha256(
        &central_bundle_digest,
        "composition-central-admin-bundle-digest",
    )?;
    let brain = store.get("brain-mux").ok_or_else(|| {
        ToolError::single(
            Exit::CompositionIncompatible,
            "composition-brain-missing",
            "the complete registry has no certified brain-mux artifact",
        )
    })?;
    let catalog = brain.identities.catalogs.as_ref().ok_or_else(|| {
        ToolError::single(
            Exit::CompositionIncompatible,
            "composition-catalogs-missing",
            "the certified brain-mux artifact carries no model/tool catalogue identities",
        )
    })?;
    let mut catalogs = BTreeMap::new();
    for (name, digest) in [
        ("model", catalog.model.as_ref()),
        ("tool", catalog.tool.as_ref()),
    ] {
        let digest = digest.ok_or_else(|| {
            ToolError::single(
                Exit::CompositionIncompatible,
                "composition-catalog-missing",
                format!("the certified brain-mux artifact carries no `{name}` catalogue digest"),
            )
        })?;
        require_sha256(digest, "composition-catalog-digest")?;
        catalogs.insert(name.to_owned(), digest.clone());
    }
    Ok((
        store,
        EnvelopeAuthorities {
            central_admin_image_digest,
            central_bundle_digest,
            catalogs,
        },
    ))
}

/// Produce the complete non-envelope input document from checked authorities.
///
/// # Errors
/// Returns a classified refusal when source and published bytes differ, a
/// generated lock is stale, provider versions disagree, or any supplied run
/// identity is not exact.
pub fn produce(
    root: &Path,
    registry: &Units,
    files: PublicInputFiles<'_>,
    run: &PublicRunIdentity,
    authorities: EnvelopeAuthorities,
) -> Result<CompositionInputs> {
    validate_run(root, run)?;
    validate_authorities(root, registry, &authorities)?;
    let published = read_published_inputs(root, files, &authorities)?;
    let base = release_base(run);
    let source = PublicSource {
        repository: run.repository.clone(),
        commit_sha: run.commit_sha.clone(),
        workflow_run_id: run.workflow_run_id.clone(),
        workflow_run_attempt: run.workflow_run_attempt,
    };

    Ok(CompositionInputs {
        contract_digest: contract_digest(root)?,
        source,
        release_tool: ReleaseToolIdentity {
            version: run.release_tool_version.clone(),
            digest: canon::digest_bytes(&published.tool_bytes),
            size_bytes: published.tool_bytes.len() as u64,
            uri: format!("{base}/aex-release-tool"),
            target: "x86_64-unknown-linux-musl".to_owned(),
        },
        // Hosted deployables are built from this exact source checkout. The
        // registry contains no npm-package unit, so an empty registry map is an
        // explicit fact rather than a missing publication claim.
        packages: BTreeMap::new(),
        migrations: Migrations {
            central: CentralMigrations {
                bundle_digest: published.central_bundle_digest,
                head: published.central_head,
                admin_image_digest: authorities.central_admin_image_digest,
            },
            regional: RegionalMigrations {
                bundle_digest: canon::digest_bytes(&published.regional_bytes),
                bundle_size_bytes: published.regional_bytes.len() as u64,
                bundle_uri: format!("{base}/regional-tables.json"),
                definitions_digest: published.regional_identity.definitions_digest,
                generation: published.regional_identity.generation,
            },
        },
        infra: Infra {
            module_bundle_digest: canon::digest_bytes(&published.module_bytes),
            module_bundle_size_bytes: published.module_bytes.len() as u64,
            module_bundle_uri: format!("{base}/terraform-modules.tar.gz"),
            terraform_version: terraform_version(root)?,
            provider_versions: provider_versions(root)?,
        },
        catalogs: authorities.catalogs,
        policy: policy(root)?,
    })
}

fn validate_authorities(
    root: &Path,
    registry: &Units,
    authorities: &EnvelopeAuthorities,
) -> Result<()> {
    require_sha256(
        &authorities.central_admin_image_digest,
        "composition-central-admin-digest",
    )?;
    let source_tool_catalog = crate::artifact::tool_catalog_digest(root)?;
    if authorities.catalogs.get("tool") != Some(&source_tool_catalog) {
        return Err(ToolError::single(
            Exit::CompositionIncompatible,
            "composition-tool-catalog-mismatch",
            format!(
                "the certified brain-mux tool catalogue is `{}`, not source-bound `{source_tool_catalog}`",
                authorities.catalogs.get("tool").map_or("<missing>", String::as_str)
            ),
        ));
    }
    if registry.units.iter().any(|unit| unit.kind == "npm-package") {
        return Err(ToolError::single(
            Exit::CompositionIncompatible,
            "composition-package-publication-missing",
            "the deployable registry contains an npm package but no registry publication identity was supplied",
        ));
    }
    Ok(())
}

struct PublishedInputs {
    tool_bytes: Vec<u8>,
    module_bytes: Vec<u8>,
    regional_bytes: Vec<u8>,
    regional_identity: crate::publication::RegionalTablesIdentity,
    central_bundle_digest: String,
    central_head: String,
}

fn read_published_inputs(
    root: &Path,
    files: PublicInputFiles<'_>,
    authorities: &EnvelopeAuthorities,
) -> Result<PublishedInputs> {
    let tool_bytes = read(files.release_tool, "composition-release-tool-missing")?;
    let module_bytes = read(files.module_bundle, "composition-module-bundle-missing")?;
    let expected_modules = crate::publication::package_module_bundle(root)?;
    if module_bytes != expected_modules {
        return Err(ToolError::single(
            Exit::ArtifactMismatch,
            "composition-module-bundle-mismatch",
            "the acquired Terraform module bundle is not the deterministic bundle of this source commit",
        ));
    }
    let (tracked_regional, regional_identity) = crate::publication::regional_tables_bundle(root)?;
    let regional_bytes = read(files.regional_tables, "composition-regional-tables-missing")?;
    if regional_bytes != tracked_regional {
        return Err(ToolError::single(
            Exit::ArtifactMismatch,
            "composition-regional-tables-mismatch",
            "the acquired regional table bundle is not the generated bundle in this source commit",
        ));
    }
    let central = crate::migration::build_bundle(root)?;
    let central_bytes = canon::to_file_bytes(&central)?;
    let central_bundle_digest = canon::digest_bytes(&central_bytes);
    let central_lock = root.join("migrations/central/bundle.lock.json");
    if read(&central_lock, "composition-central-lock-missing")? != central_bytes {
        return Err(ToolError::single(
            Exit::ManifestInvalid,
            "composition-central-lock-stale",
            "migrations/central/bundle.lock.json does not match the canonical migration source bundle",
        ));
    }
    if authorities.central_bundle_digest != central_bundle_digest {
        return Err(ToolError::single(
            Exit::CompositionIncompatible,
            "composition-central-admin-bundle-mismatch",
            format!(
                "the certified central-schema-admin bundle is `{}`, not source-bound `{central_bundle_digest}`",
                authorities.central_bundle_digest
            ),
        ));
    }
    Ok(PublishedInputs {
        tool_bytes,
        module_bytes,
        regional_bytes,
        regional_identity,
        central_bundle_digest,
        central_head: central.head,
    })
}

fn validate_run(root: &Path, run: &PublicRunIdentity) -> Result<()> {
    let exact_commit = run.commit_sha.len() == 40
        && run
            .commit_sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    let exact_repository = run.repository == "aexhq/aex";
    let positive_run = !run.workflow_run_id.is_empty()
        && !run.workflow_run_id.starts_with('0')
        && run
            .workflow_run_id
            .bytes()
            .all(|byte| byte.is_ascii_digit());
    let version = Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$")
        .expect("static version regex");
    let mut violations = Vec::new();
    if !exact_repository || !exact_commit || !positive_run || run.workflow_run_attempt == 0 {
        violations.push(Violation::new(
            "composition-source-identity",
            "composition production requires exact repository, commit, positive run and attempt identities",
        ));
    }
    if !version.is_match(&run.release_tool_version) {
        violations.push(Violation::new(
            "composition-tool-version",
            "release-tool version must be an exact semantic version",
        ));
    }
    let manifest: toml::Value = toml::from_str(&read_text(
        &root.join("tools/aex-release-tool/Cargo.toml"),
        "composition-release-tool-manifest-missing",
    )?)
    .map_err(|error| {
        ToolError::single(
            Exit::ManifestInvalid,
            "composition-release-tool-manifest-invalid",
            error.to_string(),
        )
    })?;
    let source_version = manifest
        .get("package")
        .and_then(|value| value.get("version"))
        .and_then(toml::Value::as_str);
    if source_version != Some(run.release_tool_version.as_str()) {
        violations.push(Violation::new(
            "composition-release-tool-version-mismatch",
            format!(
                "acquired release tool reports `{}`, source package declares `{}`",
                run.release_tool_version,
                source_version.unwrap_or("<missing>")
            ),
        ));
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::ManifestInvalid, violations))
    }
}

fn terraform_version(root: &Path) -> Result<String> {
    let workflow = read_text(
        &root.join(".github/workflows/_terraform-lane.yml"),
        "composition-terraform-workflow-missing",
    )?;
    let version = Regex::new(r#"(?m)^\s*terraform_version:\s*[\"']?([^\s\"']+)[\"']?\s*$"#)
        .expect("static Terraform workflow version regex");
    let versions = version
        .captures_iter(&workflow)
        .map(|capture| capture[1].to_owned())
        .collect::<Vec<_>>();
    let exact = Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$")
        .expect("static version regex");
    if versions.len() != 1 || !exact.is_match(&versions[0]) {
        return Err(ToolError::single(
            Exit::ManifestInvalid,
            "composition-terraform-version-ambiguous",
            "the public Terraform lane must carry exactly one exact terraform_version",
        ));
    }
    Ok(versions[0].clone())
}

fn contract_digest(root: &Path) -> Result<String> {
    let path = root.join("api/generated/bundle.lock.json");
    let value: serde_json::Value =
        serde_json::from_slice(&read(&path, "composition-contract-lock-missing")?).map_err(
            |error| {
                ToolError::single(
                    Exit::ManifestInvalid,
                    "composition-contract-lock-invalid",
                    error.to_string(),
                )
            },
        )?;
    let digest = value
        .get("contractDigest")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ToolError::single(
                Exit::ManifestInvalid,
                "composition-contract-digest-missing",
                "api/generated/bundle.lock.json carries no contractDigest",
            )
        })?
        .to_owned();
    require_sha256(&digest, "composition-contract-digest")?;
    Ok(digest)
}

fn policy(root: &Path) -> Result<Policy> {
    let toolchain: toml::Value = toml::from_str(&read_text(
        &root.join("rust-toolchain.toml"),
        "composition-toolchain-missing",
    )?)
    .map_err(|error| {
        ToolError::single(
            Exit::ManifestInvalid,
            "composition-toolchain-invalid",
            error.to_string(),
        )
    })?;
    let channel = toolchain
        .get("toolchain")
        .and_then(|value| value.get("channel"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| {
            ToolError::single(
                Exit::ManifestInvalid,
                "composition-toolchain-channel-missing",
                "rust-toolchain.toml carries no pinned toolchain.channel",
            )
        })?;
    let artifact = read(
        &root.join("release/policy/artifact-policy.toml"),
        "composition-artifact-policy-missing",
    )?;
    let freshness = read(
        &root.join("release/policy/freshness.toml"),
        "composition-freshness-policy-missing",
    )?;
    Ok(Policy {
        toolchain_channel: channel.to_owned(),
        artifact_policy_digest: canon::digest_bytes(&artifact),
        freshness_policy_digest: canon::digest_bytes(&freshness),
        source_policy_version: 1,
    })
}

fn provider_versions(root: &Path) -> Result<BTreeMap<String, String>> {
    let provider =
        Regex::new(r#"(?s)provider\s+\"([^\"]+)\"\s*\{.*?version\s*=\s*\"([^\"]+)\".*?\}"#)
            .expect("static provider lock regex");
    let modules = root.join("infra/modules");
    let mut versions = BTreeMap::new();
    let mut locks = 0_u64;
    for entry in walkdir::WalkDir::new(&modules).follow_links(false) {
        let entry = entry.map_err(|error| {
            ToolError::single(
                Exit::ManifestInvalid,
                "composition-provider-lock-walk",
                error.to_string(),
            )
        })?;
        if entry.file_type().is_symlink()
            || !entry.file_type().is_file()
            || entry.file_name() != ".terraform.lock.hcl"
        {
            continue;
        }
        locks += 1;
        let content = read_text(entry.path(), "composition-provider-lock-missing")?;
        for captures in provider.captures_iter(&content) {
            let name = captures[1]
                .strip_prefix("registry.terraform.io/")
                .unwrap_or(&captures[1])
                .to_owned();
            let version = captures[2].to_owned();
            if let Some(existing) = versions.insert(name.clone(), version.clone())
                && existing != version
            {
                return Err(ToolError::single(
                    Exit::ManifestInvalid,
                    "composition-provider-version-drift",
                    format!("provider `{name}` resolves to both `{existing}` and `{version}`"),
                ));
            }
        }
    }
    if locks == 0 || versions.is_empty() {
        return Err(ToolError::single(
            Exit::ManifestInvalid,
            "composition-provider-closure-empty",
            "infra/modules contains no provider lock closure",
        ));
    }
    Ok(versions)
}

fn release_base(run: &PublicRunIdentity) -> String {
    format!(
        "https://github.com/{}/releases/download/main-{}-run-{}-attempt-{}",
        run.repository, run.commit_sha, run.workflow_run_id, run.workflow_run_attempt
    )
}

fn require_sha256(value: &str, rule: &'static str) -> Result<()> {
    if value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(ToolError::single(
            Exit::ManifestInvalid,
            rule,
            format!("`{value}` is not a lowercase SHA-256 digest"),
        ))
    }
}

fn read(path: &Path, rule: &'static str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|error| {
        ToolError::single(
            Exit::ManifestInvalid,
            rule,
            format!("cannot read `{}`: {error}", path.display()),
        )
    })
}

fn read_text(path: &Path, rule: &'static str) -> Result<String> {
    std::fs::read_to_string(path).map_err(|error| {
        ToolError::single(
            Exit::ManifestInvalid,
            rule,
            format!("cannot read `{}`: {error}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::{
        EnvelopeAuthorities, PublicInputFiles, PublicRunIdentity, produce, provider_versions,
        terraform_version,
    };
    use crate::canon;
    use crate::graph::inputs::Units;

    fn workspace() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root")
    }

    fn digest(byte: u8) -> String {
        format!("sha256:{}", format!("{byte:02x}").repeat(32))
    }

    fn registry(root: &Path) -> Units {
        toml::from_str(
            &std::fs::read_to_string(root.join("release/units.toml")).expect("unit registry"),
        )
        .expect("valid registry")
    }

    fn authorities(root: &Path) -> EnvelopeAuthorities {
        let bundle = crate::migration::build_bundle(root).expect("central bundle");
        EnvelopeAuthorities {
            central_admin_image_digest: digest(1),
            central_bundle_digest: canon::digest_bytes(
                &canon::to_file_bytes(&bundle).expect("canonical central bundle"),
            ),
            catalogs: BTreeMap::from([
                ("model".to_owned(), digest(2)),
                (
                    "tool".to_owned(),
                    "sha256:b3cae3e3b5cb64b3ca274f22f67c3ba1e305ac4ca14084967348f06d0ba0fdec"
                        .to_owned(),
                ),
            ]),
        }
    }

    fn run() -> PublicRunIdentity {
        PublicRunIdentity {
            repository: "aexhq/aex".to_owned(),
            commit_sha: "1".repeat(40),
            workflow_run_id: "42".to_owned(),
            workflow_run_attempt: 3,
            release_tool_version: "0.1.0".to_owned(),
        }
    }

    fn acquired(root: &Path, temp: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let tool = temp.join("aex-release-tool");
        let modules = temp.join("terraform-modules.tar.gz");
        let regional = temp.join("regional-tables.json");
        std::fs::write(&tool, b"release tool bytes").expect("tool bytes");
        std::fs::write(
            &modules,
            crate::publication::package_module_bundle(root).expect("module bundle"),
        )
        .expect("module bytes");
        std::fs::write(
            &regional,
            crate::publication::regional_tables_bundle(root)
                .expect("regional bundle")
                .0,
        )
        .expect("regional bytes");
        (tool, modules, regional)
    }

    #[test]
    fn composition_is_derived_from_source_and_exact_acquired_bytes() {
        let root = workspace();
        let temp = tempfile::tempdir().expect("tempdir");
        let (tool, modules, regional) = acquired(&root, temp.path());
        let units = registry(&root);
        assert!(units.units.iter().all(|unit| unit.kind != "npm-package"));
        let inputs = produce(
            &root,
            &units,
            PublicInputFiles {
                release_tool: &tool,
                module_bundle: &modules,
                regional_tables: &regional,
            },
            &run(),
            authorities(&root),
        )
        .expect("authoritative composition");

        assert_eq!(inputs.source.workflow_run_attempt, 3);
        assert_eq!(
            inputs.release_tool.digest,
            canon::digest_bytes(b"release tool bytes")
        );
        assert_eq!(inputs.infra.terraform_version, "1.14.0");
        assert_eq!(inputs.infra.provider_versions["hashicorp/aws"], "6.57.1");
        assert_eq!(inputs.migrations.regional.generation, 1);
        assert_eq!(inputs.catalogs["tool"], authorities(&root).catalogs["tool"]);
        assert!(inputs.packages.is_empty());
        assert_eq!(
            inputs.release_tool.uri,
            format!(
                "https://github.com/aexhq/aex/releases/download/main-{}-run-42-attempt-3/aex-release-tool",
                "1".repeat(40)
            )
        );
    }

    #[test]
    fn acquired_module_bytes_cannot_disagree_with_source() {
        let root = workspace();
        let temp = tempfile::tempdir().expect("tempdir");
        let (tool, modules, regional) = acquired(&root, temp.path());
        std::fs::write(&modules, b"not the module closure").expect("tampered module");
        let error = produce(
            &root,
            &registry(&root),
            PublicInputFiles {
                release_tool: &tool,
                module_bundle: &modules,
                regional_tables: &regional,
            },
            &run(),
            authorities(&root),
        )
        .expect_err("mismatched acquisition must fail");
        assert_eq!(error.rules(), vec!["composition-module-bundle-mismatch"]);
    }

    #[test]
    fn provider_version_drift_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        for (module, version) in [("one", "6.57.1"), ("two", "6.58.0")] {
            let directory = temp.path().join("infra/modules").join(module);
            std::fs::create_dir_all(&directory).expect("module directory");
            std::fs::write(
                directory.join(".terraform.lock.hcl"),
                format!(
                    "provider \"registry.terraform.io/hashicorp/aws\" {{\n  version = \"{version}\"\n}}\n"
                ),
            )
            .expect("lock file");
        }
        let error = provider_versions(temp.path()).expect_err("provider drift must fail");
        assert_eq!(error.rules(), vec!["composition-provider-version-drift"]);
    }

    #[test]
    fn registry_packages_require_real_publication_identities() {
        let root = workspace();
        let mut units = registry(&root);
        units.units[0].kind = "npm-package".to_owned();
        let missing = root.join("does-not-exist");
        let error = produce(
            &root,
            &units,
            PublicInputFiles {
                release_tool: &missing,
                module_bundle: &missing,
                regional_tables: &missing,
            },
            &run(),
            authorities(&root),
        )
        .expect_err("package identities cannot be invented");
        assert_eq!(
            error.rules(),
            vec!["composition-package-publication-missing"]
        );
    }

    #[test]
    fn certified_brain_tool_catalog_must_equal_source() {
        let root = workspace();
        let mut wrong = authorities(&root);
        wrong.catalogs.insert("tool".to_owned(), digest(9));
        let missing = root.join("does-not-exist");
        let error = produce(
            &root,
            &registry(&root),
            PublicInputFiles {
                release_tool: &missing,
                module_bundle: &missing,
                regional_tables: &missing,
            },
            &run(),
            wrong,
        )
        .expect_err("artifact catalogue cannot disagree with source");
        assert_eq!(error.rules(), vec!["composition-tool-catalog-mismatch"]);
    }

    #[test]
    fn terraform_version_comes_from_the_executed_public_lane() {
        assert_eq!(
            terraform_version(&workspace()).expect("pinned version"),
            "1.14.0"
        );
    }
}
