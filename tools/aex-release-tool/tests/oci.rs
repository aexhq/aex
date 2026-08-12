//! Reproducible OCI producer, registry-readback and workflow contract tests.

use aex_release_tool::artifact;
use aex_release_tool::canon;
use aex_release_tool::graph::inputs::Units;
use aex_release_tool::oci::{
    OciPackageVisibility, OciVisibilityPhase, decide_visibility, inspect_layout, pinned_toolchain,
    prepare_context, verify_readback, verify_reproducible,
};

mod oci_support;
use oci_support::{
    fake_aarch64_elf, layout_for, prepared, provenance_fixture, repository_root, shipped_unit,
    source, workflow,
};

#[test]
fn context_is_source_stable_and_never_bakes_run_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (unit, plan, context, first) = prepared(&temp, 7);
    let second_context = temp.path().join("context-again");
    let second = prepare_context(
        &unit,
        &plan,
        &context.join("artifact"),
        &second_context,
        source(),
        pinned_toolchain(),
    )
    .expect("second context");
    assert_eq!(
        canon::to_string(&first).unwrap(),
        canon::to_string(&second).unwrap()
    );
    assert_eq!(
        std::fs::read(context.join("Dockerfile")).unwrap(),
        std::fs::read(second_context.join("Dockerfile")).unwrap()
    );

    let dockerfile = std::fs::read_to_string(context.join("Dockerfile")).unwrap();
    assert!(dockerfile.contains(unit.base_image.as_deref().unwrap()));
    assert!(dockerfile.contains("ENTRYPOINT [\"/usr/local/bin/brain-mux\"]"));
    assert!(dockerfile.contains("CMD []"));
    assert!(!dockerfile.contains("run-id"));
    assert!(!dockerfile.contains("run-attempt"));
    assert!(!dockerfile.contains("987654321"));
}

#[test]
fn context_refuses_non_oci_wrong_target_and_wrong_elf_inputs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut unit, plan, _, _) = prepared(&temp, 8);
    let binary = temp.path().join("wrong");
    std::fs::write(&binary, b"not an elf").unwrap();
    let error = prepare_context(
        &unit,
        &plan,
        &binary,
        &temp.path().join("bad"),
        source(),
        pinned_toolchain(),
    )
    .unwrap_err();
    assert_eq!(error.rules(), vec!["oci-binary-elf"]);

    unit.kind = "rust-binary".to_owned();
    let error = prepare_context(
        &unit,
        &plan,
        &temp.path().join("context-8/artifact"),
        &temp.path().join("not-oci"),
        source(),
        pinned_toolchain(),
    )
    .unwrap_err();
    assert_eq!(error.rules(), vec!["oci-unit-kind"]);
}

#[test]
fn context_refuses_a_recipe_whose_recorded_digest_does_not_match_its_command() {
    let temp = tempfile::tempdir().expect("tempdir");
    let unit = shipped_unit("brain-mux");
    let mut plan = artifact::plan(&unit).expect("build plan");
    plan.argv.push("--tampered".to_owned());
    let binary = temp.path().join("brain-mux");
    std::fs::write(&binary, fake_aarch64_elf(42)).expect("ELF fixture");
    let error = prepare_context(
        &unit,
        &plan,
        &binary,
        &temp.path().join("tampered-context"),
        source(),
        pinned_toolchain(),
    )
    .unwrap_err();
    assert_eq!(error.rules(), vec!["oci-recipe-mismatch"]);
}

