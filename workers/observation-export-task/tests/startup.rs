//! Start-up evidence: the artifact refuses to run without its exact
//! configuration, and it names the variable it is missing.
//!
//! A one-shot task that starts with a defaulted table, bucket, workspace or
//! export identifier would write a customer's artifact from the wrong plane, so
//! every one of them is proven required here against the real binary.

use std::process::Command;

/// Every variable the deployable requires, in the order it validates them.
const REQUIRED: &[&str] = &[
    "AEX_PLANE",
    "AEX_REGION",
    "AEX_EXPORT_ID",
    "AEX_WORKSPACE_ID",
    "AEX_OBSERVATION_TABLE",
    "AEX_OBSERVATION_BUCKET",
    "AEX_EXPORT_MEMORY_BUDGET_BYTES",
    "AEX_EXPORT_PART_BYTES",
    "AEX_EXPORT_ROWGROUP_BYTES",
    "AEX_EXPORT_PAGE_LIMIT",
    "AEX_EXPORT_LEASE_MS",
];

/// A workspace identifier of the right kind, for the partial-environment cases.
const WORKSPACE: &str = "wsp_0000000001e40r2081040g2081";
/// An export identifier of the right kind.
const EXPORT: &str = "exp_0000000001e40r2081040g2081";

#[test]
fn an_empty_environment_refuses_to_start_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_observation-export-task"))
        .env_clear()
        .output()
        .expect("the artifact runs");
    assert!(
        !output.status.success(),
        "an unconfigured export task must never start"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_PLANE"), "{stderr}");
    assert!(stderr.contains("refusing to start"), "{stderr}");
}

#[test]
fn the_refusal_lists_every_variable_the_deployable_requires() {
    let output = Command::new(env!("CARGO_BIN_EXE_observation-export-task"))
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
    // Everything up to the export identity. A defaulted export identifier is
    // the failure this case exists to make impossible.
    let output = Command::new(env!("CARGO_BIN_EXE_observation-export-task"))
        .env_clear()
        .env("AEX_PLANE", "dev")
        .env("AEX_REGION", "eu-west-1")
        .output()
        .expect("the artifact runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_EXPORT_ID"), "{stderr}");
}

#[test]
fn a_part_size_below_the_provider_floor_refuses_by_name() {
    let output = Command::new(env!("CARGO_BIN_EXE_observation-export-task"))
        .env_clear()
        .env("AEX_PLANE", "dev")
        .env("AEX_REGION", "eu-west-1")
        .env("AEX_EXPORT_ID", EXPORT)
        .env("AEX_WORKSPACE_ID", WORKSPACE)
        .env("AEX_OBSERVATION_TABLE", "observation-authority")
        .env("AEX_OBSERVATION_BUCKET", "aex-dev-observations")
        .env("AEX_EXPORT_MEMORY_BUDGET_BYTES", "536870912")
        // One byte under the 5 MiB S3 requires of a non-final part.
        .env("AEX_EXPORT_PART_BYTES", "5242879")
        .env("AEX_EXPORT_ROWGROUP_BYTES", "67108864")
        .env("AEX_EXPORT_PAGE_LIMIT", "500")
        .env("AEX_EXPORT_LEASE_MS", "300000")
        .output()
        .expect("the artifact runs");
    assert!(
        !output.status.success(),
        "a part size S3 cannot complete must never start an export"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_EXPORT_PART_BYTES"), "{stderr}");
    assert!(stderr.contains("5 MiB"), "{stderr}");
}

#[test]
fn a_budget_that_cannot_cover_the_reservations_refuses_before_anything_runs() {
    let output = Command::new(env!("CARGO_BIN_EXE_observation-export-task"))
        .env_clear()
        .env("AEX_PLANE", "dev")
        .env("AEX_REGION", "eu-west-1")
        .env("AEX_EXPORT_ID", EXPORT)
        .env("AEX_WORKSPACE_ID", WORKSPACE)
        .env("AEX_OBSERVATION_TABLE", "observation-authority")
        .env("AEX_OBSERVATION_BUCKET", "aex-dev-observations")
        .env("AEX_EXPORT_MEMORY_BUDGET_BYTES", "1048576")
        .env("AEX_EXPORT_PART_BYTES", "16777216")
        .env("AEX_EXPORT_ROWGROUP_BYTES", "67108864")
        .env("AEX_EXPORT_PAGE_LIMIT", "500")
        .env("AEX_EXPORT_LEASE_MS", "300000")
        .output()
        .expect("the artifact runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AEX_EXPORT_MEMORY_BUDGET_BYTES"),
        "{stderr}"
    );
    assert!(
        stderr.contains("export_capacity"),
        "the refusal names the wire code the launcher retries on: {stderr}"
    );
}
