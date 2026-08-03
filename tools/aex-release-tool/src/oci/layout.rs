//! OCI layout descriptor, runtime-config and copied-binary verification.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use serde::Deserialize;

use super::{
    OciBuildBinding, OciDescriptorIdentity, OciImageIdentity, invalid, valid_sha256,
    validate_aarch64_elf,
};
use crate::error::{Exit, Result, ToolError, io};

const OCI_LAYOUT_VERSION: &str = "1.0.0";
const OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";
const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
const OCI_CONFIG: &str = "application/vnd.oci.image.config.v1+json";
const OCI_GZIP_LAYER: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
const EPOCH: &str = "1970-01-01T00:00:00Z";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayoutVersion {
    image_layout_version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    schema_version: u32,
    media_type: String,
    manifests: Vec<Descriptor>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Descriptor {
    pub(super) media_type: String,
    pub(super) digest: String,
    pub(super) size: u64,
    #[serde(default)]
    platform: Option<Platform>,
}

#[derive(Clone, Deserialize)]
struct Platform {
    architecture: String,
    os: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Manifest {
    schema_version: u32,
    media_type: String,
    pub(super) config: Descriptor,
    pub(super) layers: Vec<Descriptor>,
}

#[derive(Deserialize)]
struct ImageConfig {
    architecture: String,
    os: String,
    created: String,
    config: RuntimeConfig,
    rootfs: RootFs,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimeConfig {
    #[serde(default)]
    cmd: Vec<String>,
    #[serde(default)]
    entrypoint: Vec<String>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RootFs {
    r#type: String,
    diff_ids: Vec<String>,
}

/// Inspect a `BuildKit` OCI layout and verify every content descriptor and the
/// copied ELF.
///
/// # Errors
/// Refuses malformed layouts, missing/tampered blobs, platform/config drift,
/// invocation-specific labels, or a copied ELF that differs from the binding.
pub fn inspect_layout(layout: &Path, binding: &OciBuildBinding) -> Result<OciImageIdentity> {
    let version: LayoutVersion = read_json(&layout.join("oci-layout"), "oci-layout")?;
    if version.image_layout_version != OCI_LAYOUT_VERSION {
        return Err(invalid(
            "oci-layout-version",
            "unsupported OCI layout version",
        ));
    }
    let index: Index = read_json(&layout.join("index.json"), "oci-index")?;
    if index.schema_version != 2 || index.media_type != OCI_INDEX || index.manifests.len() != 1 {
        return Err(invalid(
            "oci-index-shape",
            "the layout must contain exactly one linux/arm64 OCI image manifest",
        ));
    }
    let root = &index.manifests[0];
    if root.media_type != OCI_MANIFEST
        || root
            .platform
            .as_ref()
            .is_none_or(|platform| platform.os != "linux" || platform.architecture != "arm64")
    {
        return Err(invalid(
            "oci-index-platform",
            "root descriptor is not linux/arm64",
        ));
    }
    let manifest_bytes = checked_blob(layout, root)?;
    let manifest: Manifest = parse_json(&manifest_bytes, "oci-manifest-json")?;
    validate_manifest(&manifest)?;
    let config_bytes = checked_blob(layout, &manifest.config)?;
    let config: ImageConfig = parse_json(&config_bytes, "oci-config-json")?;
    validate_config(&config, binding, manifest.layers.len())?;
    for layer in &manifest.layers {
        checked_blob(layout, layer)?;
    }
    let Some(binary_layer) = manifest.layers.last() else {
        return Err(invalid("oci-binary-layer", "OCI image has no binary layer"));
    };
    verify_binary_layer(layout, binary_layer, binding)?;

    Ok(OciImageIdentity {
        schema: "aex.oci-image-identity.v1".to_owned(),
        unit: binding.unit.clone(),
        kind: binding.kind.clone(),
        bin: binding.bin.clone(),
        target: binding.target.clone(),
        image_repository: binding.image_repository.clone(),
        source: binding.source.clone(),
        base_image: binding.base_image.clone(),
        recipe_digest: binding.recipe_digest.clone(),
        binary_digest: binding.binary_digest.clone(),
        output_digest: root.digest.clone(),
        manifest: identity(root),
        config: identity(&manifest.config),
        layers: manifest.layers.iter().map(identity).collect(),
    })
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.schema_version != 2
        || manifest.media_type != OCI_MANIFEST
        || manifest.config.media_type != OCI_CONFIG
        || manifest.layers.is_empty()
        || manifest
            .layers
            .iter()
            .any(|layer| layer.media_type != OCI_GZIP_LAYER)
    {
        return Err(invalid(
            "oci-manifest-shape",
            "image manifest must contain one config and at least one gzip OCI layer",
        ));
    }
    Ok(())
}

fn validate_config(config: &ImageConfig, binding: &OciBuildBinding, layers: usize) -> Result<()> {
    if config.architecture != "arm64"
        || config.os != "linux"
        || config.created != EPOCH
        || config.rootfs.r#type != "layers"
        || config.rootfs.diff_ids.len() != layers
        || config
            .rootfs
            .diff_ids
            .iter()
            .any(|digest| !valid_sha256(digest))
        || !config.config.cmd.is_empty()
        || config.config.entrypoint != [format!("/usr/local/bin/{}", binding.bin)]
    {
        return Err(invalid(
            "oci-config-runtime",
            "image config does not preserve the exact arm64/epoch/entrypoint/layer contract",
        ));
    }
    if binding
        .labels
        .iter()
        .any(|(key, value)| config.config.labels.get(key) != Some(value))
        || config.config.labels.keys().any(|key| {
            key.starts_with("dev.aex.workflow")
                || key.contains("run-id")
                || key.contains("run-attempt")
        })
    {
        return Err(invalid(
            "oci-config-binding",
            "image labels do not match source-stable bindings or contain invocation identity",
        ));
    }
    Ok(())
}

fn checked_blob(layout: &Path, descriptor: &Descriptor) -> Result<Vec<u8>> {
    if !valid_sha256(&descriptor.digest) || descriptor.size == 0 {
        return Err(invalid(
            "oci-descriptor",
            "OCI descriptor is not a non-empty SHA-256 blob",
        ));
    }
    let bare = descriptor
        .digest
        .strip_prefix("sha256:")
        .expect("validated digest");
    let path = layout.join("blobs").join("sha256").join(bare);
    let bytes = std::fs::read(&path).map_err(|error| io(&path.display().to_string(), &error))?;
    let actual = crate::canon::digest_bytes(&bytes);
    if actual != descriptor.digest || bytes.len() as u64 != descriptor.size {
        return Err(invalid(
            "oci-blob-mismatch",
            format!("blob `{}` does not match its descriptor", descriptor.digest),
        ));
    }
    Ok(bytes)
}

fn verify_binary_layer(
    layout: &Path,
    descriptor: &Descriptor,
    binding: &OciBuildBinding,
) -> Result<()> {
    let compressed = checked_blob(layout, descriptor)?;
    let mut archive = Vec::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut archive)
        .map_err(|error| invalid("oci-layer-gzip", error.to_string()))?;
    let expected = format!("usr/local/bin/{}", binding.bin);
    let found = tar_file(&archive, &expected)?;
    let Some(bytes) = found else {
        return Err(invalid(
            "oci-binary-missing",
            format!("last image layer contains no `{expected}`"),
        ));
    };
    validate_aarch64_elf(bytes)?;
    if crate::canon::digest_bytes(bytes) != binding.binary_digest {
        return Err(invalid(
            "oci-binary-digest",
            "the ELF copied into the image differs from the independently built input",
        ));
    }
    Ok(())
}

fn tar_file<'a>(archive: &'a [u8], expected: &str) -> Result<Option<&'a [u8]>> {
    let mut offset = 0_usize;
    let mut found = None;
    while offset + 512 <= archive.len() {
        let header = &archive[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            return Ok(found);
        }
        let name = tar_name(header)?;
        let size = tar_octal(&header[124..136])?;
        let body_start = offset + 512;
        let body_end = body_start
            .checked_add(size)
            .ok_or_else(|| invalid("oci-layer-tar", "tar member size overflows the archive"))?;
        if body_end > archive.len() {
            return Err(invalid(
                "oci-layer-tar",
                "tar member extends past the archive",
            ));
        }
        if name == expected && matches!(header[156], 0 | b'0') {
            if found.is_some() {
                return Err(invalid(
                    "oci-binary-duplicate",
                    "copied OCI layer contains the binary path more than once",
                ));
            }
            if tar_octal(&header[100..108])? != 0o555
                || tar_octal(&header[108..116])? != 0
                || tar_octal(&header[116..124])? != 0
                || tar_octal(&header[136..148])? != 0
            {
                return Err(invalid(
                    "oci-binary-metadata",
                    "copied OCI binary must be root-owned mode 0555 with epoch mtime",
                ));
            }
            found = Some(&archive[body_start..body_end]);
        }
        let padded = size.div_ceil(512) * 512;
        offset = body_start
            .checked_add(padded)
            .ok_or_else(|| invalid("oci-layer-tar", "tar member offset overflows the archive"))?;
    }
    Err(invalid(
        "oci-layer-tar",
        "tar archive has no zero terminator",
    ))
}

fn tar_name(header: &[u8]) -> Result<String> {
    let field = |range: std::ops::Range<usize>| {
        let bytes = &header[range];
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        std::str::from_utf8(&bytes[..end]).map(str::to_owned)
    };
    let name = field(0..100).map_err(|error| invalid("oci-layer-tar", error.to_string()))?;
    let prefix = field(345..500).map_err(|error| invalid("oci-layer-tar", error.to_string()))?;
    let full = if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    };
    if full.starts_with('/') || full.split('/').any(|part| part == "..") {
        return Err(invalid(
            "oci-layer-tar",
            "tar member escapes the image root",
        ));
    }
    Ok(full)
}

