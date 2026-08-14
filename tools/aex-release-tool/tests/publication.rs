//! Public release input packaging and acquisition adversarial tests.

mod common;

use std::fs;
use std::io::Read as _;

use aex_release_tool::artifact::ArtifactEnvelope;
use aex_release_tool::canon;
use aex_release_tool::graph::inputs::Units;
use aex_release_tool::manifest::{CompositionInputs, CompositionManifest};
use aex_release_tool::publication::{
    package_module_bundle_from_paths, regional_tables_bundle, verify_blob,
    verify_regional_tables_bundle,
};
use common::docs::{valid_envelope, valid_manifest};

fn handoff_registry() -> Units {
    toml::from_str(
        r#"
schema = "aex.units.v1"

[[unit]]
id = "billing-worker"
kind = "rust-lambda"
plane = "central"
package = "finance-ingest"
bin = "finance-ingest"
target = "aarch64-unknown-linux-gnu.2.34"
profile = "release-lambda"
form = "zip"
entrypoint = "bootstrap"
config_env_namespace = "AEX_BILLING_WORKER_"
config_schema_version = 1
required_receipts = ["unit"]
alarm_spec = "billing-worker"

[unit.lambda]
memory_mb = 1024
timeout_s = 30
reserved_concurrency = 8
"#,
    )
    .unwrap()
}

fn handoff_envelope() -> ArtifactEnvelope {
    let mut value = valid_envelope();
    value["unit"]["id"] = serde_json::json!("billing-worker");
    value["unit"]["plane"] = serde_json::json!("central");
    value["output"]["location"]["uri"] = serde_json::json!("lambda/billing-worker/deadbeef.zip");
    value["sbom"]["uri"] = serde_json::json!("sbom/billing-worker.json");
    value["identities"]["configEnvNamespace"] = serde_json::json!("AEX_BILLING_WORKER_");
    serde_json::from_value::<ArtifactEnvelope>(value)
        .unwrap()
        .seal()
        .unwrap()
}

