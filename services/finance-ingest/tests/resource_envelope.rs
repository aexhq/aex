//! The declared deployment envelope of the consolidated billing worker.

use std::{collections::BTreeSet, path::Path};

use finance_ingest::{config, reconcile, settlement};

fn unit(id: &str) -> toml::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/units.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("`{}` reads: {error}", path.display()));
    let document: toml::Value = toml::from_str(&text)
        .unwrap_or_else(|error| panic!("`{}` parses: {error}", path.display()));
    document
        .get("unit")
        .and_then(toml::Value::as_array)
        .expect("release/units.toml declares units")
        .iter()
        .find(|row| row.get("id").and_then(toml::Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("release/units.toml declares `{id}`"))
        .clone()
}

#[test]
fn the_deployable_declares_its_artifact_and_live_evidence_owner() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("artifact = \"lambda_zip\""));
    assert!(manifest.contains("deployable = \"billing-worker\""));
    assert!(manifest.contains("live_suite = \"aex-live-billing-worker\""));
}

#[test]
fn the_release_registry_declares_the_exact_consolidated_topology() {
    let row = unit("billing-worker");
    assert_eq!(
        row.get("kind").and_then(toml::Value::as_str),
        Some("rust-lambda")
    );
    assert_eq!(
        row.get("plane").and_then(toml::Value::as_str),
        Some("central")
    );
    assert_eq!(
        row.get("package").and_then(toml::Value::as_str),
        Some("finance-ingest")
    );
    assert_eq!(
        row.get("bin").and_then(toml::Value::as_str),
        Some("finance-ingest")
    );
    assert_eq!(
        row.get("config_env_namespace")
            .and_then(toml::Value::as_str),
        Some("AEX_BILLING_WORKER_")
    );

    let shape = row
        .get("lambda")
        .expect("billing-worker declares [unit.lambda]");
    assert_eq!(
        shape.get("memory_mb").and_then(toml::Value::as_integer),
        Some(1_024)
    );
    assert_eq!(
        shape.get("timeout_s").and_then(toml::Value::as_integer),
        Some(300)
    );
    assert_eq!(
        shape
            .get("reserved_concurrency")
            .and_then(toml::Value::as_integer),
        Some(40)
    );
}

#[test]
fn the_event_driven_lambda_declares_no_http_health_route() {
    let row = unit("billing-worker");
    assert!(
        row.get("health_path").is_none() && row.get("ready_path").is_none(),
        "billing-worker is event-driven and serves no HTTP; readiness is an invocation payload"
    );
}

#[test]
fn the_three_modes_share_one_exact_environment_contract() {
    let actual = config::REQUIRED_VARS
        .into_iter()
        .chain(settlement::config::REQUIRED_VARS)
        .chain(reconcile::config::REQUIRED_VARS)
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "AEX_BILLING_WORKER_ALARM_TOPIC_ARN",
        "AEX_BILLING_WORKER_AURORA_CLUSTER_ARN",
        "AEX_BILLING_WORKER_AURORA_SECRET_ARN",
        "AEX_BILLING_WORKER_DATABASE_NAME",
        "AEX_BILLING_WORKER_MAX_GROUP_BATCH",
        "AEX_BILLING_WORKER_PINNED_STRIPE_API_VERSION",
        "AEX_BILLING_WORKER_PLANE",
        "AEX_BILLING_WORKER_RATING_QUEUE_URL",
        "AEX_BILLING_WORKER_REGION",
        "AEX_BILLING_WORKER_SERIALIZATION_RETRY_MAX",
        "AEX_BILLING_WORKER_STATEMENT_BUCKET",
        "AEX_BILLING_WORKER_STRIPE_API_SECRET_ARN",
        "AEX_BILLING_WORKER_SWEEP_PAGE",
        "AEX_BILLING_WORKER_TX_DEADLINE_MS",
        "AEX_BILLING_WORKER_UNKNOWN_EFFECT_RETRY_WINDOW_HOURS",
    ]);
    assert_eq!(actual, expected);
    assert!(
        actual
            .iter()
            .all(|name| name.starts_with(config::NAMESPACE)),
        "every mode is configured through the registered billing-worker namespace"
    );
    assert!(
        actual.iter().all(|name| !name.ends_with("_DATABASE_ROLE")),
        "database roles are code-owned authorities, not injectable configuration"
    );
    assert_eq!(
        [
            config::REQUIRED_ROLE,
            settlement::config::REQUIRED_ROLE,
            reconcile::config::REQUIRED_ROLE,
        ],
        [
            "aex_finance_ingest",
            "aex_finance_settlement",
            "aex_finance_reconcile",
        ]
    );
}
