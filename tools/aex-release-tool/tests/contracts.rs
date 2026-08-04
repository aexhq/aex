//! Adversarial tests for the public release contracts consumed by private CI.

use std::path::Path;

use aex_release_tool::canon;
use aex_release_tool::error::Exit;
use aex_release_tool::release_contract::{
    StateIdentity, parse_environment_binding, parse_resolved_placement, parse_saved_plan_envelope,
};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/contracts")
            .join(name),
    )
    .unwrap()
}

fn with_self_digest(text: &str, field: &str) -> String {
    let mut value: Value = serde_json::from_str(text).unwrap();
    let digest = canon::digest_document_excluding(&value, &[field]).unwrap();
    value[field] = Value::String(digest);
    canon::to_string(&value).unwrap()
}

fn saved_plan_with_payload(payload: &[u8]) -> Value {
    let mut value: Value =
        serde_json::from_str(&fixture("saved-plan-envelope.valid.json")).unwrap();
    value["plan"]["digest"] = Value::String(canon::digest_bytes(payload));
    value["plan"]["sizeBytes"] = Value::from(payload.len() as u64);
    value
}

#[test]
fn environment_binding_accepts_references_and_rejects_tampering() {
    let valid = with_self_digest(&fixture("environment-binding.valid.json"), "bindingDigest");
    let binding = parse_environment_binding(&valid).unwrap();
    assert_eq!(binding.plane, "dev");
    assert_eq!(binding.secrets.len(), 1);
    assert!(
        serde_json::to_value(&binding)
            .unwrap()
            .get("bindingRef")
            .is_none(),
        "the sealed binding must not claim custody of its containing Git commit"
    );

    let mut self_referential: Value = serde_json::from_str(&valid).unwrap();
    self_referential["bindingRef"] =
        Value::String("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned());
    let self_referential = with_self_digest(
        &canon::to_string(&self_referential).unwrap(),
        "bindingDigest",
    );
    assert!(
        parse_environment_binding(&self_referential)
            .unwrap_err()
            .rules()
            .contains(&"environment-binding-invalid"),
        "external source custody must not be sealed into the binding document"
    );

    let mut tampered: Value = serde_json::from_str(&valid).unwrap();
    tampered["plane"] = Value::String("prd".to_owned());
    let error = parse_environment_binding(&canon::to_string(&tampered).unwrap()).unwrap_err();
    assert_eq!(error.exit, Exit::ManifestInvalid);
    assert_eq!(error.rules(), vec!["binding-digest-mismatch"]);

    let mut rotated_stage: Value = serde_json::from_str(&valid).unwrap();
    rotated_stage["secrets"]["database_credential"]["versionStage"] =
        Value::String("AWSPREVIOUS".to_owned());
    let error = parse_environment_binding(&canon::to_string(&rotated_stage).unwrap()).unwrap_err();
    assert_eq!(error.rules(), vec!["binding-digest-mismatch"]);
}

#[test]
fn environment_binding_rejects_plaintext_secret_shapes_before_deserializing() {
    let error = parse_environment_binding(&fixture("environment-binding.secret.json")).unwrap_err();
    assert_eq!(error.exit, Exit::ManifestInvalid);
    assert!(error.rules().contains(&"release-contract-plaintext-secret"));
}

#[test]
fn environment_binding_requires_immutable_secrets_sorted_regions_and_safe_urls() {
    let original: Value = serde_json::from_str(&fixture("environment-binding.valid.json")).unwrap();

    let mut no_version = original.clone();
    no_version["secrets"]["database_credential"]
        .as_object_mut()
        .unwrap()
        .remove("versionId");
    let no_version = with_self_digest(&canon::to_string(&no_version).unwrap(), "bindingDigest");
    assert!(
        parse_environment_binding(&no_version)
            .unwrap_err()
            .rules()
            .contains(&"environment-binding-invalid")
    );

    let mut unsorted = original.clone();
    unsorted["regions"] = serde_json::json!(["us-east-1", "eu-west-1"]);
    let unsorted = with_self_digest(&canon::to_string(&unsorted).unwrap(), "bindingDigest");
    assert!(
        parse_environment_binding(&unsorted)
            .unwrap_err()
            .rules()
            .contains(&"binding-regions-not-sorted")
    );

    let mut unsafe_url = original;
    unsafe_url["resources"]["artifact_store"]
        .as_object_mut()
        .unwrap()
        .remove("arn");
    unsafe_url["resources"]["artifact_store"]["url"] = Value::String(
        "https://operator:credential@example.invalid/resource?token=value".to_owned(),
    );
    let unsafe_url = with_self_digest(&canon::to_string(&unsafe_url).unwrap(), "bindingDigest");
    assert!(
        parse_environment_binding(&unsafe_url)
            .unwrap_err()
            .rules()
            .contains(&"release-contract-plaintext-secret")
    );

    let mut mutable_url: Value =
        serde_json::from_str(&fixture("environment-binding.valid.json")).unwrap();
    mutable_url["resources"]["artifact_store"]
        .as_object_mut()
        .unwrap()
        .remove("arn");
    mutable_url["resources"]["artifact_store"]["url"] =
        Value::String("https://example.invalid/resource?revision=1".to_owned());
    let mutable_url = with_self_digest(&canon::to_string(&mutable_url).unwrap(), "bindingDigest");
    assert!(
        parse_environment_binding(&mutable_url)
            .unwrap_err()
            .rules()
            .contains(&"resource-url-invalid")
    );
}

#[test]
fn resolved_placement_is_content_addressed_and_closed() {
    let valid = with_self_digest(&fixture("resolved-placement.valid.json"), "placementDigest");
    let placement = parse_resolved_placement(&valid).unwrap();
    assert_eq!(placement.artifacts.len(), 2);

    let mut tampered: Value = serde_json::from_str(&valid).unwrap();
    tampered["artifacts"]["brain-mux"]["sizeBytes"] = Value::from(8_u64);
    let error = parse_resolved_placement(&canon::to_string(&tampered).unwrap()).unwrap_err();
    assert_eq!(error.rules(), vec!["placement-digest-mismatch"]);
}

#[test]
fn saved_plan_envelope_binds_exact_bytes_and_rejects_expiry() {
    let payload = b"opaque saved terraform plan";
    let mut value: Value =
        serde_json::from_str(&fixture("saved-plan-envelope.valid.json")).unwrap();
    value["plan"]["digest"] = Value::String(canon::digest_bytes(payload));
    value["plan"]["sizeBytes"] = Value::from(payload.len() as u64);
    let text = with_self_digest(&canon::to_string(&value).unwrap(), "envelopeDigest");

    let before_expiry = OffsetDateTime::parse("2026-08-03T12:15:00Z", &Rfc3339).unwrap();
    let envelope = parse_saved_plan_envelope(&text, before_expiry, Some(payload)).unwrap();
    assert!(matches!(
        envelope.state,
        StateIdentity::Present { serial: 41, .. }
    ));

    let stale = OffsetDateTime::parse("2026-08-03T12:31:00Z", &Rfc3339).unwrap();
    let error = parse_saved_plan_envelope(&text, stale, Some(payload)).unwrap_err();
    assert_eq!(error.exit, Exit::AdmissionDenied);
    assert_eq!(error.rules(), vec!["saved-plan-expired"]);
}

#[test]
fn saved_plan_envelope_rejects_payload_and_metadata_tampering() {
    let payload = b"opaque saved terraform plan";
    let mut value: Value =
        serde_json::from_str(&fixture("saved-plan-envelope.valid.json")).unwrap();
    value["plan"]["digest"] = Value::String(canon::digest_bytes(payload));
    value["plan"]["sizeBytes"] = Value::from(payload.len() as u64);
    let text = with_self_digest(&canon::to_string(&value).unwrap(), "envelopeDigest");
    let now = OffsetDateTime::parse("2026-08-03T12:15:00Z", &Rfc3339).unwrap();

    let payload_error = parse_saved_plan_envelope(&text, now, Some(b"different plan")).unwrap_err();
    assert_eq!(payload_error.exit, Exit::ArtifactMismatch);
    assert_eq!(
        payload_error.rules(),
        vec!["saved-plan-size-mismatch", "saved-plan-digest-mismatch"]
    );

    let mut tampered: Value = serde_json::from_str(&text).unwrap();
    tampered["state"]["serial"] = Value::from(42_u64);
    let envelope_error =
        parse_saved_plan_envelope(&canon::to_string(&tampered).unwrap(), now, Some(payload))
            .unwrap_err();
    assert_eq!(
        envelope_error.rules(),
        vec!["saved-plan-envelope-digest-mismatch"]
    );
}

#[test]
fn saved_plan_envelope_binds_execution_storage_backend_and_max_ttl() {
    let payload = b"opaque saved terraform plan";
    let original = saved_plan_with_payload(payload);
    let now = OffsetDateTime::parse("2026-08-03T12:15:00Z", &Rfc3339).unwrap();

    for pointer in [
        "/runner/arch",
        "/runner/moduleExtractionPath",
        "/regionalTables/digest",
        "/regionalTables/definitionsDigest",
        "/bindingDigest",
        "/plan/location/key",
        "/plan/location/kmsKeyArn",
        "/state/backend/configDigest",
        "/state/backendKey",
        "/terraform/binary/digest",
        "/workflow/workflowRef",
    ] {
        let mut substituted = original.clone();
        *substituted.pointer_mut(pointer).unwrap() = Value::String(
            match pointer {
                "/runner/arch" => "aarch64",
                "/runner/moduleExtractionPath" => "/tmp/substituted/modules",
                "/regionalTables/digest" => {
                    "sha256:1212121212121212121212121212121212121212121212121212121212121213"
                }
                "/regionalTables/definitionsDigest" => {
                    "blake3:3434343434343434343434343434343434343434343434343434343434343435"
                }
                "/bindingDigest" => {
                    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                }
                "/plan/location/key" => "saved-plans/dev/other/plan.tfplan",
                "/plan/location/kmsKeyArn" => {
                    "arn:aws:kms:eu-west-1:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                }
                "/state/backend/configDigest" => {
                    "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                }
                "/state/backendKey" => "composition/dev/eu-west-1/other.tfstate",
                "/terraform/binary/digest" => {
                    "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                }
                "/workflow/workflowRef" => {
                    "aexhq/platform/.github/workflows/_release-engine.yml@cccccccccccccccccccccccccccccccccccccccc"
                }
                _ => unreachable!(),
            }
            .to_owned(),
        );
        let text = canon::to_string(&substituted).unwrap();
        assert_eq!(
            parse_saved_plan_envelope(&text, now, Some(payload))
                .unwrap_err()
                .rules(),
            vec!["saved-plan-envelope-digest-mismatch"],
            "substitution at {pointer} must invalidate the envelope"
        );
    }

    let mut excessive_ttl = original;
    excessive_ttl["expiresAt"] = Value::String("2026-08-03T13:00:01Z".to_owned());
    let text = with_self_digest(&canon::to_string(&excessive_ttl).unwrap(), "envelopeDigest");
    assert!(
        parse_saved_plan_envelope(&text, now, Some(payload))
            .unwrap_err()
            .rules()
            .contains(&"saved-plan-ttl-too-long")
    );
}

#[test]
fn saved_plan_envelope_models_first_deploy_and_immutable_workflow_identity() {
    let payload = b"opaque saved terraform plan";
    let now = OffsetDateTime::parse("2026-08-03T12:15:00Z", &Rfc3339).unwrap();
    let mut first_deploy = saved_plan_with_payload(payload);
    first_deploy["state"] = serde_json::json!({
        "status": "absent",
        "backend": {
            "kind": "s3",
            "bucket": "aex-fixture-terraform-state",
            "region": "eu-west-1",
            "configDigest": "sha256:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
        },
        "backendKey": "composition/dev/eu-west-1/application.tfstate"
    });
    let first_deploy =
        with_self_digest(&canon::to_string(&first_deploy).unwrap(), "envelopeDigest");
    assert!(matches!(
        parse_saved_plan_envelope(&first_deploy, now, Some(payload))
            .unwrap()
            .state,
        StateIdentity::Absent { .. }
    ));

    let mut mutable_workflow = saved_plan_with_payload(payload);
    mutable_workflow["workflow"]["workflowRef"] = Value::String(
        "aexhq/platform/.github/workflows/_release-engine.yml@refs/heads/main".to_owned(),
    );
    let mutable_workflow = with_self_digest(
        &canon::to_string(&mutable_workflow).unwrap(),
        "envelopeDigest",
    );
    assert!(
        parse_saved_plan_envelope(&mutable_workflow, now, Some(payload))
            .unwrap_err()
            .rules()
            .contains(&"saved-plan-workflow-ref-invalid")
    );

    let mut mismatched_workflow = saved_plan_with_payload(payload);
    mismatched_workflow["workflow"]["workflowRef"] = Value::String(
        "aexhq/platform/.github/workflows/_release-engine.yml@cccccccccccccccccccccccccccccccccccccccc"
            .to_owned(),
    );
    let mismatched_workflow = with_self_digest(
        &canon::to_string(&mismatched_workflow).unwrap(),
        "envelopeDigest",
    );
    assert!(
        parse_saved_plan_envelope(&mismatched_workflow, now, Some(payload))
            .unwrap_err()
            .rules()
            .contains(&"saved-plan-workflow-ref-commit-mismatch")
    );
}
