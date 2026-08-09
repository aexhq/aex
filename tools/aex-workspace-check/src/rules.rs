//! The structural rules the workspace must satisfy.
//!
//! Every rule is a pure function of the parsed `cargo metadata` document plus a
//! directory listing, so each one is unit-tested against a fixture rather than
//! against whatever the working tree happens to contain.

use std::collections::{BTreeMap, BTreeSet};

use crate::inventory;
use crate::metadata::{DependencyKind, WorkspaceMetadata};

/// One failed rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    /// Which rule failed.
    pub rule: &'static str,
    /// What exactly failed, named precisely enough to fix.
    pub detail: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "[{}] {}", self.rule, self.detail)
    }
}

/// Directory names found on disk under each member root.
pub type TreeListing = BTreeMap<String, Vec<String>>;

/// Everything the rules need.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// The parsed `cargo metadata` document.
    pub metadata: WorkspaceMetadata,
    /// Directory names found under each of [`inventory::MEMBER_ROOTS`].
    pub tree: TreeListing,
}

/// Third-party and vendor dependencies a pure crate may never link.
///
/// Prefix matches cover the `AWS` and Lambda families; the explicit list covers
/// everything else the pinned technology set contains.
#[must_use]
pub fn is_vendor_dependency(name: &str) -> bool {
    if name.starts_with("aws-") || name.starts_with("aws_") || name.starts_with("lambda_") {
        return true;
    }
    matches!(
        name,
        "axum"
            | "hyper"
            | "hyper-util"
            | "opentelemetry"
            | "opentelemetry-otlp"
            | "opentelemetry_sdk"
            | "prost"
            | "prost-types"
            | "reqwest"
            | "rmcp"
            | "rustls"
            | "sqlx"
            | "testcontainers"
            | "tokio"
            | "tokio-util"
            | "tower"
            | "tower-http"
            | "wiremock"
    )
}

