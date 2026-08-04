//! The four graph authorities and the two release registries they merge with.
//!
//! `cargo metadata`, the npm workspace manifests, the Terraform tree and
//! `release/units.toml` each know something the others do not, and none of them
//! is transcribed by hand. `release/scenario-ownership.toml` is the single
//! exception and carries only the edges no manifest can express: an SDK user
//! test observes `brain-mux` without importing it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{Exit, Result, ToolError, Violation, io};
use crate::meta::AexMeta;

use super::pathmap::PathMap;
use super::{EdgeKind, NodeId, RawEdge};

/// One Cargo workspace member as the graph needs it.
#[derive(Debug, Clone)]
pub struct CargoPackage {
    /// Package name, which is also its directory name.
    pub name: String,
    /// Repository-relative directory.
    pub dir: String,
    /// Ownership metadata, absent when the manifest declares none.
    pub meta: Option<AexMeta>,
    /// Whether the package is published to a registry.
    pub publishable: bool,
    /// Workspace-internal dependency edges.
    pub deps: Vec<(String, EdgeKind)>,
}

/// One npm workspace package.
#[derive(Debug, Clone)]
pub struct NpmPackage {
    /// Package name as published.
    pub name: String,
    /// Repository-relative directory.
    pub dir: String,
    /// Ownership metadata from the top-level `"aex"` object.
    pub meta: Option<AexMeta>,
    /// Whether the package is published to the registry.
    pub publishable: bool,
    /// Workspace-internal dependency edges.
    pub deps: Vec<(String, EdgeKind)>,
}

/// One Terraform module or example root.
#[derive(Debug, Clone)]
pub struct TerraformUnit {
    /// Repository-relative directory below `infra/`, such as `modules/kms-key`.
    pub relative: String,
    /// Repository-relative directory.
    pub dir: String,
    /// Whether this is a root (`examples/`) rather than a reusable module.
    pub is_root: bool,
    /// Ownership metadata from the `aex.toml` sidecar.
    pub meta: Option<AexMeta>,
    /// Relative directories this unit instantiates as modules.
    pub module_deps: Vec<String>,
}

/// The Lambda resource shape a deployable declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LambdaShape {
    /// Memory allocation in MiB.
    pub memory_mb: u32,
    /// Function timeout in seconds.
    pub timeout_s: u32,
    /// Reserved concurrency. Zero throttles the function to a stop, which is a
    /// legal but deliberate choice.
    pub reserved_concurrency: u32,
}

/// The Fargate task shape a deployable declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FargateShape {
    /// Task CPU units.
    pub cpu: u32,
    /// Task memory in MiB.
    pub memory_mb: u32,
    /// Desired task count.
    pub desired_count: u32,
    /// Drain deadline in seconds.
    pub stop_timeout_s: u32,
    /// Container listening port.
    pub port: u16,
}

/// One AWS Lambda `MicroVM` image variant.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MicrovmShape {
    /// Stable variant token used by the image builder and plane binding.
    pub variant: String,
    /// Minimum guest memory accepted by this image.
    pub minimum_memory_mib: u32,
    /// Whether the optional browser package layer is present.
    pub browser: bool,
}

/// One deployable's release registration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    /// Unit id, which is the deployable name.
    pub id: String,
    /// Artifact kind, matching the envelope's `unit.kind`.
    pub kind: String,
    /// Which plane it runs in.
    pub plane: String,
    /// Owning workspace package.
    pub package: String,
    /// Binary target name, where the package builds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<String>,
    /// Recorded build target triple.
    pub target: String,
    /// Cargo profile or build mode.
    pub profile: String,
    /// Packaged media form.
    pub form: String,
    /// Archive entrypoint, such as the Lambda `bootstrap` name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// Digest-pinned base image, for OCI forms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_image: Option<String>,
    /// Configuration environment namespace.
    pub config_env_namespace: String,
    /// Configuration schema version.
    pub config_schema_version: u32,
    /// Minimum applied central schema head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_central_head: Option<String>,
    /// Liveness path, for long-lived units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_path: Option<String>,
    /// Readiness path, for long-lived units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_path: Option<String>,
    /// Receipt classes required before this unit may be published.
    pub required_receipts: Vec<String>,
    /// Declarative alarm specification id.
    pub alarm_spec: String,
    /// Companion live-test package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_suite: Option<String>,
    /// Extra input-closure roots beyond the owning package's own closure.
    #[serde(default)]
    pub extra_inputs: Vec<String>,
    /// Lambda resource shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lambda: Option<LambdaShape>,
    /// Fargate task shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fargate: Option<FargateShape>,
    /// Lambda `MicroVM` image shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microvm: Option<MicrovmShape>,
}

