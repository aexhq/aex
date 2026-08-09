//! Workspace roots and architecture classifications.
//!
//! Cargo metadata and the on-disk tree are authoritative for membership. These
//! lists classify the roots and the architecture-sensitive subsets that carry
//! additional rules; they are not a second hand-maintained member manifest.

/// Directory roots that hold Cargo workspace members.
pub const MEMBER_ROOTS: &[&str] = &[
    "crates",
    "services",
    "workers",
    "runtimes",
    "tools",
    "tests/live",
    "tests/support",
    "tests/load",
];

/// Directories inside [`MEMBER_ROOTS`] that are deliberately not Cargo members.
///
/// The two Stripe edges are `TypeScript` Lambdas retained by `P-AUTH-EDGE`, and
/// `eslint-plugin-aex` is a retained `TypeScript` package. The three `tests/load`
/// directories hold workload descriptors, tier profiles and the descriptor
/// schema: `D-11` puts load *executors* in the owning live companion, so
/// `aex-load-harness` is the only package that root will ever contain.
pub const NON_CARGO_DIRECTORIES: &[&str] = &[
    "services/stripe-command-edge",
    "services/stripe-webhook-edge",
    "tools/eslint-plugin-aex",
    "tests/load/profiles",
    "tests/load/schema",
    "tests/load/workloads",
];

/// Library crates with architecture-sensitive naming checks.
pub const CRATES: &[&str] = &[
    "aex-brain-app",
    "aex-brain-domain",
    "aex-brain-hands",
    "aex-brain-managed-web",
    "aex-brain-mcp",
    "aex-brain-provider-custody",
    "aex-brain-provider-gateway",
    "aex-brain-store-dynamodb",
    "aex-brain-test-support",
    "aex-brain-tool-catalog",
    "aex-capacity-dynamodb",
    "aex-central-aws",
    "aex-central-http",
    "aex-central-test-support",
    "aex-content-aws",
    "aex-content-domain",
    "aex-content-dynamodb",
    "aex-control-app",
    "aex-control-aurora",
    "aex-control-domain",
    "aex-finance-app",
    "aex-finance-aurora",
    "aex-finance-domain",
    "aex-hands-agent",
    "aex-hands-control-aws",
    "aex-hands-protocol",
    "aex-hands-tools",
    "aex-identity-app",
    "aex-identity-aurora",
    "aex-identity-domain",
    "aex-internal-contracts",
    "aex-model-catalog",
    "aex-observation-app",
    "aex-observation-domain",
    "aex-observation-export",
    "aex-observation-query",
    "aex-observation-store-dynamodb",
    "aex-observation-test-support",
    "aex-operation-domain",
    "aex-otlp-admission",
    "aex-payment-contracts",
    "aex-platform-telemetry",
    "aex-rds-data",
    "aex-regional-http",
    "aex-regional-test-support",
    "aex-registry-dynamodb",
    "aex-runtime-activity-dynamodb",
    "aex-runtime-control",
    "aex-runtime-control-aws",
    "aex-secret-aws",
    "aex-secret-custody-dynamodb",
    "aex-secret-domain",
    "aex-secret-keystore-dynamodb",
    "aex-session-app",
    "aex-session-domain",
    "aex-session-dynamodb",
    "aex-telemetry-schema",
    "aex-usage-app",
    "aex-usage-compute-dynamodb",
    "aex-usage-domain",
    "aex-usage-query-dynamodb",
    "aex-usage-rating",
    "aex-usage-storage-dynamodb",
    "aex-usage-transfer-dynamodb",
    "aex-wire",
    "aex-work-dynamodb",
    "aex-workspace-domain",
];

/// Deployable services that must own live companions.
pub const SERVICES: &[&str] = &[
    "central-api",
    "central-authz",
    "central-control-api",
    "central-identity-api",
    "finance-api",
    "finance-ingest",
    "regional-observation-api",
    "regional-otlp",
    "regional-secret-api",
    "session-stream-api",
];

/// Deployable workers that must own live companions.
pub const WORKERS: &[&str] = &[
    "central-control-worker",
    "central-schema-admin",
    "content-lifecycle-worker",
    "finance-reconcile",
    "finance-settlement-worker",
    "observation-export-launcher",
    "observation-export-task",
    "observation-reconciler",
    "provider-cost-reconciler",
    "regional-capacity-controller",
    "regional-control",
    "regional-secret-key-admin",
    "runtime-control-worker",
    "session-operation-worker",
    "usage-compute-worker",
    "usage-receipt-dispatcher",
    "usage-storage-worker",
    "usage-transfer-worker",
];

/// Deployable runtimes that must own live companions.
pub const RUNTIMES: &[&str] = &["brain-mux", "hands-agent", "hands-image"];

/// The workspace tools.
pub const TOOLS: &[&str] = &[
    "aex-cli",
    "aex-contract-gen",
    "aex-release-tool",
    "aex-workspace-check",
];

