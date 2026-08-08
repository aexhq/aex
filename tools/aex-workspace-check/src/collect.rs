//! Reading the workspace into the registry's inputs.
//!
//! Everything here is I/O; every rule that consumes it is a pure function, so a
//! rule is unit-tested against a fixture and this module is exercised against
//! the real tree by the integration test.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::flake::{SourceHit, SourceScan, scan_env_reads, scan_ignores};
use crate::metadata::{DependencyKind, WorkspaceMetadata};
use crate::policy::Policy;
use crate::registry::{
    Authorities, PackageKind, PackageRow, Phase, RegistryInput, SourceFindings, WorkloadRow,
};

/// npm manifest globs, relative to the workspace root.
///
/// These mirror the root `package.json` `workspaces` array. They are public
/// because `aex-release-tool` classifies the same directories and a second
/// hand-written copy of this list is exactly the drift both crates exist to
/// prevent.
pub const NPM_ROOTS: &[&str] = &["packages", "apps"];

/// npm packages outside the two wildcard workspace roots.
///
/// `tools/eslint-plugin-aex` is referenced by path. The two Stripe edges live
/// under `services/` beside Cargo members and are literal root Bun workspaces.
/// They stay explicit here because this collector expands package-group roots,
/// while Bun expands the root manifest's literal package entries.
/// Public for the same reason as [`NPM_ROOTS`]: the delivery graph must see the
/// same package set this crate does, or the two authorities derive different
/// live targets from the same tree.
pub const NPM_EXPLICIT: &[&str] = &[
    "services/stripe-command-edge",
    "services/stripe-webhook-edge",
    "tools/eslint-plugin-aex",
];

/// File names that must never exist.
const QUARANTINE_FILES: &[&str] = &[
    ".test-quarantine.json",
    "expected-failures.json",
    "expected-failures.toml",
    "expected-failures.txt",
];

/// Directory names that must never exist.
const QUARANTINE_DIRECTORIES: &[&str] = &["non-gating"];

/// Directories never walked.
const SKIPPED_DIRECTORIES: &[&str] = &["target", "node_modules", ".git", ".jj"];

/// Why the workspace could not be read.
#[derive(Debug, thiserror::Error)]
pub enum CollectError {
    /// A file could not be read.
    #[error("cannot read `{path}`: {source}")]
    Read {
        /// The path that failed.
        path: String,
        /// The underlying failure.
        source: std::io::Error,
    },
    /// A document could not be parsed.
    #[error("cannot parse `{path}`: {detail}")]
    Parse {
        /// The path that failed.
        path: String,
        /// What went wrong.
        detail: String,
    },
}

/// Everything read from the tree, ready for the rules.
#[derive(Debug)]
pub struct Collected {
    /// Cargo and npm packages.
    pub packages: Vec<PackageRow>,
    /// Declared load workloads.
    pub workloads: Vec<WorkloadRow>,
    /// Which optional authorities exist.
    pub authorities: Authorities,
    /// What the tree scan found.
    pub scan: SourceScan,
    /// Cargo targets present without arming optional feature-gated lanes.
    default_test_targets: BTreeMap<String, BTreeSet<String>>,
}

impl Collected {
    /// The registry's input view of what was collected.
    #[must_use]
    pub fn as_input<'a>(&'a self, policy: &'a Policy, phase: Phase) -> RegistryInput<'a> {
        RegistryInput {
            packages: self.packages.clone(),
            policy,
            workloads: self.workloads.clone(),
            authorities: self.authorities.clone(),
            source: SourceFindings {
                image_literals: self.scan.image_literals.clone(),
                quarantine_files: self.scan.quarantine_files.clone(),
            },
            collected: None,
            phase,
        }
    }

    /// Package name to the test target names its manifest declares, for the
    /// flake scanner's empty-target rule.
    #[must_use]
    pub fn declared_targets(&self, include_feature_gated: bool) -> BTreeMap<String, Vec<String>> {
        self.packages
            .iter()
            .filter_map(|package| {
                let meta = package.raw_meta.as_ref()?;
                let targets = meta.get("targets")?.as_object()?;
                let enabled = self.default_test_targets.get(&package.name)?;
                // Sorted explicitly. A `serde_json::Map` iterates in key order
                // only while `preserve_order` is off, and that feature is not
                // ours to control: `aws-smithy-http-client` enables it, as does
                // `serde_with` via `aws_lambda_events`. A workspace-wide build
                // therefore hands us an insertion-ordered `IndexMap` and yields
                // manifest order, while `cargo test -p aex-workspace-check`
                // yields key order. Without this sort the `flake-empty-target`
                // violations built from this list come out in an order that
                // depends on which crates happen to share the build.
                let mut names = targets
                    .keys()
                    .filter(|target| include_feature_gated || enabled.contains(target.as_str()))
                    .cloned()
                    .collect::<Vec<String>>();
                names.sort();
                Some((package.name.clone(), names))
            })
            .filter(|(_, targets): &(String, Vec<String>)| !targets.is_empty())
            .collect()
    }
}

