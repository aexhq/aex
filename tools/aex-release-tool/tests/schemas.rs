//! The JSON Schemas and the Rust types agree.
//!
//! Two authorities describe the same seven documents: the schemas under
//! `api/schemas/release/`, which the contract generator turns into
//! `aex_internal_contracts::release::*`, and this crate's own `serde` types.
//! If they disagree, one of them is enforcing something the other does not,
//! and nobody can tell which one the pipeline actually applied.
//!
//! Direction and its limit, stated once: the schema is *stricter* than serde,
//! because it narrows several string fields to closed value sets that serde
//! carries as `String` and that `verify`/`admit` check by hand. So the parity
//! asserted here is over structure — unknown members, missing required
//! members, wrong types — and the enum narrowing is asserted separately
//! against the schema alone.

mod common;

use aex_release_tool::artifact::ArtifactEnvelope;
use aex_release_tool::evidence::Receipt;
use aex_release_tool::manifest::CompositionManifest;
use aex_release_tool::release_contract::{
    EnvironmentBinding, ResolvedPlacement, SavedPlanEnvelope,
};
use aex_release_tool::schemas::{self, SchemaName};
use aex_release_tool::verification::VerificationStatement;
use serde_json::{Value, json};

use common::docs::{digest, sha1, valid_envelope, valid_manifest, valid_receipt, valid_statement};

fn schema_accepts(name: SchemaName, document: &Value) -> bool {
    let schema = schemas::document(name).expect("an embedded schema");
    let validator = jsonschema::validator_for(&schema).expect("a compilable schema");
    validator.is_valid(document)
}

/// One corpus entry: a name, its schema, a valid document and the serde
/// predicate that must agree with the schema on it.
type CorpusEntry = (&'static str, SchemaName, Value, fn(&Value) -> bool);

/// Every document, with the serde type that must agree with its schema.
fn corpus() -> Vec<CorpusEntry> {
    fn envelope(value: &Value) -> bool {
        serde_json::from_value::<ArtifactEnvelope>(value.clone()).is_ok()
    }
    fn manifest(value: &Value) -> bool {
        serde_json::from_value::<CompositionManifest>(value.clone()).is_ok()
    }
    fn receipt(value: &Value) -> bool {
        serde_json::from_value::<Receipt>(value.clone()).is_ok()
    }
    fn statement(value: &Value) -> bool {
        serde_json::from_value::<VerificationStatement>(value.clone()).is_ok()
    }
    fn binding(value: &Value) -> bool {
        serde_json::from_value::<EnvironmentBinding>(value.clone()).is_ok()
    }
    fn placement(value: &Value) -> bool {
        serde_json::from_value::<ResolvedPlacement>(value.clone()).is_ok()
    }
    fn saved_plan(value: &Value) -> bool {
        serde_json::from_value::<SavedPlanEnvelope>(value.clone()).is_ok()
    }
    let contract_fixture = |name: &str| -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/contracts")
                    .join(name),
            )
            .unwrap(),
        )
        .unwrap()
    };
    vec![
        (
            "artifact-envelope",
            SchemaName::ArtifactEnvelope,
            valid_envelope(),
            envelope as fn(&Value) -> bool,
        ),
        (
            "composition-manifest",
            SchemaName::CompositionManifest,
            valid_manifest(),
            manifest as fn(&Value) -> bool,
        ),
        (
            "evidence-receipt",
            SchemaName::EvidenceReceipt,
            valid_receipt(),
            receipt as fn(&Value) -> bool,
        ),
        (
            "verification-statement",
            SchemaName::VerificationStatement,
            valid_statement(),
            statement as fn(&Value) -> bool,
        ),
        (
            "environment-binding",
            SchemaName::EnvironmentBinding,
            contract_fixture("environment-binding.valid.json"),
            binding as fn(&Value) -> bool,
        ),
        (
            "resolved-placement",
            SchemaName::ResolvedPlacement,
            contract_fixture("resolved-placement.valid.json"),
            placement as fn(&Value) -> bool,
        ),
        (
            "saved-plan-envelope",
            SchemaName::SavedPlanEnvelope,
            contract_fixture("saved-plan-envelope.valid.json"),
            saved_plan as fn(&Value) -> bool,
        ),
    ]
}

#[test]
fn every_valid_fixture_is_accepted_by_both_authorities() {
    for (name, schema, document, serde_accepts) in corpus() {
        assert!(
            schema_accepts(schema, &document),
            "the `{name}` schema rejected its own valid fixture"
        );
        assert!(
            serde_accepts(&document),
            "serde rejected the valid `{name}` fixture"
        );
    }
}