/// `release/units.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Units {
    /// Schema discriminator.
    pub schema: String,
    /// Every deployable.
    #[serde(default, rename = "unit")]
    pub units: Vec<Unit>,
}

/// Permission for one scenario to provision in `prd` (OD-35).
///
/// Absence is the default and means "runs in `dev` only". Presence is a claim
/// the registry checks mechanically: the rule must exist, and every resource
/// kind the scenario creates must be one the janitor can reclaim from tags.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrdProvisioning {
    /// Which `[prd_provisioning.rule]` admits this scenario.
    pub rule: String,
    /// Every `[janitor.resource]` kind the scenario creates in `prd`.
    #[serde(default)]
    pub provisions: Vec<String>,
}

/// One scenario and what it observes.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Scenario id, referenced from `[package.metadata.aex].scenarios`.
    pub id: String,
    /// Owning stream.
    pub owner: String,
    /// Nodes this scenario exercises without importing them.
    pub observes: Vec<String>,
    /// Workspace package that contains the runnable scenario target. This is a
    /// namespaced graph node such as `cargo:aex-live-demo-api` or
    /// `npm:@aexhq/user-tests`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// Exact test target declared by the package's `aex.targets` metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Why this scenario has no executable target yet.
    ///
    /// A deferral is architecture debt, never evidence. It is mutually
    /// exclusive with `package`/`target`, is omitted from runnable matrices,
    /// and remains visible in the graph summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<String>,
    /// Whether this scenario may provision in `prd`, and under which rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prd: Option<PrdProvisioning>,
}

/// `release/scenario-ownership.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioOwnership {
    /// Schema discriminator.
    pub schema: String,
    /// Every scenario.
    #[serde(default, rename = "scenario")]
    pub scenarios: Vec<Scenario>,
}

/// Everything the graph is built from.
#[derive(Debug, Clone)]
pub struct GraphInputs {
    /// Repository root.
    pub root: PathBuf,
    /// Cargo workspace members.
    pub cargo: Vec<CargoPackage>,
    /// npm workspace packages.
    pub npm: Vec<NpmPackage>,
    /// Terraform modules and example roots.
    pub terraform: Vec<TerraformUnit>,
    /// Deployable registrations.
    pub units: Units,
    /// Scenario ownership.
    pub scenarios: ScenarioOwnership,
    /// Path classification rules.
    pub path_map: PathMap,
    /// Every tracked repository file, `/`-separated.
    pub files: Vec<String>,
    /// Violations found while reading the inputs, deferred so `graph verify`
    /// reports them alongside everything else instead of aborting on the first.
    pub input_violations: Vec<Violation>,
}

impl GraphInputs {
    /// Load every authority from a repository root.
    ///
    /// A checked-in `cargo-metadata.json` beside the root is used when present,
    /// which is how fixture workspaces stay hermetic and fast; otherwise
    /// `cargo metadata --no-deps` is invoked with a fixed argv.
    ///
    /// # Errors
    /// Returns [`Exit::GraphVerification`] when a required registry is missing
    /// or unparseable.
    pub fn load(root: &Path) -> Result<Self> {
        let mut input_violations = Vec::new();
        let cargo_json = read_cargo_metadata(root)?;
        let cargo = parse_cargo(&cargo_json, root, &mut input_violations)?;
        let npm = parse_npm(root, &mut input_violations)?;
        let terraform = parse_terraform(root, &mut input_violations)?;
        let units: Units = read_registry(root, "release/units.toml", "aex.units.v1")?;
        let scenarios: ScenarioOwnership = read_registry(
            root,
            "release/scenario-ownership.toml",
            "aex.scenario-ownership.v1",
        )?;
        let path_map_path = root.join("release/path-map.toml");
        let path_map = PathMap::parse(
            &std::fs::read_to_string(&path_map_path)
                .map_err(|err| io(&path_map_path.display().to_string(), &err))?,
        )?;
        let files = tracked_files(root)?;
        Ok(Self {
            root: root.to_path_buf(),
            cargo,
            npm,
            terraform,
            units,
            scenarios,
            path_map,
            files,
            input_violations,
        })
    }

