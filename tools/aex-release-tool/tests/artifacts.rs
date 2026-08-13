//! Artifact identity: byte-stable packaging, and an envelope that refuses to
//! describe bytes it does not match.

mod common;

#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::io::Read as _;

use aex_release_tool::artifact::{
    ArtifactEnvelope, CatalogBuildInputs, Form, package, plan, plan_with_catalog,
    publish_destination,
};
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

#[cfg(unix)]
fn tar_gz_members(bytes: &[u8]) -> BTreeMap<String, (u8, String, Vec<u8>)> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut tar)
        .unwrap();
    let mut members = BTreeMap::new();
    let mut offset = 0;
    while offset + 512 <= tar.len() && tar[offset..offset + 512].iter().any(|byte| *byte != 0) {
        let header = &tar[offset..offset + 512];
        let name_end = header[..100]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(100);
        let prefix_end = header[345..500]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(155);
        let name = std::str::from_utf8(&header[..name_end]).unwrap();
        let prefix = std::str::from_utf8(&header[345..345 + prefix_end]).unwrap();
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let size_end = header[124..136]
            .iter()
            .position(|byte| *byte == 0 || *byte == b' ')
            .unwrap_or(12);
        let size = usize::from_str_radix(
            std::str::from_utf8(&header[124..124 + size_end])
                .unwrap()
                .trim(),
            8,
        )
        .unwrap();
        let start = offset + 512;
        let end = start + size;
        let link_end = header[157..257]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(100);
        let link = std::str::from_utf8(&header[157..157 + link_end])
            .unwrap()
            .to_owned();
        members.insert(path, (header[156], link, tar[start..end].to_vec()));
        offset = start + size.div_ceil(512) * 512;
    }
    members
}

#[cfg(unix)]
fn standalone_build_output() -> tempfile::TempDir {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join(".vercel/output");
    let shared = output.join("functions/shared.func");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(output.join("config.json"), b"{\"version\":3}\n").unwrap();
    std::fs::write(
        shared.join(".vc-config.json"),
        b"{\"runtime\":\"nodejs22.x\"}\n",
    )
    .unwrap();
    std::fs::write(shared.join("index.js"), b"export default 'standalone';\n").unwrap();
    symlink("shared.func", output.join("functions/page.func")).unwrap();
    symlink("shared.func/index.js", output.join("functions/route.js")).unwrap();
    temp
}

#[cfg(unix)]
#[test]
fn build_output_preserves_safe_standalone_file_and_directory_symlinks() {
    let temp = standalone_build_output();
    let output = temp.path().join(".vercel/output");
    let first = package(Form::BuildOutput, &output, 0, "bootstrap").unwrap();
    let second = package(Form::BuildOutput, &output, 0, "bootstrap").unwrap();
    assert_eq!(first, second);

    let members = tar_gz_members(&first);
    assert_eq!(
        members["functions/page.func"],
        (b'2', "shared.func".to_owned(), Vec::new())
    );
    assert_eq!(
        members["functions/route.js"],
        (b'2', "shared.func/index.js".to_owned(), Vec::new())
    );
    assert!(!members.contains_key("functions/page.func/index.js"));
    assert_eq!(members["functions/shared.func/index.js"].0, b'0');
}

#[cfg(unix)]
#[test]
fn build_output_refuses_broken_escaping_and_cyclic_symlinks() {
    use std::os::unix::fs::symlink;

    for (case, configure, rule) in [
        (
            "broken",
            Box::new(|output: &std::path::Path| {
                symlink("missing.js", output.join("broken.js")).unwrap();
            }) as Box<dyn Fn(&std::path::Path)>,
            "build-output-symlink-invalid",
        ),
        (
            "escape",
            Box::new(|output: &std::path::Path| {
                let workspace = output.parent().unwrap().parent().unwrap();
                std::fs::write(workspace.join("secret"), b"secret").unwrap();
                symlink("../../secret", output.join("escape.js")).unwrap();
            }),
            "build-output-symlink-escape",
        ),
        (
            "cycle",
            Box::new(|output: &std::path::Path| {
                std::fs::create_dir_all(output.join("loop")).unwrap();
                symlink("..", output.join("loop/back")).unwrap();
            }),
            "build-output-symlink-cycle",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join(".vercel/output");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join("config.json"), b"{\"version\":3}\n").unwrap();
        configure(&output);
        let error = package(Form::BuildOutput, &output, 0, "bootstrap").unwrap_err();
        assert_eq!(error.rules(), vec![rule], "case {case}");
    }
}