/// Whether a workspace crate name denotes an AEX adapter crate.
///
/// The `aex-` prefix is required so a third-party dependency such as
/// `aws-sdk-dynamodb` is classified as a vendor crate and reported once, rather
/// than as both a vendor crate and an adapter.
#[must_use]
pub fn is_adapter_crate(name: &str) -> bool {
    const SUFFIXES: [&str; 7] = [
        "-aws",
        "-dynamodb",
        "-aurora",
        "-http",
        "-gateway",
        "-mcp",
        "-rds-data",
    ];
    name.starts_with("aex-") && SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// Whether a workspace crate name denotes a pure domain crate.
#[must_use]
pub fn is_domain_crate(name: &str) -> bool {
    name.ends_with("-domain")
}

/// Whether a workspace crate name denotes an application crate.
///
/// One suffix, not two. `-application` was retired because the anchored search
/// an agent actually types — `grep -- '-app$'` — returned four of the seven
/// application crates under the split and now returns all seven.
#[must_use]
pub fn is_application_crate(name: &str) -> bool {
    name.ends_with("-app")
}

/// Whether a workspace crate name denotes test-only code.
///
/// Four kinds qualify: the per-area `*-test-support` fixture crates, the shared
/// `aex-test-harness`, the shared `aex-load-harness`, and every `aex-live-*`
/// companion. All four are `publish = false` and may only ever appear in
/// `[dev-dependencies]`, which is the mechanical half of "no fault hook in a
/// production binary".
#[must_use]
pub fn is_test_support_crate(name: &str) -> bool {
    name.ends_with("-test-support")
        || name == "aex-test-harness"
        || name == "aex-load-harness"
        || name.starts_with("aex-live-")
}

/// Runs every rule and returns the violations, sorted.
#[must_use]
pub fn check(workspace: &Workspace) -> Vec<Violation> {
    let mut violations = Vec::new();
    violations.extend(members_match_the_tree(workspace));
    violations.extend(names_match_directories(workspace));
    violations.extend(library_targets_are_underscored(workspace));
    violations.extend(dependency_direction_holds(workspace));
    violations.extend(test_support_is_dev_only(workspace));
    violations.extend(no_dependency_cycles(workspace));
    violations.extend(every_member_is_unpublished(workspace));
    violations.extend(application_suffix_is_app(workspace));
    violations.extend(adapter_suffix_names_the_service(workspace));
    violations.extend(deployables_expose_a_library(workspace));
    violations.sort();
    violations
}

/// Every directory under a member root is a member, and every member lives in
/// one of those roots.
#[must_use]
pub fn members_match_the_tree(workspace: &Workspace) -> Vec<Violation> {
    const RULE: &str = "members-match-tree";
    let mut violations = Vec::new();
    let Ok(directories) = workspace.metadata.member_directories() else {
        violations.push(Violation {
            rule: RULE,
            detail: "a member manifest lives outside the workspace root".to_owned(),
        });
        return violations;
    };
    let declared: BTreeSet<&str> = directories.values().map(String::as_str).collect();
    let exempt: BTreeSet<&str> = inventory::NON_CARGO_DIRECTORIES.iter().copied().collect();

    for root in inventory::MEMBER_ROOTS {
        for entry in workspace.tree.get(*root).into_iter().flatten() {
            let path = format!("{root}/{entry}");
            if !declared.contains(path.as_str()) && !exempt.contains(path.as_str()) {
                violations.push(Violation {
                    rule: RULE,
                    detail: format!("`{path}` exists on disk but is not a workspace member"),
                });
            }
        }
    }

    for (name, path) in &directories {
        let inside = inventory::MEMBER_ROOTS.iter().any(|root| {
            path.starts_with(&format!("{root}/"))
                && path.matches('/').count() == root.matches('/').count() + 1
        });
        if !inside {
            violations.push(Violation {
                rule: RULE,
                detail: format!("member `{name}` lives at `{path}`, outside every member root"),
            });
        }
    }
    violations
}

/// A package name always equals its directory name.
#[must_use]
pub fn names_match_directories(workspace: &Workspace) -> Vec<Violation> {
    const RULE: &str = "name-equals-directory";
    let Ok(directories) = workspace.metadata.member_directories() else {
        return Vec::new();
    };
    directories
        .iter()
        .filter_map(|(name, path)| {
            let directory = path.rsplit('/').next().unwrap_or(path.as_str());
            (directory != name).then(|| Violation {
                rule: RULE,
                detail: format!("package `{name}` lives in directory `{directory}`"),
            })
        })
        .collect()
}

/// A library target is named exactly the package name with dashes underscored.
#[must_use]
pub fn library_targets_are_underscored(workspace: &Workspace) -> Vec<Violation> {
    const RULE: &str = "lib-target-name";
    workspace
        .metadata
        .members()
        .into_iter()
        .filter_map(|package| {
            let library = package.library()?;
            let expected = package.name.replace('-', "_");
            (library.name != expected).then(|| Violation {
                rule: RULE,
                detail: format!(
                    "`{}` has library target `{}`, expected `{expected}`",
                    package.name, library.name
                ),
            })
        })
        .collect()
}

/// Domain crates link no vendor SDK and no adapter; application crates link no
/// vendor SDK.
#[must_use]
pub fn dependency_direction_holds(workspace: &Workspace) -> Vec<Violation> {
    const RULE: &str = "dependency-direction";
    let mut violations = Vec::new();
    for package in workspace.metadata.members() {
        let domain = is_domain_crate(&package.name);
        let application = is_application_crate(&package.name);
        if !domain && !application {
            continue;
        }
        for dependency in &package.dependencies {
            if dependency.kind() != DependencyKind::Normal {
                continue;
            }
            if is_vendor_dependency(&dependency.name) {
                violations.push(Violation {
                    rule: RULE,
                    detail: format!(
                        "`{}` is a pure crate but depends on vendor crate `{}`",
                        package.name, dependency.name
                    ),
                });
            }
            if domain && is_adapter_crate(&dependency.name) {
                violations.push(Violation {
                    rule: RULE,
                    detail: format!(
                        "domain crate `{}` depends on adapter crate `{}`",
                        package.name, dependency.name
                    ),
                });
            }
        }
    }
    violations
}

/// A test-support crate may only appear in `[dev-dependencies]`, so it can never
/// be linked into a production artifact.
#[must_use]
pub fn test_support_is_dev_only(workspace: &Workspace) -> Vec<Violation> {
    const RULE: &str = "test-support-is-dev-only";
    let mut violations = Vec::new();
    for package in workspace.metadata.members() {
        if is_test_support_crate(&package.name) {
            // Test-only code may compose with test-only code: the harness is a
            // normal dependency of the load harness and of every companion.
            // The rule exists to keep test code out of *production* graphs.
            continue;
        }
        for dependency in &package.dependencies {
            if !is_test_support_crate(&dependency.name) {
                continue;
            }
            if dependency.kind() != DependencyKind::Development {
                violations.push(Violation {
                    rule: RULE,
                    detail: format!(
                        "`{}` depends on test-support crate `{}` outside [dev-dependencies]",
                        package.name, dependency.name
                    ),
                });
            }
        }
    }
    violations
}

/// The member graph is acyclic over normal and build dependencies.
#[must_use]
pub fn no_dependency_cycles(workspace: &Workspace) -> Vec<Violation> {
    const RULE: &str = "no-dependency-cycles";
    let members: BTreeSet<&str> = workspace
        .metadata
        .members()
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let mut graph: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for package in workspace.metadata.members() {
        let edges = package
            .dependencies
            .iter()
            .filter(|dependency| dependency.kind() != DependencyKind::Development)
            .map(|dependency| dependency.name.as_str())
            .filter(|name| members.contains(name))
            .collect();
        graph.insert(package.name.as_str(), edges);
    }

    let mut settled: BTreeSet<&str> = BTreeSet::new();
    let mut violations = Vec::new();
    for start in graph.keys().copied() {
        let mut stack = vec![(start, 0_usize)];
        let mut path = vec![start];
        let mut on_path: BTreeSet<&str> = BTreeSet::from([start]);
        while let Some((node, index)) = stack.pop() {
            let edges = graph.get(node).map_or(&[][..], Vec::as_slice);
            if index >= edges.len() {
                settled.insert(node);
                on_path.remove(node);
                path.pop();
                continue;
            }
            stack.push((node, index + 1));
            let Some(next) = edges.get(index).copied() else {
                continue;
            };
            if on_path.contains(next) {
                let mut cycle: Vec<&str> = path.clone();
                cycle.push(next);
                violations.push(Violation {
                    rule: RULE,
                    detail: format!("dependency cycle: {}", cycle.join(" -> ")),
                });
                continue;
            }
            if settled.contains(next) {
                continue;
            }
            on_path.insert(next);
            path.push(next);
            stack.push((next, 0));
        }
    }
    violations.sort();
    violations.dedup();
    violations
}

/// Every member declares `publish = false`.
#[must_use]
pub fn every_member_is_unpublished(workspace: &Workspace) -> Vec<Violation> {
    const RULE: &str = "publish-false";
    workspace
        .metadata
        .members()
        .into_iter()
        .filter(|package| !package.is_unpublished())
        .map(|package| Violation {
            rule: RULE,
            detail: format!("`{}` does not declare publish = false", package.name),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        TreeListing, Violation, Workspace, dependency_direction_holds, every_member_is_unpublished,
        is_adapter_crate, is_domain_crate, is_vendor_dependency, library_targets_are_underscored,
        members_match_the_tree, names_match_directories, no_dependency_cycles,
        test_support_is_dev_only,
    };
    use crate::metadata::WorkspaceMetadata;

    fn package(name: &str, directory: &str, dependencies: &str, targets: &str) -> String {
        format!(
            r#"{{
              "id": "{name} 0.1.0 (path+file:///w/{directory})",
              "name": "{name}",
              "manifest_path": "/w/{directory}/Cargo.toml",
              "publish": [],
              "targets": [{targets}],
              "dependencies": [{dependencies}]
            }}"#
        )
    }

    fn workspace(packages: &[String], tree: TreeListing) -> Workspace {
        let ids: Vec<String> = packages
            .iter()
            .filter_map(|package| {
                let start = package.find("\"id\": \"")? + 7;
                let rest = package.get(start..)?;
                let end = rest.find('"')?;
                Some(format!("\"{}\"", rest.get(..end)?))
            })
            .collect();
        let json = format!(
            r#"{{ "workspace_root": "/w", "workspace_members": [{}], "packages": [{}] }}"#,
            ids.join(", "),
            packages.join(", ")
        );
        Workspace {
            metadata: WorkspaceMetadata::parse(&json).expect("the fixture parses"),
            tree,
        }
    }

    fn listing(pairs: &[(&str, &[&str])]) -> TreeListing {
        pairs
            .iter()
            .map(|(root, entries)| {
                (
                    (*root).to_owned(),
                    entries.iter().map(|entry| (*entry).to_owned()).collect(),
                )
            })
            .collect()
    }

    fn rules(violations: &[Violation]) -> Vec<&'static str> {
        violations.iter().map(|violation| violation.rule).collect()
    }

    #[test]
    fn vendor_and_role_predicates_classify_the_pinned_set() {
        assert!(is_vendor_dependency("aws-sdk-dynamodb"));
        assert!(is_vendor_dependency("aws_lambda_events"));
        assert!(is_vendor_dependency("lambda_http"));
        assert!(is_vendor_dependency("tokio"));
        assert!(!is_vendor_dependency("serde"));
        assert!(!is_vendor_dependency("thiserror"));
        assert!(is_adapter_crate("aex-session-dynamodb"));
        assert!(is_adapter_crate("aex-rds-data"));
        assert!(!is_adapter_crate("aex-session-domain"));
        assert!(
            !is_adapter_crate("aws-sdk-dynamodb"),
            "a vendor crate is not an AEX adapter"
        );
        assert!(is_domain_crate("aex-session-domain"));
    }

    #[test]
    fn an_undeclared_directory_is_reported() {
        let workspace = workspace(
            &[package(
                "aex-a",
                "crates/aex-a",
                "",
                r#"{ "name": "aex_a", "kind": ["lib"] }"#,
            )],
            listing(&[("crates", &["aex-a", "aex-orphan"])]),
        );
        let violations = members_match_the_tree(&workspace);
        assert_eq!(rules(&violations), vec!["members-match-tree"]);
        assert!(
            violations[0].detail.contains("aex-orphan"),
            "{:?}",
            violations[0]
        );
    }

    #[test]
    fn an_exempt_typescript_directory_is_accepted() {
        let workspace = workspace(
            &[package(
                "aex-a",
                "crates/aex-a",
                "",
                r#"{ "name": "aex_a", "kind": ["lib"] }"#,
            )],
            listing(&[
                ("crates", &["aex-a"]),
                ("services", &["stripe-command-edge"]),
            ]),
        );
        assert!(members_match_the_tree(&workspace).is_empty());
    }

    #[test]
    fn a_member_outside_every_root_is_reported() {
        let workspace = workspace(
            &[package(
                "aex-a",
                "stray/aex-a",
                "",
                r#"{ "name": "aex_a", "kind": ["lib"] }"#,
            )],
            listing(&[]),
        );
        let violations = members_match_the_tree(&workspace);
        assert_eq!(rules(&violations), vec!["members-match-tree"]);
        assert!(violations[0].detail.contains("outside every member root"));
    }

    #[test]
    fn a_directory_that_disagrees_with_the_package_name_is_reported() {
        let workspace = workspace(
            &[package(
                "aex-a",
                "crates/aex-b",
                "",
                r#"{ "name": "aex_a", "kind": ["lib"] }"#,
            )],
            listing(&[("crates", &["aex-b"])]),
        );
        assert_eq!(
            rules(&names_match_directories(&workspace)),
            vec!["name-equals-directory"]
        );
    }

    #[test]
    fn a_mismatched_library_target_is_reported() {
        let workspace = workspace(
            &[package(
                "aex-a",
                "crates/aex-a",
                "",
                r#"{ "name": "wrong", "kind": ["lib"] }"#,
            )],
            listing(&[("crates", &["aex-a"])]),
        );
        let violations = library_targets_are_underscored(&workspace);
        assert_eq!(rules(&violations), vec!["lib-target-name"]);
        assert!(violations[0].detail.contains("expected `aex_a`"));
    }

    #[test]
    fn a_binary_only_package_has_no_library_rule_to_break() {
        let workspace = workspace(
            &[package(
                "brain-mux",
                "runtimes/brain-mux",
                "",
                r#"{ "name": "brain-mux", "kind": ["bin"] }"#,
            )],
            listing(&[("runtimes", &["brain-mux"])]),
        );
        assert!(library_targets_are_underscored(&workspace).is_empty());
    }

    #[test]
    fn a_domain_crate_may_not_link_a_vendor_sdk() {
        let workspace = workspace(
            &[package(
                "aex-session-domain",
                "crates/aex-session-domain",
                r#"{ "name": "aws-sdk-dynamodb", "kind": null }"#,
                r#"{ "name": "aex_session_domain", "kind": ["lib"] }"#,
            )],
            listing(&[("crates", &["aex-session-domain"])]),
        );
        let violations = dependency_direction_holds(&workspace);
        assert_eq!(rules(&violations), vec!["dependency-direction"]);
        assert!(violations[0].detail.contains("aws-sdk-dynamodb"));
    }

    #[test]
    fn a_domain_crate_may_not_link_an_adapter_crate() {
        let workspace = workspace(
            &[package(
                "aex-session-domain",
                "crates/aex-session-domain",
                r#"{ "name": "aex-session-dynamodb", "kind": null }"#,
                r#"{ "name": "aex_session_domain", "kind": ["lib"] }"#,
            )],
            listing(&[("crates", &["aex-session-domain"])]),
        );
        let violations = dependency_direction_holds(&workspace);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains("adapter crate"));
    }

    #[test]
    fn an_application_crate_may_not_link_a_vendor_sdk() {
        let workspace = workspace(
            &[package(
                "aex-session-app",
                "crates/aex-session-app",
                r#"{ "name": "reqwest", "kind": null }"#,
                r#"{ "name": "aex_session_app", "kind": ["lib"] }"#,
            )],
            listing(&[("crates", &["aex-session-app"])]),
        );
        assert_eq!(
            rules(&dependency_direction_holds(&workspace)),
            vec!["dependency-direction"]
        );
    }

    #[test]
    fn a_pure_crate_may_link_a_vendor_sdk_in_dev_dependencies() {
        let workspace = workspace(
            &[package(
                "aex-session-domain",
                "crates/aex-session-domain",
                r#"{ "name": "wiremock", "kind": "dev" }"#,
                r#"{ "name": "aex_session_domain", "kind": ["lib"] }"#,
            )],
            listing(&[("crates", &["aex-session-domain"])]),
        );
        assert!(dependency_direction_holds(&workspace).is_empty());
    }

    #[test]
    fn a_production_dependency_on_test_support_is_reported() {
        let workspace = workspace(
            &[package(
                "regional-session-api",
                "services/regional-session-api",
                r#"{ "name": "aex-regional-test-support", "kind": null }"#,
                r#"{ "name": "regional-session-api", "kind": ["bin"] }"#,
            )],
            listing(&[("services", &["regional-session-api"])]),
        );
        let violations = test_support_is_dev_only(&workspace);
        assert_eq!(rules(&violations), vec!["test-support-is-dev-only"]);
        assert!(violations[0].detail.contains("outside [dev-dependencies]"));
    }

    #[test]
    fn test_only_packages_may_compose_with_each_other() {
        let workspace = workspace(
            &[
                package(
                    "aex-load-harness",
                    "tests/load/aex-load-harness",
                    r#"{ "name": "aex-test-harness", "kind": null }"#,
                    r#"{ "name": "aex_load_harness", "kind": ["lib"] }"#,
                ),
                package(
                    "aex-live-brain-mux",
                    "tests/live/aex-live-brain-mux",
                    r#"{ "name": "aex-test-harness", "kind": null }"#,
                    r#"{ "name": "aex_live_brain_mux", "kind": ["lib"] }"#,
                ),
            ],
            listing(&[
                ("tests/load", &["aex-load-harness"]),
                ("tests/live", &["aex-live-brain-mux"]),
            ]),
        );
        assert!(test_support_is_dev_only(&workspace).is_empty());
    }

    #[test]
    fn a_dev_dependency_on_test_support_is_accepted() {
        let workspace = workspace(
            &[package(
                "regional-session-api",
                "services/regional-session-api",
                r#"{ "name": "aex-regional-test-support", "kind": "dev" }"#,
                r#"{ "name": "regional-session-api", "kind": ["bin"] }"#,
            )],
            listing(&[("services", &["regional-session-api"])]),
        );
        assert!(test_support_is_dev_only(&workspace).is_empty());
    }

    #[test]
    fn a_cycle_between_two_members_is_reported() {
        let workspace = workspace(
            &[
                package(
                    "aex-a",
                    "crates/aex-a",
                    r#"{ "name": "aex-b", "kind": null }"#,
                    r#"{ "name": "aex_a", "kind": ["lib"] }"#,
                ),
                package(
                    "aex-b",
                    "crates/aex-b",
                    r#"{ "name": "aex-a", "kind": null }"#,
                    r#"{ "name": "aex_b", "kind": ["lib"] }"#,
                ),
            ],
            listing(&[("crates", &["aex-a", "aex-b"])]),
        );
        let violations = no_dependency_cycles(&workspace);
        assert!(!violations.is_empty(), "a two-node cycle must be reported");
        assert!(
            violations
                .iter()
                .all(|violation| violation.rule == "no-dependency-cycles")
        );
    }

    #[test]
    fn a_diamond_without_a_cycle_is_accepted() {
        let workspace = workspace(
            &[
                package(
                    "aex-a",
                    "crates/aex-a",
                    r#"{ "name": "aex-b", "kind": null }, { "name": "aex-c", "kind": null }"#,
                    r#"{ "name": "aex_a", "kind": ["lib"] }"#,
                ),
                package(
                    "aex-b",
                    "crates/aex-b",
                    r#"{ "name": "aex-d", "kind": null }"#,
                    r#"{ "name": "aex_b", "kind": ["lib"] }"#,
                ),
                package(
                    "aex-c",
                    "crates/aex-c",
                    r#"{ "name": "aex-d", "kind": null }"#,
                    r#"{ "name": "aex_c", "kind": ["lib"] }"#,
                ),
                package(
                    "aex-d",
                    "crates/aex-d",
                    "",
                    r#"{ "name": "aex_d", "kind": ["lib"] }"#,
                ),
            ],
            listing(&[("crates", &["aex-a", "aex-b", "aex-c", "aex-d"])]),
        );
        assert!(no_dependency_cycles(&workspace).is_empty());
    }

    #[test]
    fn a_publishable_member_is_reported() {
        let json = r#"{
          "workspace_root": "/w",
          "workspace_members": ["aex-a 0.1.0 (path+file:///w/crates/aex-a)"],
          "packages": [{
            "id": "aex-a 0.1.0 (path+file:///w/crates/aex-a)",
            "name": "aex-a",
            "manifest_path": "/w/crates/aex-a/Cargo.toml",
            "publish": null,
            "targets": [{ "name": "aex_a", "kind": ["lib"] }],
            "dependencies": []
          }]
        }"#;
        let workspace = Workspace {
            metadata: WorkspaceMetadata::parse(json).expect("the fixture parses"),
            tree: listing(&[("crates", &["aex-a"])]),
        };
        assert_eq!(
            rules(&every_member_is_unpublished(&workspace)),
            vec!["publish-false"]
        );
    }
}
/// Members under `services/` or `workers/` that do not yet expose a library.
///
/// Frozen, and shrink-only: the rule below fails on any member outside this
/// list, and fails again on a listed member that has since grown a library, so
/// the list cannot silently outlive the debt it records. There is no `--write`
/// and no number in it — the only legal edit is deleting a row, which is what
/// keeps two branches from merge-summing their way back to green.
pub const MAIN_ONLY_DEPLOYABLES: &[&str] = &[
    "central-authz",
    "central-control-worker",
    "observation-export-launcher",
    "observation-export-task",
    "regional-control",
    "regional-otlp",
    "runtime-control-worker",
    "usage-compute-worker",
    "usage-storage-worker",
    "usage-transfer-worker",
];