fn handoff_inputs() -> CompositionInputs {
    let mut value = valid_manifest();
    let object = value.as_object_mut().unwrap();
    for field in ["schema", "releaseId", "units", "order", "annotations"] {
        object.remove(field);
    }
    serde_json::from_value(value).unwrap()
}

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
        "{{\"schema\":\"aex.regional-tables.v1\",\"generation\":1,\"digest\":\"{definitions_digest}\",\"tables\":[{{\"table\":\"sessions\"}}]}}\n"
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
    assert_eq!(identity.generation, 1);

    let acquired = root.path().join("acquired.json");
    fs::write(&acquired, &bytes).unwrap();
    verify_regional_tables_bundle(
        &acquired,
        &identity.digest,
        identity.size_bytes,
        &identity.definitions_digest,
        identity.generation,
    )
    .unwrap();
    let err = verify_regional_tables_bundle(
        &acquired,
        &identity.digest,
        identity.size_bytes,
        &format!("blake3:{}", "cd".repeat(32)),
        identity.generation,
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
fn composition_handoff_emits_the_exact_verified_envelope_store() {
    let envelope = handoff_envelope();
    let registry = handoff_registry();
    let store =
        aex_release_tool::manifest::verify_handoff_envelopes(&registry, vec![envelope.clone()])
            .unwrap();
    let manifest =
        aex_release_tool::manifest::new_handoff_manifest(handoff_inputs(), &store, &registry)
            .expect("a complete certified store must produce a strict manifest");

    assert_eq!(store.len(), 1);
    assert_eq!(
        store["billing-worker"].envelope_digest,
        envelope.envelope_digest
    );
    assert_eq!(
        manifest.units["billing-worker"].envelope_digest,
        envelope.envelope_digest
    );
    assert_eq!(
        manifest.units["billing-worker"].artifact_digest,
        envelope.output.digest
    );
    let manifest_shape = manifest.units["billing-worker"].lambda.unwrap();
    let registry_shape = registry.units[0].lambda.unwrap();
    assert_eq!(manifest_shape.memory_mb, registry_shape.memory_mb);
    assert_eq!(manifest_shape.timeout_s, registry_shape.timeout_s);
    assert_eq!(
        manifest_shape.reserved_concurrency,
        registry_shape.reserved_concurrency
    );
    let original_release_id = manifest.release_id.clone();
    let mut changed_shape = manifest;
    let changed_lambda = aex_release_tool::manifest::ManifestLambdaShape {
        memory_mb: 2_048,
        ..changed_shape.units["billing-worker"].lambda.unwrap()
    };
    changed_shape
        .units
        .get_mut("billing-worker")
        .unwrap()
        .lambda = Some(changed_lambda);
    let changed_shape = changed_shape.seal().unwrap();
    assert_ne!(changed_shape.release_id, original_release_id);
    changed_shape.validate(true).unwrap();
}

#[test]
fn manifest_validation_rejects_a_missing_or_wrong_kind_shape() {
    let mut missing = valid_manifest();
    missing["units"]["regional-otlp"]
        .as_object_mut()
        .unwrap()
        .remove("lambda");
    let missing = serde_json::from_value::<CompositionManifest>(missing)
        .unwrap()
        .seal()
        .unwrap();
    assert!(
        missing
            .validate(false)
            .unwrap_err()
            .rules()
            .contains(&"manifest-unit-shape-missing")
    );

    let mut conflict = valid_manifest();
    conflict["units"]["regional-otlp"]["fargate"] = serde_json::json!({
        "cpu": 256,
        "memoryMiB": 512,
        "desiredCount": 1,
        "stopTimeoutS": 30,
        "port": 8080
    });
    let conflict = serde_json::from_value::<CompositionManifest>(conflict)
        .unwrap()
        .seal()
        .unwrap();
    assert!(
        conflict
            .validate(false)
            .unwrap_err()
            .rules()
            .contains(&"manifest-unit-shape-conflict")
    );
}

#[test]
fn every_resource_shape_field_participates_in_the_release_id() {
    let sealed = |value: serde_json::Value| {
        serde_json::from_value::<CompositionManifest>(value)
            .unwrap()
            .seal()
            .unwrap()
    };

    let lambda = valid_manifest();
    let lambda_id = sealed(lambda.clone()).release_id;
    for (field, value) in [
        ("memoryMiB", serde_json::json!(2048)),
        ("timeoutS", serde_json::json!(31)),
        ("reservedConcurrency", serde_json::json!(9)),
    ] {
        let mut changed = lambda.clone();
        changed["units"]["regional-otlp"]["lambda"][field] = value;
        assert_ne!(sealed(changed).release_id, lambda_id, "Lambda `{field}`");
    }

    let mut fargate = valid_manifest();
    let unit = &mut fargate["units"]["regional-otlp"];
    unit["kind"] = serde_json::json!("rust-oci-service");
    unit.as_object_mut().unwrap().remove("lambda");
    unit["fargate"] = serde_json::json!({
        "cpu": 256,
        "memoryMiB": 512,
        "desiredCount": 1,
        "stopTimeoutS": 30,
        "port": 8080
    });
    let fargate_id = sealed(fargate.clone()).release_id;
    for (field, value) in [
        ("cpu", serde_json::json!(512)),
        ("memoryMiB", serde_json::json!(1024)),
        ("desiredCount", serde_json::json!(2)),
        ("stopTimeoutS", serde_json::json!(60)),
        ("port", serde_json::json!(9090)),
    ] {
        let mut changed = fargate.clone();
        changed["units"]["regional-otlp"]["fargate"][field] = value;
        assert_ne!(sealed(changed).release_id, fargate_id, "Fargate `{field}`");
    }

    let mut microvm = valid_manifest();
    let unit = &mut microvm["units"]["regional-otlp"];
    unit["kind"] = serde_json::json!("microvm-image");
    unit.as_object_mut().unwrap().remove("lambda");
    unit["microvm"] = serde_json::json!({
        "variant": "2gb",
        "minimumMemoryMiB": 2048,
        "browser": false
    });
    let microvm_id = sealed(microvm.clone()).release_id;
    for (field, value) in [
        ("variant", serde_json::json!("2gb-browser")),
        ("minimumMemoryMiB", serde_json::json!(4096)),
        ("browser", serde_json::json!(true)),
    ] {
        let mut changed = microvm.clone();
        changed["units"]["regional-otlp"]["microvm"][field] = value;
        assert_ne!(sealed(changed).release_id, microvm_id, "MicroVM `{field}`");
    }
}

#[test]
fn composition_handoff_reports_concrete_certification_and_receipt_gaps() {
    let mut envelope = handoff_envelope();
    envelope.receipts.clear();
    envelope.output.location.immutable = false;
    envelope = envelope.seal().unwrap();

    let error =
        aex_release_tool::manifest::verify_handoff_envelopes(&handoff_registry(), vec![envelope])
            .expect_err("a draft cannot enter the public handoff store");
    assert_eq!(error.exit.code(), 40);
    assert!(error.rules().contains(&"envelope-mutable-location"));
    assert!(error.rules().contains(&"envelope-no-receipts"));
    assert!(error.rules().contains(&"handoff-receipt-missing"));
    assert!(
        error
            .violations
            .iter()
            .all(|violation| violation.detail.contains("billing-worker"))
    );
}

#[test]
fn composition_handoff_rejects_duplicate_and_cross_run_envelopes() {
    let envelope = handoff_envelope();
    let error = aex_release_tool::manifest::verify_handoff_envelopes(
        &handoff_registry(),
        vec![envelope.clone(), envelope.clone()],
    )
    .expect_err("one unit cannot have two publication identities");
    assert_eq!(error.exit.code(), 32);
    assert!(error.rules().contains(&"handoff-envelope-duplicate"));

    let store =
        aex_release_tool::manifest::verify_handoff_envelopes(&handoff_registry(), vec![envelope])
            .unwrap();
    let mut inputs = handoff_inputs();
    inputs.source.workflow_run_id = "999".to_owned();
    let error =
        aex_release_tool::manifest::new_handoff_manifest(inputs, &store, &handoff_registry())
            .expect_err("composition inputs from another run must not cross-bind");
    assert_eq!(error.exit.code(), 32);
    assert!(error.rules().contains(&"handoff-source-mismatch"));
}

#[test]
fn handoff_cli_names_missing_composition_inputs_without_writing_outputs() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("release")).unwrap();
    fs::write(
        root.path().join("release/units.toml"),
        toml::to_string(&handoff_registry()).unwrap(),
    )
    .unwrap();
    let envelope = root.path().join("envelope.json");
    fs::write(
        &envelope,
        canon::to_file_bytes(&handoff_envelope()).unwrap(),
    )
    .unwrap();
    let manifest = root.path().join("composition-manifest.json");
    let store = root.path().join("artifact-store.json");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_aex-release-tool"))
        .arg("--root")
        .arg(root.path())
        .args(["manifest", "handoff", "--envelope"])
        .arg(&envelope)
        .arg("--composition")
        .arg(root.path().join("missing-composition-inputs.json"))
        .arg("--manifest-out")
        .arg(&manifest)
        .arg("--store-out")
        .arg(&store)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(40));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("[handoff-composition-inputs-missing]")
    );
    assert!(!manifest.exists());
    assert!(!store.exists());
}