    /// Package directory to npm package name, for path classification.
    #[must_use]
    pub fn npm_dirs(&self) -> BTreeMap<String, String> {
        self.npm
            .iter()
            .map(|package| (package.dir.clone(), package.name.clone()))
            .collect()
    }

    /// Resolve a `release/units.toml` `package` to the node that holds it.
    ///
    /// A deployable's owning package is usually a Cargo member, but the two
    /// Stripe edges are `TypeScript` Lambdas whose package is an npm member. The
    /// unit registry names a package, not a node namespace, so the resolution
    /// happens here rather than being spelled out in every row: writing
    /// `npm:@aexhq/stripe-command-edge` into the registry would make the file
    /// carry the graph's internal encoding.
    ///
    /// An unknown name resolves to the Cargo namespace so that graph
    /// construction reports the dangling edge, and `registry_reference_violations`
    /// reports the row, exactly as it did before npm packages could be named.
    #[must_use]
    pub fn package_node(&self, package: &str) -> NodeId {
        if self.cargo.iter().any(|member| member.name == package) {
            return NodeId::cargo(package);
        }
        if self.npm.iter().any(|member| member.name == package) {
            return NodeId::npm(package);
        }
        NodeId::cargo(package)
    }

    /// Every edge the authorities declare, in a stable order.
    #[must_use]
    pub fn edges(&self) -> Vec<RawEdge> {
        let cargo_names: BTreeSet<&str> = self.cargo.iter().map(|p| p.name.as_str()).collect();
        let npm_names: BTreeSet<&str> = self.npm.iter().map(|p| p.name.as_str()).collect();
        let mut edges = Vec::new();
        for package in &self.cargo {
            for (dep, kind) in &package.deps {
                if cargo_names.contains(dep.as_str()) {
                    edges.push(RawEdge {
                        from: NodeId::cargo(&package.name),
                        to: NodeId::cargo(dep),
                        kind: *kind,
                    });
                }
            }
        }
        for package in &self.npm {
            for (dep, kind) in &package.deps {
                if npm_names.contains(dep.as_str()) {
                    edges.push(RawEdge {
                        from: NodeId::npm(&package.name),
                        to: NodeId::npm(dep),
                        kind: *kind,
                    });
                }
            }
        }
        for unit in &self.terraform {
            for dep in &unit.module_deps {
                edges.push(RawEdge {
                    from: NodeId::terraform(&unit.relative),
                    to: NodeId::terraform(dep),
                    kind: EdgeKind::TerraformModule,
                });
            }
        }
        for unit in &self.units.units {
            edges.push(RawEdge {
                from: NodeId::artifact(&unit.id),
                to: self.package_node(&unit.package),
                kind: EdgeKind::ArtifactInput,
            });
            for extra in &unit.extra_inputs {
                edges.push(RawEdge {
                    from: NodeId::artifact(&unit.id),
                    to: NodeId(extra.clone()),
                    kind: EdgeKind::ArtifactInput,
                });
            }
        }
        for scenario in &self.scenarios.scenarios {
            for observed in &scenario.observes {
                edges.push(RawEdge {
                    from: NodeId::scenario(&scenario.id),
                    to: NodeId(observed.clone()),
                    kind: EdgeKind::ScenarioObserves,
                });
            }
        }
        edges
    }
}

