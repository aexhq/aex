//! Closed producer-toolchain identity for reproducible OCI builds.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::artifact::{BuildPlan, Toolchain};
use crate::error::{Exit, Result, ToolError, io};

/// Docker Buildx release used by the OCI producer.
pub const BUILDX_VERSION: &str = "v0.34.1";
/// SHA-256 of the official Linux AMD64 Buildx release executable.
pub const BUILDX_BINARY_SHA256: &str =
    "sha256:f1332ddb9010bd0b72628266c3a906d9a6979848033df4c8d9bd2cd113bae12b";
/// Immutable `BuildKit` daemon image used by `Buildx`.
pub const BUILDKIT_IMAGE: &str =
    "moby/buildkit:v0.30.0@sha256:0168606be2315b7c807a03b3d8aa79beefdb31c98740cebdffdfeebf31190c9f";
/// Zig release used by `cargo-zigbuild`.
pub const ZIG_VERSION: &str = "0.15.2";
/// SHA-256 published in Zig's official download index for the Linux AMD64 archive.
pub const ZIG_DOWNLOAD_SHA256: &str =
    "sha256:02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239";
/// SHA-256 of `zig` extracted from the pinned official archive.
pub const ZIG_BINARY_SHA256: &str =
    "sha256:2858dc89dbbfdd08cceda1b841e7fd0a793a1a67b49f150bc3d0d1de44ed7f51";
/// `cargo-zigbuild` release used by the OCI producer.
pub const CARGO_ZIGBUILD_VERSION: &str = "0.22.3";
/// SHA-256 of the official Linux GNU AMD64 release archive.
pub const CARGO_ZIGBUILD_DOWNLOAD_SHA256: &str =
    "sha256:6a014d41ba41ca4b69ca4c4819b9f78a41b0197b5d486904e31c1244e3686190";
/// SHA-256 of `cargo-zigbuild` extracted from the pinned official archive.
pub const CARGO_ZIGBUILD_BINARY_SHA256: &str =
    "sha256:c3a62288419645c4172ba8bda7f6af6ef24df8a2cc264a401e4c4373e22649cf";

/// One downloaded producer executable and both identities that close it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciToolIdentity {
    /// Exact observed release version.
    pub version: String,
    /// SHA-256 of the publisher's downloaded release artifact.
    pub download_digest: String,
    /// SHA-256 of the executable that actually ran.
    pub binary_digest: String,
}

/// Every non-Rust producer component that shapes OCI bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciToolchain {
    /// Schema discriminator.
    pub schema: String,
    /// Docker Buildx client.
    pub buildx: OciToolIdentity,
    /// Immutable `BuildKit` daemon image.
    pub buildkit_image: String,
    /// Zig linker/toolchain.
    pub zig: OciToolIdentity,
    /// Cargo's Zig build adapter.
    pub cargo_zigbuild: OciToolIdentity,
}

impl OciToolchain {
    /// Require this document to be the producer identity compiled into the
    /// release tool; a hand-written toolchain JSON is not authority.
    ///
    /// # Errors
    /// Refuses any producer component that differs from the compiled pins.
    pub fn validate_pins(&self) -> Result<()> {
        if self == &pinned() {
            Ok(())
        } else {
            Err(ToolError::single(
                Exit::ArtifactMismatch,
                "oci-toolchain-pin",
                "OCI producer identity differs from the compiled release pins",
            ))
        }
    }

    /// Digest over the complete producer identity.
    ///
    /// # Errors
    /// Propagates canonical JSON serialization failure.
    pub fn digest(&self) -> Result<String> {
        crate::canon::digest_document(&serde_json::json!(self))
    }

    /// Digest over the authoritative Cargo plan and exact producer identity.
    ///
    /// # Errors
    /// Propagates canonical JSON serialization failure.
    pub fn recipe_digest(&self, plan: &BuildPlan) -> Result<String> {
        crate::canon::digest_document(&serde_json::json!({
            "buildPlan": plan,
            "producerToolchain": self,
        }))
    }

    /// Add exact OCI producer identities to an artifact envelope toolchain.
    pub fn enrich_envelope(&self, toolchain: &mut Toolchain) {
        toolchain.components = vec![
            format!(
                "buildx@{}#{}",
                self.buildx.version, self.buildx.binary_digest
            ),
            format!("buildkit@{}", self.buildkit_image),
            format!("zig@{}#{}", self.zig.version, self.zig.binary_digest),
            format!(
                "cargo-zigbuild@{}#{}",
                self.cargo_zigbuild.version, self.cargo_zigbuild.binary_digest
            ),
        ];
        toolchain.packager_version = Some(format!("buildx-{}", self.buildx.version));
    }