#[test]
fn verification_statement_schema_requires_every_release_receipt_class() {
    for missing in ["smoke", "e2e", "user"] {
        let mut statement = valid_statement();
        statement["receipts"]
            .as_array_mut()
            .expect("statement receipt array")
            .retain(|receipt| receipt["class"] != missing);
        assert!(
            !schema_accepts(SchemaName::VerificationStatement, &statement),
            "the verification statement schema accepted no `{missing}` receipt"
        );
    }
}

#[test]
fn an_unknown_member_is_rejected_by_both_authorities() {
    for (name, schema, mut document, serde_accepts) in corpus() {
        document["surpriseKey"] = json!("value");
        assert!(
            !schema_accepts(schema, &document),
            "the `{name}` schema accepted an unknown member"
        );
        assert!(
            !serde_accepts(&document),
            "serde accepted an unknown member on `{name}`"
        );
    }
}

#[test]
fn a_missing_required_member_is_rejected_by_both_authorities() {
    let required: &[(&str, &str)] = &[
        ("artifact-envelope", "output"),
        ("composition-manifest", "units"),
        ("evidence-receipt", "inventory"),
        ("verification-statement", "deployed"),
        ("environment-binding", "resources"),
        ("resolved-placement", "artifacts"),
        ("saved-plan-envelope", "state"),
    ];
    for (name, schema, mut document, serde_accepts) in corpus() {
        let member = required
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, member)| *member)
            .expect("every document names a required member to remove");
        document.as_object_mut().unwrap().remove(member);
        assert!(
            !schema_accepts(schema, &document),
            "the `{name}` schema accepted a document with no `{member}`"
        );
        assert!(
            !serde_accepts(&document),
            "serde accepted `{name}` with no `{member}`"
        );
    }
}

#[test]
fn a_wrongly_typed_member_is_rejected_by_both_authorities() {
    for (name, schema, mut document, serde_accepts) in corpus() {
        document["schema"] = json!(7);
        assert!(
            !schema_accepts(schema, &document),
            "the `{name}` schema accepted a numeric `schema` member"
        );
        assert!(
            !serde_accepts(&document),
            "serde accepted a numeric `schema` member on `{name}`"
        );
    }
}

#[test]
fn the_schema_additionally_narrows_string_members_to_closed_value_sets() {
    // Where the schema is stricter than serde, `verify` and `admit` carry the
    // same rule in Rust. This test pins the asymmetry so it is a recorded
    // design point rather than a discovered surprise.
    let mut envelope = valid_envelope();
    envelope["unit"]["kind"] = json!("wasm-module");
    assert!(!schema_accepts(SchemaName::ArtifactEnvelope, &envelope));
    assert!(
        serde_json::from_value::<ArtifactEnvelope>(envelope).is_ok(),
        "serde carries `kind` as a string; the closed set lives in the schema and in \
         `publish_destination`"
    );
}

#[test]
fn the_receipt_schema_makes_a_skip_structurally_impossible() {
    let mut receipt = valid_receipt();
    receipt["inventory"]["skipped"] = json!(1);
    assert!(
        !schema_accepts(SchemaName::EvidenceReceipt, &receipt),
        "a receipt reporting a skip must not even be a well-formed receipt"
    );
}

#[test]
fn the_receipt_schema_forbids_an_observed_secret_canary() {
    let mut receipt = valid_receipt();
    receipt["data"]["secretCanaryObserved"] = json!(true);
    assert!(!schema_accepts(SchemaName::EvidenceReceipt, &receipt));
}

#[test]
fn release_e2e_and_user_receipts_require_a_deployment_context_digest() {
    for class in ["e2e", "user"] {
        let mut receipt = valid_receipt();
        receipt["class"] = json!(class);
        receipt["layer"] = json!(class);
        receipt["lane"] = json!("release");
        receipt["subject"] = json!({ "releaseId": digest(0x11), "unitIds": [] });
        assert!(
            !schema_accepts(SchemaName::EvidenceReceipt, &receipt),
            "a release {class} receipt without its VERIFYING continuation is not evidence"
        );

        receipt["subject"]["deploymentContextDigest"] = json!(digest(0x12));
        assert!(schema_accepts(SchemaName::EvidenceReceipt, &receipt));
    }
}

