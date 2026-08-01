//! H-BOUNDARY B6: the guest binary has no cloud authority.
//!
//! The falsifying test is a scan of this crate's whole dependency closure. It
//! fails if an AWS SDK, a credential provider, a cloud client or a signing crate
//! ever appears — including transitively, which is how such a dependency actually
//! arrives.
//!
//! This is a structural control, not a review promise: the closure is read from
//! `cargo metadata`, so adding `aws-config` three crates deep still fails here.

use std::collections::BTreeSet;

/// Crate-name prefixes that indicate cloud authority of any kind.
const FORBIDDEN_PREFIXES: [&str; 9] = [
    "aws-",
    "aws_",
    "rusoto",
    "azure_",
    "google-cloud",
    "gcp-",
    "lambda_runtime",
    "lambda_http",
    "aex-rds-data",
];

/// Exact crate names that are forbidden even though their prefix is innocuous.
const FORBIDDEN_EXACT: [&str; 4] = [
    "aex-secret-aws",
    "aex-content-aws",
    "aex-hands-control-aws",
    "aex-runtime-control-aws",
];

/// The normal-and-build dependency closure of `aex-hands-agent`, by package name.
fn closure() -> BTreeSet<String> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .exec()
        .expect("cargo metadata resolves the graph");
    let resolve = metadata
        .resolve
        .as_ref()
        .expect("a resolved graph is present");
    let root = metadata
        .packages
        .iter()
        .find(|package| package.name.as_str() == "aex-hands-agent")
        .expect("this package is in the graph");

    let mut seen = BTreeSet::new();
    let mut frontier = vec![root.id.clone()];
    while let Some(id) = frontier.pop() {
        let Some(node) = resolve.nodes.iter().find(|node| node.id == id) else {
            continue;
        };
        for dependency in &node.deps {
            // Dev-dependencies are not in the shipped binary and are deliberately
            // excluded: `cargo_metadata` itself is one of them.
            let is_normal = dependency.dep_kinds.iter().any(|kind| {
                matches!(
                    kind.kind,
                    cargo_metadata::DependencyKind::Normal | cargo_metadata::DependencyKind::Build
                )
            });
            if !is_normal {
                continue;
            }
            let name = metadata
                .packages
                .iter()
                .find(|package| package.id == dependency.pkg)
                .map(|package| package.name.to_string())
                .unwrap_or_default();
            if seen.insert(name) {
                frontier.push(dependency.pkg.clone());
            }
        }
    }
    seen
}

#[test]
fn the_guest_agent_links_no_aws_sdk_or_credential_provider() {
    let closure = closure();
    assert!(
        !closure.is_empty(),
        "an empty closure would make this assertion vacuous"
    );
    let offenders: Vec<&String> = closure
        .iter()
        .filter(|name| {
            FORBIDDEN_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
                || FORBIDDEN_EXACT.contains(&name.as_str())
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "the guest agent must hold no cloud authority, but its closure contains {offenders:?}"
    );
}

#[test]
fn the_scan_would_actually_catch_a_forbidden_crate() {
    // A self-check on the matcher, so a broken predicate cannot make the control
    // above pass vacuously.
    let planted: BTreeSet<String> = ["serde", "aws-config", "tokio"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let offenders: Vec<&String> = planted
        .iter()
        .filter(|name| {
            FORBIDDEN_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
                || FORBIDDEN_EXACT.contains(&name.as_str())
        })
        .collect();
    assert_eq!(offenders, vec![&"aws-config".to_owned()]);
}