    /// Build arguments recorded in the artifact envelope.
    #[must_use]
    pub fn build_args(&self) -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::from([
            ("BUILDKIT_IMAGE".to_owned(), self.buildkit_image.clone()),
            (
                "BUILDX_BINARY_SHA256".to_owned(),
                self.buildx.binary_digest.clone(),
            ),
            ("BUILDX_VERSION".to_owned(), self.buildx.version.clone()),
            (
                "CARGO_ZIGBUILD_BINARY_SHA256".to_owned(),
                self.cargo_zigbuild.binary_digest.clone(),
            ),
            (
                "CARGO_ZIGBUILD_DOWNLOAD_SHA256".to_owned(),
                self.cargo_zigbuild.download_digest.clone(),
            ),
            (
                "CARGO_ZIGBUILD_VERSION".to_owned(),
                self.cargo_zigbuild.version.clone(),
            ),
            (
                "ZIG_BINARY_SHA256".to_owned(),
                self.zig.binary_digest.clone(),
            ),
            (
                "ZIG_DOWNLOAD_SHA256".to_owned(),
                self.zig.download_digest.clone(),
            ),
            ("ZIG_VERSION".to_owned(), self.zig.version.clone()),
        ])
    }
}

/// Inspect the exact executables installed in the workflow and require all
/// publisher and extracted-binary pins to agree.
///
/// # Errors
/// Refuses missing executables, digest drift, version drift, or command failure.
pub fn inspect(
    buildx_binary: &Path,
    zig_binary: &Path,
    cargo_zigbuild_binary: &Path,
) -> Result<OciToolchain> {
    require_digest(buildx_binary, BUILDX_BINARY_SHA256, "buildx")?;
    require_digest(zig_binary, ZIG_BINARY_SHA256, "zig")?;
    require_digest(
        cargo_zigbuild_binary,
        CARGO_ZIGBUILD_BINARY_SHA256,
        "cargo-zigbuild",
    )?;
    let buildx_version = command_output(buildx_binary, &["version"], "buildx")?;
    let zig_version = command_output(zig_binary, &["version"], "zig")?;
    let cargo_zigbuild_version =
        command_output(cargo_zigbuild_binary, &["--version"], "cargo-zigbuild")?;
    if !buildx_version.contains(BUILDX_VERSION)
        || zig_version != ZIG_VERSION
        || cargo_zigbuild_version != format!("cargo-zigbuild {CARGO_ZIGBUILD_VERSION}")
    {
        return Err(ToolError::single(
            Exit::ArtifactMismatch,
            "oci-toolchain-version",
            format!(
                "producer versions differ from pins: buildx `{buildx_version}`, zig `{zig_version}`, cargo-zigbuild `{cargo_zigbuild_version}`"
            ),
        ));
    }
    let toolchain = pinned();
    toolchain.validate_pins()?;
    Ok(toolchain)
}

/// The closed pin document used by tests and after executable verification.
#[must_use]
pub fn pinned() -> OciToolchain {
    OciToolchain {
        schema: "aex.oci-toolchain.v1".to_owned(),
        buildx: OciToolIdentity {
            version: BUILDX_VERSION.to_owned(),
            download_digest: BUILDX_BINARY_SHA256.to_owned(),
            binary_digest: BUILDX_BINARY_SHA256.to_owned(),
        },
        buildkit_image: BUILDKIT_IMAGE.to_owned(),
        zig: OciToolIdentity {
            version: ZIG_VERSION.to_owned(),
            download_digest: ZIG_DOWNLOAD_SHA256.to_owned(),
            binary_digest: ZIG_BINARY_SHA256.to_owned(),
        },
        cargo_zigbuild: OciToolIdentity {
            version: CARGO_ZIGBUILD_VERSION.to_owned(),
            download_digest: CARGO_ZIGBUILD_DOWNLOAD_SHA256.to_owned(),
            binary_digest: CARGO_ZIGBUILD_BINARY_SHA256.to_owned(),
        },
    }
}

fn require_digest(path: &Path, expected: &str, name: &str) -> Result<()> {
    let bytes = std::fs::read(path).map_err(|error| io(&path.display().to_string(), &error))?;
    let actual = crate::canon::digest_bytes(&bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(ToolError::single(
            Exit::ArtifactMismatch,
            "oci-toolchain-binary",
            format!("{name} executable digest `{actual}` is not `{expected}`"),
        ))
    }
}

fn command_output(binary: &Path, args: &[&str], name: &str) -> Result<String> {
    let output = Command::new(binary)
        .args(args)
        .output()
        .map_err(|error| io(&binary.display().to_string(), &error))?;
    if !output.status.success() {
        return Err(ToolError::single(
            Exit::ArtifactMismatch,
            "oci-toolchain-version",
            format!("{name} version command failed"),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
