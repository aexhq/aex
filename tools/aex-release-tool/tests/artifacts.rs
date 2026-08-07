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

fn described(
    unit_id: &str,
    artifact: &std::path::Path,
    ran: Option<Vec<String>>,
) -> (
    aex_release_tool::artifact::ArtifactEnvelope,
    Vec<aex_release_tool::describe::UnearnedField>,
) {
    let units = shipped_units();
    let unit = units
        .units
        .iter()
        .find(|candidate| candidate.id == unit_id)
        .expect("the unit must be in the shipped registry");
    let recipe = plan(unit).unwrap();
    let build = aex_release_tool::describe::LocalBuild {
        unit,
        plan: &recipe,
        artifact,
        repository: "aexhq/aex".to_owned(),
        commit_sha: "b".repeat(40),
        tree_clean: true,
        git_ref: None,
        toolchain: aex_release_tool::artifact::Toolchain {
            channel: "1.97.1".to_owned(),
            rustc_version: "1.97.1".to_owned(),
            rustc_commit_hash: "a".repeat(40),
            host: "x86_64-pc-windows-msvc".to_owned(),
            target: unit.target.clone(),
            components: Vec::new(),
            packager_version: None,
        },
        lockfile_digest: digest(9),
        contract_digest: digest(8),
        actual_argv: ran,
        closure: std::collections::BTreeMap::from([("Cargo.lock".to_owned(), digest(9))]),
        location_uri: "target/release-artifacts/x.zip".to_owned(),
        receipts: Vec::new(),
        created_at: "2026-08-01T00:00:00Z".to_owned(),
    };
    aex_release_tool::describe::describe(&build).expect("a describable build")
}

#[test]
fn an_envelope_records_the_command_that_ran_and_says_when_it_was_not_the_recipe() {
    // The recipe is `cargo lambda build`. On a host where that cannot run, the
    // bytes come from the `cargo zigbuild` invocation it wraps. Recording the
    // recipe anyway would put the one unverifiable claim in the document into
    // the field whose whole purpose is to be reproducible.
    let temp = tempfile::tempdir().unwrap();
    let artifact = temp.path().join("bootstrap.zip");
    std::fs::write(&artifact, b"PK\x03\x04 fixture archive").unwrap();

    let (recipe_envelope, recipe_ledger) = described("regional-session-api", &artifact, None);
    assert_eq!(recipe_envelope.inputs.build_command.argv[1], "lambda");
    assert!(
        !recipe_ledger
            .iter()
            .any(|field| field.pointer == "/inputs/buildCommand/argv"),
        "running the recipe as written owes no explanation"
    );

    let ran = vec![
        "cargo".to_owned(),
        "zigbuild".to_owned(),
        "--profile".to_owned(),
        "release-lambda".to_owned(),
    ];
    let (envelope, ledger) = described("regional-session-api", &artifact, Some(ran.clone()));
    assert_eq!(envelope.inputs.build_command.argv, ran);
    let row = ledger
        .iter()
        .find(|field| field.pointer == "/inputs/buildCommand/argv")
        .expect("the substitution must be recorded");
    assert!(row.reason.contains("cargo lambda build"));
    assert!(row.reason.contains("cargo zigbuild"));
}

fn local_build_of(
    unit_id: &str,
    artifact: &std::path::Path,
) -> aex_release_tool::error::Result<()> {
    let (envelope, unearned) = described(unit_id, artifact, None);
    let bytes = std::fs::read(artifact).unwrap();
    assert_eq!(envelope.output.digest, canon::digest_bytes(&bytes));
    assert_eq!(envelope.output.size_bytes, bytes.len() as u64);
    assert!(
        unearned.iter().any(|field| field.pointer == "/provenance"),
        "the ledger must name every field a local build cannot fill"
    );
    envelope.verify(Some(artifact), false)
}

#[test]
fn a_locally_described_envelope_records_real_bytes_and_is_still_refused() {
    // The whole point of `artifact describe`. It reads the digest, the size,
    // the argv and the closure from the tree, and it refuses to claim the
    // provenance and the immutable location that only a published build has —
    // so `artifact verify` rejects it rather than passing on nothing.
    let temp = tempfile::tempdir().unwrap();
    let artifact = temp.path().join("bootstrap.zip");
    std::fs::write(&artifact, b"PK\x03\x04 fixture archive").unwrap();

    let err = local_build_of("regional-session-api", &artifact).unwrap_err();
    assert_eq!(
        err.exit.code(),
        20,
        "a local location is not an identity: {:?}",
        err.rules()
    );
    assert!(err.rules().contains(&"envelope-mutable-location"));
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
