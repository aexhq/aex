//! Hermetic OCI layout and GitHub provenance fixtures.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use aex_release_tool::artifact::{self, BuildPlan};
use aex_release_tool::canon;
use aex_release_tool::graph::inputs::{Unit, Units};
use aex_release_tool::oci::{
    OciBuildBinding, OciImageIdentity, OciSource, OciWorkflowRun, pinned_toolchain, prepare_context,
};
use flate2::{Compression, GzBuilder};
use tempfile::TempDir;

pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

pub fn shipped_unit(id: &str) -> Unit {
    let text = std::fs::read_to_string(repository_root().join("release/units.toml"))
        .expect("release/units.toml");
    let units: Units = toml::from_str(&text).expect("unit registry");
    units
        .units
        .into_iter()
        .find(|unit| unit.id == id)
        .expect("registered unit")
}

pub fn fake_aarch64_elf(marker: u8) -> Vec<u8> {
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

pub fn source() -> OciSource {
    OciSource {
        repository: "aexhq/aex".to_owned(),
        commit_sha: "0123456789abcdef0123456789abcdef01234567".to_owned(),
    }
}

pub fn workflow() -> OciWorkflowRun {
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

pub fn prepared(temp: &TempDir, marker: u8) -> (Unit, BuildPlan, PathBuf, OciBuildBinding) {
    let unit = shipped_unit("brain-mux");
    let plan = artifact::plan(&unit).expect("build plan");
    let binary = temp.path().join(format!("brain-mux-{marker}"));
    std::fs::write(&binary, fake_aarch64_elf(marker)).expect("ELF fixture");
    let context = temp.path().join(format!("context-{marker}"));
    let binding = prepare_context(
        &unit,
        &plan,
        &binary,
        &context,
        source(),
        pinned_toolchain(),
    )
    .expect("prepared OCI context");
    (unit, plan, context, binding)
}

pub fn provenance_fixture(
    temp: &TempDir,
    identity: &OciImageIdentity,
    workflow: &OciWorkflowRun,
) -> (PathBuf, PathBuf) {
    let bundle = serde_json::json!({
        "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
        "verificationMaterial": {},
        "dsseEnvelope": {"payload": "fixture", "payloadType": "application/vnd.in-toto+json", "signatures": []},
    });
    let invocation = format!(
        "https://github.com/aexhq/aex/actions/runs/{}/attempts/{}",
        workflow.run_id, workflow.run_attempt
    );
    let verified = serde_json::json!([{
        "attestation": {"bundle": bundle, "bundle_url": "", "initiator": "fixture"},
        "verificationResult": {
            "mediaType": "application/vnd.dev.sigstore.verificationresult+json;version=0.1",
            "statement": {
                "_type": "https://in-toto.io/Statement/v1",
                "subject": [{
                    "name": identity.image_repository,
                    "digest": {"sha256": identity.output_digest.trim_start_matches("sha256:")},
                }],
                "predicateType": "https://slsa.dev/provenance/v1",
                "predicate": {
                    "buildDefinition": {
                        "buildType": "https://actions.github.io/buildtypes/workflow/v1",
                        "externalParameters": {"workflow": {
                            "ref": workflow.r#ref,
                            "repository": "https://github.com/aexhq/aex",
                            "path": ".github/workflows/main.yml",
                        }},
                        "internalParameters": {"github": {
                            "event_name": "push",
                            "repository_id": "1",
                            "repository_owner_id": "2",
                            "runner_environment": "github-hosted",
                        }},
                        "resolvedDependencies": [{
                            "uri": "git+https://github.com/aexhq/aex@refs/heads/main",
                            "digest": {"gitCommit": identity.source.commit_sha},
                        }],
                    },
                    "runDetails": {
                        "builder": {"id": workflow.builder_id},
                        "metadata": {"invocationId": invocation},
                    },
                },
            },
            "signature": {"certificate": {
                "certificateIssuer": "CN=sigstore-intermediate",
                "subjectAlternativeName": "https://github.com/aexhq/aex/.github/workflows/main.yml@refs/heads/main",
                "issuer": "https://token.actions.githubusercontent.com",
                "buildSignerURI": workflow.builder_id,
                "buildSignerDigest": identity.source.commit_sha,
                "runnerEnvironment": "github-hosted",
                "sourceRepositoryURI": "https://github.com/aexhq/aex",
                "sourceRepositoryDigest": identity.source.commit_sha,
                "sourceRepositoryRef": workflow.r#ref,
                "runInvocationURI": invocation,
            }},
            "verifiedTimestamps": [{"type": "Tlog", "uri": "https://rekor.sigstore.dev", "timestamp": "2026-08-03T00:00:00Z"}],
        },
    }]);
    let bundle_path = temp.path().join("attestation-bundle.json");
    let verified_path = temp.path().join("verified-provenance.json");
    std::fs::write(&bundle_path, serde_json::to_vec(&bundle).unwrap()).unwrap();
    std::fs::write(&verified_path, serde_json::to_vec(&verified).unwrap()).unwrap();
    (verified_path, bundle_path)
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

pub fn layout_for(root: &Path, binding: &OciBuildBinding, binary: &[u8]) -> Vec<u8> {
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