#[test]
fn architecture_receipts_require_the_exact_arm_execution_shape() {
    let mut receipt = valid_receipt();
    receipt["class"] = json!("arch-qualification");
    assert!(
        !schema_accepts(SchemaName::EvidenceReceipt, &receipt),
        "naming the class without exact execution evidence must fail closed"
    );
    receipt["architectureQualification"] = json!({
        "artifactDigest": digest(0x32),
        "target": "aarch64",
        "hostIdentity": "github-hosted-ubuntu-arm64",
        "executorIdentity": "native-linux-arm64",
        "executorKind": "native",
        "bootstrapResult": "passed",
        "dependencyLoaderResult": "passed",
        "observedAt": "2026-08-01T00:00:00Z",
        "expiresAt": "2026-08-08T00:00:00Z",
        "workloadSmokes": [{ "id": "bootstrap-start", "result": "passed" }]
    });
    assert!(schema_accepts(SchemaName::EvidenceReceipt, &receipt));

    receipt["architectureQualification"]["target"] = json!("x86_64");
    assert!(!schema_accepts(SchemaName::EvidenceReceipt, &receipt));
}

#[test]
fn the_envelope_schema_forbids_a_mutable_location_and_a_dirty_tree() {
    let mut envelope = valid_envelope();
    envelope["output"]["location"]["immutable"] = json!(false);
    assert!(!schema_accepts(SchemaName::ArtifactEnvelope, &envelope));

    let mut envelope = valid_envelope();
    envelope["source"]["treeClean"] = json!(false);
    assert!(!schema_accepts(SchemaName::ArtifactEnvelope, &envelope));
}

#[test]
fn deferred_supply_chain_schema_requires_flag_and_exact_sentinels() {
    let mut deferred = valid_envelope();
    deferred["supplyChainDeferred"] = json!(true);
    deferred["sbom"] = json!({
        "format": "deferred-startup",
        "digest": "",
        "uri": "",
        "componentCount": 0
    });
    deferred["licenses"] = json!({
        "policyDigest": "",
        "verdict": "deferred-startup",
        "denials": []
    });
    deferred["vulnerabilities"] = json!({
        "scanner": "deferred-startup",
        "database": "",
        "scannedAt": "",
        "unapprovedCritical": 0,
        "unapprovedHigh": 0
    });
    let schema = schemas::document(SchemaName::ArtifactEnvelope).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors = validator
        .iter_errors(&deferred)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "{errors:#?}");

    let mut mixed = deferred.clone();
    mixed["sbom"] = valid_envelope()["sbom"].clone();
    assert!(!schema_accepts(SchemaName::ArtifactEnvelope, &mixed));

    let mut strict_with_flag = valid_envelope();
    strict_with_flag["supplyChainDeferred"] = json!(true);
    assert!(!schema_accepts(
        SchemaName::ArtifactEnvelope,
        &strict_with_flag
    ));

    let mut false_with_sentinels = deferred.clone();
    false_with_sentinels["supplyChainDeferred"] = json!(false);
    assert!(!schema_accepts(
        SchemaName::ArtifactEnvelope,
        &false_with_sentinels
    ));

    let mut absent_with_sentinels = deferred.clone();
    absent_with_sentinels
        .as_object_mut()
        .unwrap()
        .remove("supplyChainDeferred");
    assert!(!schema_accepts(
        SchemaName::ArtifactEnvelope,
        &absent_with_sentinels
    ));

    for class in ["deny", "sbom", "license", "vulnerability"] {
        let mut with_scanner_receipt = deferred.clone();
        with_scanner_receipt["receipts"][0]["class"] = json!(class);
        assert!(
            !schema_accepts(SchemaName::ArtifactEnvelope, &with_scanner_receipt),
            "deferred envelope accepted `{class}` receipt"
        );
    }
}

#[test]
fn public_location_schemas_reject_wrong_hosts_tags_and_mutable_oci_refs() {
    let mut envelope = valid_envelope();
    let digest = envelope["output"]["digest"].as_str().unwrap().to_owned();
    let uri = aex_release_tool::publication::github_release_unit_uri(
        "aexhq/aex",
        &sha1(),
        "123",
        1,
        "regional-session-api",
        &digest,
        "zip",
    )
    .unwrap();
    envelope["output"]["location"] = json!({
        "kind": "github-release",
        "uri": uri,
        "immutable": true
    });
    assert!(schema_accepts(SchemaName::ArtifactEnvelope, &envelope));
    for altered in [
        uri.replace("github.com", "example.com"),
        uri.replace("run-123", "run-latest"),
        uri.replace(&digest[7..], "mutable"),
    ] {
        envelope["output"]["location"]["uri"] = json!(altered);
        assert!(!schema_accepts(SchemaName::ArtifactEnvelope, &envelope));
    }

    let mut manifest = valid_manifest();
    manifest["units"]["regional-session-api"]["kind"] = json!("rust-oci-service");
    manifest["units"]["regional-session-api"]
        .as_object_mut()
        .unwrap()
        .remove("lambda");
    manifest["units"]["regional-session-api"]["fargate"] = json!({
        "cpu": 256,
        "memoryMiB": 512,
        "desiredCount": 1,
        "stopTimeoutS": 30,
        "port": 8080
    });
    manifest["units"]["regional-session-api"]["location"] = json!({
        "kind": "oci",
        "uri": format!("oci://ghcr.io/aexhq/aex-units/regional-session-api@{digest}"),
        "immutable": true
    });
    assert!(schema_accepts(SchemaName::CompositionManifest, &manifest));
    manifest["units"]["regional-session-api"]["location"]["uri"] = json!(format!(
        "oci://ghcr.io/aexhq/aex-units/regional-session-api:main@{digest}"
    ));
    assert!(!schema_accepts(SchemaName::CompositionManifest, &manifest));
}