/// No member name ends `-application`; the application layer suffix is `-app`.
///
/// Both suffixes named the same layer, so `grep -- '-app$'` returned four of
/// the seven application crates and an agent that anchored its search concluded
/// the layer did not exist.
#[must_use]
pub fn application_suffix_is_app(workspace: &Workspace) -> Vec<Violation> {
    const RULE: &str = "application-suffix";
    workspace
        .metadata
        .members()
        .into_iter()
        .filter(|package| package.name.ends_with("-application"))
        .map(|package| Violation {
            rule: RULE,
            detail: format!(
                "`{}` ends `-application`; the application layer suffix is `-app`",
                package.name
            ),
        })
        .collect()
}

/// A `crates/` member's adapter suffix names what it actually talks to.
///
/// Three clauses, all decidable from `cargo metadata` alone:
///
/// - declaring `aws-sdk-dynamodb` means the name ends `-dynamodb`, so
///   `ls crates | grep dynamodb` finds every `DynamoDB` adapter rather than 8 of 14
/// - ending `-aws` means it declares some `aws-sdk-*` and not `aws-sdk-dynamodb`,
///   so `-aws` reads as "an AWS adapter that is not a table adapter"
/// - declaring any `aws-sdk-*` means it carries one of the frozen adapter
///   suffixes, so a new service cannot quietly widen the set
#[must_use]
pub fn adapter_suffix_names_the_service(workspace: &Workspace) -> Vec<Violation> {
    const RULE: &str = "adapter-suffix";
    let Ok(directories) = workspace.metadata.member_directories() else {
        return Vec::new();
    };
    let mut violations = Vec::new();
    for package in workspace.metadata.members() {
        let Some(path) = directories.get(&package.name) else {
            continue;
        };
        if !path.starts_with("crates/") {
            continue;
        }
        let normal: Vec<&str> = package
            .dependencies
            .iter()
            .filter(|dependency| dependency.kind() == DependencyKind::Normal)
            .map(|dependency| dependency.name.as_str())
            .collect();
        let dynamodb = normal.contains(&"aws-sdk-dynamodb");
        let any_aws = normal.iter().any(|name| name.starts_with("aws-sdk-"));

        if dynamodb && !package.name.ends_with("-dynamodb") {
            violations.push(Violation {
                rule: RULE,
                detail: format!(
                    "`{}` declares `aws-sdk-dynamodb` but does not end `-dynamodb`",
                    package.name
                ),
            });
        }
        if package.name.ends_with("-aws") {
            if dynamodb {
                violations.push(Violation {
                    rule: RULE,
                    detail: format!(
                        "`{}` ends `-aws` but declares `aws-sdk-dynamodb`; a table adapter ends `-dynamodb`",
                        package.name
                    ),
                });
            } else if !any_aws {
                violations.push(Violation {
                    rule: RULE,
                    detail: format!(
                        "`{}` ends `-aws` but declares no `aws-sdk-*` dependency",
                        package.name
                    ),
                });
            }
        }
        if any_aws && !is_adapter_crate(&package.name) {
            violations.push(Violation {
                rule: RULE,
                detail: format!(
                    "`{}` declares an `aws-sdk-*` dependency but carries no adapter suffix",
                    package.name
                ),
            });
        }
    }
    violations
}

