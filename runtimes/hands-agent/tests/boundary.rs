//! H-BOUNDARY B6, asserted against the binary that actually ships.
//!
//! `crates/aex-hands-agent` already scans its own closure, but the rootfs carries
//! *this* package: the composition root, its HTTP server, its async runtime and
//! its syscall wrappers. A cloud dependency arriving through any of those would
//! not be caught there, so the scan is repeated here against the artifact.

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

/// The normal-and-build dependency closure of the shipped binary, by package name.
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
        .find(|package| package.name.as_str() == "hands-agent")
        .expect("this package is in the graph");

    let mut seen = BTreeSet::new();
    let mut frontier = vec![root.id.clone()];
    while let Some(id) = frontier.pop() {
        let Some(node) = resolve.nodes.iter().find(|node| node.id == id) else {
            continue;
        };
        for dependency in &node.deps {
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
fn the_shipped_guest_binary_links_no_aws_sdk_or_credential_provider() {
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
        "the guest binary must hold no cloud authority, but its closure contains {offenders:?}"
    );
}

#[test]
fn the_binary_carries_no_tls_stack_it_could_reach_a_cloud_endpoint_with() {
    // Not a hygiene sweep: the workspace's pinned TLS backend is `aws-lc-rs`,
    // whose crate name the scan above already rejects. That is why workspace
    // materialize and persist — which need presigned HTTPS — are refused by this
    // guest and recorded as a gap rather than quietly linked in.
    let closure = closure();
    for client in ["reqwest", "rustls", "hyper-rustls", "native-tls"] {
        assert!(
            !closure.contains(client),
            "the guest links `{client}`, which is how a credential-free boundary stops being one"
        );
    }
}