/// Reads the whole workspace.
///
/// # Errors
///
/// Returns [`CollectError`] when a manifest or descriptor cannot be read or
/// parsed. A document that cannot be read is never treated as absent, because
/// "absent" and "unreadable" have different fixes.
pub fn collect(root: &Path, metadata: &WorkspaceMetadata) -> Result<Collected, CollectError> {
    let default_test_targets = default_test_targets(metadata);
    let mut packages = cargo_packages(metadata);
    packages.extend(npm_packages(root)?);
    packages.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(Collected {
        workloads: workloads(root)?,
        authorities: authorities(root)?,
        scan: scan_tree(root, &packages)?,
        default_test_targets,
        packages,
    })
}

fn default_test_targets(metadata: &WorkspaceMetadata) -> BTreeMap<String, BTreeSet<String>> {
    metadata
        .members()
        .into_iter()
        .map(|package| {
            let targets = package
                .targets
                .iter()
                .filter(|target| {
                    target.required_features.is_empty()
                        && target
                            .kind
                            .iter()
                            .any(|kind| kind == "test" || kind == "bench")
                })
                .map(|target| target.name.clone())
                .collect();
            (package.name.clone(), targets)
        })
        .collect()
}

fn cargo_packages(metadata: &WorkspaceMetadata) -> Vec<PackageRow> {
    let directories = metadata.member_directories().unwrap_or_default();
    metadata
        .members()
        .into_iter()
        .map(|package| PackageRow {
            path: directories
                .get(&package.name)
                .cloned()
                .unwrap_or_else(|| package.name.clone()),
            name: package.name.clone(),
            kind: PackageKind::Cargo,
            raw_meta: package.aex_metadata().cloned(),
            test_targets: package
                .test_target_names()
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            features: package.features.keys().cloned().collect(),
            normal_dependencies: package
                .dependencies
                .iter()
                .filter(|dependency| dependency.kind() == DependencyKind::Normal)
                .map(|dependency| dependency.name.clone())
                .collect(),
        })
        .collect()
}

fn npm_packages(root: &Path) -> Result<Vec<PackageRow>, CollectError> {
    let mut directories: Vec<PathBuf> = Vec::new();
    for npm_root in NPM_ROOTS {
        let path = root.join(npm_root);
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                directories.push(entry.path());
            }
        }
    }
    for explicit in NPM_EXPLICIT {
        directories.push(root.join(explicit));
    }
    directories.sort();

    let mut packages = Vec::new();
    for directory in directories {
        let manifest = directory.join("package.json");
        if !manifest.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&manifest).map_err(|source| CollectError::Read {
            path: display(&manifest),
            source,
        })?;
        let document: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| CollectError::Parse {
                path: display(&manifest),
                detail: error.to_string(),
            })?;
        let name = document
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        packages.push(PackageRow {
            path: relative(root, &directory),
            name,
            kind: PackageKind::Npm,
            raw_meta: document.get("aex").cloned(),
            test_targets: Vec::new(),
            features: Vec::new(),
            normal_dependencies: Vec::new(),
        });
    }
    Ok(packages)
}