#[test]
fn context_accepts_complete_release_bound_catalog_recipes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let signing = p256::ecdsa::SigningKey::from_slice(&[7; 32]).expect("fixture key");
    let trust_roots_json = canon::to_string(&serde_json::json!({
        "keys": [{
            "keyId": "aex-catalog-fixture",
            "sec1": hex::encode(signing.verifying_key().to_sec1_point(false).as_bytes()),
        }],
        "schema": "aex.model-catalog-trust-roots.v1",
    }))
    .expect("canonical trust roots");
    let inputs = artifact::ModelCatalogBuildInputs {
        trust_roots_sha256: canon::digest_bytes(trust_roots_json.as_bytes()),
        trust_roots_json,
        collection_file: "release-inputs/model-catalog-collection.json".to_owned(),
        collection_sha256:
            "sha256:7777777777777777777777777777777777777777777777777777777777777777".to_owned(),
        tool_catalog_sha256:
            "sha256:51e0b52e74bfd7883bf6dd5ac915d745cb54a7360ecb447cbeec59955ae61fdb".to_owned(),
    };
    for unit_name in ["brain-mux", "session-stream-api"] {
        let unit = shipped_unit(unit_name);
        let plan = artifact::plan_with_model_catalog(&unit, Some(&inputs)).expect("release plan");
        let binary = temp.path().join(unit_name);
        std::fs::write(&binary, fake_aarch64_elf(43)).expect("ELF fixture");

        prepare_context(
            &unit,
            &plan,
            &binary,
            &temp.path().join(format!("release-bound-{unit_name}")),
            source(),
            pinned_toolchain(),
        )
        .expect("complete release-bound recipe");
    }
}

#[test]
fn context_refuses_a_declared_toolchain_that_differs_from_compiled_pins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let unit = shipped_unit("brain-mux");
    let plan = artifact::plan(&unit).expect("build plan");
    let binary = temp.path().join("brain-mux");
    std::fs::write(&binary, fake_aarch64_elf(44)).unwrap();
    let mut toolchain = pinned_toolchain();
    toolchain.zig.binary_digest =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned();
    let error = prepare_context(
        &unit,
        &plan,
        &binary,
        &temp.path().join("drifted-context"),
        source(),
        toolchain,
    )
    .unwrap_err();
    assert_eq!(error.rules(), vec!["oci-toolchain-pin"]);
}

#[test]
fn two_independent_layouts_have_exact_manifest_config_layer_and_elf_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (_, _, first_context, first_binding) = prepared(&temp, 9);
    let first_binary = std::fs::read(first_context.join("artifact")).unwrap();
    let first_layout = temp.path().join("layout-1");
    layout_for(&first_layout, &first_binding, &first_binary);
    let first = inspect_layout(&first_layout, &first_binding).expect("first identity");

    let second_context = temp.path().join("independent-context");
    let unit = shipped_unit("brain-mux");
    let plan = artifact::plan(&unit).unwrap();
    let independent_binary = temp.path().join("independent-elf");
    std::fs::write(&independent_binary, fake_aarch64_elf(9)).unwrap();
    let second_binding = prepare_context(
        &unit,
        &plan,
        &independent_binary,
        &second_context,
        source(),
        pinned_toolchain(),
    )
    .unwrap();
    let second_layout = temp.path().join("layout-2");
    layout_for(
        &second_layout,
        &second_binding,
        &std::fs::read(second_context.join("artifact")).unwrap(),
    );
    let second = inspect_layout(&second_layout, &second_binding).expect("second identity");

    verify_reproducible(&first, &second).expect("byte-identical image identities");
    assert_eq!(first.manifest.digest, first.output_digest);
    assert_eq!(first.layers.len(), 1);
    assert_eq!(first.binary_digest, first_binding.binary_digest);
}

#[test]
fn a_changed_layer_or_config_binding_is_not_reproducible() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (_, _, context, binding) = prepared(&temp, 10);
    let first_layout = temp.path().join("layout-first");
    let first_manifest = layout_for(
        &first_layout,
        &binding,
        &std::fs::read(context.join("artifact")).unwrap(),
    );
    let first = inspect_layout(&first_layout, &binding).unwrap();

    let changed_layout = temp.path().join("layout-changed");
    layout_for(&changed_layout, &binding, &fake_aarch64_elf(11));
    let error = inspect_layout(&changed_layout, &binding).unwrap_err();
    assert!(error.rules().contains(&"oci-binary-digest"));

    let raw_path = temp.path().join("manifest.json");
    std::fs::write(&raw_path, first_manifest).unwrap();
    let expected_workflow = workflow();
    let (verified, bundle) = provenance_fixture(&temp, &first, &expected_workflow);
    let error = verify_readback(
        &first,
        &raw_path,
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        context.join("artifact").as_path(),
        expected_workflow,
        &verified,
        &bundle,
    )
    .unwrap_err();
    assert!(error.rules().contains(&"oci-readback-config"));
}

