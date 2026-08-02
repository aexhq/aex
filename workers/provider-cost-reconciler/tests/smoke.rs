//! The artifact starts, and refuses to start without its exact configuration.

use std::process::Command;

#[test]
fn the_artifact_fails_closed_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_provider-cost-reconciler"))
        .env_clear()
        .output()
        .expect("the cost reconciler artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_PROVIDER_COST_PLANE"),
        "the refusal names the missing variable: {stderr}"
    );
}

#[test]
fn an_unnormalized_export_prefix_is_refused_at_start_up() {
    let output = Command::new(env!("CARGO_BIN_EXE_provider-cost-reconciler"))
        .env_clear()
        .env("AEX_PROVIDER_COST_PLANE", "dev")
        .env("AEX_PROVIDER_COST_REGION", "eu-west-1")
        .env(
            "AEX_PROVIDER_COST_AURORA_CLUSTER_ARN",
            "arn:aws:rds:eu-west-1:000000000000:cluster:aex-central",
        )
        .env(
            "AEX_PROVIDER_COST_AURORA_SECRET_ARN",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/provider-cost",
        )
        .env("AEX_PROVIDER_COST_DATABASE_NAME", "aex")
        .env("AEX_PROVIDER_COST_DATABASE_ROLE", "aex_provider_cost")
        .env("AEX_PROVIDER_COST_CUR_BUCKET", "aex-dev-cost-exports")
        .env("AEX_PROVIDER_COST_CUR_PREFIX", "/cur/aex-dev/")
        .env("AEX_PROVIDER_COST_MARGIN_ALERT_THRESHOLD_BPS", "1500")
        .env(
            "AEX_PROVIDER_COST_ALARM_TOPIC_ARN",
            "arn:aws:sns:eu-west-1:000000000000:aex-dev-finance-ops",
        )
        .env("AEX_PROVIDER_COST_MAX_SCAN_BYTES", "536870912")
        .env("AEX_PROVIDER_COST_TX_DEADLINE_MS", "60000")
        .output()
        .expect("the cost reconciler artifact starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_PROVIDER_COST_CUR_PREFIX"),
        "the refusal names the rejected variable: {stderr}"
    );
}