fn workloads(root: &Path) -> Result<Vec<WorkloadRow>, CollectError> {
    let directory = root.join("tests").join("load").join("workloads");
    let mut rows = Vec::new();
    for path in files_under(&directory, "toml") {
        let text = std::fs::read_to_string(&path).map_err(|source| CollectError::Read {
            path: display(&path),
            source,
        })?;
        let document: toml::Value = toml::from_str(&text).map_err(|error| CollectError::Parse {
            path: display(&path),
            detail: error.to_string(),
        })?;
        let string = |key: &str| {
            document
                .get(key)
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let gates = document
            .get("gate")
            .and_then(toml::Value::as_array)
            .map(|gates| {
                gates
                    .iter()
                    .filter_map(|gate| Some(gate.get("id")?.as_str()?.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        rows.push(WorkloadRow {
            path: relative(root, &path),
            id: string("id"),
            owner: string("owner"),
            target: string("target"),
            gates,
        });
    }
    Ok(rows)
}

fn authorities(root: &Path) -> Result<Authorities, CollectError> {
    let routes_path = root
        .join("api")
        .join("generated")
        .join("registries")
        .join("routes.json");
    let routes = if routes_path.is_file() {
        let text = std::fs::read_to_string(&routes_path).map_err(|source| CollectError::Read {
            path: display(&routes_path),
            source,
        })?;
        let document: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| CollectError::Parse {
                path: display(&routes_path),
                detail: error.to_string(),
            })?;
        Some(
            document
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| Some(row.get("operationId")?.as_str()?.to_owned()))
                        .collect()
                })
                .unwrap_or_default(),
        )
    } else {
        None
    };

    let ownership = root.join("release").join("scenario-ownership.toml");
    let scenario_owners = ownership.is_file().then(Vec::new);

    let central = root.join("migrations").join("central");
    let files: Vec<String> = files_under(&central, "sql")
        .into_iter()
        .map(|path| relative(root, &path))
        .collect();
    let migrations = if files.is_empty() { None } else { Some(files) };

    Ok(Authorities {
        routes,
        scenario_owners,
        migrations,
    })
}

fn scan_tree(root: &Path, packages: &[PackageRow]) -> Result<SourceScan, CollectError> {
    let mut scan = SourceScan::default();
    let harness = "tests/support/aex-test-harness";

    for package in packages {
        if package.kind != PackageKind::Cargo {
            continue;
        }
        let directory = root.join(package.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        for path in files_under(&directory, "rs") {
            let relative_path = relative(root, &path);
            let text = std::fs::read_to_string(&path).map_err(|source| CollectError::Read {
                path: relative_path.clone(),
                source,
            })?;
            scan.ignore_attributes
                .extend(scan_ignores(&relative_path, &text));
            if relative_path.contains("/tests/") {
                scan.env_reads.extend(scan_env_reads(&relative_path, &text));
            }
            if !relative_path.starts_with(harness) && contains_image_literal(&text) {
                scan.image_literals.push(relative_path);
            }
        }
    }

    scan.quarantine_files = quarantine(root);
    scan.ignore_attributes.sort();
    scan.env_reads.sort();
    scan.image_literals.sort();
    scan.image_literals.dedup();
    Ok(scan)
}

/// The image references no source file outside the harness may contain.
///
/// Derived from `release/policy/test-images.toml` so adding an image to the
/// substrate automatically bans its literal everywhere else.
#[must_use]
pub fn image_needles() -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Document {
        image: BTreeMap<String, Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        repository: String,
    }
    // Assembled rather than written whole, so this file does not report itself.
    let mut needles = vec![concat!("GenericImage", "::new(").to_owned()];
    if let Ok(document) = toml::from_str::<Document>(crate::policy::TEST_IMAGES_TOML) {
        for entry in document.image.values() {
            needles.push(entry.repository.clone());
            if let Some(short) = entry.repository.rsplit('/').next() {
                needles.push(format!("\"{short}:"));
            }
        }
    }
    needles.sort();
    needles.dedup();
    needles
}

fn contains_image_literal(text: &str) -> bool {
    image_needles().iter().any(|needle| text.contains(needle))
}

fn quarantine(root: &Path) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if path.is_dir() {
                if SKIPPED_DIRECTORIES.contains(&name) {
                    continue;
                }
                if QUARANTINE_DIRECTORIES.contains(&name) {
                    found.insert(relative(root, &path));
                }
                stack.push(path);
            } else if QUARANTINE_FILES.contains(&name) {
                found.insert(relative(root, &path));
            }
        }
    }
    found.into_iter().collect()
}