#[test]
fn registry_readback_binds_raw_descriptors_pulled_elf_and_workflow_attempt() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (_, _, context, binding) = prepared(&temp, 12);
    let layout = temp.path().join("layout");
    let manifest = layout_for(
        &layout,
        &binding,
        &std::fs::read(context.join("artifact")).unwrap(),
    );
    let identity = inspect_layout(&layout, &binding).unwrap();
    let manifest_path = temp.path().join("readback-manifest.json");
    std::fs::write(&manifest_path, manifest).unwrap();
    let expected_workflow = workflow();
    let (verified, bundle) = provenance_fixture(&temp, &identity, &expected_workflow);
    let publication = verify_readback(
        &identity,
        &manifest_path,
        &identity.config.digest,
        &context.join("artifact"),
        expected_workflow,
        &verified,
        &bundle,
    )
    .expect("verified registry readback");

    assert_eq!(publication.schema, "aex.oci-publication.v1");
    assert_eq!(publication.image.output_digest, identity.output_digest);
    assert_eq!(publication.workflow.run_id, "987654321");
    assert_eq!(publication.workflow.run_attempt, 2);
    assert_eq!(
        publication.provenance.invocation_id,
        "https://github.com/aexhq/aex/actions/runs/987654321/attempts/2"
    );
    assert!(publication.provenance.bundle_digest.starts_with("sha256:"));
    assert_eq!(
        publication.location.uri,
        format!(
            "oci://{}@{}",
            identity.image_repository, identity.output_digest
        )
    );
    assert!(publication.location.immutable);
}