/// Every `services/` and `workers/` member exposes a library.
///
/// `main.rs` is composition only. A main-only deployable cannot be reached by an
/// integration test. Without a library to call, tests tend to fall back to
/// brittle assertions against the text and layout of `main.rs`.
#[must_use]
pub fn deployables_expose_a_library(workspace: &Workspace) -> Vec<Violation> {
    deployables_expose_a_library_against(workspace, MAIN_ONLY_DEPLOYABLES)
}

/// [`deployables_expose_a_library`] against an explicit frozen list.
///
/// Separate so a unit test can exercise the rule on a one-member fixture
/// without the real twelve-row list reporting each of its members as missing.
#[must_use]
pub fn deployables_expose_a_library_against(
    workspace: &Workspace,
    frozen: &[&str],
) -> Vec<Violation> {
    const RULE: &str = "deployable-exposes-library";
    let Ok(directories) = workspace.metadata.member_directories() else {
        return Vec::new();
    };
    let mut violations = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for package in workspace.metadata.members() {
        let Some(path) = directories.get(&package.name) else {
            continue;
        };
        if !(path.starts_with("services/") || path.starts_with("workers/")) {
            continue;
        }
        let grandfathered = frozen.contains(&package.name.as_str());
        if package.library().is_some() {
            if grandfathered {
                violations.push(Violation {
                    rule: RULE,
                    detail: format!(
                        "`{}` now exposes a library; remove it from `MAIN_ONLY_DEPLOYABLES`",
                        package.name
                    ),
                });
            }
            continue;
        }
        seen.insert(package.name.as_str());
        if !grandfathered {
            violations.push(Violation {
                rule: RULE,
                detail: format!(
                    "`{}` exposes no library; `main.rs` is composition only",
                    package.name
                ),
            });
        }
    }
    for name in frozen {
        if !seen.contains(name) {
            violations.push(Violation {
                rule: RULE,
                detail: format!(
                    "`{name}` is listed in `MAIN_ONLY_DEPLOYABLES` but is not a main-only deployable; remove the row"
                ),
            });
        }
    }
    violations
}

