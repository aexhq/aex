//! `graph matrix` — turn a selection into GitHub Actions matrix inputs.
//!
//! Two properties matter and both are tested. Output keys are identical on the
//! healthy and the degraded path, so a consumer never reads `undefined` from a
//! router that failed; and longest-processing-time partitioning never yields an
//! empty shard, because an empty shard is a job that reports success having run
//! nothing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Exit, Result, ToolError};

use super::NodeKind;
use super::inputs::{CargoPackage, NpmPackage, ScenarioOwnership, Units};
use super::select::Selection;

/// Which slice of the selection to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum MatrixKind {
    /// Cargo packages to test in the Rust lane.
    Test,
    /// Cargo packages that own an engine-backed target, for the integration
    /// lane.
    ///
    /// Separate from [`Self::Test`] because these targets sit behind
    /// `required-features` and need a container runtime, so the ordinary unit
    /// lane neither builds nor links them. Deriving the slice from
    /// `[package.metadata.aex.targets]` rather than from
    /// `release/semantic-receipts.json` is deliberate: a semantic receipt
    /// producer must name a deployable unit's own package, and an adapter crate
    /// has no unit row, so an engine-backed target in a library could otherwise
    /// never reach any lane at all.
    Integration,
    /// npm packages that own deployable units to test in the Node lane.
    Node,
    /// Artifacts to build.
    Artifact,
    /// Scenarios to exercise.
    Scenario,
    /// Terraform modules and roots.
    Terraform,
    /// Live companion packages.
    Live,
}

/// One matrix entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixEntry {
    /// Node id.
    pub id: String,
    /// Node identity within its authority: the Cargo package name, the npm
    /// package name, the module directory, the unit id.
    pub name: String,
    /// Runnable package node for a scenario entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// Exact package target for a scenario entry, or the nextest filterset
    /// selecting an integration entry's engine-backed targets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Repository-relative npm package directory for a Node entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Deployable unit ids whose own package this entry exercises.
    #[serde(default)]
    pub units: Vec<String>,
    /// Zero-based shard index.
    pub partition: usize,
    /// Total shard count.
    pub partitions: usize,
}

/// The full emission, including the keys every consumer reads.
///
/// The key set is fixed. A router that fails still emits every key, because a
/// missing key routes downstream guards to "run nothing", which is
/// indistinguishable from "everything passed".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixOutput {
    /// Whether routing failed.
    pub routing_failed: bool,
    /// Whether the run widened to the whole repository.
    pub repo_wide: bool,
    /// Whether the matrix holds at least one entry.
    pub has_entries: bool,
    /// How many entries.
    pub count: usize,
    /// The entries themselves.
    pub include: Vec<MatrixEntry>,
}

/// The exact GitHub Actions output keys, checked for parity across paths.
pub const OUTPUT_KEYS: &[&str] = &[
    "routing_failed",
    "repo_wide",
    "has_entries",
    "count",
    "matrix",
];

/// Build the matrix for one slice of a selection.
///
/// `durations` optionally supplies observed per-node runtimes; longest-first
/// packing then balances the shards. Without it every node weighs the same.
///
/// # Errors
/// Returns [`Exit::Usage`] when `partitions` is zero, or
/// [`Exit::GraphVerification`] when a selected scenario has no exact runnable
/// package/target claim.
#[allow(
    clippy::too_many_arguments,
    reason = "one parameter per authority the matrix is derived from; bundling them into a struct would hide which registry a slice actually reads"
)]
pub fn build(
    selection: &Selection,
    kind: MatrixKind,
    partitions: usize,
    durations: &BTreeMap<String, u64>,
    scenarios: &ScenarioOwnership,
    units: &Units,
    npm: &[NpmPackage],
    cargo: &[CargoPackage],
) -> Result<MatrixOutput> {
    build_with_artifacts(
        selection, None, kind, partitions, durations, scenarios, units, npm, cargo,
    )
}