#[test]
fn verified_provenance_refuses_hostile_subject_invocation_workflow_and_bundle_inputs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (_, _, context, binding) = prepared(&temp, 13);
    let layout = temp.path().join("layout-hostile");
    let manifest = layout_for(
        &layout,
        &binding,
        &std::fs::read(context.join("artifact")).unwrap(),
    );
    let identity = inspect_layout(&layout, &binding).unwrap();
    let manifest_path = temp.path().join("hostile-manifest.json");
    std::fs::write(&manifest_path, manifest).unwrap();
    let expected_workflow = workflow();
    let (verified, bundle) = provenance_fixture(&temp, &identity, &expected_workflow);
    let pristine: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&verified).unwrap()).unwrap();

    for (name, pointer, hostile) in [
        (
            "subject",
            "/0/verificationResult/statement/subject/0/name",
            "ghcr.io/aexhq/aex-units/another-unit",
        ),
        (
            "invocation",
            "/0/verificationResult/statement/predicate/runDetails/metadata/invocationId",
            "https://github.com/aexhq/aex/actions/runs/987654321/attempts/3",
        ),
        (
            "workflow",
            "/0/verificationResult/signature/certificate/buildSignerURI",
            "https://github.com/aexhq/aex/.github/workflows/hostile.yml@refs/heads/main",
        ),
        (
            "source",
            "/0/verificationResult/signature/certificate/sourceRepositoryDigest",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
    ] {
        let mut document = pristine.clone();
        *document.pointer_mut(pointer).expect("fixture pointer") =
            serde_json::Value::String(hostile.to_owned());
        let hostile_path = temp.path().join(format!("verified-{name}.json"));
        std::fs::write(&hostile_path, serde_json::to_vec(&document).unwrap()).unwrap();
        let error = verify_readback(
            &identity,
            &manifest_path,
            &identity.config.digest,
            &context.join("artifact"),
            expected_workflow.clone(),
            &hostile_path,
            &bundle,
        )
        .unwrap_err();
        assert!(error.rules().contains(&"oci-provenance-binding"));
    }

    let hostile_bundle = temp.path().join("hostile-bundle.json");
    std::fs::write(&hostile_bundle, br#"{"hostile":true}"#).unwrap();
    let error = verify_readback(
        &identity,
        &manifest_path,
        &identity.config.digest,
        &context.join("artifact"),
        expected_workflow,
        &verified,
        &hostile_bundle,
    )
    .unwrap_err();
    assert!(error.rules().contains(&"oci-provenance-binding"));
}

#[test]
fn ghcr_visibility_is_fail_closed_and_bootstrap_never_claims_to_change_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (_, _, context, binding) = prepared(&temp, 14);
    let layout = temp.path().join("layout-visibility");
    layout_for(
        &layout,
        &binding,
        &std::fs::read(context.join("artifact")).unwrap(),
    );
    let identity = inspect_layout(&layout, &binding).unwrap();
    let normal_public = decide_visibility(
        &identity,
        OciPackageVisibility::Public,
        OciVisibilityPhase::BeforePush,
        false,
    );
    assert_eq!(normal_public.action, "proceed");
    assert!(normal_public.blocker.is_none());

    let normal_missing = decide_visibility(
        &identity,
        OciPackageVisibility::Missing,
        OciVisibilityPhase::BeforePush,
        false,
    );
    assert_eq!(normal_missing.action, "blocked");
    assert_eq!(
        normal_missing.blocker.unwrap().code,
        "oci-ghcr-package-missing"
    );

    let bootstrap = decide_visibility(
        &identity,
        OciPackageVisibility::Missing,
        OciVisibilityPhase::BeforePush,
        true,
    );
    assert_eq!(bootstrap.action, "bootstrap-push");
    let after_private = decide_visibility(
        &identity,
        OciPackageVisibility::Private,
        OciVisibilityPhase::AfterPush,
        true,
    );
    assert_eq!(after_private.action, "blocked");
    assert_eq!(
        after_private.blocker.unwrap().code,
        "oci-ghcr-bootstrap-awaiting-public"
    );
}