fn read_cargo_metadata(root: &Path) -> Result<serde_json::Value> {
    let checked_in = root.join("cargo-metadata.json");
    let text = if checked_in.is_file() {
        std::fs::read_to_string(&checked_in)
            .map_err(|err| io(&checked_in.display().to_string(), &err))?
    } else {
        let output = Command::new("cargo")
            .args([
                "metadata",
                "--no-deps",
                "--format-version",
                "1",
                "--locked",
                "--manifest-path",
            ])
            .arg(root.join("Cargo.toml"))
            .output()
            .map_err(|err| {
                ToolError::single(
                    Exit::GraphVerification,
                    "cargo-metadata-unavailable",
                    format!("`cargo metadata` could not be run: {err}"),
                )
            })?;
        if !output.status.success() {
            return Err(ToolError::single(
                Exit::GraphVerification,
                "cargo-metadata-unavailable",
                format!(
                    "`cargo metadata` failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    serde_json::from_str(&text).map_err(|err| {
        ToolError::single(
            Exit::GraphVerification,
            "cargo-metadata-unavailable",
            format!("`cargo metadata` output does not parse: {err}"),
        )
    })
}

fn parse_cargo(
    metadata: &serde_json::Value,
    root: &Path,
    violations: &mut Vec<Violation>,
) -> Result<Vec<CargoPackage>> {
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            ToolError::single(
                Exit::GraphVerification,
                "cargo-metadata-unavailable",
                "`cargo metadata` output has no `packages` array",
            )
        })?;
    let mut parsed = Vec::new();
    for package in packages {
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let manifest = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let dir = relative_dir(root, Path::new(manifest).parent().unwrap_or(root));
        let meta = match AexMeta::from_metadata_object(&dir, package.get("metadata")) {
            Ok(meta) => meta,
            Err(violation) => {
                violations.push(violation);
                None
            }
        };
        // `cargo metadata` reports `publish` as null for "publish anywhere"
        // and as an array of registries otherwise; an empty array is
        // `publish = false`.
        let publishable = package
            .get("publish")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|registries| !registries.is_empty());
        let mut deps = Vec::new();
        if let Some(list) = package
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
        {
            for dependency in list {
                let dep_name = dependency
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let kind = match dependency.get("kind").and_then(serde_json::Value::as_str) {
                    Some("dev") => EdgeKind::CargoDev,
                    Some("build") => EdgeKind::CargoBuild,
                    _ => EdgeKind::CargoNormal,
                };
                deps.push((dep_name, kind));
            }
        }
        parsed.push(CargoPackage {
            name,
            dir,
            meta,
            publishable,
            deps,
        });
    }
    parsed.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(parsed)
}

fn parse_npm(root: &Path, violations: &mut Vec<Violation>) -> Result<Vec<NpmPackage>> {
    let root_manifest = root.join("package.json");
    if !root_manifest.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&root_manifest)
        .map_err(|err| io(&root_manifest.display().to_string(), &err))?;
    let document: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
        ToolError::single(
            Exit::GraphVerification,
            "npm-workspace-unparseable",
            format!("package.json does not parse: {err}"),
        )
    })?;
    let mut dirs = BTreeSet::new();
    if let Some(globs) = document
        .get("workspaces")
        .and_then(serde_json::Value::as_array)
    {
        for glob in globs.iter().filter_map(serde_json::Value::as_str) {
            expand_workspace_glob(root, glob, &mut dirs);
        }
    }
    // Extra npm inventory comes from `aex-workspace-check` rather than a
    // second list. The Stripe edges are also literal root workspaces so Bun's
    // frozen install and this graph read the same manifests; the set de-dupes
    // their two discovery paths. The path-referenced lint plugin exists only
    // in the explicit set.
    for explicit in aex_workspace_check::collect::NPM_EXPLICIT {
        if root.join(explicit).join("package.json").is_file() {
            dirs.insert((*explicit).to_owned());
        }
    }
    let mut parsed = Vec::new();
    for dir in dirs {
        let manifest = root.join(&dir).join("package.json");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<serde_json::Value>(&text) else {
            violations.push(Violation::new(
                "npm-workspace-unparseable",
                format!("`{dir}/package.json` does not parse"),
            ));
            continue;
        };
        let Some(name) = document.get("name").and_then(serde_json::Value::as_str) else {
            violations.push(Violation::new(
                "npm-workspace-unparseable",
                format!("`{dir}/package.json` declares no name"),
            ));
            continue;
        };
        let meta = match document.get("aex") {
            Some(table) => match AexMeta::from_json(&dir, table) {
                Ok(meta) => Some(meta),
                Err(violation) => {
                    violations.push(violation);
                    None
                }
            },
            None => None,
        };
        let publishable = !document
            .get("private")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let mut deps = Vec::new();
        for (field, kind) in [
            ("dependencies", EdgeKind::NpmRuntime),
            ("devDependencies", EdgeKind::NpmDev),
            ("peerDependencies", EdgeKind::NpmRuntime),
        ] {
            if let Some(map) = document.get(field).and_then(serde_json::Value::as_object) {
                for (dep, range) in map {
                    if let Some(range) = range.as_str()
                        && range.starts_with("workspace:")
                        && range != "workspace:*"
                        && range != "workspace:^"
                        && range != "workspace:~"
                    {
                        violations.push(Violation::new(
                            "npm-unresolved-specifier",
                            format!(
                                "`{dir}` depends on `{dep}` with `{range}`; workspace \
                                 specifiers resolve only as `workspace:*`, `^` or `~`"
                            ),
                        ));
                    }
                    deps.push((dep.clone(), kind));
                }
            }
        }
        parsed.push(NpmPackage {
            name: name.to_owned(),
            dir,
            meta,
            publishable,
            deps,
        });
    }
    parsed.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(parsed)
}