/// Build a matrix while retaining validation for every selected artifact owner.
///
/// A publication may narrow ordinary checks to the affected closure only when
/// the packages that own its exhaustive artifact selection remain present to
/// emit the same-run receipts required for certification.
///
/// # Errors
/// Returns the same errors as [`build`], and rejects an artifact selection that
/// names no registered unit.
#[allow(clippy::too_many_arguments)]
pub fn build_with_artifacts(
    selection: &Selection,
    artifact_selection: Option<&Selection>,
    kind: MatrixKind,
    partitions: usize,
    durations: &BTreeMap<String, u64>,
    scenarios: &ScenarioOwnership,
    units: &Units,
    npm: &[NpmPackage],
    cargo: &[CargoPackage],
) -> Result<MatrixOutput> {
    if partitions == 0 {
        return Err(ToolError::single(
            Exit::Usage,
            "usage",
            "--partitions must be at least 1",
        ));
    }
    let candidates = if kind == MatrixKind::Scenario {
        scenario_candidates(selection, scenarios)?
    } else {
        candidates(selection, artifact_selection, kind, units, npm, cargo)?
    };

    let shards = partition(&candidates, partitions, durations);
    let scenario_claims: BTreeMap<&str, (&str, &str)> = scenarios
        .scenarios
        .iter()
        .filter_map(|scenario| {
            Some((
                scenario.id.as_str(),
                (scenario.package.as_deref()?, scenario.target.as_deref()?),
            ))
        })
        .collect();
    let mut include = Vec::new();
    let used = shards.len();
    for (index, shard) in shards.iter().enumerate() {
        for (id, name) in shard {
            let (package, target) = if kind == MatrixKind::Scenario {
                let Some((package, target)) = scenario_claims.get(name.as_str()) else {
                    return Err(ToolError::single(
                        Exit::GraphVerification,
                        "scenario-runnable-missing",
                        format!("selected scenario `{name}` has no runnable package/target claim"),
                    ));
                };
                (Some((*package).to_owned()), Some((*target).to_owned()))
            } else if kind == MatrixKind::Integration {
                // `candidates` already refused a package with no engine-backed
                // target, so this is never the empty filterset.
                (
                    None,
                    Some(integration_filterset(&integration_targets(cargo, name))),
                )
            } else {
                (None, None)
            };
            let directory = if kind == MatrixKind::Node {
                Some(
                    npm.iter()
                        .find(|candidate| candidate.name == *name)
                        .ok_or_else(|| {
                            ToolError::single(
                                Exit::GraphVerification,
                                "node-package-directory-missing",
                                format!(
                                    "selected Node package `{name}` has no npm package directory"
                                ),
                            )
                        })?
                        .dir
                        .clone(),
                )
            } else {
                None
            };
            include.push(MatrixEntry {
                id: id.clone(),
                name: name.clone(),
                package,
                target,
                directory,
                units: if matches!(
                    kind,
                    MatrixKind::Test | MatrixKind::Integration | MatrixKind::Node
                ) {
                    units
                        .units
                        .iter()
                        .filter(|unit| unit.package == *name)
                        .map(|unit| unit.id.clone())
                        .collect()
                } else {
                    Vec::new()
                },
                partition: index,
                partitions: used,
            });
        }
    }
    Ok(MatrixOutput {
        routing_failed: selection.routing_failed,
        repo_wide: selection.repo_wide,
        has_entries: !include.is_empty(),
        count: include.len(),
        include,
    })
}

/// The Cargo feature every engine-backed target sits behind.
///
/// One spelling workspace-wide, so the lane turns on exactly what
/// `required-features` demands without inspecting manifests at run time.
pub const ENGINE_FEATURE: &str = "integration-engines";