#[test]
fn workflow_closes_toolchain_source_visibility_and_digest_only_publication() {
    // Producing the image and pushing it are two workflows now. Both halves are
    // asserted here, against the file that actually owns each step, so moving a
    // step across the boundary fails rather than silently disappearing.
    let compile =
        std::fs::read_to_string(repository_root().join(".github/workflows/_compile-artifacts.yml"))
            .expect("compile workflow");
    let workflow =
        std::fs::read_to_string(repository_root().join(".github/workflows/_build-artifacts.yml"))
            .expect("artifact workflow");

    for required in [
        "rewrite-timestamp=true",
        "moby/buildkit:v0.30.0@sha256:0168606be2315b7c807a03b3d8aa79beefdb31c98740cebdffdfeebf31190c9f",
        "version: v0.34.1",
        "02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239",
        "2858dc89dbbfdd08cceda1b841e7fd0a793a1a67b49f150bc3d0d1de44ed7f51",
        "6a014d41ba41ca4b69ca4c4819b9f78a41b0197b5d486904e31c1244e3686190",
        "c3a62288419645c4172ba8bda7f6af6ef24df8a2cc264a401e4c4373e22649cf",
        "f1332ddb9010bd0b72628266c3a906d9a6979848033df4c8d9bd2cd113bae12b",
        "--platform linux/arm64",
        "artifact oci-prepare",
        "artifact oci-inspect",
        "artifact oci-compare",
        "artifact oci-toolchain",
        "artifact oci-source-clean",
    ] {
        assert!(
            compile.contains(required),
            "compile workflow lacks `{required}`"
        );
    }
    assert!(
        !compile.contains("tags:"),
        "the compiled layout must not mint a tag"
    );

    for required in [
        "push-by-digest=true",
        "name-canonical=true",
        "rewrite-timestamp=true",
        "moby/buildkit:v0.30.0@sha256:0168606be2315b7c807a03b3d8aa79beefdb31c98740cebdffdfeebf31190c9f",
        "version: v0.34.1",
        "f1332ddb9010bd0b72628266c3a906d9a6979848033df4c8d9bd2cd113bae12b",
        "--platform linux/arm64",
        "actions/attest@",
        "gh attestation verify",
        "anonymous-docker",
        "docker buildx imagetools inspect --raw",
        "docker pull --platform linux/arm64",
        "docker cp",
        "artifact oci-readback",
        "artifact oci-source-clean",
        "artifact oci-visibility",
        "AEX_GHCR_VISIBILITY_BOOTSTRAP",
        "--verified-provenance",
        "--provenance-bundle",
    ] {
        assert!(workflow.contains(required), "workflow lacks `{required}`");
    }
    assert!(
        !workflow.contains("tags:"),
        "OCI publication must not mint a tag"
    );
    assert!(!workflow.contains("--method PATCH"));
    assert!(!workflow.contains("--method PUT"));

    // One clean-source proof before planning, two around the push. The total is
    // what the receipt claims, so it is asserted as a sum rather than per file.
    let compile_cleans = compile.matches("artifact oci-source-clean").count();
    let publish_cleans = workflow.matches("artifact oci-source-clean").count();
    assert!(compile_cleans >= 1, "compile proves a clean source once");
    assert!(
        publish_cleans >= 2,
        "publication proves a clean source twice"
    );
    assert!(compile_cleans + publish_cleans >= 3);

    let clean_before_plan = compile
        .find("Verify clean source before planning and building")
        .unwrap();
    let plan = compile.find("Print the recipe").unwrap();
    let visibility = workflow
        .find("Require a public GHCR package or explicit bootstrap authority")
        .unwrap();
    let authenticate = workflow
        .find("Authenticate to GHCR without minting a mutable tag")
        .unwrap();
    let publish = workflow
        .find("Publish the exact OCI manifest by digest")
        .unwrap();
    let clean_before_attest = workflow
        .find("Reverify clean source immediately before OCI attestation")
        .unwrap();
    let attest = workflow.find("Attest the immutable OCI digest").unwrap();
    assert!(clean_before_plan < plan);
    assert!(visibility < authenticate && authenticate < publish);
    assert!(clean_before_attest < attest);
    let publish_step = &workflow[publish..clean_before_attest];
    assert!(
        publish_step.find("artifact oci-source-clean").unwrap()
            < publish_step.find("docker buildx build").unwrap()
    );
}

#[test]
fn artifact_schema_carries_config_and_every_layer_digest() {
    let schema = std::fs::read_to_string(
        repository_root().join("api/schemas/release/artifact-envelope.json"),
    )
    .expect("artifact schema");
    for field in ["ociConfigDigest", "ociLayerDigests"] {
        assert!(schema.contains(field), "artifact schema lacks `{field}`");
    }
}

#[test]
fn all_declared_oci_units_share_the_supported_shape() {
    let text = std::fs::read_to_string(repository_root().join("release/units.toml")).unwrap();
    let units: Units = toml::from_str(&text).unwrap();
    let oci: Vec<_> = units
        .units
        .iter()
        .filter(|unit| unit.kind.starts_with("rust-oci-"))
        .collect();
    assert!(!oci.is_empty(), "the declared OCI class has units");
    for unit in oci {
        assert_eq!(unit.target, "aarch64-unknown-linux-gnu.2.34");
        assert_eq!(unit.form, "oci-image");
        assert_eq!(unit.bin.as_deref(), Some(unit.id.as_str()));
        assert!(
            unit.base_image
                .as_deref()
                .is_some_and(|image| image.contains("@sha256:"))
        );
    }
}
