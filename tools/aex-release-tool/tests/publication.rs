//! Public release input packaging and acquisition adversarial tests.

mod common;

use std::fs;
use std::io::Read as _;

use aex_release_tool::canon;
use aex_release_tool::manifest::CompositionManifest;
use aex_release_tool::publication::{
    package_module_bundle_from_paths, regional_tables_bundle, verify_blob,
    verify_regional_tables_bundle,
};
use common::docs::valid_manifest;

#[test]
fn module_bundle_is_deterministic_and_rooted_at_modules() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("infra/modules/example/tests")).unwrap();
    fs::write(
        root.path().join("infra/modules/example/main.tf"),
        b"module body\n",
    )
    .unwrap();
    fs::write(
        root.path()
            .join("infra/modules/example/tests/example.tftest.hcl"),
        b"test body\n",
    )
    .unwrap();
    let reversed = vec![
        "infra/modules/example/tests/example.tftest.hcl".to_owned(),
        "infra/modules/example/main.tf".to_owned(),
    ];
    let forward = reversed.iter().rev().cloned().collect::<Vec<_>>();

    let first = package_module_bundle_from_paths(root.path(), &forward).unwrap();
    let second = package_module_bundle_from_paths(root.path(), &reversed).unwrap();

    assert_eq!(canon::digest_bytes(&first), canon::digest_bytes(&second));
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(first.as_slice())
        .read_to_end(&mut tar)
        .unwrap();
    assert!(
        tar.windows(b"modules/example/main.tf".len())
            .any(|bytes| { bytes == b"modules/example/main.tf" })
    );
    assert!(
        !tar.windows(b"infra/modules".len())
            .any(|bytes| bytes == b"infra/modules")
    );
}

#[test]
fn unit_asset_names_reject_path_and_case_injection() {
    let digest = canon::digest_bytes(b"artifact");
    for hostile in ["../unit", "Unit-name", "a/b", "ab"] {
        let err =
            aex_release_tool::publication::unit_asset_name(hostile, &digest, "zip").unwrap_err();
        assert!(err.rules().contains(&"publication-unit-id"));
    }
    let err = aex_release_tool::publication::unit_asset_name("safe-unit", &digest, "oci-image")
        .unwrap_err();
    assert!(err.rules().contains(&"publication-form"));
}

#[test]
fn module_bundle_rejects_unsafe_shapes_and_non_regular_members() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("infra/modules/example")).unwrap();
    fs::write(
        root.path().join("infra/modules/example/secret.tfvars"),
        b"secret",
    )
    .unwrap();
    fs::write(root.path().join("outside.tf"), b"outside").unwrap();

    let err = package_module_bundle_from_paths(
        root.path(),
        &["infra/modules/example/secret.tfvars".to_owned()],
    )
    .unwrap_err();
    assert!(err.rules().contains(&"module-bundle-member-forbidden"));

    fs::create_dir(root.path().join("infra/modules/example/not-a-file.tf")).unwrap();
    let err = package_module_bundle_from_paths(
        root.path(),
        &["infra/modules/example/not-a-file.tf".to_owned()],
    )
    .unwrap_err();
    assert!(err.rules().contains(&"module-bundle-member-not-regular"));

    let err =
        package_module_bundle_from_paths(root.path(), &["outside.tf".to_owned()]).unwrap_err();
    assert!(err.rules().contains(&"module-bundle-member-outside-root"));

    fs::create_dir_all(root.path().join("infra/modules/example/.terraform")).unwrap();
    fs::write(
        root.path()
            .join("infra/modules/example/.terraform/cache.tf"),
        b"generated",
    )
    .unwrap();
    let err = package_module_bundle_from_paths(
        root.path(),
        &["infra/modules/example/.terraform/cache.tf".to_owned()],
    )
    .unwrap_err();
    assert!(err.rules().contains(&"module-bundle-member-forbidden"));
}

#[test]
fn blob_identity_rejects_missing_tampered_and_truncated_inputs() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("subject");
    fs::write(&path, b"published bytes").unwrap();
    let digest = canon::digest_bytes(b"published bytes");

    verify_blob(&path, &digest, 15).unwrap();

    fs::write(&path, b"tampered bytes!").unwrap();
    let err = verify_blob(&path, &digest, 15).unwrap_err();
    assert_eq!(err.exit.code(), 21);
    assert!(err.rules().contains(&"published-blob-digest-mismatch"));

    let err = verify_blob(&path, &canon::digest_bytes(b"tampered bytes!"), 999).unwrap_err();
    assert_eq!(err.exit.code(), 21);
    assert!(err.rules().contains(&"published-blob-size-mismatch"));

    let missing = root.path().join("missing");
    let err = verify_blob(&missing, &digest, 15).unwrap_err();
    assert_eq!(err.exit.code(), 21);
    assert!(err.rules().contains(&"published-blob-missing"));
}

fn regional_document(definitions_digest: &str) -> Vec<u8> {
    format!(
        "{{\"schema\":\"aex.regional-tables.v1\",\"digest\":\"{definitions_digest}\",\"tables\":[{{\"table\":\"sessions\"}}]}}\n"
    )
    .into_bytes()
}