#[test]
fn manifest_cross_binds_unit_blob_and_oci_locations_to_its_source() {
    let mut value = valid_manifest();
    let digest = value["units"]["regional-otlp"]["artifactDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    let uri = aex_release_tool::publication::github_release_unit_uri(
        "aexhq/aex",
        &"a".repeat(40),
        "123",
        1,
        "regional-otlp",
        &digest,
        "zip",
    )
    .unwrap();
    value["units"]["regional-otlp"]["location"] = serde_json::json!({
        "kind": "github-release",
        "uri": uri,
        "immutable": true
    });
    let manifest = serde_json::from_value::<CompositionManifest>(value.clone())
        .unwrap()
        .seal()
        .unwrap();
    manifest.validate(true).unwrap();

    value["units"]["regional-otlp"]["location"]["uri"] =
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
        .remove("regional-otlp")
        .unwrap();
    value["units"]["brain-mux"] = entry;
    value["units"]["brain-mux"]["kind"] = serde_json::json!("rust-oci-service");
    value["units"]["brain-mux"]
        .as_object_mut()
        .unwrap()
        .remove("lambda");
    value["units"]["brain-mux"]["fargate"] = serde_json::json!({
        "cpu": 2048,
        "memoryMiB": 4096,
        "desiredCount": 1,
        "stopTimeoutS": 120,
        "port": 8080
    });
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

// --- registry publication ------------------------------------------------------
//
// npm's identity is a name and a version, not a digest, so every test below
// exists to stop something that *looks* like a location from being accepted as
// one: a mutable tag, a redirect, a prerelease, or a name npm would not serve.

use aex_release_tool::publication::{
    exact_npm_version, npm_integrity, npm_tarball_identity, npm_tarball_uri, split_npm_package,
    valid_npm_integrity,
};

#[test]
fn a_published_version_is_located_by_its_canonical_registry_tarball() {
    assert_eq!(
        npm_tarball_uri("@aexhq/sdk", "0.50.0").unwrap(),
        "https://registry.npmjs.org/@aexhq/sdk/-/sdk-0.50.0.tgz",
        "the scope is dropped from the basename, exactly as the registry serves it"
    );
    assert_eq!(
        npm_tarball_uri("aex", "1.0.0").unwrap(),
        "https://registry.npmjs.org/aex/-/aex-1.0.0.tgz"
    );
}

#[test]
fn a_registry_location_round_trips_to_the_name_and_version_it_names() {
    let uri = npm_tarball_uri("@aexhq/sdk", "0.50.0").unwrap();
    let (package, version) = npm_tarball_identity(&uri).unwrap();
    assert_eq!(package, "@aexhq/sdk");
    assert_eq!(version, "0.50.0");
}

#[test]
fn a_location_that_is_not_exactly_the_registry_tarball_is_refused() {
    for uri in [
        // Another host serving a plausible path.
        "https://registry.example.invalid/@aexhq/sdk/-/sdk-0.50.0.tgz",
        // A tag rather than a version: npm resolves it differently over time.
        "https://registry.npmjs.org/@aexhq/sdk/-/sdk-latest.tgz",
        // The basename does not belong to the package it claims.
        "https://registry.npmjs.org/@aexhq/sdk/-/wire-0.50.0.tgz",
        // A query string is a different resource with the same prefix.
        "https://registry.npmjs.org/@aexhq/sdk/-/sdk-0.50.0.tgz?token=1",
        // The registry's metadata document, not its bytes.
        "https://registry.npmjs.org/@aexhq/sdk",
        // Prerelease and build metadata are not exact release versions.
        "https://registry.npmjs.org/@aexhq/sdk/-/sdk-0.50.0-rc.1.tgz",
    ] {
        assert!(
            npm_tarball_identity(uri).is_err(),
            "`{uri}` must not resolve to a published identity"
        );
    }
}

#[test]
fn only_a_publishable_lowercase_name_can_be_located() {
    assert!(split_npm_package("@aexhq/sdk").is_ok());
    assert!(split_npm_package("aex").is_ok());
    for name in [
        "",
        "@aexhq",     // a scope with no package
        "@AEXHQ/sdk", // npm refuses uppercase for new packages
        "@aexhq/SDK",
        "@aexhq/sdk/extra", // a path, not a name
        "../etc/passwd",
        "@aexhq/../sdk",
    ] {
        assert!(
            split_npm_package(name).is_err(),
            "`{name}` is not a publishable npm package name"
        );
    }
}

#[test]
fn the_registry_integrity_is_sha512_over_the_same_bytes_the_envelope_digests() {
    let bytes = b"a packed tarball";
    let integrity = npm_integrity(bytes);
    assert!(valid_npm_integrity(&integrity));
    assert!(integrity.starts_with("sha512-"));
    assert_ne!(
        integrity,
        canon::digest_bytes(bytes),
        "the registry identity and the envelope identity are different functions"
    );
    assert!(!valid_npm_integrity("sha512-not-base64!"));
    assert!(
        !valid_npm_integrity(&format!("sha256-{}", &integrity[7..])),
        "npm's integrity is SHA-512; a SHA-256 SRI is a different claim"
    );
}

#[test]
fn only_exact_release_versions_are_published_identities() {
    assert!(exact_npm_version("0.50.0"));
    assert!(exact_npm_version("10.0.1"));
    for version in ["0.50", "0.50.0.1", "v0.50.0", "0.50.0-rc.1", "01.0.0", ""] {
        assert!(
            !exact_npm_version(version),
            "`{version}` is not an exact published version"
        );
    }
}

#[test]
fn a_published_package_envelope_requires_a_registry_location_and_the_packer_form() {
    let mut value = valid_envelope();
    value["unit"]["kind"] = serde_json::json!("npm-package");
    value["unit"]["id"] = serde_json::json!("sdk");
    value["unit"]["plane"] = serde_json::json!("public");
    value["media"]["form"] = serde_json::json!("npm-tarball");
    value["media"]["mediaType"] = serde_json::json!("application/gzip");
    value["output"]["target"] =
        serde_json::json!({ "os": "none", "architecture": "none", "triple": "none" });
    value["output"]["location"] = serde_json::json!({
        "kind": "npm",
        "uri": npm_tarball_uri("@aexhq/sdk", "0.50.0").unwrap(),
        "immutable": true
    });
    // The registry is the signer, so a published package is never unsigned:
    // `artifact-policy.toml` gives this kind `signature = "npm-provenance"`.
    value["signature"] = serde_json::json!({
        "present": true,
        "kind": "npm-provenance",
        "keyId": "https://github.com/aexhq/aex/.github/workflows/_build-artifacts.yml@refs/heads/main"
    });
    let sealed = |value: serde_json::Value| {
        serde_json::from_value::<ArtifactEnvelope>(value)
            .expect("fixture envelope")
            .seal()
            .expect("sealed")
    };
    sealed(value.clone())
        .verify(None, true)
        .expect("a registry-located npm package envelope verifies");

    // A published package that claims a release asset instead.
    let mut blob = value.clone();
    blob["output"]["location"] = serde_json::json!({
        "kind": "github-release",
        "uri": "https://github.com/aexhq/aex/releases/download/main-0000000000000000000000000000000000000000-run-1-attempt-1/unit-sdk-0000000000000000000000000000000000000000000000000000000000000000.tar.gz",
        "immutable": true
    });
    let error = sealed(blob).verify(None, true).unwrap_err();
    assert!(error.rules().contains(&"envelope-location-kind"));

    // A deployed unit that claims a registry location.
    let mut hosted = value.clone();
    hosted["unit"]["kind"] = serde_json::json!("rust-lambda");
    let error = sealed(hosted).verify(None, true).unwrap_err();
    assert!(error.rules().contains(&"envelope-location-kind"));

    // The right location, the wrong media form.
    let mut form = value.clone();
    form["media"]["form"] = serde_json::json!("tar.gz");
    let error = sealed(form).verify(None, true).unwrap_err();
    assert!(error.rules().contains(&"envelope-media-form"));

    // A dist-tag rather than a version. `latest` moves, so an envelope located
    // by one would describe whichever bytes the registry serves today.
    let mut tagged = value.clone();
    tagged["output"]["location"]["uri"] =
        serde_json::json!("https://registry.npmjs.org/@aexhq/sdk/-/sdk-latest.tgz");
    let error = sealed(tagged).verify(None, true).unwrap_err();
    assert!(error.rules().contains(&"publication-npm-version"));

    // Another host serving the same path shape.
    let mut elsewhere = value;
    elsewhere["output"]["location"]["uri"] =
        serde_json::json!("https://registry.example.invalid/@aexhq/sdk/-/sdk-0.50.0.tgz");
    let error = sealed(elsewhere).verify(None, true).unwrap_err();
    assert!(error.rules().contains(&"publication-npm-location"));
}

// --- registry readback ---------------------------------------------------------
//
// Publication is not believed because `npm publish` exited zero. These tests
// pin the four ways the registry can disagree with what this run packed, and
// the one way provenance can be absent.

mod npm_readback {
    use aex_release_tool::graph::inputs::Unit;
    use aex_release_tool::npm::{Readback, validate_publication, verify_readback};
    use aex_release_tool::oci::OciWorkflowRun;

    const TARBALL: &[u8] = b"the exact bytes handed to the registry";

    fn unit() -> Unit {
        toml::from_str(
            r#"
id = "sdk"
kind = "npm-package"
plane = "public"
package = "@aexhq/sdk"
target = "none"
profile = "release"
form = "npm-tarball"
config_schema_version = 1
required_receipts = ["unit"]
"#,
        )
        .expect("registry row")
    }

    fn workflow() -> OciWorkflowRun {
        OciWorkflowRun {
            repository: "aexhq/aex".to_owned(),
            r#ref: "refs/heads/main".to_owned(),
            path: ".github/workflows/_build-artifacts.yml".to_owned(),
            run_id: "42".to_owned(),
            run_attempt: 3,
            job_name: "sdk".to_owned(),
            builder_id:
                "https://github.com/aexhq/aex/.github/workflows/_build-artifacts.yml@refs/heads/main"
                    .to_owned(),
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        tarball: std::path::PathBuf,
        manifest: std::path::PathBuf,
        metadata: std::path::PathBuf,
    }

    impl Fixture {
        fn new(metadata: &serde_json::Value) -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let tarball = temp.path().join("artifact.bin");
            let manifest = temp.path().join("package.json");
            let metadata_path = temp.path().join("registry-metadata.json");
            std::fs::write(&tarball, TARBALL).expect("tarball");
            std::fs::write(
                &manifest,
                serde_json::json!({ "name": "@aexhq/sdk", "version": "0.50.0" }).to_string(),
            )
            .expect("manifest");
            std::fs::write(&metadata_path, metadata.to_string()).expect("metadata");
            Self {
                _temp: temp,
                tarball,
                manifest,
                metadata: metadata_path,
            }
        }

        fn readback(&self) -> Readback<'_> {
            Readback {
                tarball: &self.tarball,
                manifest: &self.manifest,
                registry_metadata: &self.metadata,
            }
        }
    }

    fn served(mutate: impl FnOnce(&mut serde_json::Value)) -> serde_json::Value {
        let mut value = serde_json::json!({
            "name": "@aexhq/sdk",
            "version": "0.50.0",
            "dist": {
                "integrity": aex_release_tool::publication::npm_integrity(TARBALL),
                "tarball": aex_release_tool::publication::npm_tarball_uri("@aexhq/sdk", "0.50.0")
                    .expect("canonical location"),
                "attestations": { "provenance": { "predicateType": "https://slsa.dev/provenance/v1" } }
            }
        });
        mutate(&mut value);
        value
    }

    #[test]
    fn a_registry_serving_this_runs_bytes_with_provenance_is_a_publication() {
        let fixture = Fixture::new(&served(|_| {}));
        let publication =
            verify_readback(&unit(), fixture.readback(), workflow()).expect("verified publication");
        assert_eq!(publication.package, "@aexhq/sdk");
        assert_eq!(publication.version, "0.50.0");
        assert_eq!(
            publication.tarball_digest,
            aex_release_tool::canon::digest_bytes(TARBALL),
            "the envelope's SHA-256 identity is kept alongside the registry's SHA-512 one"
        );
        assert!(publication.integrity.starts_with("sha512-"));
        assert!(publication.provenance);
        assert!(publication.location.immutable);
        validate_publication(&publication).expect("its own document re-validates");
    }

    #[test]
    fn a_registry_serving_different_bytes_is_refused() {
        let fixture = Fixture::new(&served(|value| {
            value["dist"]["integrity"] =
                serde_json::json!(aex_release_tool::publication::npm_integrity(b"other bytes"));
        }));
        let error = verify_readback(&unit(), fixture.readback(), workflow())
            .expect_err("integrity differs");
        assert_eq!(error.rules(), vec!["npm-readback-integrity"]);
    }

    #[test]
    fn a_registry_serving_a_different_version_is_refused() {
        let fixture = Fixture::new(&served(|value| {
            value["version"] = serde_json::json!("0.43.0");
        }));
        let error =
            verify_readback(&unit(), fixture.readback(), workflow()).expect_err("version differs");
        assert!(error.rules().contains(&"npm-readback-package"));
    }

    #[test]
    fn a_registry_serving_the_tarball_from_somewhere_else_is_refused() {
        let fixture = Fixture::new(&served(|value| {
            value["dist"]["tarball"] =
                serde_json::json!("https://mirror.example.invalid/@aexhq/sdk/-/sdk-0.50.0.tgz");
        }));
        let error = verify_readback(&unit(), fixture.readback(), workflow())
            .expect_err("only the canonical registry location is an identity");
        assert_eq!(error.rules(), vec!["npm-readback-location"]);
    }

    #[test]
    fn a_version_published_without_provenance_is_refused() {
        let fixture = Fixture::new(&served(|value| {
            value["dist"]["attestations"] = serde_json::Value::Null;
        }));
        let error = verify_readback(&unit(), fixture.readback(), workflow())
            .expect_err("a token publish leaves no attestation");
        assert_eq!(error.rules(), vec!["npm-readback-provenance"]);
        assert_eq!(error.exit.code(), 22);
    }

    #[test]
    fn publication_must_come_from_the_protected_reusable_workflow() {
        let fixture = Fixture::new(&served(|_| {}));
        let mut elsewhere = workflow();
        elsewhere.path = ".github/workflows/attacker.yml".to_owned();
        let error = verify_readback(&unit(), fixture.readback(), elsewhere)
            .expect_err("another workflow cannot mint a publication");
        assert_eq!(error.rules(), vec!["npm-readback-workflow"]);
    }

    #[test]
    fn a_unit_that_publishes_to_no_registry_has_no_readback() {
        let fixture = Fixture::new(&served(|_| {}));
        let mut hosted = unit();
        hosted.kind = "rust-lambda".to_owned();
        let error = verify_readback(&hosted, fixture.readback(), workflow())
            .expect_err("a deployed unit is not published to npm");
        assert_eq!(error.rules(), vec!["npm-readback-kind"]);
    }
}