/// Expand the two workspace glob forms npm and bun both accept: a literal
/// directory, and a single trailing `*`.
fn expand_workspace_glob(root: &Path, glob: &str, out: &mut BTreeSet<String>) {
    if let Some(parent) = glob.strip_suffix("/*") {
        let Ok(entries) = std::fs::read_dir(root.join(parent)) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.path().join("package.json").is_file()
                && let Some(name) = entry.file_name().to_str()
            {
                out.insert(format!("{parent}/{name}"));
            }
        }
    } else if root.join(glob).join("package.json").is_file() {
        out.insert(glob.trim_end_matches('/').to_owned());
    }
}

// The signature keeps `Result` because reading a sidecar is I/O; the current
// control flow happens to funnel every such failure into `violations`.
#[allow(clippy::unnecessary_wraps)]
fn parse_terraform(root: &Path, violations: &mut Vec<Violation>) -> Result<Vec<TerraformUnit>> {
    let infra = root.join("infra");
    if !infra.is_dir() {
        return Ok(Vec::new());
    }
    let mut units: BTreeMap<String, TerraformUnit> = BTreeMap::new();
    for tree in ["modules", "examples"] {
        let base = infra.join(tree);
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let relative = format!("{tree}/{name}");
            let dir = format!("infra/{relative}");
            let has_tf = std::fs::read_dir(entry.path()).is_ok_and(|mut read| {
                read.any(|e| {
                    e.as_ref()
                        .ok()
                        .and_then(|e| {
                            e.path()
                                .extension()
                                .map(|ext| ext.eq_ignore_ascii_case("tf"))
                        })
                        .unwrap_or(false)
                })
            });
            if !has_tf {
                continue;
            }
            let sidecar = entry.path().join("aex.toml");
            let meta = if sidecar.is_file() {
                match std::fs::read_to_string(&sidecar)
                    .map_err(|err| io(&sidecar.display().to_string(), &err))
                    .and_then(|text| {
                        AexMeta::from_sidecar_toml(&dir, &text).map_err(|violation| {
                            ToolError::many(Exit::GraphVerification, vec![violation])
                        })
                    }) {
                    Ok(meta) => Some(meta),
                    Err(err) => {
                        violations.extend(err.violations);
                        None
                    }
                }
            } else {
                None
            };
            let module_deps = terraform_module_sources(&entry.path(), tree);
            units.insert(
                relative.clone(),
                TerraformUnit {
                    relative,
                    dir,
                    is_root: tree == "examples",
                    meta,
                    module_deps,
                },
            );
        }
    }
    Ok(units.into_values().collect())
}