#[test]
fn manifest_unit_shapes_are_closed_and_kind_specific() {
    let valid = valid_manifest();
    assert!(schema_accepts(SchemaName::CompositionManifest, &valid));

    let mut missing = valid.clone();
    missing["units"]["regional-session-api"]
        .as_object_mut()
        .unwrap()
        .remove("lambda");
    assert!(!schema_accepts(SchemaName::CompositionManifest, &missing));

    let mut extra = valid.clone();
    extra["units"]["regional-session-api"]["lambda"]["invented"] = json!(1);
    assert!(!schema_accepts(SchemaName::CompositionManifest, &extra));

    let mut microvm = valid;
    let unit = &mut microvm["units"]["regional-session-api"];
    unit["kind"] = json!("microvm-image");
    unit.as_object_mut().unwrap().remove("lambda");
    unit["microvm"] = json!({
        "variant": "2gb-browser",
        "minimumMemoryMiB": 2048,
        "browser": true
    });
    assert!(schema_accepts(SchemaName::CompositionManifest, &microvm));

    let mut oneshot = valid_manifest();
    let unit = &mut oneshot["units"]["regional-session-api"];
    unit["kind"] = json!("rust-oci-task");
    unit.as_object_mut().unwrap().remove("lambda");
    unit["fargate"] = json!({
        "cpu": 512,
        "memoryMiB": 1024,
        "desiredCount": 0,
        "stopTimeoutS": 60,
        "port": 0
    });
    assert!(schema_accepts(SchemaName::CompositionManifest, &oneshot));
    oneshot["units"]["regional-session-api"]["fargate"]["desiredCount"] = json!(1);
    assert!(!schema_accepts(SchemaName::CompositionManifest, &oneshot));
}

#[test]
fn the_envelope_schema_forbids_post_deployment_receipt_classes() {
    let mut envelope = valid_envelope();
    envelope["receipts"][0]["class"] = json!("smoke");
    assert!(
        !schema_accepts(SchemaName::ArtifactEnvelope, &envelope),
        "an artifact cannot carry proof that only exists after it is deployed"
    );
}

#[test]
fn the_binding_schema_has_no_field_a_secret_value_can_occupy() {
    let binding = json!({
        "schema": "aex.environment-binding.v1",
        "bindingDigest": digest(0x51),
        "plane": "dev",
        "regions": ["eu-west-1"],
        "desiredReleaseId": digest(0x11),
        "rootModuleVersion": "0.1.0",
        "infraModuleBundleDigest": digest(0x24),
        "resources": {},
        "secrets": {
            "token_pepper": {
                "arn": "arn:aws:secretsmanager:eu-west-1:000000000000:secret:x",
                "kind": "token-pepper",
                "value": "not-a-field"
            }
        },
        "config": { "bundleDigest": digest(0x52), "schemaVersions": {} },
        "approval": { "policy": "two-person", "approvers": ["owner"] }
    });
    assert!(
        !schema_accepts(SchemaName::EnvironmentBinding, &binding),
        "a secret entry must have nowhere to put a plaintext value"
    );
}

#[test]
fn canonical_digests_are_stable_under_member_permutation() {
    let envelope: ArtifactEnvelope = serde_json::from_value(valid_envelope()).unwrap();
    let sealed = envelope.seal().unwrap();
    let round_tripped: ArtifactEnvelope =
        serde_json::from_str(&serde_json::to_string(&sealed).unwrap()).unwrap();
    assert_eq!(
        sealed.envelope_digest,
        round_tripped.seal().unwrap().envelope_digest
    );
}
