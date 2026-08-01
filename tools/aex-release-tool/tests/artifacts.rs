//! Artifact identity: byte-stable packaging, and an envelope that refuses to
//! describe bytes it does not match.

mod common;

use aex_release_tool::artifact::{ArtifactEnvelope, Form, package, plan, publish_destination};
use aex_release_tool::canon;
use aex_release_tool::graph::inputs::Units;
use common::docs::{digest, valid_envelope};

fn envelope_from(value: serde_json::Value) -> ArtifactEnvelope {
    serde_json::from_value::<ArtifactEnvelope>(value)
        .expect("fixture envelope")
        .seal()
        .expect("a sealed envelope")
}

fn shipped_units() -> Units {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/units.toml"),
    )
    .expect("release/units.toml");
    toml::from_str(&text).expect("the shipped unit registry must parse")
}

#[test]
fn every_shipped_unit_has_a_recipe() {
    let units = shipped_units();
    assert!(!units.units.is_empty());
    for unit in &units.units {
        let recipe = plan(unit).unwrap_or_else(|err| panic!("`{}`: {err}", unit.id));
        assert!(!recipe.argv.is_empty());
        assert_eq!(recipe.target, unit.target);
    }
}

#[test]
fn recipes_are_byte_stable_across_runs() {
    let units = shipped_units();
    let first = aex_release_tool::artifact::recipes(&units).unwrap();
    let second = aex_release_tool::artifact::recipes(&units).unwrap();
    assert_eq!(
        canon::to_string(&first).unwrap(),
        canon::to_string(&second).unwrap()
    );
}

#[test]
fn every_ts_lambda_recipe_names_a_source_entry_that_exists() {
    // The recipe is the only record of how the bytes were produced, so a path
    // in it that nobody can `bun build` is a recipe that documents nothing.
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let units = shipped_units();
    let mut checked = 0;
    for unit in units.units.iter().filter(|unit| unit.kind == "ts-lambda") {
        let recipe = plan(unit).unwrap();
        let entry = recipe.argv.last().expect("an entry path");
        assert!(
            repo.join(entry).is_file(),
            "unit `{}` builds `{entry}`, which is not a file",
            unit.id
        );
        checked += 1;
    }
    assert!(checked > 0, "the registry declares no ts-lambda unit");
}

#[test]
fn a_lambda_archive_is_byte_identical_across_packagings() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("bootstrap");
    std::fs::write(&input, b"\x7fELF fixture payload").unwrap();
    let first = package(Form::LambdaZip, &input, 0, "bootstrap").unwrap();
    let second = package(Form::LambdaZip, &input, 0, "bootstrap").unwrap();
    assert_eq!(canon::digest_bytes(&first), canon::digest_bytes(&second));
}

#[test]
fn a_lambda_archive_carries_the_entrypoint_the_unit_declares() {
    // A Node runtime loads `handler.js`; the custom runtime loads `bootstrap`.
    // Packaging every ZIP under one hard-coded name would ship an archive the
    // runtime cannot start and would only be found on a live plane.
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("build-output");
    std::fs::write(&input, b"export const handler = () => {};").unwrap();

    let node = package(Form::LambdaZip, &input, 0, "handler.js").unwrap();
    let rust = package(Form::LambdaZip, &input, 0, "bootstrap").unwrap();
    assert!(
        node.windows(10).any(|window| window == b"handler.js"),
        "the archive must name the declared entrypoint"
    );
    assert!(!node.windows(9).any(|window| window == b"bootstrap"));
    assert!(rust.windows(9).any(|window| window == b"bootstrap"));
}

#[test]
fn a_sound_envelope_verifies_against_its_own_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let artifact = temp.path().join("bootstrap.zip");
    let bytes = b"artifact bytes".to_vec();
    std::fs::write(&artifact, &bytes).unwrap();

    let mut value = valid_envelope();
    value["output"]["digest"] = serde_json::json!(canon::digest_bytes(&bytes));
    value["output"]["sizeBytes"] = serde_json::json!(bytes.len());
    let envelope = envelope_from(value);
    envelope
        .verify(Some(&artifact), false)
        .expect("must verify");
}

#[test]
fn an_altered_artifact_fails_with_a_digest_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let artifact = temp.path().join("bootstrap.zip");
    let bytes = b"artifact bytes".to_vec();
    std::fs::write(&artifact, &bytes).unwrap();
    let mut value = valid_envelope();
    value["output"]["digest"] = serde_json::json!(canon::digest_bytes(&bytes));
    value["output"]["sizeBytes"] = serde_json::json!(bytes.len());
    let envelope = envelope_from(value);

    std::fs::write(&artifact, b"artifact bytes, tampered").unwrap();
    let err = envelope.verify(Some(&artifact), false).unwrap_err();
    assert_eq!(err.exit.code(), 21);
    assert!(err.rules().contains(&"artifact-digest-mismatch"));
}

