//! The pinned `opentelemetry-proto` revision.
//!
//! The `.proto` tree under `proto/` is a vendored copy of one exact upstream
//! release. [`VENDORED_DIGEST`] is a `SHA-256` over the vendored bytes in a
//! fixed file order, and the test below recomputes it, so an edit to the
//! vendored tree — a silent upstream bump, a hand patch, a partial sync — is a
//! red test rather than a wire change nobody noticed.
//!
//! The digest deliberately covers the **sources** and not `prost-build`'s
//! output: the output's formatting moves with a code-generator patch release,
//! which would make the pin fail for a reason that is not a protocol change.

use sha2::{Digest as _, Sha256};

/// The vendored upstream release tag.
///
/// Registered as `AEX_OTLP_PROTO_REVISION` in `api/registries/enums.yaml`,
/// which the contracts stream reserves for `BodyClass::Otlp`.
pub const PROTO_REVISION: &str = "v1.9.0";

/// Every vendored file, in the order the digest consumes them.
pub const VENDORED_FILES: &[&str] = &[
    "opentelemetry/proto/collector/logs/v1/logs_service.proto",
    "opentelemetry/proto/collector/metrics/v1/metrics_service.proto",
    "opentelemetry/proto/collector/trace/v1/trace_service.proto",
    "opentelemetry/proto/common/v1/common.proto",
    "opentelemetry/proto/logs/v1/logs.proto",
    "opentelemetry/proto/metrics/v1/metrics.proto",
    "opentelemetry/proto/resource/v1/resource.proto",
    "opentelemetry/proto/trace/v1/trace.proto",
];

/// The pinned digest of the vendored tree, lowercase hex.
pub const VENDORED_DIGEST: &str = include_str!("../proto/VENDORED.sha256").trim_ascii_end();

/// Recomputes the digest over a caller-supplied file list.
///
/// Each file is length-prefixed with its relative path and its normalized
/// (LF-only) bytes, so a checkout that translated line endings cannot change the
/// pin and a rename cannot go unnoticed.
#[must_use]
pub fn digest_of(files: &[(&str, &[u8])]) -> String {
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    for (name, bytes) in files {
        let normalized: Vec<u8> = bytes
            .iter()
            .copied()
            .filter(|byte| *byte != b'\r')
            .collect();
        hasher.update(u64::try_from(name.len()).unwrap_or(u64::MAX).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update(
            u64::try_from(normalized.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        hasher.update(&normalized);
    }
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{PROTO_REVISION, VENDORED_DIGEST, VENDORED_FILES, digest_of};

    fn vendored() -> Vec<(&'static str, Vec<u8>)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("proto");
        VENDORED_FILES
            .iter()
            .map(|name| {
                let bytes = std::fs::read(root.join(name))
                    .unwrap_or_else(|error| panic!("vendored `{name}` is readable: {error}"));
                (*name, bytes)
            })
            .collect()
    }

    #[test]
    fn the_vendored_tree_matches_its_pinned_digest() {
        let owned = vendored();
        let borrowed: Vec<(&str, &[u8])> = owned
            .iter()
            .map(|(name, bytes)| (*name, bytes.as_slice()))
            .collect();
        assert_eq!(
            digest_of(&borrowed),
            VENDORED_DIGEST,
            "the vendored opentelemetry-proto tree changed; \
             bump PROTO_REVISION and proto/VENDORED.sha256 deliberately"
        );
    }

    #[test]
    fn every_vendored_file_declares_the_pinned_proto3_syntax() {
        for (name, bytes) in vendored() {
            let text = String::from_utf8(bytes).expect("a .proto file is UTF-8");
            assert!(
                text.contains("syntax = \"proto3\";"),
                "`{name}` is not proto3"
            );
            assert!(
                text.contains("package opentelemetry.proto."),
                "`{name}` is not in the pinned package tree"
            );
        }
    }

    #[test]
    fn no_grpc_service_stub_is_generated_from_the_collector_definitions() {
        // The collector files do declare `service`s upstream. `build.rs`
        // configures no service generator, so OTLP reaches AEX over
        // HTTP/protobuf only and `tonic` stays absent from the workspace.
        let out = std::path::Path::new(env!("OUT_DIR"));
        let generated: String = std::fs::read_dir(out)
            .expect("OUT_DIR is readable")
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .collect();
        assert!(!generated.contains("::tonic"), "no gRPC stub is generated");
        assert!(!generated.contains("tonic::"), "no gRPC stub is generated");
        assert!(!generated.contains("trait LogsService"));
        assert!(generated.contains("ExportLogsServiceRequest"));
        assert!(generated.contains("ExportTraceServiceRequest"));
        assert!(generated.contains("ExportMetricsServiceRequest"));
    }

    #[test]
    fn the_revision_is_recorded_and_non_empty() {
        assert!(PROTO_REVISION.starts_with('v'));
        assert_eq!(VENDORED_DIGEST.len(), 64);
        assert!(VENDORED_DIGEST.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(VENDORED_FILES.len(), 8);
    }
}