#[test]
fn build_output_refuses_non_standalone_function_file_maps() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join(".vercel/output");
    let function = output.join("functions/api.func");
    std::fs::create_dir_all(&function).unwrap();
    std::fs::write(output.join("config.json"), b"{\"version\":3}\n").unwrap();
    std::fs::write(
        function.join(".vc-config.json"),
        b"{\"runtime\":\"nodejs22.x\",\"filePathMap\":{}}\n",
    )
    .unwrap();
    let error = package(Form::BuildOutput, &output, 0, "bootstrap").unwrap_err();
    assert_eq!(error.rules(), vec!["build-output-not-standalone"]);
}

#[cfg(unix)]
#[test]
fn build_output_refuses_a_link_to_an_empty_unpacked_directory() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join(".vercel/output");
    let function = output.join("functions/api.func");
    std::fs::create_dir_all(&function).unwrap();
    std::fs::create_dir_all(output.join("empty-target")).unwrap();
    std::fs::write(output.join("config.json"), b"{\"version\":3}\n").unwrap();
    std::fs::write(
        function.join(".vc-config.json"),
        b"{\"runtime\":\"nodejs22.x\"}\n",
    )
    .unwrap();
    symlink("empty-target", output.join("empty-link")).unwrap();
    let error = package(Form::BuildOutput, &output, 0, "bootstrap").unwrap_err();
    assert_eq!(error.rules(), vec!["build-output-symlink-unpackaged"]);
}

#[test]
fn every_shipped_unit_has_a_recipe() {
    let units = shipped_units();
    assert!(!units.units.is_empty());
    for unit in &units.units {
        let recipe = plan(unit).unwrap_or_else(|err| panic!("`{}`: {err}", unit.id));
        assert!(!recipe.argv.is_empty());
        assert_eq!(recipe.target, unit.target);
        assert!(!recipe.input.is_empty());
    }
}

#[test]
fn publication_inventory_has_unique_unit_identities() {
    let units = shipped_units();
    let unique = units
        .units
        .iter()
        .map(|unit| unit.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        units.units.len(),
        "no two deployables may share an identity"
    );
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
fn recipes_name_the_real_build_output_instead_of_guessing_from_the_unit_id() {
    let units = shipped_units();
    let recipe = |id: &str| {
        plan(
            units
                .units
                .iter()
                .find(|unit| unit.id == id)
                .expect("registered unit"),
        )
        .expect("runnable recipe")
    };

    assert_eq!(
        recipe("regional-observation-api").input,
        "target/lambda/regional-observation-api/bootstrap"
    );
    assert_eq!(
        recipe("session-stream-api").input,
        "target/aarch64-unknown-linux-gnu/release/session-stream-api"
    );
    assert_eq!(
        recipe("central-api").input,
        "target/aarch64-unknown-linux-gnu/release/central-api"
    );
    assert_eq!(
        recipe("stripe-command-edge").input,
        "services/stripe-command-edge/dist/handler.js"
    );
    assert_eq!(
        recipe("brain-mux").input,
        "target/aarch64-unknown-linux-gnu/release/brain-mux"
    );
    assert_eq!(
        recipe("hands-agent").input,
        "target/aarch64-unknown-linux-musl/release/hands-agent"
    );
    assert_eq!(
        recipe("hands-image-4gb").input,
        "target/microvm/hands-image-4gb"
    );
}

#[test]
fn every_catalog_consumer_records_the_exact_catalog_build_bindings() {
    let units = shipped_units();
    let inputs = CatalogBuildInputs {
        tool_catalog_sha256:
            "sha256:51e0b52e74bfd7883bf6dd5ac915d745cb54a7360ecb447cbeec59955ae61fdb".to_owned(),
    };
    for id in ["brain-mux", "session-stream-api"] {
        let unit = units
            .units
            .iter()
            .find(|unit| unit.id == id)
            .unwrap_or_else(|| panic!("{id} unit"));
        let unstamped = plan(unit).expect("ordinary plan");
        let stamped = plan_with_catalog(unit, Some(&inputs)).expect("release plan");

        assert_eq!(
            stamped.env[aex_release_tool::artifact::TOOL_CATALOG_SHA256_VAR],
            inputs.tool_catalog_sha256
        );
        assert_ne!(stamped.digest, unstamped.digest, "{id} was not stamped");
    }
}

#[test]
fn published_microvm_recipes_are_variant_specific_service_zips() {
    let units = shipped_units();
    let images: Vec<_> = units
        .units
        .iter()
        .filter(|unit| unit.kind == "microvm-image")
        .collect();
    assert!(!images.is_empty(), "the declared MicroVM class has recipes");
    for unit in images {
        let recipe = plan(unit).expect("MicroVM recipe");
        let shape = unit.microvm.as_ref().expect("declared shape");
        assert_eq!(recipe.form, "microvm-zip");
        assert!(
            recipe
                .argv
                .windows(2)
                .any(|pair| pair == ["--variant", &shape.variant])
        );
        assert!(
            recipe
                .base_image
                .as_deref()
                .is_some_and(|image| image.contains("@sha256:"))
        );
    }
}

#[test]
fn a_microvm_context_zip_is_byte_stable_and_names_the_service_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let context = temp.path().join("context");
    std::fs::create_dir(&context).unwrap();
    std::fs::write(
        context.join("Dockerfile"),
        b"FROM example.invalid@sha256:1\n",
    )
    .unwrap();
    std::fs::write(context.join("hands-agent"), b"ELF fixture").unwrap();
    std::fs::write(
        context.join("agent.cdx.json"),
        b"{\"bomFormat\":\"CycloneDX\"}\n",
    )
    .unwrap();
    let first = package(Form::MicrovmZip, &context, 0, "bootstrap").unwrap();
    let second = package(Form::MicrovmZip, &context, 0, "bootstrap").unwrap();
    assert_eq!(first, second);
    for name in [b"Dockerfile".as_slice(), b"hands-agent", b"agent.cdx.json"] {
        assert!(first.windows(name.len()).any(|window| window == name));
    }
}

