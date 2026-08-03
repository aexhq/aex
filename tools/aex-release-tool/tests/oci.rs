//! Reproducible OCI producer, registry-readback and workflow contract tests.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use aex_release_tool::artifact::{self, BuildPlan};
use aex_release_tool::canon;
use aex_release_tool::graph::inputs::{Unit, Units};
use aex_release_tool::oci::{
    OciSource, OciWorkflowRun, inspect_layout, prepare_context, verify_readback,
    verify_reproducible,
};
use flate2::{Compression, GzBuilder};
use tempfile::TempDir;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn shipped_unit(id: &str) -> Unit {
    let text = std::fs::read_to_string(repository_root().join("release/units.toml"))
        .expect("release/units.toml");
    let units: Units = toml::from_str(&text).expect("unit registry");
    units
        .units
        .into_iter()
        .find(|unit| unit.id == id)
        .expect("registered OCI unit")
}

fn fake_aarch64_elf(marker: u8) -> Vec<u8> {
    let mut bytes = vec![0_u8; 64];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&183_u16.to_le_bytes());
    bytes.push(marker);
    bytes
}

fn source() -> OciSource {
    OciSource {
        repository: "aexhq/aex".to_owned(),
        commit_sha: "0123456789abcdef0123456789abcdef01234567".to_owned(),
    }
}

fn workflow() -> OciWorkflowRun {
    OciWorkflowRun {
        repository: "aexhq/aex".to_owned(),
        r#ref: "refs/heads/main".to_owned(),
        path: ".github/workflows/_build-artifacts.yml".to_owned(),
        run_id: "987654321".to_owned(),
        run_attempt: 2,
        job_name: "brain-mux".to_owned(),
        builder_id:
            "https://github.com/aexhq/aex/.github/workflows/_build-artifacts.yml@refs/heads/main"
                .to_owned(),
    }
}

fn prepared(
    temp: &TempDir,
    marker: u8,
) -> (
    Unit,
    BuildPlan,
    PathBuf,
    aex_release_tool::oci::OciBuildBinding,
) {
    let unit = shipped_unit("brain-mux");
    let plan = artifact::plan(&unit).expect("build plan");
    let binary = temp.path().join(format!("brain-mux-{marker}"));
    std::fs::write(&binary, fake_aarch64_elf(marker)).expect("ELF fixture");
    let context = temp.path().join(format!("context-{marker}"));
    let binding =
        prepare_context(&unit, &plan, &binary, &context, source()).expect("prepared OCI context");
    (unit, plan, context, binding)
}

fn append_tar_file(out: &mut Vec<u8>, name: &str, body: &[u8], mode: u32) {
    let mut header = [0_u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    write_octal(&mut header[100..108], u64::from(mode));
    write_octal(&mut header[108..116], 0);
    write_octal(&mut header[116..124], 0);
    write_octal(&mut header[124..136], body.len() as u64);
    write_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|byte| u64::from(*byte)).sum();
    write_octal(&mut header[148..156], checksum);
    out.extend_from_slice(&header);
    out.extend_from_slice(body);
    let padding = (512 - body.len() % 512) % 512;
    out.resize(out.len() + padding, 0);
}

fn write_octal(field: &mut [u8], value: u64) {
    let end = field.len() - 1;
    let digits = format!("{value:0end$o}");
    field[..end].copy_from_slice(digits.as_bytes());
    field[end] = 0;
}

fn gzip_layer(binary_path: &str, binary: &[u8]) -> (Vec<u8>, String) {
    let mut tar = Vec::new();
    append_tar_file(&mut tar, binary_path, binary, 0o555);
    tar.resize(tar.len() + 1024, 0);
    let diff_id = canon::digest_bytes(&tar);
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    encoder.write_all(&tar).expect("gzip layer");
    (encoder.finish().expect("finish gzip"), diff_id)
}

fn write_blob(layout: &Path, bytes: &[u8]) -> (String, u64) {
    let digest = canon::digest_bytes(bytes);
    let path = layout
        .join("blobs/sha256")
        .join(digest.strip_prefix("sha256:").expect("sha256"));
    std::fs::write(path, bytes).expect("OCI blob");
    (digest, bytes.len() as u64)
}