#[test]
fn an_unattested_envelope_fails_with_a_provenance_error() {
    let mut value = valid_envelope();
    value["provenance"]["attested"] = serde_json::json!(false);
    let envelope = envelope_from(value);
    let err = envelope.verify(None, false).unwrap_err();
    assert_eq!(err.exit.code(), 22);
    assert!(err.rules().contains(&"provenance-unattested"));
}

#[test]
fn an_unsigned_envelope_fails_when_policy_requires_a_signature() {
    let envelope = envelope_from(valid_envelope());
    envelope.verify(None, false).expect("no signature required");
    let err = envelope.verify(None, true).unwrap_err();
    assert_eq!(err.exit.code(), 22);
    assert!(err.rules().contains(&"signature-missing"));
}

#[test]
fn a_tampered_envelope_field_breaks_the_self_digest() {
    let mut envelope = envelope_from(valid_envelope());
    envelope.output.size_bytes = 1;
    let err = envelope.verify(None, false).unwrap_err();
    assert_eq!(err.exit.code(), 20);
    assert!(err.rules().contains(&"envelope-digest-mismatch"));
}

#[test]
fn a_mutable_location_is_not_an_identity() {
    let mut value = valid_envelope();
    value["output"]["location"]["uri"] = serde_json::json!("registry/brain-mux:latest");
    let envelope = envelope_from(value);
    let err = envelope.verify(None, false).unwrap_err();
    assert_eq!(err.exit.code(), 20);
    assert!(err.rules().contains(&"envelope-mutable-location"));
}

#[test]
fn a_post_deployment_receipt_class_is_refused_by_the_rust_check_too() {
    // The schema forbids it structurally. The Rust check forbids it as well,
    // because the envelope reaching `admit` may have come from anywhere.
    let mut value = valid_envelope();
    value["receipts"][0]["class"] = serde_json::json!("smoke");
    let envelope = envelope_from(value);
    let err = envelope.verify(None, false).unwrap_err();
    assert!(err.rules().contains(&"envelope-receipt-class"));
}

#[test]
fn a_licence_denial_or_an_unapproved_advisory_denies_the_supply_chain() {
    let mut value = valid_envelope();
    value["licenses"]["verdict"] = serde_json::json!("denied");
    let err = envelope_from(value).verify(None, false).unwrap_err();
    assert_eq!(err.exit.code(), 23);

    let mut value = valid_envelope();
    value["vulnerabilities"]["unapprovedHigh"] = serde_json::json!(2);
    let err = envelope_from(value).verify(None, false).unwrap_err();
    assert_eq!(err.exit.code(), 23);
    assert!(err.rules().contains(&"advisory-denied"));
}

#[test]
fn an_envelope_with_no_receipt_proves_nothing() {
    let mut value = valid_envelope();
    value["receipts"] = serde_json::json!([]);
    let err = envelope_from(value).verify(None, false).unwrap_err();
    assert_eq!(err.exit.code(), 20);
    assert!(err.rules().contains(&"envelope-no-receipts"));
}

#[test]
fn the_publication_destination_is_content_addressed_and_immutable() {
    let destination = publish_destination(&envelope_from(valid_envelope())).unwrap();
    assert_eq!(destination.kind, "s3");
    assert!(destination.immutable);
    assert!(
        destination.key.starts_with("lambda/regional-session-api/")
            && std::path::Path::new(&destination.key)
                .extension()
                .is_some_and(|ext| ext == "zip"),
        "key was `{}`",
        destination.key
    );
    assert!(
        !destination.key.contains("sha256:"),
        "an S3 key holds the bare digest, not the scheme prefix"
    );

    let mut value = valid_envelope();
    value["unit"]["kind"] = serde_json::json!("rust-oci-service");
    value["unit"]["id"] = serde_json::json!("brain-mux");
    let destination = publish_destination(&envelope_from(value)).unwrap();
    assert_eq!(destination.kind, "ecr");
    assert!(destination.key.contains("@sha256:"));
}

#[test]
fn an_envelope_records_the_toolchain_and_build_command_that_shaped_the_bytes() {
    let envelope = envelope_from(valid_envelope());
    assert_eq!(envelope.inputs.toolchain.channel, "1.97.1");
    assert!(!envelope.inputs.build_command.argv.is_empty());
    assert_eq!(envelope.inputs.input_closure_digest, digest(4));
}
