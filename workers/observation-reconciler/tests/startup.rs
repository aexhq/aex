//! Start-up evidence: the artifact refuses to run without its exact
//! configuration, and it names the variable it is missing.
//!
//! A defaulted table, bucket, queue, duty or region silently binds the process
//! to the wrong plane, so every one of them is proven absent-fatal here against
//! the **built binary**, not against a library call.

use std::process::Command;

/// Every variable the deployable requires, in the order it validates them.
const REQUIRED: &[&str] = &[
    "AEX_PLANE",
    "AEX_REGION",
    "AEX_OBSERVATION_TABLE",
    "AEX_SESSION_TABLE",
    "AEX_OBSERVATION_BUCKET",
    "AEX_OBS_DUTY",
    "AEX_OBS_RECONCILE_PAGE",
    "AEX_OBS_DUTY_SHARDS",
    "AEX_OBS_MAX_ATTEMPTS",
    "AEX_USAGE_QUEUE_URL",
];

#[test]
fn an_empty_environment_refuses_to_start_and_names_the_first_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_observation-reconciler"))
        .env_clear()
        .output()
        .expect("the artifact runs");
    assert!(
        !output.status.success(),
        "an unconfigured reconciler must never start"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_PLANE"), "{stderr}");
    assert!(stderr.contains("refusing to start"), "{stderr}");
}

#[test]
fn the_refusal_lists_every_variable_the_deployment_owes() {
    let output = Command::new(env!("CARGO_BIN_EXE_observation-reconciler"))
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
    // Everything but the duty. A defaulted duty would silently make one
    // deployment do another deployment's work under another deployment's role.
    let output = Command::new(env!("CARGO_BIN_EXE_observation-reconciler"))
        .env_clear()
        .env("AEX_PLANE", "dev")
        .env("AEX_REGION", "eu-west-1")
        .env("AEX_OBSERVATION_TABLE", "observation-authority")
        .env("AEX_SESSION_TABLE", "session-authority")
        .env("AEX_OBSERVATION_BUCKET", "aex-dev-observations")
        .output()
        .expect("the artifact runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_OBS_DUTY"), "{stderr}");
}

#[test]
fn the_launcher_duty_is_refused_by_the_built_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_observation-reconciler"))
        .env_clear()
        .env("AEX_PLANE", "dev")
        .env("AEX_REGION", "eu-west-1")
        .env("AEX_OBSERVATION_TABLE", "observation-authority")
        .env("AEX_SESSION_TABLE", "session-authority")
        .env("AEX_OBSERVATION_BUCKET", "aex-dev-observations")
        .env("AEX_OBS_DUTY", "export.launch")
        .env("AEX_OBS_RECONCILE_PAGE", "100")
        .env("AEX_OBS_DUTY_SHARDS", "16")
        .env("AEX_OBS_MAX_ATTEMPTS", "12")
        .env(
            "AEX_USAGE_QUEUE_URL",
            "https://sqs.eu-west-1.amazonaws.com/1/aex-dev-usage.fifo",
        )
        .output()
        .expect("the artifact runs");
    assert!(
        !output.status.success(),
        "`export.launch` belongs to `observation-export-launcher`"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("AEX_OBS_DUTY"), "{stderr}");
    assert!(stderr.contains("observation-export-launcher"), "{stderr}");
}