#[cfg(test)]
mod suffix_and_shape_tests {
    use super::{
        TreeListing, Workspace, adapter_suffix_names_the_service, application_suffix_is_app,
        deployables_expose_a_library_against,
    };
    use crate::metadata::WorkspaceMetadata;

    fn one(name: &str, directory: &str, dependencies: &str, targets: &str) -> Workspace {
        let json = format!(
            r#"{{ "workspace_root": "/w",
                  "workspace_members": ["{name} 0.1.0 (path+file:///w/{directory})"],
                  "packages": [{{
                    "id": "{name} 0.1.0 (path+file:///w/{directory})",
                    "name": "{name}",
                    "manifest_path": "/w/{directory}/Cargo.toml",
                    "publish": [],
                    "targets": [{targets}],
                    "dependencies": [{dependencies}]
                  }}] }}"#
        );
        Workspace {
            metadata: WorkspaceMetadata::parse(&json).expect("the fixture parses"),
            tree: TreeListing::new(),
        }
    }

    const LIB: &str = r#"{ "name": "x", "kind": ["lib"] }"#;
    const BIN: &str = r#"{ "name": "x", "kind": ["bin"] }"#;
    const DDB: &str = r#"{ "name": "aws-sdk-dynamodb", "kind": null }"#;
    const S3: &str = r#"{ "name": "aws-sdk-s3", "kind": null }"#;

    #[test]
    fn an_application_suffix_is_refused() {
        let workspace = one("aex-a-application", "crates/aex-a-application", "", LIB);
        let violations = application_suffix_is_app(&workspace);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains("ends `-application`"));
    }

    #[test]
    fn an_app_suffix_is_silent() {
        let workspace = one("aex-a-app", "crates/aex-a-app", "", LIB);
        assert!(application_suffix_is_app(&workspace).is_empty());
    }

    #[test]
    fn a_dynamodb_crate_must_say_so() {
        let workspace = one("aex-a-aws", "crates/aex-a-aws", DDB, LIB);
        let violations = adapter_suffix_names_the_service(&workspace);
        assert!(
            violations
                .iter()
                .any(|v| v.detail.contains("does not end `-dynamodb`"))
        );
        assert!(
            violations
                .iter()
                .any(|v| v.detail.contains("a table adapter ends"))
        );
    }

    #[test]
    fn an_aws_crate_without_an_aws_sdk_is_refused() {
        let workspace = one("aex-a-aws", "crates/aex-a-aws", "", LIB);
        assert!(
            adapter_suffix_names_the_service(&workspace)
                .iter()
                .any(|v| v.detail.contains("declares no `aws-sdk-*`"))
        );
    }

    #[test]
    fn an_aws_sdk_without_an_adapter_suffix_is_refused() {
        let workspace = one("aex-a-domain", "crates/aex-a-domain", S3, LIB);
        assert!(
            adapter_suffix_names_the_service(&workspace)
                .iter()
                .any(|v| v.detail.contains("carries no adapter suffix"))
        );
    }

    #[test]
    fn a_correctly_named_adapter_is_silent() {
        let ddb = one("aex-a-dynamodb", "crates/aex-a-dynamodb", DDB, LIB);
        assert!(adapter_suffix_names_the_service(&ddb).is_empty());
        let s3 = one("aex-a-aws", "crates/aex-a-aws", S3, LIB);
        assert!(adapter_suffix_names_the_service(&s3).is_empty());
    }

    #[test]
    fn the_adapter_rule_only_judges_crates() {
        // A deployable may declare any SDK; only `crates/` carries the taxonomy.
        let workspace = one("a-worker", "workers/a-worker", DDB, BIN);
        assert!(adapter_suffix_names_the_service(&workspace).is_empty());
    }

    #[test]
    fn a_main_only_deployable_outside_the_frozen_list_is_refused() {
        let workspace = one("a-worker", "workers/a-worker", "", BIN);
        assert!(
            deployables_expose_a_library_against(&workspace, &[])
                .iter()
                .any(|v| v.detail.contains("exposes no library"))
        );
    }

    #[test]
    fn a_deployable_with_a_library_is_silent() {
        let workspace = one("a-worker", "workers/a-worker", "", LIB);
        assert!(deployables_expose_a_library_against(&workspace, &[]).is_empty());
    }

    #[test]
    fn a_grandfathered_member_that_grew_a_library_must_leave_the_list() {
        let workspace = one("central-authz", "services/central-authz", "", LIB);
        assert!(
            deployables_expose_a_library_against(&workspace, &["central-authz"])
                .iter()
                .any(|v| v.detail.contains("now exposes a library"))
        );
    }

    #[test]
    fn runtimes_are_exempt() {
        let workspace = one("brain-mux", "runtimes/brain-mux", "", BIN);
        assert!(deployables_expose_a_library_against(&workspace, &[]).is_empty());
    }
}