fn files_under(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![directory.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if path.is_dir() {
                if !SKIPPED_DIRECTORIES.contains(&name) {
                    stack.push(path);
                }
            } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn relative(root: &Path, path: &Path) -> String {
    crate::metadata::normalise(path.strip_prefix(root).unwrap_or(path))
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

/// The library target names of every package that declares layer `unit`, for
/// the doctest rule.
#[must_use]
pub fn doctest_crates(packages: &[PackageRow], metadata: &WorkspaceMetadata) -> BTreeSet<String> {
    let library_targets: BTreeMap<&str, &str> = metadata
        .members()
        .into_iter()
        .filter_map(|package| Some((package.name.as_str(), package.library()?.name.as_str())))
        .collect();
    packages
        .iter()
        .filter(|package| package.kind == PackageKind::Cargo)
        .filter(|package| {
            package
                .raw_meta
                .as_ref()
                .and_then(|meta| meta.get("layers"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|layers| layers.iter().any(|layer| layer.as_str() == Some("unit")))
        })
        .filter_map(|package| library_targets.get(package.name.as_str()).copied())
        .map(ToOwned::to_owned)
        .collect()
}

/// The environment reads and ignore attributes the tree scan found, as the
/// flake scanner consumes them.
#[must_use]
pub fn source_hits(scan: &SourceScan) -> (Vec<SourceHit>, Vec<SourceHit>) {
    (scan.ignore_attributes.clone(), scan.env_reads.clone())
}

#[cfg(test)]
mod tests {
    use super::{
        Authorities, Collected, QUARANTINE_DIRECTORIES, QUARANTINE_FILES, cargo_packages,
        default_test_targets, doctest_crates, image_needles,
    };
    use crate::flake::SourceScan;
    use crate::metadata::WorkspaceMetadata;

    const TARGET_FIXTURE: &str = r#"{
      "workspace_root": "C:/w",
      "workspace_members": ["aex-foo", "brain-mux"],
      "packages": [
        {
          "id": "aex-foo",
          "name": "aex-foo",
          "manifest_path": "C:/w/crates/aex-foo/Cargo.toml",
          "targets": [
            { "name": "aex_foo", "kind": ["lib"] },
            { "name": "properties", "kind": ["test"] },
            { "name": "integration", "kind": ["test"],
              "required-features": ["integration-engines"] }
          ],
          "metadata": { "aex": {
            "layers": ["unit", "integration"],
            "targets": { "properties": "unit", "integration": "integration" }
          }}
        },
        {
          "id": "brain-mux",
          "name": "brain-mux",
          "manifest_path": "C:/w/runtimes/brain-mux/Cargo.toml",
          "targets": [{ "name": "brain-mux", "kind": ["bin"] }],
          "metadata": { "aex": { "layers": ["unit"] } }
        }
      ]
    }"#;

    /// Expected needles are assembled rather than written whole, so this file
    /// is not reported by the scan it defines.
    #[test]
    fn the_image_needles_are_derived_from_the_pinned_substrate() {
        let needles = image_needles();
        for expected in [
            concat!("amazon/", "dynamodb-local"),
            concat!("minio/", "minio"),
            concat!("motoserver/", "moto"),
            concat!("shopify/", "toxiproxy"),
            concat!("stripe/", "stripe-mock"),
            concat!("library/", "postgres"),
            concat!("\"postgres", ":"),
            concat!("GenericImage", "::new("),
        ] {
            assert!(
                needles.iter().any(|needle| needle == expected),
                "`{expected}` is not banned outside the harness; needles: {needles:?}"
            );
        }
    }

    #[test]
    fn the_quarantine_names_cover_the_declared_forms() {
        assert!(QUARANTINE_FILES.contains(&".test-quarantine.json"));
        assert!(
            QUARANTINE_FILES
                .iter()
                .any(|name| name.starts_with("expected-failures."))
        );
        assert!(QUARANTINE_DIRECTORIES.contains(&"non-gating"));
    }

    #[test]
    fn the_unit_lane_excludes_only_targets_that_require_features() {
        let metadata = WorkspaceMetadata::parse(TARGET_FIXTURE).expect("the fixture parses");
        let packages = cargo_packages(&metadata);
        let collected = Collected {
            default_test_targets: default_test_targets(&metadata),
            packages,
            workloads: Vec::new(),
            authorities: Authorities::default(),
            scan: SourceScan::default(),
        };

        assert_eq!(
            collected.declared_targets(false),
            std::collections::BTreeMap::from([(
                "aex-foo".to_owned(),
                vec!["properties".to_owned()]
            )])
        );
        assert_eq!(
            collected.declared_targets(true),
            std::collections::BTreeMap::from([(
                "aex-foo".to_owned(),
                vec!["integration".to_owned(), "properties".to_owned()]
            )])
        );
    }

    #[test]
    fn only_real_library_targets_require_doctest_results() {
        let metadata = WorkspaceMetadata::parse(TARGET_FIXTURE).expect("the fixture parses");
        let packages = cargo_packages(&metadata);
        assert_eq!(
            doctest_crates(&packages, &metadata),
            std::collections::BTreeSet::from(["aex_foo".to_owned()])
        );
    }
}