fn tar_octal(field: &[u8]) -> Result<usize> {
    let text = std::str::from_utf8(field)
        .map_err(|error| invalid("oci-layer-tar", error.to_string()))?
        .trim_matches(['\0', ' ']);
    usize::from_str_radix(text, 8)
        .map_err(|error| invalid("oci-layer-tar", format!("invalid tar size: {error}")))
}

pub(super) fn identity(descriptor: &Descriptor) -> OciDescriptorIdentity {
    OciDescriptorIdentity {
        media_type: descriptor.media_type.clone(),
        digest: descriptor.digest.clone(),
        size_bytes: descriptor.size,
    }
}

/// Require two clean OCI builds to have exactly the same stable identity.
///
/// # Errors
/// Returns [`Exit::ArtifactMismatch`] when any source, manifest, config, layer,
/// base, recipe, or ELF identity differs.
pub fn verify_reproducible(first: &OciImageIdentity, second: &OciImageIdentity) -> Result<()> {
    if first == second {
        Ok(())
    } else {
        Err(ToolError::single(
            Exit::ArtifactMismatch,
            "oci-not-reproducible",
            format!(
                "independent OCI builds diverged: first manifest `{}`, second `{}`",
                first.output_digest, second.output_digest
            ),
        ))
    }
}

pub(super) fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8], rule: &str) -> Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid(rule, format!("invalid OCI JSON: {error}")))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path, rule: &str) -> Result<T> {
    let bytes = std::fs::read(path).map_err(|error| io(&path.display().to_string(), &error))?;
    parse_json(&bytes, rule)
}

/// Path of the manifest blob described by an inspected identity.
#[must_use]
pub fn manifest_blob_path(layout: &Path, identity: &OciImageIdentity) -> PathBuf {
    layout
        .join("blobs")
        .join("sha256")
        .join(identity.manifest.digest.trim_start_matches("sha256:"))
}
