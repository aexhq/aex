//! The frozen member inventory.
//!
//! These lists are the mechanical form of the accepted architecture: Area 9's
//! crate and deployable registries as amended by Area 11 (no `ClickHouse`, no
//! Kinesis, so `aex-observation-clickhouse`, `observation-materializer` and
//! `observation-schema-admin` are absent). Adding a member without adding it
//! here fails `cargo run -p aex-workspace-check`, which is the point: a new
//! crate is an architecture decision, not an incidental file.

/// Directory roots that hold Cargo workspace members.
pub const MEMBER_ROOTS: &[&str] = &[
    "crates",
    "services",
    "workers",
    "runtimes",
    "tools",
    "tests/live",
];

/// Directories inside [`MEMBER_ROOTS`] that are deliberately not Cargo members.
///
/// The two Stripe edges are `TypeScript` Lambdas retained by `P-AUTH-EDGE`, and
/// `eslint-plugin-aex` is a retained `TypeScript` package.
pub const NON_CARGO_DIRECTORIES: &[&str] = &[
    "services/stripe-command-edge",
    "services/stripe-webhook-edge",
    "tools/eslint-plugin-aex",
];

/// The 64 library crates under `crates/`.
pub const CRATES: &[&str] = &[
    "aex-brain-application",
    "aex-brain-domain",
    "aex-brain-hands",
    "aex-brain-managed-web",
    "aex-brain-mcp",
    "aex-brain-provider-gateway",
    "aex-brain-store-aws",
    "aex-brain-test-support",
    "aex-brain-tool-catalog",
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
    "aex-observation-application",
    "aex-observation-domain",
    "aex-observation-export",
    "aex-observation-query",
    "aex-observation-store-aws",
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
    "aex-usage-application",
    "aex-usage-compute-aws",
    "aex-usage-domain",
    "aex-usage-query-aws",
    "aex-usage-rating",
    "aex-usage-storage-aws",
    "aex-usage-transfer-aws",
    "aex-wire",
    "aex-work-dynamodb",
    "aex-workspace-domain",
];

/// The 10 deployable services.
pub const SERVICES: &[&str] = &[
    "central-authz",
    "central-control-api",
    "central-identity-api",
    "finance-api",
    "finance-ingest",
    "regional-observation-api",
    "regional-otlp",
    "regional-secret-api",
    "regional-session-api",
    "regional-stream",
];

/// The 16 deployable workers.
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
    "regional-secret-key-admin",
    "runtime-control-worker",
    "session-operation-worker",
    "usage-compute-worker",
    "usage-receipt-dispatcher",
    "usage-storage-worker",
    "usage-transfer-worker",
];

/// The 3 deployable runtimes.
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
/// Every Rust deployable, both `TypeScript` Stripe edges, the dashboard, the
/// site and the model catalog, exactly as Area 9 names them.
pub const LIVE_TARGETS: &[&str] = &[
    "brain-mux",
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
    "regional-observation-api",
    "regional-otlp",
    "regional-secret-api",
    "regional-secret-key-admin",
    "regional-session-api",
    "regional-stream",
    "runtime-control-worker",
    "session-operation-worker",
    "site",
    "stripe-command-edge",
    "stripe-webhook-edge",
    "usage-compute-worker",
    "usage-receipt-dispatcher",
    "usage-storage-worker",
    "usage-transfer-worker",
];

/// Every expected member as `(root, package name)`.
#[must_use]
pub fn expected_members() -> Vec<(&'static str, String)> {
    let mut members: Vec<(&'static str, String)> = Vec::new();
    members.extend(CRATES.iter().map(|name| ("crates", (*name).to_owned())));
    members.extend(SERVICES.iter().map(|name| ("services", (*name).to_owned())));
    members.extend(WORKERS.iter().map(|name| ("workers", (*name).to_owned())));
    members.extend(RUNTIMES.iter().map(|name| ("runtimes", (*name).to_owned())));
    members.extend(TOOLS.iter().map(|name| ("tools", (*name).to_owned())));
    members.extend(
        LIVE_TARGETS
            .iter()
            .map(|name| ("tests/live", format!("aex-live-{name}"))),
    );
    members
}

#[cfg(test)]
mod tests {
    use super::{CRATES, LIVE_TARGETS, RUNTIMES, SERVICES, TOOLS, WORKERS, expected_members};

    fn is_sorted_and_unique(names: &[&str]) -> bool {
        names.windows(2).all(|pair| pair[0] < pair[1])
    }

    #[test]
    fn every_frozen_list_is_sorted_and_free_of_duplicates() {
        assert!(is_sorted_and_unique(CRATES), "crates");
        assert!(is_sorted_and_unique(SERVICES), "services");
        assert!(is_sorted_and_unique(WORKERS), "workers");
        assert!(is_sorted_and_unique(RUNTIMES), "runtimes");
        assert!(is_sorted_and_unique(TOOLS), "tools");
        assert!(is_sorted_and_unique(LIVE_TARGETS), "live targets");
    }

    #[test]
    fn the_frozen_counts_match_the_accepted_architecture() {
        assert_eq!(
            CRATES.len(),
            64,
            "Area 9 inventory minus aex-observation-clickhouse"
        );
        assert_eq!(SERVICES.len(), 10);
        assert_eq!(WORKERS.len(), 16);
        assert_eq!(RUNTIMES.len(), 3);
        assert_eq!(TOOLS.len(), 4);
        assert_eq!(LIVE_TARGETS.len(), 34);
        assert_eq!(expected_members().len(), 131);
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
