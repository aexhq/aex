//! Start-up, configuration and trigger facts for `session-operation-worker`.

use std::collections::BTreeMap;

use aex_regional_http::config::RegionalHttpConfigError;
use session_operation_worker::config::{self, Config};
use session_operation_worker::{
    BatchItem, DUE_SHARD_CONCURRENCY, Trigger, batch_response, due_shards,
};

fn complete() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (config::PLANE, "dev".to_owned()),
        (config::REGION, "eu-west-1".to_owned()),
        (config::RELEASE_DIGEST, "sha256:deadbeef".to_owned()),
        (
            config::OPERATION_QUEUE_URL,
            "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-regional-session-operations"
                .to_owned(),
        ),
        (
            config::OPERATION_DLQ_URL,
            "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-regional-session-operations-dlq"
                .to_owned(),
        ),
        (config::WORK_TABLE, "aex-dev-regional-work".to_owned()),
        (config::SESSION_TABLE, "aex-dev-session-authority".to_owned()),
        (config::CONTENT_TABLE, "aex-dev-regional-content".to_owned()),
        (config::REGISTRY_TABLE, "aex-dev-regional-registry".to_owned()),
        (config::CONTENT_BUCKET, "aex-dev-eu-west-1-content".to_owned()),
        (config::CONTENT_BUCKET_OWNER, "000000000000".to_owned()),
        (
            config::DENIAL_PROJECTION_TABLE,
            "aex-dev-deletion-denial".to_owned(),
        ),
        (config::DUE_SCAN_SHARDS, "64".to_owned()),
        (config::LEASE_MS, "60000".to_owned()),
        (config::STEP_DEADLINE_MS, "30000".to_owned()),
        (config::MAX_ATTEMPTS, "8".to_owned()),
    ])
}

fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, RegionalHttpConfigError> {
    Config::read(&|name: &str| vars.get(name).cloned())
}

#[test]
fn a_complete_environment_is_accepted() {
    let config = read(&complete()).expect("the complete environment is accepted");
    assert_eq!(config.due_scan_shards, 64);
    assert_eq!(config.lease_ms, 60_000);
    assert_eq!(config.max_attempts, 8);
}

#[test]
fn every_required_variable_is_required() {
    let vars = complete();
    assert_eq!(vars.len(), config::REQUIRED.len());
    for name in config::REQUIRED {
        let mut missing = vars.clone();
        missing.remove(name);
        let error = read(&missing).expect_err("a missing variable refuses the process");
        assert!(
            matches!(error, RegionalHttpConfigError::Missing { name: reported } if reported == name),
            "removing {name} reported {error:?}"
        );
    }
}

#[test]
fn a_queue_in_another_region_refuses_the_process() {
    let mut vars = complete();
    vars.insert(
        config::OPERATION_QUEUE_URL,
        "https://sqs.us-east-1.amazonaws.com/000000000000/aex-dev-regional-session-operations"
            .to_owned(),
    );
    let error = read(&vars).expect_err("a cross-region queue is refused");
    let RegionalHttpConfigError::Invalid { name, reason } = error else {
        panic!("expected an invalid-value refusal");
    };
    assert_eq!(name, config::OPERATION_QUEUE_URL);
    assert!(reason.contains("us-east-1"), "{reason}");
}

#[test]
fn the_worker_cannot_be_bound_to_a_secret_key_or_the_content_queue() {
    for (name, _) in config::FORBIDDEN {
        let mut vars = complete();
        vars.insert(name, "bound".to_owned());
        assert!(
            matches!(read(&vars), Err(RegionalHttpConfigError::Forbidden { name: reported, .. }) if reported == name),
            "binding {name} was accepted"
        );
    }
}

#[test]
fn a_zero_shard_count_refuses_the_process() {
    let mut vars = complete();
    vars.insert(config::DUE_SCAN_SHARDS, "0".to_owned());
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::DUE_SCAN_SHARDS
    ));
}

#[test]
fn an_sqs_batch_and_a_due_scan_are_told_apart_structurally() {
    let batch = serde_json::json!({ "Records": [] });
    assert_eq!(Trigger::classify(&batch), Trigger::Queue);

    let scan = serde_json::json!({ "detail-type": "aex.due_scan", "detail": {} });
    assert_eq!(Trigger::classify(&scan), Trigger::DueScan);

    // Anything else must fail rather than drain the queue silently.
    let other = serde_json::json!({ "detail-type": "aex.something_else" });
    assert_eq!(Trigger::classify(&other), Trigger::Unknown);
    assert_eq!(Trigger::classify(&serde_json::json!({})), Trigger::Unknown);
}

#[test]
fn the_due_scan_sweeps_every_shard_and_never_one_hot_partition() {
    let shards = due_shards(64).expect("the declared shard count");
    assert_eq!(shards.len(), 64);
    assert_eq!(shards.first().copied(), Some(0));
    assert_eq!(shards.last().copied(), Some(63));
    assert!(
        due_shards(0).is_err(),
        "a literal single due key is refused"
    );
    assert_eq!(DUE_SHARD_CONCURRENCY, 16, "AWS fanout stays bounded");
}

#[test]
fn a_shard_count_that_disagrees_with_the_table_contract_refuses_startup() {
    let mut vars = complete();
    vars.insert(config::DUE_SCAN_SHARDS, "16".to_owned());
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::DUE_SCAN_SHARDS
    ));
}

#[test]
fn an_attempt_boundary_that_disagrees_with_strict_v1_refuses_startup() {
    let mut vars = complete();
    vars.insert(config::MAX_ATTEMPTS, "7".to_owned());
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::MAX_ATTEMPTS
    ));
}

#[test]
fn a_partial_batch_reports_only_the_failed_items() {
    let items = [
        BatchItem::succeeded("m1"),
        BatchItem::failed("m2"),
        BatchItem::succeeded("m3"),
    ];
    let response = batch_response(&items);
    assert_eq!(response.batch_item_failures, vec!["m2".to_owned()]);
}
