//! Start-up evidence: the artifact refuses to run without its exact
//! configuration, and it names the variable it is missing.
//!
//! A launcher that started with a defaulted cluster, task definition or table
//! would launch real Fargate tasks into the wrong plane. Every one of these
//! variables is therefore required and none has a default.

use std::process::Command;

/// Every variable the deployable requires, in the order it validates them.
const REQUIRED: &[&str] = &[
    "AEX_PLANE",
    "AEX_REGION",
    "AEX_OBSERVATION_TABLE",
    "AEX_EXPORT_CLUSTER",
    "AEX_EXPORT_TASK_DEFINITION",
    "AEX_EXPORT_SUBNETS",
    "AEX_EXPORT_SECURITY_GROUPS",
    "AEX_EXPORT_MAX_CONCURRENT",
    "AEX_EXPORT_LAUNCH_SHARDS",
    "AEX_EXPORT_LEASE_MS",
];

#[test]
fn an_empty_environment_refuses_to_start_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_observation-export-launcher"))
        .env_clear()
        .output()
        .expect("the artifact runs");
    assert!(
        !output.status.success(),
        "an unconfigured launcher must never start"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_PLANE"), "{stderr}");
    assert!(stderr.contains("refusing to start"), "{stderr}");
}

#[test]
fn the_refusal_lists_every_variable_the_deployable_requires() {
    let output = Command::new(env!("CARGO_BIN_EXE_observation-export-launcher"))
        .env_clear()
        .output()
        .expect("the artifact runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    for name in REQUIRED {
        assert!(
            stderr.contains(name),
            "`{name}` is required and absent from the refusal: {stderr}"
        );
    }
}

#[test]
fn a_partial_environment_still_refuses_and_names_the_gap() {
    // Everything but the observation table. A defaulted table identifier is the
    // failure this case exists to make impossible.
    let output = Command::new(env!("CARGO_BIN_EXE_observation-export-launcher"))
        .env_clear()
        .env("AEX_PLANE", "dev")
        .env("AEX_REGION", "eu-west-1")
        .output()
        .expect("the artifact runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_OBSERVATION_TABLE"), "{stderr}");
}

#[test]
fn a_cross_region_cluster_refuses_to_start_and_names_the_cluster_variable() {
    // The most dangerous misconfiguration this deployable can hold: a cluster
    // ARN from another region, which would launch export tasks outside the
    // regional plane the process is bound to.
    let output = Command::new(env!("CARGO_BIN_EXE_observation-export-launcher"))
        .env_clear()
        .env("AEX_PLANE", "dev")
        .env("AEX_REGION", "eu-west-1")
        .env("AEX_OBSERVATION_TABLE", "observation-authority")
        .env(
            "AEX_EXPORT_CLUSTER",
            "arn:aws:ecs:us-east-1:123456789012:cluster/aex-dev-export",
        )
        .output()
        .expect("the artifact runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_EXPORT_CLUSTER"), "{stderr}");
    assert!(stderr.contains("us-east-1"), "{stderr}");
}
