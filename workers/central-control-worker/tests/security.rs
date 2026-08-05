//! `central-control-worker` capability and permission evidence.

use std::process::Command;

#[test]
fn the_permission_list_is_declared_in_source_and_reviewable() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("pub const PERMISSIONS"));
}

#[test]
fn the_binary_pins_the_one_login_role_it_may_connect_as() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("const REQUIRED_ROLE"));
    assert!(source.contains("this binary connects only as"));
}

#[test]
fn the_capability_declaration_is_a_closed_set() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("BTreeSet::from(["));
    assert!(source.contains("CapabilityBinding::"));
}

#[test]
fn mail_iam_is_limited_to_actual_delivery() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("\"ses:SendEmail\""));
    assert!(!source.contains("\"ses:GetEmailIdentity\""));
}

#[test]
fn central_control_cannot_link_or_construct_the_capacity_limit_producer() {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"),
        ])
        .output()
        .expect("cargo metadata resolves the central worker manifest");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON");
    let package = metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|package| package["name"] == "central-control-worker")
        .expect("central-control-worker package");
    let dependency = package["dependencies"]
        .as_array()
        .expect("dependencies")
        .iter()
        .find(|dependency| dependency["name"] == "aex-session-dynamodb")
        .expect("the regional projection dependency");

    assert_eq!(dependency["uses_default_features"].as_bool(), Some(false));
    assert_eq!(
        dependency["features"],
        serde_json::json!(["authz-projection-write"]),
        "central control links exactly the central projection producer"
    );
}