fn layout_for(
    root: &Path,
    binding: &aex_release_tool::oci::OciBuildBinding,
    binary: &[u8],
) -> Vec<u8> {
    std::fs::create_dir_all(root.join("blobs/sha256")).expect("layout dirs");
    std::fs::write(
        root.join("oci-layout"),
        br#"{"imageLayoutVersion":"1.0.0"}"#,
    )
    .expect("oci-layout");

    let binary_path = format!("usr/local/bin/{}", binding.bin);
    let (layer, diff_id) = gzip_layer(&binary_path, binary);
    let (layer_digest, layer_size) = write_blob(root, &layer);
    let config = serde_json::to_vec(&serde_json::json!({
        "architecture": "arm64",
        "config": {
            "Cmd": [],
            "Entrypoint": [format!("/usr/local/bin/{}", binding.bin)],
            "Labels": binding.labels,
        },
        "created": "1970-01-01T00:00:00Z",
        "history": [],
        "os": "linux",
        "rootfs": {"diff_ids": [diff_id], "type": "layers"},
    }))
    .expect("config JSON");
    let (config_digest, config_size) = write_blob(root, &config);
    let manifest = serde_json::to_vec(&serde_json::json!({
        "config": {
            "digest": config_digest,
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "size": config_size,
        },
        "layers": [{
            "digest": layer_digest,
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "size": layer_size,
        }],
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "schemaVersion": 2,
    }))
    .expect("manifest JSON");
    let (manifest_digest, manifest_size) = write_blob(root, &manifest);
    let index = serde_json::to_vec(&serde_json::json!({
        "manifests": [{
            "digest": manifest_digest,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "platform": {"architecture": "arm64", "os": "linux"},
            "size": manifest_size,
        }],
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "schemaVersion": 2,
    }))
    .expect("index JSON");
    std::fs::write(root.join("index.json"), index).expect("index");
    manifest
}

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
    let error =
        prepare_context(&unit, &plan, &binary, &temp.path().join("bad"), source()).unwrap_err();
    assert_eq!(error.rules(), vec!["oci-binary-elf"]);

    unit.kind = "rust-binary".to_owned();
    let error = prepare_context(
        &unit,
        &plan,
        &temp.path().join("context-8/artifact"),
        &temp.path().join("not-oci"),
        source(),
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
    )
    .unwrap_err();
    assert_eq!(error.rules(), vec!["oci-recipe-mismatch"]);
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
    let second_binding =
        prepare_context(&unit, &plan, &independent_binary, &second_context, source()).unwrap();
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
    let error = verify_readback(
        &first,
        &raw_path,
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        context.join("artifact").as_path(),
        workflow(),
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
    let publication = verify_readback(
        &identity,
        &manifest_path,
        &identity.config.digest,
        &context.join("artifact"),
        workflow(),
    )
    .expect("verified registry readback");

    assert_eq!(publication.schema, "aex.oci-publication.v1");
    assert_eq!(publication.image.output_digest, identity.output_digest);
    assert_eq!(publication.workflow.run_id, "987654321");
    assert_eq!(publication.workflow.run_attempt, 2);
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
fn workflow_has_no_oci_blocker_or_mutable_tag_publication() {
    let workflow =
        std::fs::read_to_string(repository_root().join(".github/workflows/_build-artifacts.yml"))
            .expect("artifact workflow");
    assert!(!workflow.contains("Record the explicit OCI publication blocker"));
    assert!(!workflow.contains("blocked-unit-"));
    for required in [
        "push-by-digest=true",
        "name-canonical=true",
        "rewrite-timestamp=true",
        "moby/buildkit:v0.30.0@sha256:0168606be2315b7c807a03b3d8aa79beefdb31c98740cebdffdfeebf31190c9f",
        "version: v0.34.1",
        "--platform linux/arm64",
        "actions/attest@",
        "gh attestation verify",
        "anonymous-docker",
        "docker buildx imagetools inspect --raw",
        "docker pull --platform linux/arm64",
        "docker cp",
        "artifact oci-prepare",
        "artifact oci-inspect",
        "artifact oci-compare",
        "artifact oci-readback",
    ] {
        assert!(workflow.contains(required), "workflow lacks `{required}`");
    }
    assert!(
        !workflow.contains("tags:"),
        "OCI publication must not mint a tag"
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
fn all_five_oci_units_share_the_closed_supported_shape() {
    let text = std::fs::read_to_string(repository_root().join("release/units.toml")).unwrap();
    let units: Units = toml::from_str(&text).unwrap();
    let oci: BTreeMap<_, _> = units
        .units
        .iter()
        .filter(|unit| unit.kind.starts_with("rust-oci-"))
        .map(|unit| (unit.id.as_str(), unit))
        .collect();
    assert_eq!(oci.len(), 5);
    for unit in oci.values() {
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