/// The engine-backed target names one Cargo package declares, sorted.
///
/// The authority is `[package.metadata.aex.targets]`, which maps every test
/// target to the layer it covers; `aex-workspace-check` already refuses a
/// mapping that names no real `[[test]]` target, so a name here is runnable.
///
/// A package that maps a target to the integration layer but declares no
/// [`ENGINE_FEATURE`] yields nothing: its target needs no container, so it is
/// an ordinary target the unit lane already runs, and selecting it here would
/// run the same evidence twice under two different receipts.
fn integration_targets(cargo: &[CargoPackage], name: &str) -> Vec<String> {
    let Some(package) = cargo.iter().find(|package| package.name == name) else {
        return Vec::new();
    };
    if !package
        .features
        .iter()
        .any(|feature| feature == ENGINE_FEATURE)
    {
        return Vec::new();
    }
    package
        .meta
        .as_ref()
        .map(|meta| {
            meta.targets
                .iter()
                .filter(|(_, layer)| layer.as_str() == "integration")
                .map(|(target, _)| target.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The nextest filterset selecting exactly one package's engine-backed targets.
///
/// Emitted onto the matrix entry so the lane never has to re-derive it, and so
/// a package whose integration target is renamed changes the filterset rather
/// than silently running nothing: every lane passes `--no-tests=fail`.
fn integration_filterset(targets: &[String]) -> String {
    targets
        .iter()
        .map(|target| format!("binary({target})"))
        .collect::<Vec<_>>()
        .join(" or ")
}

fn candidates(
    selection: &Selection,
    artifact_selection: Option<&Selection>,
    kind: MatrixKind,
    units: &Units,
    npm: &[NpmPackage],
    cargo: &[CargoPackage],
) -> Result<Vec<(String, String)>> {
    let selected: Vec<(String, String)> = match kind {
        MatrixKind::Test => selection
            .test
            .iter()
            .filter(|selected| {
                selected.id.namespace() == "cargo" && !selected.id.local().starts_with("aex-live-")
            })
            .map(|selected| (selected.id.to_string(), selected.id.local().to_owned()))
            .collect(),
        MatrixKind::Integration => selection
            .test
            .iter()
            .filter(|selected| {
                selected.id.namespace() == "cargo"
                    && !selected.id.local().starts_with("aex-live-")
                    && !integration_targets(cargo, selected.id.local()).is_empty()
            })
            .map(|selected| (selected.id.to_string(), selected.id.local().to_owned()))
            .collect(),
        MatrixKind::Node => selection
            .test
            .iter()
            .filter(|selected| {
                selected.id.namespace() == "npm"
                    && units
                        .units
                        .iter()
                        .any(|unit| unit.package == selected.id.local())
            })
            .map(|selected| (selected.id.to_string(), selected.id.local().to_owned()))
            .collect(),
        MatrixKind::Live => selection
            .test
            .iter()
            .filter(|selected| selected.id.local().starts_with("aex-live-"))
            .map(|selected| (selected.id.to_string(), selected.id.local().to_owned()))
            .collect(),
        MatrixKind::Terraform => selection
            .test
            .iter()
            .filter(|selected| selected.id.namespace() == "tf")
            .map(|selected| (selected.id.to_string(), selected.id.local().to_owned()))
            .collect(),
        MatrixKind::Artifact => selection
            .deploy
            .iter()
            .map(|selected| (selected.id.to_string(), selected.id.local().to_owned()))
            .collect(),
        MatrixKind::Scenario => selection
            .scenarios
            .iter()
            .map(|selected| (selected.id.to_string(), selected.id.local().to_owned()))
            .collect(),
    };
    let mut selected: BTreeMap<String, String> = selected.into_iter().collect();
    if matches!(kind, MatrixKind::Test | MatrixKind::Node)
        && let Some(artifact_selection) = artifact_selection
    {
        for artifact in &artifact_selection.deploy {
            let unit = units
                .units
                .iter()
                .find(|unit| unit.id == artifact.id.local())
                .ok_or_else(|| {
                    ToolError::single(
                        Exit::GraphVerification,
                        "artifact-unit-missing",
                        format!(
                            "selected artifact `{}` has no registered deployable unit",
                            artifact.id
                        ),
                    )
                })?;
            let is_npm = npm.iter().any(|package| package.name == unit.package);
            if (kind == MatrixKind::Node && is_npm) || (kind == MatrixKind::Test && !is_npm) {
                let namespace = if is_npm { "npm" } else { "cargo" };
                selected
                    .entry(format!("{namespace}:{}", unit.package))
                    .or_insert_with(|| unit.package.clone());
            }
        }
    }
    Ok(selected.into_iter().collect())
}

fn scenario_candidates(
    selection: &Selection,
    scenarios: &ScenarioOwnership,
) -> Result<Vec<(String, String)>> {
    let registry: BTreeMap<&str, &super::inputs::Scenario> = scenarios
        .scenarios
        .iter()
        .map(|scenario| (scenario.id.as_str(), scenario))
        .collect();
    let mut candidates = Vec::new();
    for selected in &selection.scenarios {
        let name = selected.id.local();
        let Some(scenario) = registry.get(name) else {
            return Err(ToolError::single(
                Exit::GraphVerification,
                "scenario-runnable-missing",
                format!("selected scenario `{name}` is absent from the scenario registry"),
            ));
        };
        match (&scenario.package, &scenario.target, &scenario.deferred) {
            (Some(_), Some(_), None) => {
                candidates.push((selected.id.to_string(), name.to_owned()));
            }
            (None, None, Some(reason)) if !reason.trim().is_empty() => {}
            _ => {
                return Err(ToolError::single(
                    Exit::GraphVerification,
                    "scenario-runnable-missing",
                    format!(
                        "selected scenario `{name}` has neither a sound runnable claim nor an explicit deferral"
                    ),
                ));
            }
        }
    }
    Ok(candidates)
}

/// The emission produced when routing itself failed.
///
/// Identical key set, zero entries, `routing_failed` true. The aggregator turns
/// that into a failing receipt rather than a green skip.
#[must_use]
pub fn degraded(reason: &str) -> MatrixOutput {
    let _ = reason;
    MatrixOutput {
        routing_failed: true,
        repo_wide: true,
        has_entries: false,
        count: 0,
        include: Vec::new(),
    }
}

/// Longest-processing-time-first packing.
///
/// Shard count is reduced to the entry count rather than emitting an empty
/// shard: a job that runs nothing and reports success is worse than one fewer
/// job.
fn partition(
    candidates: &[(String, String)],
    partitions: usize,
    durations: &BTreeMap<String, u64>,
) -> Vec<Vec<(String, String)>> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let shard_count = partitions.min(candidates.len());
    let mut ordered: Vec<&(String, String)> = candidates.iter().collect();
    ordered.sort_by(|left, right| {
        let left_weight = durations.get(&left.0).copied().unwrap_or(1);
        let right_weight = durations.get(&right.0).copied().unwrap_or(1);
        right_weight
            .cmp(&left_weight)
            .then_with(|| left.0.cmp(&right.0))
    });
    let mut shards: Vec<Vec<(String, String)>> = vec![Vec::new(); shard_count];
    let mut loads = vec![0u64; shard_count];
    for entry in ordered {
        let lightest = loads
            .iter()
            .enumerate()
            .min_by_key(|(index, load)| (**load, *index))
            .map_or(0, |(index, _)| index);
        loads[lightest] += durations.get(&entry.0).copied().unwrap_or(1);
        shards[lightest].push(entry.clone());
    }
    for shard in &mut shards {
        shard.sort();
    }
    shards
}

/// Render the emission as `key=value` lines for `$GITHUB_OUTPUT`.
///
/// # Errors
/// Propagates canonicalization failure of the matrix payload.
pub fn to_github_output(output: &MatrixOutput) -> Result<String> {
    let matrix = crate::canon::to_string(&serde_json::json!({ "include": output.include }))?;
    Ok(format!(
        "routing_failed={}\nrepo_wide={}\nhas_entries={}\ncount={}\nmatrix={matrix}\n",
        output.routing_failed, output.repo_wide, output.has_entries, output.count
    ))
}

/// Which node kinds a matrix kind draws from, used by `graph explain`.
#[must_use]
pub const fn source_kind(kind: MatrixKind) -> NodeKind {
    match kind {
        MatrixKind::Test | MatrixKind::Integration | MatrixKind::Live => NodeKind::Crate,
        MatrixKind::Node => NodeKind::NpmPackage,
        MatrixKind::Artifact => NodeKind::Artifact,
        MatrixKind::Scenario => NodeKind::Scenario,
        MatrixKind::Terraform => NodeKind::TerraformModule,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        MatrixKind, OUTPUT_KEYS, build, build_with_artifacts, degraded, partition, to_github_output,
    };
    use crate::graph::NodeId;
    use crate::graph::inputs::{CargoPackage, ScenarioOwnership, Units};
    use crate::graph::select::{Lane, Mode, Selected, Selection, SelectionReason};
    use crate::meta::AexMeta;

    fn selection(ids: &[NodeId]) -> Selection {
        Selection {
            schema: "aex.selection.v1".to_owned(),
            lane: Lane::Pr,
            mode: Mode::Affected,
            test: ids
                .iter()
                .map(|id| Selected {
                    id: id.clone(),
                    reason: SelectionReason::RouterChanged,
                })
                .collect(),
            deploy: Vec::new(),
            scenarios: Vec::new(),
            repo_wide: false,
            unowned: Vec::new(),
            changed_paths: 1,
            routing_failed: false,
            routing_failure_reason: None,
            shadow: None,
        }
    }

    fn no_scenarios() -> ScenarioOwnership {
        ScenarioOwnership {
            schema: "aex.scenario-ownership.v1".to_owned(),
            scenarios: Vec::new(),
        }
    }

    fn no_units() -> Units {
        Units {
            schema: "aex.units.v1".to_owned(),
            units: Vec::new(),
        }
    }

    /// One Cargo package carrying `targets` mapped to the given layers.
    fn cargo_package(name: &str, targets: &[(&str, &str)]) -> CargoPackage {
        CargoPackage {
            name: name.to_owned(),
            dir: format!("crates/{name}"),
            meta: Some(AexMeta {
                owner: "test-architecture".to_owned(),
                role: "adapter".to_owned(),
                artifact: "none".to_owned(),
                deployable: None,
                live_suite: None,
                layers: Vec::new(),
                concerns: Vec::new(),
                seams: Vec::new(),
                security_tier: "internal".to_owned(),
                risk: Vec::new(),
                scenarios: Vec::new(),
                targets: targets
                    .iter()
                    .map(|(target, layer)| ((*target).to_owned(), (*layer).to_owned()))
                    .collect(),
                not_applicable: BTreeMap::new(),
            }),
            publishable: false,
            features: vec![crate::graph::matrix::ENGINE_FEATURE.to_owned()],
            deps: Vec::new(),
        }
    }

    #[test]
    fn the_integration_matrix_selects_only_packages_that_own_an_engine_backed_target() {
        let cargo = [
            cargo_package("aex-secret-aws", &[("integration", "integration")]),
            // Declares targets, but none of them at the integration layer.
            cargo_package("aex-wire", &[("conformance", "unit")]),
        ];
        let selection = selection(&[
            NodeId::cargo("aex-secret-aws"),
            NodeId::cargo("aex-wire"),
            // Never selected: a live companion belongs to the live matrix, and
            // it holds no engine-backed target either way.
            NodeId::cargo("aex-live-brain-mux"),
        ]);

        let integration = build(
            &selection,
            MatrixKind::Integration,
            1,
            &BTreeMap::new(),
            &no_scenarios(),
            &no_units(),
            &[],
            &cargo,
        )
        .unwrap();

        let names: Vec<&str> = integration
            .include
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["aex-secret-aws"],
            "a package with no integration-layer target must not reach the engine lane"
        );

        // The unit matrix still carries every one of them, so narrowing the
        // integration lane never narrows the lane that gates on unit evidence.
        let unit = build(
            &selection,
            MatrixKind::Test,
            1,
            &BTreeMap::new(),
            &no_scenarios(),
            &no_units(),
            &[],
            &cargo,
        )
        .unwrap();
        assert_eq!(
            unit.count, 2,
            "aex-wire and aex-secret-aws, never the live one"
        );
    }

    #[test]
    fn an_integration_entry_carries_a_filterset_naming_each_of_its_targets() {
        let cargo = [cargo_package(
            "aex-control-aurora",
            &[
                ("migrations", "integration"),
                ("integration", "integration"),
                ("properties", "unit"),
            ],
        )];
        let output = build(
            &selection(&[NodeId::cargo("aex-control-aurora")]),
            MatrixKind::Integration,
            1,
            &BTreeMap::new(),
            &no_scenarios(),
            &no_units(),
            &[],
            &cargo,
        )
        .unwrap();

        let entry = output.include.first().expect("one selected package");
        // Sorted, because `targets` is a `BTreeMap`: the filterset a lane runs
        // must not depend on manifest ordering.
        assert_eq!(
            entry.target.as_deref(),
            Some("binary(integration) or binary(migrations)"),
            "the unit-layer target must not be dragged into the engine lane"
        );
    }

    #[test]
    fn healthy_and_degraded_paths_emit_the_same_output_keys() {
        let healthy = to_github_output(
            &build(
                &selection(&[NodeId::cargo("aex-wire")]),
                MatrixKind::Test,
                1,
                &BTreeMap::new(),
                &no_scenarios(),
                &no_units(),
                &[],
                &[],
            )
            .unwrap(),
        )
        .unwrap();
        let broken = to_github_output(&degraded("cargo metadata failed")).unwrap();
        let keys = |text: &str| -> Vec<String> {
            text.lines()
                .filter_map(|line| line.split_once('='))
                .map(|(key, _)| key.to_owned())
                .collect()
        };
        assert_eq!(keys(&healthy), keys(&broken));
        assert_eq!(keys(&healthy), OUTPUT_KEYS);
    }

    #[test]
    fn the_degraded_path_reports_failure_rather_than_an_empty_success() {
        let broken = degraded("cargo metadata failed");
        assert!(broken.routing_failed);
        assert!(!broken.has_entries);
        assert!(broken.repo_wide, "a router that failed must not narrow");
    }

    #[test]
    fn live_packages_are_excluded_from_the_unit_matrix_and_selected_by_the_live_matrix() {
        let selection = selection(&[
            NodeId::cargo("aex-wire"),
            NodeId::cargo("aex-live-brain-mux"),
            NodeId::npm("@aexhq/sdk"),
        ]);
        let unit = build(
            &selection,
            MatrixKind::Test,
            4,
            &BTreeMap::new(),
            &no_scenarios(),
            &no_units(),
            &[],
            &[],
        )
        .unwrap();
        let live = build(
            &selection,
            MatrixKind::Live,
            4,
            &BTreeMap::new(),
            &no_scenarios(),
            &no_units(),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            unit.include
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["aex-wire"]
        );
        assert_eq!(
            live.include
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["aex-live-brain-mux"]
        );
        assert!(
            unit.include
                .iter()
                .all(|entry| entry.id.starts_with("cargo:")),
            "the Rust lane must never receive an npm package"
        );
    }

    #[test]
    fn publication_validation_keeps_artifact_owners_and_skips_unrelated_packages() {
        let affected = selection(&[NodeId::cargo("aex-changed")]);
        let mut artifacts = selection(&[NodeId::cargo("demo-api"), NodeId::cargo("aex-unrelated")]);
        artifacts.mode = Mode::Full;
        artifacts.deploy = vec![Selected {
            id: NodeId::artifact("demo-api"),
            reason: SelectionReason::RouterChanged,
        }];
        let units: Units = toml::from_str(
            r#"
schema = "aex.units.v1"
[[unit]]
id = "demo-api"
kind = "rust-lambda"
plane = "regional"
package = "demo-api"
target = "aarch64-unknown-linux-gnu.2.34"
profile = "release-lambda"
form = "zip"
config_env_namespace = "AEX_DEMO_"
config_schema_version = 1
required_receipts = ["unit", "lint"]
alarm_spec = "demo-api"
"#,
        )
        .unwrap();

        let output = build_with_artifacts(
            &affected,
            Some(&artifacts),
            MatrixKind::Test,
            4,
            &BTreeMap::new(),
            &no_scenarios(),
            &units,
            &[],
            &[],
        )
        .unwrap();
        let names: Vec<&str> = output
            .include
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["aex-changed", "demo-api"]);
        assert!(
            !names.contains(&"aex-unrelated"),
            "an unchanged package that owns no selected artifact must not rerun"
        );
    }

    #[test]
    fn node_matrix_is_bounded_to_npm_packages_that_own_deployable_units() {
        let selection = selection(&[
            NodeId::npm("@aexhq/sdk"),
            NodeId::npm("@aexhq/stripe-command-edge"),
            NodeId::npm("@aexhq/stripe-webhook-edge"),
            NodeId::cargo("aex-wire"),
        ]);
        let units: Units = toml::from_str(
            r#"
schema = "aex.units.v1"
[[unit]]
id = "stripe-command-edge"
kind = "ts-lambda"
plane = "central"
package = "@aexhq/stripe-command-edge"
target = "none"
profile = "release"
form = "zip"
config_env_namespace = "AEX_STRIPE_COMMAND_"
config_schema_version = 1
required_receipts = ["unit"]
alarm_spec = "stripe-command-edge"
[[unit]]
id = "stripe-webhook-edge"
kind = "ts-lambda"
plane = "central"
package = "@aexhq/stripe-webhook-edge"
target = "none"
profile = "release"
form = "zip"
config_env_namespace = "AEX_STRIPE_WEBHOOK_"
config_schema_version = 1
required_receipts = ["unit"]
alarm_spec = "stripe-webhook-edge"
"#,
        )
        .unwrap();
        let npm = vec![
            crate::graph::inputs::NpmPackage {
                name: "@aexhq/sdk".to_owned(),
                dir: "packages/sdk".to_owned(),
                meta: None,
                publishable: true,
                deps: Vec::new(),
            },
            crate::graph::inputs::NpmPackage {
                name: "@aexhq/stripe-command-edge".to_owned(),
                dir: "services/stripe-command-edge".to_owned(),
                meta: None,
                publishable: false,
                deps: Vec::new(),
            },
            crate::graph::inputs::NpmPackage {
                name: "@aexhq/stripe-webhook-edge".to_owned(),
                dir: "services/stripe-webhook-edge".to_owned(),
                meta: None,
                publishable: false,
                deps: Vec::new(),
            },
        ];

        let output = build(
            &selection,
            MatrixKind::Node,
            8,
            &BTreeMap::new(),
            &no_scenarios(),
            &units,
            &npm,
            &[],
        )
        .unwrap();

        assert_eq!(output.include.len(), 2);
        assert_eq!(output.include[0].name, "@aexhq/stripe-command-edge");
        assert_eq!(
            output.include[0].directory.as_deref(),
            Some("services/stripe-command-edge")
        );
        assert_eq!(output.include[0].units, vec!["stripe-command-edge"]);
        assert_eq!(output.include[1].name, "@aexhq/stripe-webhook-edge");
        assert_eq!(
            output.include[1].directory.as_deref(),
            Some("services/stripe-webhook-edge")
        );
        assert_eq!(output.include[1].units, vec!["stripe-webhook-edge"]);
        assert_eq!(output.include[0].partitions, 2);
        assert_eq!(output.include[1].partitions, 2);
    }

    #[test]
    fn scenario_matrix_carries_the_verified_package_and_target_claim() {
        let mut selected = selection(&[]);
        selected.scenarios.push(Selected {
            id: NodeId::scenario("SC-DEMO"),
            reason: SelectionReason::RouterChanged,
        });
        let registry: ScenarioOwnership = toml::from_str(
            r#"
schema = "aex.scenario-ownership.v1"
[[scenario]]
id = "SC-DEMO"
owner = "delivery"
observes = ["artifact:demo-api"]
package = "cargo:aex-live-demo-api"
target = "live"
"#,
        )
        .expect("scenario registry");
        let output = build(
            &selected,
            MatrixKind::Scenario,
            1,
            &BTreeMap::new(),
            &registry,
            &no_units(),
            &[],
            &[],
        )
        .expect("scenario matrix");
        assert_eq!(output.include.len(), 1);
        assert_eq!(output.include[0].name, "SC-DEMO");
        assert_eq!(
            output.include[0].package.as_deref(),
            Some("cargo:aex-live-demo-api")
        );
        assert_eq!(output.include[0].target.as_deref(), Some("live"));
    }

    #[test]
    fn selected_scenario_without_a_claim_cannot_emit_a_green_matrix() {
        let mut selected = selection(&[]);
        selected.scenarios.push(Selected {
            id: NodeId::scenario("SC-GHOST"),
            reason: SelectionReason::RouterChanged,
        });
        let err = build(
            &selected,
            MatrixKind::Scenario,
            1,
            &BTreeMap::new(),
            &no_scenarios(),
            &no_units(),
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.rules(), vec!["scenario-runnable-missing"]);
    }

    #[test]
    fn explicitly_deferred_scenario_is_excluded_from_the_matrix() {
        let mut selected = selection(&[]);
        selected.scenarios.push(Selected {
            id: NodeId::scenario("SC-DEFERRED"),
            reason: SelectionReason::RouterChanged,
        });
        let registry: ScenarioOwnership = toml::from_str(
            r#"
schema = "aex.scenario-ownership.v1"
[[scenario]]
id = "SC-DEFERRED"
owner = "delivery"
observes = ["artifact:demo-api"]
deferred = "the production composition does not yet expose this surface"
"#,
        )
        .expect("scenario registry");
        let output = build(
            &selected,
            MatrixKind::Scenario,
            1,
            &BTreeMap::new(),
            &registry,
            &no_units(),
            &[],
            &[],
        )
        .expect("deferred scenario matrix");
        assert!(!output.routing_failed);
        assert!(!output.has_entries);
        assert_eq!(output.count, 0);
        assert!(output.include.is_empty());
    }

    #[test]
    fn partitioning_never_yields_an_empty_shard() {
        let candidates: Vec<(String, String)> = (0..3)
            .map(|i| (format!("cargo:c{i}"), format!("c{i}")))
            .collect();
        let shards = partition(&candidates, 10, &BTreeMap::new());
        assert_eq!(shards.len(), 3);
        assert!(shards.iter().all(|shard| !shard.is_empty()));
    }

    #[test]
    fn longest_first_packing_balances_by_observed_duration() {
        let candidates: Vec<(String, String)> = ["a", "b", "c", "d"]
            .iter()
            .map(|n| (format!("cargo:{n}"), (*n).to_owned()))
            .collect();
        let durations = BTreeMap::from([
            ("cargo:a".to_owned(), 100),
            ("cargo:b".to_owned(), 60),
            ("cargo:c".to_owned(), 40),
            ("cargo:d".to_owned(), 30),
        ]);
        let shards = partition(&candidates, 2, &durations);
        let load = |shard: &Vec<(String, String)>| -> u64 {
            shard.iter().map(|(id, _)| durations[id]).sum()
        };
        let loads: Vec<u64> = shards.iter().map(load).collect();
        assert_eq!(loads.iter().sum::<u64>(), 230);
        let spread = loads.iter().max().unwrap() - loads.iter().min().unwrap();
        assert!(spread <= 30, "shard loads were {loads:?}");
    }

    #[test]
    fn test_entries_carry_units_owned_by_the_selected_package() {
        let units: Units = toml::from_str(
            r#"
schema = "aex.units.v1"
[[unit]]
id = "hands-image-small"
kind = "microvm-image"
plane = "regional"
package = "hands-image"
target = "aarch64-unknown-linux-musl"
profile = "release"
form = "raw"
config_env_namespace = "AEX_HANDS_IMAGE_"
config_schema_version = 1
required_receipts = ["unit"]
alarm_spec = "hands-image-small"
[[unit]]
id = "hands-image-large"
kind = "microvm-image"
plane = "regional"
package = "hands-image"
target = "aarch64-unknown-linux-musl"
profile = "release"
form = "raw"
config_env_namespace = "AEX_HANDS_IMAGE_"
config_schema_version = 1
required_receipts = ["unit"]
alarm_spec = "hands-image-large"
"#,
        )
        .unwrap();
        let output = build(
            &selection(&[NodeId::cargo("hands-image")]),
            MatrixKind::Test,
            1,
            &BTreeMap::new(),
            &no_scenarios(),
            &units,
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            output.include[0].units,
            vec!["hands-image-small", "hands-image-large"]
        );
    }

    #[test]
    fn an_empty_selection_yields_no_shards_and_says_so() {
        let output = build(
            &selection(&[]),
            MatrixKind::Test,
            4,
            &BTreeMap::new(),
            &no_scenarios(),
            &no_units(),
            &[],
            &[],
        )
        .unwrap();
        assert!(!output.has_entries);
        assert_eq!(output.count, 0);
        assert!(!output.routing_failed, "empty is not the same as broken");
    }

    #[test]
    fn zero_partitions_is_a_usage_error() {
        let err = build(
            &selection(&[]),
            MatrixKind::Test,
            0,
            &BTreeMap::new(),
            &no_scenarios(),
            &no_units(),
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.exit.code(), 2);
    }
}