/// Collect `module "x" { source = "../../modules/y" }` targets.
///
/// Only relative sources inside `infra/modules` become edges; a registry or Git
/// source is an external dependency the graph does not own.
fn terraform_module_sources(dir: &Path, _tree: &str) -> Vec<String> {
    let mut sources = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("tf"))
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let trimmed = line.trim();
            let Some(rest) = trimmed.strip_prefix("source") else {
                continue;
            };
            let Some(value) = rest.split('"').nth(1) else {
                continue;
            };
            if let Some(position) = value.find("modules/") {
                let name = value[position + "modules/".len()..].trim_matches('/');
                if !name.is_empty() && !name.contains('/') {
                    sources.insert(format!("modules/{name}"));
                }
            }
        }
    }
    sources.into_iter().collect()
}

fn read_registry<T>(root: &Path, relative: &str, expected_schema: &str) -> Result<T>
where
    T: serde::de::DeserializeOwned + HasSchema,
{
    let path = root.join(relative);
    let text =
        std::fs::read_to_string(&path).map_err(|err| io(&path.display().to_string(), &err))?;
    let parsed: T = toml::from_str(&text).map_err(|err| {
        ToolError::single(
            Exit::GraphVerification,
            "registry-unparseable",
            format!("`{relative}` does not parse: {err}"),
        )
    })?;
    if parsed.schema() != expected_schema {
        return Err(ToolError::single(
            Exit::GraphVerification,
            "registry-unparseable",
            format!(
                "`{relative}` declares schema `{}`; expected `{expected_schema}`",
                parsed.schema()
            ),
        ));
    }
    Ok(parsed)
}

/// Registries carry a schema discriminator so a shape change is a rejection
/// rather than a silently-defaulted field.
pub trait HasSchema {
    /// The declared schema string.
    fn schema(&self) -> &str;
}

impl HasSchema for Units {
    fn schema(&self) -> &str {
        &self.schema
    }
}

impl HasSchema for ScenarioOwnership {
    fn schema(&self) -> &str {
        &self.schema
    }
}

/// Every tracked file, `/`-separated and repository-relative.
///
/// `git ls-files` is the authority when the root is a work tree, because it is
/// the same set the checkout and the diff see. Fixture roots are not
/// repositories, so a filtered walk is the fallback.
///
/// # Errors
/// Returns a usage error when the tree cannot be walked.
pub fn tracked_files(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output();
    if let Ok(output) = output
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let mut files: Vec<String> = text
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect();
        if !files.is_empty() {
            files.sort();
            return Ok(files);
        }
    }
    let skip = [
        ".git",
        "target",
        "node_modules",
        ".terraform",
        "dist",
        ".next",
        ".vercel",
    ];
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !skip.contains(&name))
        })
    {
        let entry = entry.map_err(|err| {
            ToolError::single(
                Exit::Usage,
                "io",
                format!("walking `{}`: {err}", root.display()),
            )
        })?;
        if entry.file_type().is_file() {
            files.push(relative_dir(root, entry.path()));
        }
    }
    files.sort();
    Ok(files)
}