#[test]
fn regional_table_bundle_keeps_transport_and_definition_identities_separate() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("migrations/regional/generated");
    fs::create_dir_all(&directory).unwrap();
    let definitions_digest = format!("blake3:{}", "ab".repeat(32));
    let bytes = regional_document(&definitions_digest);
    fs::write(directory.join("regional-tables.json"), &bytes).unwrap();

    let (observed, identity) = regional_tables_bundle(root.path()).unwrap();
    assert_eq!(observed, bytes);
    assert_eq!(identity.digest, canon::digest_bytes(&bytes));
    assert_eq!(identity.size_bytes, bytes.len() as u64);
    assert_eq!(identity.definitions_digest, definitions_digest);

    let acquired = root.path().join("acquired.json");
    fs::write(&acquired, &bytes).unwrap();
    verify_regional_tables_bundle(
        &acquired,
        &identity.digest,
        identity.size_bytes,
        &identity.definitions_digest,
    )
    .unwrap();
    let err = verify_regional_tables_bundle(
        &acquired,
        &identity.digest,
        identity.size_bytes,
        &format!("blake3:{}", "cd".repeat(32)),
    )
    .unwrap_err();
    assert!(err.rules().contains(&"regional-tables-identity"));
}

#[test]
fn manifest_admission_cross_checks_all_downloaded_subjects() {
    let root = tempfile::tempdir().unwrap();
    let tool = root.path().join("aex-release-tool");
    let modules = root.path().join("terraform-modules.tar.gz");
    let regional = root.path().join("regional-tables.json");
    let definitions_digest = format!("blake3:{}", "23".repeat(32));
    let regional_bytes = regional_document(&definitions_digest);
    fs::write(&tool, b"tool bytes").unwrap();
    fs::write(&modules, b"module bytes").unwrap();
    fs::write(&regional, &regional_bytes).unwrap();

    let mut value = valid_manifest();
    value["releaseTool"]["digest"] = serde_json::json!(canon::digest_bytes(b"tool bytes"));
    value["releaseTool"]["sizeBytes"] = serde_json::json!(10);
    value["infra"]["moduleBundleDigest"] = serde_json::json!(canon::digest_bytes(b"module bytes"));
    value["infra"]["moduleBundleSizeBytes"] = serde_json::json!(12);
    value["migrations"]["regional"]["bundleDigest"] =
        serde_json::json!(canon::digest_bytes(&regional_bytes));
    value["migrations"]["regional"]["bundleSizeBytes"] = serde_json::json!(regional_bytes.len());
    value["migrations"]["regional"]["definitionsDigest"] = serde_json::json!(definitions_digest);
    let manifest = serde_json::from_value::<CompositionManifest>(value)
        .unwrap()
        .seal()
        .unwrap();

    manifest
        .validate_acquired_inputs(&tool, &modules, &regional, true)
        .unwrap();
    fs::write(&modules, b"altered bytes").unwrap();
    let err = manifest
        .validate_acquired_inputs(&tool, &modules, &regional, true)
        .unwrap_err();
    assert_eq!(err.exit.code(), 21);
    assert!(err.rules().contains(&"published-blob-digest-mismatch"));
}

#[test]
fn manifest_rejects_a_release_asset_uri_from_another_run() {
    let mut value = valid_manifest();
    value["releaseTool"]["uri"] = serde_json::json!(
        "https://github.com/aexhq/aex/releases/download/main-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-run-999-attempt-1/aex-release-tool"
    );
    let manifest = serde_json::from_value::<CompositionManifest>(value)
        .unwrap()
        .seal()
        .unwrap();
    let err = manifest.validate(false).unwrap_err();
    assert_eq!(err.exit.code(), 30);
    assert!(err.rules().contains(&"manifest-release-tool-uri"));
}

#[test]
fn manifest_cross_binds_unit_blob_and_oci_locations_to_its_source() {
    let mut value = valid_manifest();
    let digest = value["units"]["regional-session-api"]["artifactDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    let uri = aex_release_tool::publication::github_release_unit_uri(
        "aexhq/aex",
        &"a".repeat(40),
        "123",
        1,
        "regional-session-api",
        &digest,
        "zip",
    )
    .unwrap();
    value["units"]["regional-session-api"]["location"] = serde_json::json!({
        "kind": "github-release",
        "uri": uri,
        "immutable": true
    });
    let manifest = serde_json::from_value::<CompositionManifest>(value.clone())
        .unwrap()
        .seal()
        .unwrap();
    manifest.validate(true).unwrap();

    value["units"]["regional-session-api"]["location"]["uri"] =
        serde_json::json!(uri.replace("run-123", "run-999"));
    let err = serde_json::from_value::<CompositionManifest>(value)
        .unwrap()
        .seal()
        .unwrap()
        .validate(true)
        .unwrap_err();
    assert!(err.rules().contains(&"manifest-github-release-location"));

    let mut value = valid_manifest();
    let entry = value["units"]
        .as_object_mut()
        .unwrap()
        .remove("regional-session-api")
        .unwrap();
    value["units"]["brain-mux"] = entry;
    value["units"]["brain-mux"]["kind"] = serde_json::json!("rust-oci-service");
    let digest = value["units"]["brain-mux"]["artifactDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    value["units"]["brain-mux"]["location"] = serde_json::json!({
        "kind": "oci",
        "uri": aex_release_tool::publication::ghcr_unit_uri("aexhq/aex", "brain-mux", &digest).unwrap(),
        "immutable": true
    });
    value["order"][0]["units"] = serde_json::json!(["brain-mux"]);
    serde_json::from_value::<CompositionManifest>(value.clone())
        .unwrap()
        .seal()
        .unwrap()
        .validate(true)
        .unwrap();
    value["units"]["brain-mux"]["location"]["uri"] = serde_json::json!(format!(
        "oci://ghcr.io/aexhq/aex-units/brain-mux:main@{digest}"
    ));
    let err = serde_json::from_value::<CompositionManifest>(value)
        .unwrap()
        .seal()
        .unwrap()
        .validate(true)
        .unwrap_err();
    assert!(err.rules().contains(&"manifest-oci-location"));
}