/// The targets that own a `tests/live/aex-live-<target>` companion package.
///
/// This set is checked against metadata-derived `live_suite` declarations plus
/// companions that explicitly record their deployable as not yet applicable.
pub const LIVE_TARGETS: &[&str] = &[
    "brain-mux",
    "central-api",
    "central-authz",
    "central-control-api",
    "central-control-worker",
    "central-identity-api",
    "central-schema-admin",
    "content-lifecycle-worker",
    "dashboard",
    "finance-api",
    "finance-ingest",
    "finance-reconcile",
    "finance-settlement-worker",
    "hands-agent",
    "hands-image",
    "model-catalog",
    "observation-export-launcher",
    "observation-export-task",
    "observation-reconciler",
    "provider-cost-reconciler",
    "regional-capacity-controller",
    "regional-control",
    "regional-observation-api",
    "regional-otlp",
    "regional-secret-api",
    "regional-secret-key-admin",
    "runtime-control-worker",
    "session-operation-worker",
    "session-stream-api",
    "site",
    "stripe-command-edge",
    "stripe-webhook-edge",
    "usage-compute-worker",
    "usage-receipt-dispatcher",
    "usage-storage-worker",
    "usage-transfer-worker",
];

/// The shared test-infrastructure packages, as `(root, package name)`.
///
/// These live outside `crates/` so the crate classification remains limited to
/// product libraries. They are `publish = false` dev-dependency-only
/// packages that must never appear in a production link graph.
/// `aex-test-harness` owns run identity, prefixes, budget, TTL, the cleanup
/// ledger, the secret canary, the fault ports and the pinned image registry;
/// `aex-load-harness` owns the load driver, arrival process, recorder and
/// sampler. They are split so load code is not built by the unit lane.
pub const HARNESSES: &[(&str, &str)] = &[
    ("tests/support", "aex-test-harness"),
    ("tests/load", "aex-load-harness"),
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        CRATES, HARNESSES, LIVE_TARGETS, MEMBER_ROOTS, NON_CARGO_DIRECTORIES, RUNTIMES, SERVICES,
        TOOLS, WORKERS,
    };

    fn is_sorted_and_unique(names: &[&str]) -> bool {
        names.windows(2).all(|pair| pair[0] < pair[1])
    }

    #[test]
    fn every_classification_is_sorted_and_free_of_duplicates() {
        assert!(is_sorted_and_unique(CRATES), "crates");
        assert!(is_sorted_and_unique(SERVICES), "services");
        assert!(is_sorted_and_unique(WORKERS), "workers");
        assert!(is_sorted_and_unique(RUNTIMES), "runtimes");
        assert!(is_sorted_and_unique(TOOLS), "tools");
        assert!(is_sorted_and_unique(LIVE_TARGETS), "live targets");
        assert_eq!(
            HARNESSES.iter().copied().collect::<BTreeSet<_>>().len(),
            HARNESSES.len(),
            "duplicate harness classification"
        );
    }

    #[test]
    fn each_harness_lives_under_a_declared_member_root() {
        for (root, name) in HARNESSES {
            assert!(MEMBER_ROOTS.contains(root), "{root}");
            assert!(!CRATES.contains(name), "`{name}` must stay out of crates/");
        }
    }

    #[test]
    fn every_non_cargo_directory_sits_inside_a_member_root() {
        for path in NON_CARGO_DIRECTORIES {
            let root = path
                .rsplit_once('/')
                .map(|(root, _)| root)
                .expect("a non-Cargo directory is always `<root>/<name>`");
            assert!(MEMBER_ROOTS.contains(&root), "{path}");
        }
    }

    #[test]
    fn every_rust_deployable_owns_a_live_companion() {
        for name in SERVICES.iter().chain(WORKERS).chain(RUNTIMES) {
            assert!(
                LIVE_TARGETS.contains(name),
                "`{name}` has no live companion"
            );
        }
    }

    #[test]
    fn the_dropped_clickhouse_members_are_absent() {
        assert!(!CRATES.contains(&"aex-observation-clickhouse"));
        assert!(!WORKERS.contains(&"observation-materializer"));
        assert!(!WORKERS.contains(&"observation-schema-admin"));
        assert!(!LIVE_TARGETS.contains(&"observation-materializer"));
        assert!(!LIVE_TARGETS.contains(&"observation-schema-admin"));
    }

    #[test]
    fn every_crate_uses_the_aex_prefix() {
        for name in CRATES {
            assert!(name.starts_with("aex-"), "{name}");
        }
        for name in ["shared", "core", "utils", "common"] {
            assert!(!CRATES.contains(&name), "{name}");
            assert!(
                !CRATES.iter().any(|crate_name| crate_name.ends_with(name)),
                "{name}"
            );
        }
    }
}