fn relative_dir(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::{
        EdgeKind, GraphInputs, Units, expand_workspace_glob, parse_cargo, terraform_module_sources,
    };
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn cargo_dependency_kinds_map_to_edge_kinds() {
        let metadata = json!({
            "packages": [{
                "name": "aex-wire",
                "manifest_path": "/repo/crates/aex-wire/Cargo.toml",
                "publish": [],
                "dependencies": [
                    { "name": "serde", "kind": null },
                    { "name": "aex-internal-contracts", "kind": "dev" },
                    { "name": "prost-build", "kind": "build" }
                ]
            }]
        });
        let mut violations = Vec::new();
        let packages = parse_cargo(&metadata, Path::new("/repo"), &mut violations).unwrap();
        assert!(violations.is_empty());
        assert_eq!(packages[0].dir, "crates/aex-wire");
        assert!(!packages[0].publishable, "publish = [] is publish = false");
        assert_eq!(
            packages[0].deps,
            vec![
                ("serde".to_owned(), EdgeKind::CargoNormal),
                ("aex-internal-contracts".to_owned(), EdgeKind::CargoDev),
                ("prost-build".to_owned(), EdgeKind::CargoBuild),
            ]
        );
    }

    #[test]
    fn a_package_with_no_publish_key_is_publishable() {
        let metadata = json!({
            "packages": [{
                "name": "aex-sdk",
                "manifest_path": "/repo/crates/aex-sdk/Cargo.toml",
                "dependencies": []
            }]
        });
        let mut violations = Vec::new();
        let packages = parse_cargo(&metadata, Path::new("/repo"), &mut violations).unwrap();
        assert!(packages[0].publishable);
    }

    #[test]
    fn an_unknown_metadata_key_is_reported_rather_than_dropped() {
        let metadata = json!({
            "packages": [{
                "name": "aex-wire",
                "manifest_path": "/repo/crates/aex-wire/Cargo.toml",
                "dependencies": [],
                "metadata": { "aex": { "owner": "contracts", "role": "contract",
                                       "security_tier": "public_edge", "tier": "gold" } }
            }]
        });
        let mut violations = Vec::new();
        parse_cargo(&metadata, Path::new("/repo"), &mut violations).unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "aex-metadata-unknown-key");
    }

    #[test]
    fn workspace_globs_accept_both_a_star_and_a_literal_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for dir in ["packages/sdk", "packages/other", "apps/site"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("package.json"), "{}").unwrap();
        }
        let mut out = BTreeSet::new();
        expand_workspace_glob(root, "packages/*", &mut out);
        expand_workspace_glob(root, "apps/site", &mut out);
        assert_eq!(
            out.into_iter().collect::<Vec<_>>(),
            vec!["apps/site", "packages/other", "packages/sdk"]
        );
    }

    #[test]
    fn terraform_module_sources_collect_only_relative_module_paths() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("main.tf"),
            r#"
module "keys" {
  source = "../../modules/kms-key"
}
module "queue" {
  source = "../../modules/sqs-queue"
}
module "external" {
  source = "registry.terraform.io/hashicorp/vpc/aws"
}
"#,
        )
        .unwrap();
        assert_eq!(
            terraform_module_sources(temp.path(), "examples"),
            vec!["modules/kms-key", "modules/sqs-queue"]
        );
    }

    #[test]
    fn a_registry_with_the_wrong_schema_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("release")).unwrap();
        std::fs::write(
            temp.path().join("release/units.toml"),
            "schema = \"aex.units.v2\"\n",
        )
        .unwrap();
        let err = super::read_registry::<Units>(temp.path(), "release/units.toml", "aex.units.v1")
            .unwrap_err();
        assert_eq!(err.rules(), vec!["registry-unparseable"]);
    }

    #[test]
    fn artifact_and_scenario_edges_are_derived_from_the_registries() {
        let inputs = GraphInputs {
            root: std::path::PathBuf::from("/repo"),
            cargo: Vec::new(),
            npm: Vec::new(),
            terraform: Vec::new(),
            units: toml::from_str(
                r#"
schema = "aex.units.v1"
[[unit]]
id = "regional-session-api"
kind = "rust-lambda"
plane = "regional"
package = "regional-session-api"
target = "aarch64-unknown-linux-gnu.2.34"
profile = "release-lambda"
form = "zip"
config_env_namespace = "AEX_REGIONAL_SESSION_"
config_schema_version = 1
required_receipts = ["unit"]
alarm_spec = "regional-session-api"
extra_inputs = ["bundle:contract"]
"#,
            )
            .unwrap(),
            scenarios: toml::from_str(
                r#"
schema = "aex.scenario-ownership.v1"
[[scenario]]
id = "SC-SESSION-ADMIT"
owner = "regional-services"
observes = ["artifact:regional-session-api"]
"#,
            )
            .unwrap(),
            path_map: super::PathMap::parse("schema = \"aex.path-map.v1\"").unwrap(),
            files: Vec::new(),
            input_violations: Vec::new(),
        };
        let edges = inputs.edges();
        assert_eq!(edges.len(), 3);
        assert!(edges.iter().any(|edge| {
            edge.from.as_str() == "artifact:regional-session-api"
                && edge.to.as_str() == "cargo:regional-session-api"
                && edge.kind == EdgeKind::ArtifactInput
        }));
        assert!(edges.iter().any(|edge| {
            edge.from.as_str() == "scenario:SC-SESSION-ADMIT"
                && edge.kind == EdgeKind::ScenarioObserves
        }));
    }
}