#[test]
fn oci_recipes_pin_a_runtime_base_and_lambda_recipes_pin_the_archive_name() {
    let units = shipped_units();
    for unit in &units.units {
        if unit.kind.starts_with("rust-oci-") {
            let recipe = plan(unit).expect("OCI compile recipe");
            assert!(
                recipe
                    .base_image
                    .as_deref()
                    .is_some_and(|image| image.contains("@sha256:")),
                "unit `{}` must pin the base image by digest",
                unit.id
            );
        }
        if unit.kind.ends_with("lambda") {
            let recipe = plan(unit).expect("Lambda recipe");
            assert_eq!(recipe.entrypoint.as_deref(), unit.entrypoint.as_deref());
        }
    }
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

/// Describes a local build with no inspected image identity, so `unit_id` must
/// name a blob unit: an OCI unit without an `oci_identity` is refused outright
/// by `oci-identity-missing`, which is a different behaviour than the one the
/// callers below are about.
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
        oci_identity: None,
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

    let (recipe_envelope, recipe_ledger) = described("regional-observation-api", &artifact, None);
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
    let (envelope, ledger) = described("regional-observation-api", &artifact, Some(ran.clone()));
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

    let err = local_build_of("regional-observation-api", &artifact).unwrap_err();
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
fn receipt_refs_are_bound_to_the_exact_build_attempt() {
    let mut envelope = envelope_from(valid_envelope());
    envelope.receipts[0].source.run_attempt += 1;
    envelope = envelope.seal().unwrap();

    let err = envelope.verify(None, false).unwrap_err();
    assert_eq!(err.exit.code(), 20);
    assert!(
        err.violations
            .iter()
            .any(|violation| violation.rule == "envelope-receipt-binding")
    );
}

#[test]
fn artifact_subject_excludes_receipt_refs_but_the_envelope_digest_does_not() {
    let envelope = envelope_from(valid_envelope()).seal().unwrap();
    let subject_digest = envelope.artifact_subject_digest.clone();
    let envelope_digest = envelope.envelope_digest.clone();

    let mut with_another_receipt = envelope;
    with_another_receipt.receipts[0].receipt_digest = digest(0x7a);
    let with_another_receipt = with_another_receipt.seal().unwrap();

    assert_eq!(with_another_receipt.artifact_subject_digest, subject_digest);
    assert_ne!(with_another_receipt.envelope_digest, envelope_digest);
}

#[test]
fn artifact_subject_binds_bytes_source_inputs_and_complete_oci_identity() {
    let envelope = envelope_from(valid_envelope()).seal().unwrap();
    let subject_digest = envelope.artifact_subject_digest.clone();

    let mut changed = envelope.clone();
    changed.source.commit_sha = "b".repeat(40);
    assert_ne!(
        changed.seal().unwrap().artifact_subject_digest,
        subject_digest
    );

    let mut changed = envelope.clone();
    changed.inputs.input_closure_digest = digest(0x71);
    assert_ne!(
        changed.seal().unwrap().artifact_subject_digest,
        subject_digest
    );

    let mut changed = envelope.clone();
    changed.output.digest = digest(0x72);
    assert_ne!(
        changed.seal().unwrap().artifact_subject_digest,
        subject_digest
    );

    let mut changed = envelope.clone();
    changed.output.oci_child_digest = Some(digest(0x73));
    changed.output.oci_config_digest = Some(digest(0x74));
    changed.output.oci_layer_digests = vec![digest(0x75), digest(0x76)];
    assert_ne!(
        changed.seal().unwrap().artifact_subject_digest,
        subject_digest
    );
}

#[test]
fn artifact_subject_survives_workflow_execution_and_content_addressed_relocation() {
    let envelope = envelope_from(valid_envelope()).seal().unwrap();
    let subject_digest = envelope.artifact_subject_digest.clone();
    let envelope_digest = envelope.envelope_digest.clone();

    let mut certified_elsewhere = envelope;
    certified_elsewhere.source.workflow.run_id = "456".to_owned();
    certified_elsewhere.output.location.uri =
        "s3://immutable-bucket/sha256/another-location".to_owned();
    let certified_elsewhere = certified_elsewhere.seal().unwrap();

    assert_eq!(certified_elsewhere.artifact_subject_digest, subject_digest);
    assert_ne!(certified_elsewhere.envelope_digest, envelope_digest);
}

#[test]
fn a_licence_denial_or_an_unapproved_advisory_denies_the_supply_chain() {
    let mut value = valid_envelope();
    value["licenses"]["verdict"] = serde_json::json!("denied");
    let err = envelope_from(value).verify(None, false).unwrap_err();
    assert_eq!(err.exit.code(), 23);
    assert!(err.rules().contains(&"license-denied"));

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

/// A registry row matching a fixture envelope.
///
/// Deliberately not the shipped row for the same id: these tests fix the
/// envelope's kind, and reading the real registry would make them fail whenever
/// a deployable legitimately changes how it is packaged.
fn row(id: &str, kind: &str, package: &str) -> aex_release_tool::graph::inputs::Unit {
    toml::from_str(&format!(
        r#"
id = "{id}"
kind = "{kind}"
plane = "regional"
package = "{package}"
target = "aarch64-unknown-linux-gnu.2.34"
profile = "release-lambda"
form = "zip"
config_schema_version = 1
required_receipts = ["unit"]
"#
    ))
    .expect("fixture registry row")
}

#[test]
fn the_publication_destination_is_content_addressed_and_immutable() {
    let destination = publish_destination(
        &envelope_from(valid_envelope()),
        &row("regional-otlp", "rust-lambda", "regional-otlp"),
    )
    .unwrap();
    assert_eq!(destination.kind, "github-release");
    assert!(destination.immutable);
    assert_eq!(
        destination.key,
        format!("unit-regional-otlp-{}.zip", &digest(5)[7..])
    );
    assert!(
        !destination.key.contains("sha256:"),
        "a release asset basename holds the bare digest, not the scheme prefix"
    );

    let mut value = valid_envelope();
    value["unit"]["kind"] = serde_json::json!("rust-oci-service");
    value["unit"]["id"] = serde_json::json!("brain-mux");
    let destination = publish_destination(
        &envelope_from(value),
        &row("brain-mux", "rust-oci-service", "brain-mux"),
    )
    .unwrap();
    assert_eq!(destination.kind, "oci");
    assert!(destination.key.contains("@sha256:"));
}

#[test]
fn a_published_package_destination_is_the_registry_declared_npm_name() {
    // The whole reason the registry row is required here: a unit id matches
    // `^[a-z][a-z0-9-]{2,63}$`, so `@aexhq/sdk` can never be derived from it.
    let mut value = valid_envelope();
    value["unit"]["kind"] = serde_json::json!("npm-package");
    value["unit"]["id"] = serde_json::json!("sdk");
    let destination = publish_destination(
        &envelope_from(value),
        &row("sdk", "npm-package", "@aexhq/sdk"),
    )
    .unwrap();
    assert_eq!(destination.kind, "npm");
    assert_eq!(destination.key, "@aexhq/sdk");
    assert!(destination.immutable);
}

#[test]
fn a_destination_cannot_be_derived_from_someone_elses_registry_row() {
    let mut value = valid_envelope();
    value["unit"]["kind"] = serde_json::json!("npm-package");
    value["unit"]["id"] = serde_json::json!("sdk");
    let error = publish_destination(
        &envelope_from(value),
        &row("brain-mux", "rust-oci-service", "brain-mux"),
    )
    .expect_err("a row for another unit cannot name this one's destination");
    assert_eq!(error.rules(), vec!["publish-destination-unit-mismatch"]);
}

#[test]
fn github_release_locations_bind_repo_commit_run_attempt_unit_and_digest() {
    let mut value = valid_envelope();
    let digest = value["output"]["digest"].as_str().unwrap().to_owned();
    let expected = aex_release_tool::publication::github_release_unit_uri(
        "aexhq/aex",
        &common::docs::sha1(),
        "123",
        1,
        "regional-otlp",
        &digest,
        "zip",
    )
    .unwrap();
    value["output"]["location"] = serde_json::json!({
        "kind": "github-release",
        "uri": expected,
        "immutable": true
    });
    envelope_from(value.clone()).verify(None, false).unwrap();

    for altered in [
        expected.replace("github.com", "example.com"),
        expected.replace("run-123", "run-124"),
        expected.replace("attempt-1", "attempt-2"),
        expected.replace("regional-otlp", "central-authz"),
        expected.replace(&digest[7..], &"f".repeat(64)),
    ] {
        value["output"]["location"]["uri"] = serde_json::json!(altered);
        let err = envelope_from(value.clone())
            .verify(None, false)
            .unwrap_err();
        assert!(
            err.rules().contains(&"envelope-github-release-location"),
            "{:?}",
            err.rules()
        );
    }
}

#[test]
fn oci_locations_are_ghcr_digest_only_and_kind_bound() {
    let mut value = valid_envelope();
    value["unit"]["kind"] = serde_json::json!("rust-oci-service");
    value["unit"]["id"] = serde_json::json!("brain-mux");
    value["media"]["mediaType"] = serde_json::json!("application/vnd.oci.image.manifest.v1+json");
    value["media"]["form"] = serde_json::json!("oci-image");
    let digest = value["output"]["digest"].as_str().unwrap().to_owned();
    value["output"]["ociChildDigest"] = serde_json::json!(digest.clone());
    value["output"]["ociConfigDigest"] = serde_json::json!(crate::common::docs::digest(11));
    value["output"]["ociLayerDigests"] = serde_json::json!([crate::common::docs::digest(12)]);
    let expected =
        aex_release_tool::publication::ghcr_unit_uri("aexhq/aex", "brain-mux", &digest).unwrap();
    value["output"]["location"] = serde_json::json!({
        "kind": "oci",
        "uri": expected,
        "immutable": true
    });
    envelope_from(value.clone()).verify(None, false).unwrap();

    for altered in [
        expected.replace("ghcr.io", "docker.io"),
        expected.replace("@sha256:", ":main@sha256:"),
        expected.replace(&digest[7..], &"f".repeat(64)),
    ] {
        value["output"]["location"]["uri"] = serde_json::json!(altered);
        let err = envelope_from(value.clone())
            .verify(None, false)
            .unwrap_err();
        assert!(err.rules().contains(&"envelope-oci-location"));
    }

    value["unit"]["kind"] = serde_json::json!("rust-lambda");
    let err = envelope_from(value).verify(None, false).unwrap_err();
    assert!(err.rules().contains(&"envelope-location-kind"));
}

#[test]
fn unknown_location_kinds_are_refused_before_admission() {
    let mut value = valid_envelope();
    value["output"]["location"]["kind"] = serde_json::json!("bucket-ish");
    let err = envelope_from(value).verify(None, false).unwrap_err();
    assert!(err.rules().contains(&"envelope-location-kind"));
}

#[test]
fn an_envelope_records_the_toolchain_and_build_command_that_shaped_the_bytes() {
    let envelope = envelope_from(valid_envelope());
    assert_eq!(envelope.inputs.toolchain.channel, "1.97.1");
    assert!(!envelope.inputs.build_command.argv.is_empty());
    assert_eq!(envelope.inputs.input_closure_digest, digest(4));
}
