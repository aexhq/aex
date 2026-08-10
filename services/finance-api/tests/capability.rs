//! Capability separation, proved from the link graph rather than from review.
//!
//! A grant table nobody can violate is worth more than a grant table nobody
//! checks. These cases walk the workspace-internal dependency closure of each
//! central deployable and assert what it *cannot* link: a finance deployable
//! cannot reach identity or control DML, an identity deployable cannot reach
//! the finance authority, and only the one-shot schema task links `sqlx`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The repository root, from this package's manifest directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root resolves")
}

/// One manifest's declared dependency names, workspace-internal and external.
#[derive(Debug, Default, Clone)]
struct Manifest {
    dependencies: BTreeSet<String>,
}

/// Every Cargo manifest under `crates/`, `services/` and `workers/`.
fn manifests() -> BTreeMap<String, Manifest> {
    let root = workspace_root();
    let mut found = BTreeMap::new();
    for group in ["crates", "services", "workers"] {
        let directory = root.join(group);
        let entries = std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("`{}` is readable: {error}", directory.display()));
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            let manifest_path = path.join("Cargo.toml");
            if !manifest_path.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&manifest_path)
                .unwrap_or_else(|error| panic!("`{}` reads: {error}", manifest_path.display()));
            let document: toml::Value = toml::from_str(&text)
                .unwrap_or_else(|error| panic!("`{}` parses: {error}", manifest_path.display()));
            let name = document
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| panic!("`{}` names a package", manifest_path.display()))
                .to_owned();
            let mut dependencies = BTreeSet::new();
            for table in ["dependencies", "build-dependencies"] {
                if let Some(toml::Value::Table(entries)) = document.get(table) {
                    dependencies.extend(entries.keys().cloned());
                }
            }
            found.insert(name, Manifest { dependencies });
        }
    }
    found
}

/// Every workspace-internal and external dependency reachable from `root`.
fn closure(manifests: &BTreeMap<String, Manifest>, root: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut frontier = vec![root.to_owned()];
    while let Some(name) = frontier.pop() {
        let Some(manifest) = manifests.get(&name) else {
            continue;
        };
        for dependency in &manifest.dependencies {
            if seen.insert(dependency.clone()) {
                frontier.push(dependency.clone());
            }
        }
    }
    seen
}

/// The seven deployables this stream owns.
const FINANCE_DEPLOYABLES: [&str; 7] = [
    "finance-api",
    "finance-ingest",
    "finance-settlement-worker",
    "finance-reconcile",
    "usage-receipt-dispatcher",
    "provider-cost-reconciler",
    "central-schema-admin",
];

/// The adapters that carry identity and control DML.
const IDENTITY_DML: [&str; 2] = ["aex-identity-aurora", "aex-control-aurora"];

/// The libraries that carry the money authority.
///
/// There is no shared finance SQL adapter: each finance deployable owns its own
/// statements, so the authority a central identity deployable must not link is
/// the domain and its use cases.
const FINANCE_AUTHORITY: [&str; 2] = ["aex-finance-domain", "aex-finance-app"];

#[test]
fn no_finance_deployable_can_link_identity_or_control_dml() {
    let manifests = manifests();
    for deployable in FINANCE_DEPLOYABLES {
        let linked = closure(&manifests, deployable);
        for forbidden in IDENTITY_DML {
            assert!(
                !linked.contains(forbidden),
                "`{deployable}` links `{forbidden}`; a finance deployable holds no identity or \
                 control DML"
            );
        }
    }
}

#[test]
fn no_central_identity_deployable_can_link_the_money_authority() {
    let manifests = manifests();
    for deployable in [
        "central-identity-api",
        "central-authz",
        "central-control-api",
        "central-control-worker",
    ] {
        if !manifests.contains_key(deployable) {
            continue;
        }
        let linked = closure(&manifests, deployable);
        for forbidden in FINANCE_AUTHORITY {
            assert!(
                !linked.contains(forbidden),
                "`{deployable}` links `{forbidden}`; the money authority is finance-owned"
            );
        }
    }
}

#[test]
fn only_the_one_shot_schema_task_links_a_native_postgresql_driver() {
    let manifests = manifests();
    for deployable in FINANCE_DEPLOYABLES {
        let linked = closure(&manifests, deployable);
        let native = linked.contains("sqlx");
        assert_eq!(
            native,
            deployable == "central-schema-admin",
            "`{deployable}` must {} link `sqlx`; every request and worker Lambda reaches Aurora \
             through the Data API and stays out of the VPC",
            if deployable == "central-schema-admin" {
                ""
            } else {
                "not"
            }
        );
    }
}

#[test]
fn no_finance_deployable_links_a_regional_store_or_a_provider_sdk() {
    let manifests = manifests();
    for deployable in FINANCE_DEPLOYABLES {
        let linked = closure(&manifests, deployable);
        for forbidden in ["aws-sdk-dynamodb", "aws-sdk-kms", "rmcp", "reqwest"] {
            assert!(
                !linked.contains(forbidden),
                "`{deployable}` links `{forbidden}`; central finance holds no regional store, no \
                 data key and no outbound provider client"
            );
        }
    }
}

#[test]
fn every_owned_deployable_is_a_real_workspace_member() {
    let manifests = manifests();
    for deployable in FINANCE_DEPLOYABLES {
        assert!(
            manifests.contains_key(deployable),
            "`{deployable}` is not a workspace member"
        );
    }
}
